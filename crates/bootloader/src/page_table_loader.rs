use core::ptr::NonNull;

use bootloader::{
    BootstrapStack, LoadedKernel, PageBackend, PageTableMemory, PlannedPageTables,
    VerifiedInactivePageTables,
};
use memory_layout::{FrameBackend, PhysicalFrame, TableIndex};
use uefi::boot::{self, AllocateType, MemoryType};

use crate::serial::SerialPort;

pub(crate) const PLAN_ACCEPTED_MARKER: &[u8] = b"UNOS:P1J:PLAN_ACCEPTED";
pub(crate) const FRAMES_ALLOCATED_MARKER: &[u8] = b"UNOS:P1J:FRAMES_ALLOCATED";
pub(crate) const MATERIALIZED_MARKER: &[u8] = b"UNOS:P1J:HIERARCHY_MATERIALIZED";
pub(crate) const VERIFIED_MARKER: &[u8] = b"UNOS:P1J:HIERARCHY_VERIFIED";
pub(crate) const FINAL_MAP_RESERVED_MARKER: &[u8] = b"UNOS:P1J:FINAL_MAP_RESERVED";
pub(crate) const OWNERSHIP_TRANSFERRED_MARKER: &[u8] = b"UNOS:P1J:OWNERSHIP_TRANSFERRED";
#[cfg(feature = "page-table-allocation-failure-test")]
const ROLLBACK_MARKER: &[u8] = b"UNOS:P1J:ROLLBACK_COMPLETE";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PageTableLoadError {
    Plan,
    Allocate,
    Materialize,
    Verify,
    Reserve,
}

impl PageTableLoadError {
    pub(crate) const fn marker(self) -> &'static [u8] {
        match self {
            Self::Plan => b"UNOS:P1J:FAIL:PLAN",
            Self::Allocate => b"UNOS:P1J:FAIL:ALLOC",
            Self::Materialize => b"UNOS:P1J:FAIL:MATERIALIZE",
            Self::Verify => b"UNOS:P1J:FAIL:VERIFY",
            Self::Reserve => b"UNOS:P1J:FAIL:RESERVATION",
        }
    }
}

pub(crate) fn construct_inactive<KB: PageBackend, SB: PageBackend>(
    loaded: &LoadedKernel<KB>,
    stack: &BootstrapStack<SB>,
    serial: &mut SerialPort,
) -> Result<VerifiedInactivePageTables<UefiPageTableBackend>, PageTableLoadError> {
    let trampoline_page = loaded.entry_point() & !(bootloader::UEFI_PAGE_SIZE - 1);
    let planned = PlannedPageTables::for_transition(trampoline_page, stack.bottom())
        .map_err(|_| PageTableLoadError::Plan)?;
    serial.write_line(PLAN_ACCEPTED_MARKER);

    let backend = UefiPageTableBackend::new();
    let allocated = match planned.allocate(backend) {
        Ok(allocated) => allocated,
        Err(_error) => {
            #[cfg(feature = "page-table-allocation-failure-test")]
            if _error.remaining_frames() == 0 {
                serial.write_line(ROLLBACK_MARKER);
            }
            return Err(PageTableLoadError::Allocate);
        }
    };
    serial.write_line(FRAMES_ALLOCATED_MARKER);

    let mut memory = UefiPageTableMemory;
    let materialized = allocated
        .materialize(&mut memory)
        .map_err(|_| PageTableLoadError::Materialize)?;
    serial.write_line(MATERIALIZED_MARKER);
    let verified = materialized
        .verify(&memory)
        .map_err(|_| PageTableLoadError::Verify)?;
    serial.write_line(VERIFIED_MARKER);
    Ok(verified)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UefiPageTableError {
    Firmware,
    Range,
    #[cfg(feature = "page-table-allocation-failure-test")]
    InjectedAllocation,
}

pub(crate) struct UefiPageTableBackend {
    allocation_attempt: usize,
}

impl UefiPageTableBackend {
    const fn new() -> Self {
        Self {
            allocation_attempt: 0,
        }
    }
}

impl FrameBackend for UefiPageTableBackend {
    type Error = UefiPageTableError;

    fn allocate_frame(&mut self) -> Result<u64, Self::Error> {
        #[cfg(feature = "page-table-allocation-failure-test")]
        if self.allocation_attempt == 2 {
            return Err(UefiPageTableError::InjectedAllocation);
        }
        let pointer = boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)
            .map_err(|_| UefiPageTableError::Firmware)?;
        self.allocation_attempt += 1;
        Ok(pointer.as_ptr() as u64)
    }

    fn zero_frame(&mut self, frame: PhysicalFrame) -> Result<(), Self::Error> {
        let pointer = NonNull::new(frame.address() as *mut u8).ok_or(UefiPageTableError::Range)?;
        // SAFETY: `frame` came from this backend's one-page LOADER_DATA
        // allocation and was validated as nonzero, 4 KiB aligned, unique, and
        // below the physical cap before this call. The exact 4096-byte slice is
        // exclusively owned by the incomplete frame owner, does not overlap
        // another owned frame, lives through this call, and belongs to an
        // inactive hierarchy that firmware and the CPU do not reference.
        unsafe { core::slice::from_raw_parts_mut(pointer.as_ptr(), 4096) }.fill(0);
        Ok(())
    }

    fn free_frame(&mut self, address: u64) -> Result<(), Self::Error> {
        let pointer = NonNull::new(address as *mut u8).ok_or(UefiPageTableError::Range)?;
        // SAFETY: the owner calls this for exactly one still-owned one-page
        // LOADER_DATA allocation. No table slice survives the backend call,
        // rollback is reverse ordered, and ownership is removed only after the
        // firmware accepts the free.
        unsafe { boot::free_pages(pointer, 1) }.map_err(|_| UefiPageTableError::Firmware)
    }
}

