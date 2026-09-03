#![no_std]
#![forbid(unsafe_code)]

//! Host-testable policy for acquiring the staged bootstrap kernel.

mod boot_info;
mod bootstrap_stack;
mod exit_state;
mod load_plan;
mod page_tables;

pub use boot_info::*;
pub use bootstrap_stack::*;
pub use exit_state::*;
pub use load_plan::*;
pub use page_tables::*;

pub const KERNEL_PATH: &str = r"\EFI\UNNAMEDOS\KERNEL.ELF";
pub const MAX_KERNEL_BYTES: u64 = 16 * 1024 * 1024;
pub const UEFI_PAGE_SIZE: u64 = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SizeError {
    Empty,
    TooLarge,
    AddressSpaceOverflow,
    PageCountOverflow,
}

pub fn checked_kernel_size(size: u64) -> Result<usize, SizeError> {
    if size == 0 {
        return Err(SizeError::Empty);
    }
    if size > MAX_KERNEL_BYTES {
        return Err(SizeError::TooLarge);
    }
    usize::try_from(size).map_err(|_| SizeError::AddressSpaceOverflow)
}

pub fn pages_for_bytes(bytes: usize) -> Result<usize, SizeError> {
    if bytes == 0 {
        return Err(SizeError::Empty);
    }
    let bytes = u64::try_from(bytes).map_err(|_| SizeError::AddressSpaceOverflow)?;
    let rounded = bytes
        .checked_add(UEFI_PAGE_SIZE - 1)
        .ok_or(SizeError::PageCountOverflow)?;
    usize::try_from(rounded / UEFI_PAGE_SIZE).map_err(|_| SizeError::PageCountOverflow)
}

pub trait PositionedFile {
    type Error;

