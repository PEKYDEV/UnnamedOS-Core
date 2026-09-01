use core::ptr::NonNull;

use bootloader::{
    ByteReader, ExactReadError, PageLeaseState, PositionedFile, SourceAllocation, acquire_with,
    finalize_with_release, measure_and_rewind, pages_for_bytes, read_exact_and_check_eof,
};
use kernel_image::validate_bootstrap_image;
use uefi::boot::{self, AllocateType, MemoryType};
use uefi::proto::media::file::{File, FileAttribute, FileMode, FileType, RegularFile};

use crate::serial::SerialPort;

const KERNEL_PATH: &uefi::CStr16 = uefi::cstr16!(r"\EFI\UNNAMEDOS\KERNEL.ELF");
const OPEN_MARKER: &[u8] = b"UNOS:P1D:KERNEL_OPEN";
const READ_MARKER: &[u8] = b"UNOS:P1D:KERNEL_READ";
const VALID_MARKER: &[u8] = b"UNOS:P1D:KERNEL_VALID";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadError {
    Open,
    Size,
    Alloc,
    Read,
    ShortRead,
    Elf,
    Free,
}

impl LoadError {
    pub const fn marker(self) -> &'static [u8] {
        match self {
            Self::Open => b"UNOS:P1D:FAIL:OPEN",
            Self::Size => b"UNOS:P1D:FAIL:SIZE",
            Self::Alloc => b"UNOS:P1D:FAIL:ALLOC",
            Self::Read => b"UNOS:P1D:FAIL:READ",
            Self::ShortRead => b"UNOS:P1D:FAIL:SHORT_READ",
            Self::Elf => b"UNOS:P1D:FAIL:ELF",
            Self::Free => b"UNOS:P1D:FAIL:FREE",
        }
    }
}

pub struct ValidatedKernel {
    pub entry: u64,
    pub load_segment_count: u16,
    scratch: ScratchPages,
}

impl ValidatedKernel {
    pub fn bytes(&self) -> &[u8] {
        self.scratch.bytes()
    }

    pub fn source_allocation(&self) -> SourceAllocation {
        SourceAllocation {
            page_start: self.scratch.address(),
            page_count: self.scratch.page_count as u64,
            file_length: self.scratch.byte_len as u64,
        }
    }

    pub fn release(self) -> Result<(), LoadError> {
        self.scratch.release()
    }
}

pub fn load_and_validate(serial: &mut SerialPort) -> Result<ValidatedKernel, LoadError> {
    let mut file_system =
        boot::get_image_file_system(boot::image_handle()).map_err(|_| LoadError::Open)?;
    let mut root = file_system.open_volume().map_err(|_| LoadError::Open)?;
    let handle = root
        .open(KERNEL_PATH, FileMode::Read, FileAttribute::empty())
        .map_err(|_| LoadError::Open)?;
    let mut file = match handle.into_type().map_err(|_| LoadError::Open)? {
        FileType::Regular(file) => file,
        FileType::Dir(_) => return Err(LoadError::Open),
    };
    serial.write_line(OPEN_MARKER);

    let byte_len = file_length_and_rewind(&mut file)?;
    let mut pages = ScratchPages::allocate(byte_len)?;

    match read_and_validate(&mut file, pages.bytes_mut(), serial) {
        Ok((entry, load_segment_count)) => Ok(ValidatedKernel {
            entry,
            load_segment_count,
            scratch: pages,
        }),
        Err(error) => finalize_with_release(Err(error), pages.release()),
    }
}

fn file_length_and_rewind(file: &mut RegularFile) -> Result<usize, LoadError> {
    measure_and_rewind(&mut UefiFile(file)).map_err(|_| LoadError::Size)
}

fn read_and_validate(
    file: &mut RegularFile,
    buffer: &mut [u8],
    serial: &mut SerialPort,
) -> Result<(u64, u16), LoadError> {
    read_exact_and_check_eof(&mut UefiFile(file), buffer).map_err(|error| match error {
        ExactReadError::Source(_) => LoadError::Read,
        ExactReadError::ShortRead => LoadError::ShortRead,
        ExactReadError::ExtraData => LoadError::Size,
    })?;
    serial.write_line(READ_MARKER);

    let image = validate_bootstrap_image(buffer).map_err(|_| LoadError::Elf)?;
    let metadata = (image.entry(), image.load_segment_count());
    serial.write_line(VALID_MARKER);
    Ok(metadata)
}

struct UefiFile<'a>(&'a mut RegularFile);

impl PositionedFile for UefiFile<'_> {
    type Error = uefi::Error;

    fn seek_to_end(&mut self) -> Result<(), Self::Error> {
        self.0.set_position(RegularFile::END_OF_FILE)
    }

    fn position(&mut self) -> Result<u64, Self::Error> {
        self.0.get_position()
    }

    fn rewind(&mut self) -> Result<(), Self::Error> {
        self.0.set_position(0)
    }
}

impl ByteReader for UefiFile<'_> {
    type Error = uefi::Error;

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
        self.0.read(buffer)
    }
}

struct ScratchPages {
    pointer: Option<NonNull<u8>>,
    page_count: usize,
    byte_len: usize,
    lease: PageLeaseState,
}

impl ScratchPages {
    fn allocate(byte_len: usize) -> Result<Self, LoadError> {
        let page_count = pages_for_bytes(byte_len).map_err(|_| LoadError::Size)?;
        let (pointer, lease) = acquire_with(|| {
            boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, page_count)
        })
        .map_err(|_| LoadError::Alloc)?;

        // SAFETY: `pointer` names `page_count * 4096` writable bytes returned
        // by UEFI. The checked page calculation guarantees at least `byte_len`
        // bytes. Initializing that range makes the subsequent byte slice valid.
        unsafe {
            pointer.as_ptr().write_bytes(0, byte_len);
        }
        Ok(Self {
            pointer: Some(pointer),
            page_count,
            byte_len,
            lease,
        })
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        let pointer = self.pointer.expect("live scratch allocation");
        // SAFETY: allocation and initialization are established in `allocate`;
        // `&mut self` provides exclusive access, and release cannot occur while
        // the returned slice is borrowed.
        unsafe { core::slice::from_raw_parts_mut(pointer.as_ptr(), self.byte_len) }
    }

    fn bytes(&self) -> &[u8] {
        let pointer = self.pointer.expect("live scratch allocation");
        // SAFETY: `allocate` initialized the file-length range, the UEFI read
        // filled it, and `&self` prevents mutation or release during the borrow.
        unsafe { core::slice::from_raw_parts(pointer.as_ptr(), self.byte_len) }
    }

    fn address(&self) -> u64 {
        self.pointer.expect("live scratch allocation").as_ptr() as u64
    }

    fn release(mut self) -> Result<(), LoadError> {
        self.lease.begin_release().map_err(|_| LoadError::Free)?;
        let pointer = self.pointer.take().ok_or(LoadError::Free)?;
        // SAFETY: this exact pointer and page count came from `allocate_pages`;
        // the exclusive `self` value ensures no slice into it remains live.
        unsafe { boot::free_pages(pointer, self.page_count) }.map_err(|_| LoadError::Free)
    }
}

impl Drop for ScratchPages {
    fn drop(&mut self) {
        if let Some(pointer) = self.pointer.take() {
            if self.lease.begin_release().is_err() {
                return;
            }
            // SAFETY: this is the same still-owned allocation. Taking the
            // option first prevents a second free even when firmware fails.
            let _ = unsafe { boot::free_pages(pointer, self.page_count) };
        }
    }
}