struct UefiPageTableMemory;

impl UefiPageTableMemory {
    fn entry_pointer(
        frame: PhysicalFrame,
        index: TableIndex,
    ) -> Result<NonNull<u64>, UefiPageTableError> {
        let offset = usize::from(index.get())
            .checked_mul(core::mem::size_of::<u64>())
            .ok_or(UefiPageTableError::Range)?;
        let end = offset
            .checked_add(core::mem::size_of::<u64>())
            .ok_or(UefiPageTableError::Range)?;
        if end > 4096 {
            return Err(UefiPageTableError::Range);
        }
        let address = frame
            .address()
            .checked_add(u64::try_from(offset).map_err(|_| UefiPageTableError::Range)?)
            .ok_or(UefiPageTableError::Range)?;
        NonNull::new(address as *mut u64).ok_or(UefiPageTableError::Range)
    }
}

impl PageTableMemory for UefiPageTableMemory {
    type Error = UefiPageTableError;

    fn write_entry(
        &mut self,
        frame: PhysicalFrame,
        index: TableIndex,
        value: u64,
    ) -> Result<(), Self::Error> {
        let pointer = Self::entry_pointer(frame, index)?;
        // SAFETY: the caller supplies only a frame retained exclusively by the
        // allocated hierarchy owner. Provenance is a one-page LOADER_DATA
        // allocation; the checked index selects one aligned u64 wholly inside
        // its exact 4096-byte bounds. All frames are unique and non-overlapping,
        // the temporary access ends before release, and the hierarchy is
        // inactive, so neither firmware nor page-table walking aliases it.
        unsafe { pointer.as_ptr().write(value.to_le()) };
        Ok(())
    }

    fn read_entry(&self, frame: PhysicalFrame, index: TableIndex) -> Result<u64, Self::Error> {
        let pointer = Self::entry_pointer(frame, index)?;
        // SAFETY: allocation provenance, 4 KiB bounds, u64 alignment,
        // ownership, lifetime, non-overlap, and inactive-state invariants are
        // identical to `write_entry`; this access copies one initialized u64
        // and retains no reference.
        Ok(unsafe { pointer.as_ptr().read() }.to_le())
    }
}