    fn seek_to_end(&mut self) -> Result<(), Self::Error>;
    fn position(&mut self) -> Result<u64, Self::Error>;
    fn rewind(&mut self) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeasureError<E> {
    Seek(E),
    Position(E),
    Rewind(E),
    Size(SizeError),
}

pub fn measure_and_rewind<F: PositionedFile>(
    file: &mut F,
) -> Result<usize, MeasureError<F::Error>> {
    file.seek_to_end().map_err(MeasureError::Seek)?;
    let length = file.position().map_err(MeasureError::Position)?;
    file.rewind().map_err(MeasureError::Rewind)?;
    checked_kernel_size(length).map_err(MeasureError::Size)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadError {
    ShortRead,
    TooManyBytes,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadProgress {
    expected: usize,
    completed: usize,
}

impl ReadProgress {
    pub const fn new(expected: usize) -> Self {
        Self {
            expected,
            completed: 0,
        }
    }

    pub const fn completed(self) -> usize {
        self.completed
    }

    pub const fn remaining(self) -> usize {
        self.expected - self.completed
    }

    pub fn record(&mut self, bytes: usize) -> Result<(), ReadError> {
        if bytes == 0 && self.remaining() != 0 {
            return Err(ReadError::ShortRead);
        }
        if bytes > self.remaining() {
            return Err(ReadError::TooManyBytes);
        }
        self.completed += bytes;
        Ok(())
    }

    pub fn finish(self) -> Result<(), ReadError> {
        if self.completed == self.expected {
            Ok(())
        } else {
            Err(ReadError::ShortRead)
        }
    }
}

pub trait ByteReader {
    type Error;

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExactReadError<E> {
    Source(E),
    ShortRead,
    ExtraData,
}

pub fn read_exact_and_check_eof<R: ByteReader>(
    reader: &mut R,
    buffer: &mut [u8],
) -> Result<(), ExactReadError<R::Error>> {
    let mut progress = ReadProgress::new(buffer.len());
    while progress.remaining() != 0 {
        let read = reader
            .read(&mut buffer[progress.completed()..])
            .map_err(ExactReadError::Source)?;
        progress.record(read).map_err(|error| match error {
            ReadError::ShortRead => ExactReadError::ShortRead,
            ReadError::TooManyBytes => ExactReadError::ExtraData,
        })?;
    }
    progress.finish().map_err(|_| ExactReadError::ShortRead)?;

    let mut extra = [0_u8; 1];
    match reader.read(&mut extra).map_err(ExactReadError::Source)? {
        0 => Ok(()),
        _ => Err(ExactReadError::ExtraData),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseError {
    NotOwned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageLeaseState {
    owned: bool,
}

impl PageLeaseState {
    pub const fn acquired() -> Self {
        Self { owned: true }
    }

    pub const fn is_owned(self) -> bool {
        self.owned
    }

    pub fn begin_release(&mut self) -> Result<(), LeaseError> {
        if !self.owned {
            return Err(LeaseError::NotOwned);
        }
        self.owned = false;
        Ok(())
    }
}

pub fn acquire_with<T, E>(
    allocate: impl FnOnce() -> Result<T, E>,
) -> Result<(T, PageLeaseState), E> {
    allocate().map(|allocation| (allocation, PageLeaseState::acquired()))
}

pub fn finalize_with_release<T, E>(
    operation: Result<T, E>,
    release: Result<(), E>,
) -> Result<T, E> {
    release?;
    operation
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use super::*;

    struct FakeFile<'a> {
        data: &'a [u8],
        cursor: usize,
        fail_seek: bool,
        fail_rewind: bool,
        max_read: usize,
    }

    impl PositionedFile for FakeFile<'_> {
        type Error = ();

        fn seek_to_end(&mut self) -> Result<(), Self::Error> {
            if self.fail_seek {
                Err(())
            } else {
                self.cursor = self.data.len();
                Ok(())
            }
        }

        fn position(&mut self) -> Result<u64, Self::Error> {
            Ok(self.cursor as u64)
        }

        fn rewind(&mut self) -> Result<(), Self::Error> {
            if self.fail_rewind {
                Err(())
            } else {
                self.cursor = 0;
                Ok(())
            }
        }
    }

    impl ByteReader for FakeFile<'_> {
        type Error = Infallible;

        fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
            let available = self.data.len().saturating_sub(self.cursor);
            let length = available.min(buffer.len()).min(self.max_read);
            buffer[..length].copy_from_slice(&self.data[self.cursor..self.cursor + length]);
            self.cursor += length;
            Ok(length)
        }
    }

    #[test]
    fn kernel_path_is_fixed_absolute_uefi_path() {
        assert_eq!(KERNEL_PATH, r"\EFI\UNNAMEDOS\KERNEL.ELF");
        assert!(KERNEL_PATH.is_ascii());
    }

    #[test]
    fn size_policy_rejects_zero_and_accepts_exact_limit() {
        assert_eq!(checked_kernel_size(0), Err(SizeError::Empty));
        assert_eq!(
            checked_kernel_size(MAX_KERNEL_BYTES),
            Ok(MAX_KERNEL_BYTES as usize)
        );
        assert_eq!(
            checked_kernel_size(MAX_KERNEL_BYTES + 1),
            Err(SizeError::TooLarge)
        );
    }

    #[test]
    fn page_rounding_is_exact_and_overflow_safe() {
        assert_eq!(pages_for_bytes(0), Err(SizeError::Empty));
        assert_eq!(pages_for_bytes(1), Ok(1));
        assert_eq!(pages_for_bytes(4096), Ok(1));
        assert_eq!(pages_for_bytes(4097), Ok(2));
        if usize::BITS == 64 {
            assert_eq!(
                pages_for_bytes(usize::MAX),
                Err(SizeError::PageCountOverflow)
            );
        }
    }

    #[test]
    fn partial_reads_accumulate_and_zero_is_short_read() {
        let mut progress = ReadProgress::new(8);
        progress.record(3).expect("first read");
        progress.record(5).expect("second read");
        assert_eq!(progress.completed(), 8);
        assert_eq!(progress.finish(), Ok(()));

        let mut short = ReadProgress::new(8);
        short.record(3).expect("partial read");
        assert_eq!(short.record(0), Err(ReadError::ShortRead));
        assert_eq!(short.finish(), Err(ReadError::ShortRead));
    }

    #[test]
    fn read_progress_rejects_unexpected_additional_data() {
        let mut progress = ReadProgress::new(4);
        assert_eq!(progress.record(5), Err(ReadError::TooManyBytes));
        progress.record(4).expect("exact read");
        assert_eq!(progress.record(1), Err(ReadError::TooManyBytes));
    }

    #[test]
    fn lease_state_prevents_double_free() {
        let mut state = PageLeaseState::acquired();
        assert!(state.is_owned());
        assert_eq!(state.begin_release(), Ok(()));
        assert!(!state.is_owned());
        assert_eq!(state.begin_release(), Err(LeaseError::NotOwned));
    }

    #[test]
    fn allocation_and_cleanup_failures_are_explicit() {
        assert_eq!(acquire_with::<u8, _>(|| Err("alloc")), Err("alloc"));
        let (value, state) = acquire_with::<u8, &str>(|| Ok(7)).expect("allocation");
        assert_eq!(value, 7);
        assert!(state.is_owned());

        assert_eq!(finalize_with_release::<(), &str>(Ok(()), Ok(())), Ok(()));
        assert_eq!(
            finalize_with_release::<(), _>(Err("operation"), Ok(())),
            Err("operation")
        );
        assert_eq!(finalize_with_release(Ok(()), Err("free")), Err("free"));
        assert_eq!(
            finalize_with_release::<(), _>(Err("operation"), Err("free")),
            Err("free")
        );
    }

    #[test]
    fn measurement_seeks_to_end_and_resets_to_start() {
        let mut file = FakeFile {
            data: b"kernel",
            cursor: 2,
            fail_seek: false,
            fail_rewind: false,
            max_read: usize::MAX,
        };
        assert_eq!(measure_and_rewind(&mut file), Ok(6));
        assert_eq!(file.cursor, 0);
        file.fail_seek = true;
        assert_eq!(measure_and_rewind(&mut file), Err(MeasureError::Seek(())));
        file.fail_seek = false;
        file.fail_rewind = true;
        assert_eq!(measure_and_rewind(&mut file), Err(MeasureError::Rewind(())));
    }

    #[test]
    fn exact_reader_handles_partial_short_and_extra_data() {
        let mut partial = FakeFile {
            data: b"kernel",
            cursor: 0,
            fail_seek: false,
            fail_rewind: false,
            max_read: 2,
        };
        let mut exact = [0_u8; 6];
        assert_eq!(read_exact_and_check_eof(&mut partial, &mut exact), Ok(()));
        assert_eq!(&exact, b"kernel");

        let mut short = FakeFile {
            data: b"short",
            cursor: 0,
            fail_seek: false,
            fail_rewind: false,
            max_read: usize::MAX,
        };
        assert_eq!(
            read_exact_and_check_eof(&mut short, &mut [0_u8; 6]),
            Err(ExactReadError::ShortRead)
        );

        let mut extra = FakeFile {
            data: b"extra",
            cursor: 0,
            fail_seek: false,
            fail_rewind: false,
            max_read: usize::MAX,
        };
        assert_eq!(
            read_exact_and_check_eof(&mut extra, &mut [0_u8; 4]),
            Err(ExactReadError::ExtraData)
        );
    }
}
