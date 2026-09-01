use core::ptr::NonNull;

#[cfg(all(feature = "qemu-test", not(feature = "exit-boot-services-test")))]
use bootloader::prove_page_released;
#[cfg(feature = "qemu-test")]
use bootloader::{AddressProbe, prove_page_owned};
use bootloader::{
    LoadItem, LoadPlan, LoadStage, LoadWorkFailure, LoadWorkKind, LoadedKernel, MemoryError,
    PageBackend, SegmentBackend, copy_target, prepare_targets, verify_target,
};
use kernel_image::validate_bootstrap_image;
use uefi::boot::{self, AllocateType, MemoryType};

use crate::boot_info_loader::{self, BootInfoLoadError};
use crate::kernel_loader::ValidatedKernel;
use crate::serial::SerialPort;

const PLAN_MARKER: &[u8] = b"UNOS:P1E:PLAN_VALID";
const ALLOCATED_MARKER: &[u8] = b"UNOS:P1E:SEGMENTS_ALLOCATED";
const ZEROED_MARKER: &[u8] = b"UNOS:P1E:SEGMENTS_ZEROED";
const COPIED_MARKER: &[u8] = b"UNOS:P1E:SEGMENTS_COPIED";
const VERIFIED_MARKER: &[u8] = b"UNOS:P1E:LOAD_VERIFIED";
#[cfg(not(feature = "exit-boot-services-test"))]
const RELEASED_MARKER: &[u8] = b"UNOS:P1E:MEMORY_RELEASED";
#[cfg(not(feature = "exit-boot-services-test"))]
const PHASE_1E_PASS_MARKER: &[u8] = b"UNOS:P1E:PASS";
const OWNERSHIP_READY_MARKER: &[u8] = b"UNOS:P1F:OWNERSHIP_READY";
const METADATA_VALID_MARKER: &[u8] = b"UNOS:P1F:METADATA_VALID";
const SOURCE_RELEASED_MARKER: &[u8] = b"UNOS:P1F:SOURCE_RELEASED";
const OWNERSHIP_PROVEN_MARKER: &[u8] = b"UNOS:P1F:OWNERSHIP_PROVEN";
#[cfg(not(feature = "exit-boot-services-test"))]
const PHASE_1F_MEMORY_RELEASED_MARKER: &[u8] = b"UNOS:P1F:MEMORY_RELEASED";
#[cfg(not(feature = "exit-boot-services-test"))]
const RELEASE_PROVEN_MARKER: &[u8] = b"UNOS:P1F:RELEASE_PROVEN";
#[cfg(not(feature = "exit-boot-services-test"))]
const PHASE_1F_PASS_MARKER: &[u8] = b"UNOS:P1F:PASS";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadError {
    Plan,
    SourceOverlap,
    Alloc,
    Zero,
    Copy,
    Verify,
    Free,
    Ownership,
    Metadata,
    SourceFree,
    #[cfg(feature = "qemu-test")]
    OwnershipProbe,
    #[cfg(not(feature = "exit-boot-services-test"))]
    Release,
    #[cfg(all(feature = "qemu-test", not(feature = "exit-boot-services-test")))]
    ReleaseProbe,
    BootInfo(BootInfoLoadError),
}

impl LoadError {
    pub const fn marker(self) -> &'static [u8] {
        match self {
            Self::Plan => b"UNOS:P1E:FAIL:PLAN",
            Self::SourceOverlap => b"UNOS:P1E:FAIL:SOURCE_OVERLAP",
            Self::Alloc => b"UNOS:P1E:FAIL:ALLOC",
            Self::Zero => b"UNOS:P1E:FAIL:ZERO",
            Self::Copy => b"UNOS:P1E:FAIL:COPY",
            Self::Verify => b"UNOS:P1E:FAIL:VERIFY",
            Self::Free => b"UNOS:P1E:FAIL:FREE",
            Self::Ownership => b"UNOS:P1F:FAIL:OWNERSHIP",
            Self::Metadata => b"UNOS:P1F:FAIL:METADATA",
            Self::SourceFree => b"UNOS:P1F:FAIL:SOURCE_FREE",
            #[cfg(feature = "qemu-test")]
            Self::OwnershipProbe => b"UNOS:P1F:FAIL:OWNERSHIP_PROBE",
            #[cfg(not(feature = "exit-boot-services-test"))]
            Self::Release => b"UNOS:P1F:FAIL:RELEASE",
            #[cfg(all(feature = "qemu-test", not(feature = "exit-boot-services-test")))]
            Self::ReleaseProbe => b"UNOS:P1F:FAIL:RELEASE_PROBE",
            Self::BootInfo(error) => error.marker(),
        }
    }
}

pub fn load_verify_and_release(
    kernel: ValidatedKernel,
    serial: &mut SerialPort,
) -> Result<(), LoadError> {
    let image = match validate_bootstrap_image(kernel.bytes()) {
        Ok(image) => image,
        Err(_) => return release_source_after_error(kernel, LoadError::Plan),
    };
    let plan = match LoadPlan::from_validated(&image, kernel.source_allocation()) {
        Ok(plan) => plan,
        Err(bootloader::PlanError::SourceOverlap) => {
            return release_source_after_error(kernel, LoadError::SourceOverlap);
        }
        Err(_) => return release_source_after_error(kernel, LoadError::Plan),
    };
    serial.write_line(PLAN_MARKER);

    let source_fingerprint = fingerprint(kernel.bytes());
    let prepared = prepare_targets(
        &plan,
        kernel.bytes(),
        UefiTargetBackend,
        |stage| match stage {
            LoadStage::Allocated => serial.write_line(ALLOCATED_MARKER),
            LoadStage::Zeroed => serial.write_line(ZEROED_MARKER),
            LoadStage::Copied => serial.write_line(COPIED_MARKER),
            LoadStage::Verified => {}
        },
    );
    let mut verified = match prepared {
        Ok(verified) => verified,
        Err(mut failure) => {
            let primary = map_work_error(&failure);
            let targets_released = failure.try_release().is_ok();
            let source_released = kernel.release().is_ok();
            return if targets_released && source_released {
                Err(primary)
            } else {
                Err(LoadError::Free)
            };
        }
    };

    let source_is_unchanged = fingerprint(kernel.bytes()) == source_fingerprint
        && validate_bootstrap_image(kernel.bytes())
            .map(|image| {
                image.entry() == plan.entry()
                    && plan.items().any(|item| {
                        item.is_executable()
                            && image.entry() >= item.target_start
                            && image.entry() < item.target_end
                    })
            })
            .unwrap_or(false);
    if !source_is_unchanged {
        let Some(mut loaded) = verified.into_loaded_kernel(&plan) else {
            return Err(LoadError::Ownership);
        };
        let targets_released = loaded.try_release().is_ok();
        let source_released = kernel.release().is_ok();
        return if targets_released && source_released {
            Err(LoadError::Verify)
        } else {
            Err(LoadError::Free)
        };
    }
    serial.write_line(VERIFIED_MARKER);

    let Some(mut loaded) = verified.into_loaded_kernel(&plan) else {
        return Err(LoadError::Ownership);
    };
    if !verified.is_empty() || loaded.is_released() {
        return Err(LoadError::Ownership);
    }
    serial.write_line(OWNERSHIP_READY_MARKER);

    #[cfg(feature = "qemu-test")]
    let first_page = loaded.load_range().start;
    if !metadata_matches_plan(&loaded, &plan) {
        return Err(LoadError::Metadata);
    }
    serial.write_line(METADATA_VALID_MARKER);

    if kernel.release().is_err() {
        let _ = loaded.try_release();
        return Err(LoadError::SourceFree);
    }
    serial.write_line(SOURCE_RELEASED_MARKER);

    #[cfg(feature = "qemu-test")]
    if !reference_metadata_matches(&loaded) {
        return Err(LoadError::Metadata);
    }
    #[cfg(feature = "qemu-test")]
    if !ownership_probe(first_page) {
        return Err(LoadError::OwnershipProbe);
    }
    serial.write_line(OWNERSHIP_PROVEN_MARKER);

    #[cfg(feature = "exit-boot-services-test")]
    {
        match boot_info_loader::prepare_and_exit(loaded, serial) {
            Ok(never) => match never {},
            Err(error) => Err(LoadError::BootInfo(error)),
        }
    }

    #[cfg(not(feature = "exit-boot-services-test"))]
    {
        boot_info_loader::prepare_validate_and_release(&loaded, serial)
            .map_err(LoadError::BootInfo)?;

        loaded.try_release().map_err(|_| LoadError::Release)?;
        serial.write_line(RELEASED_MARKER);
        serial.write_line(PHASE_1E_PASS_MARKER);
        serial.write_line(PHASE_1F_MEMORY_RELEASED_MARKER);

        #[cfg(feature = "qemu-test")]
        if !release_probe(first_page) {
            return Err(LoadError::ReleaseProbe);
        }
        if !loaded.is_released() {
            return Err(LoadError::Release);
        }
        serial.write_line(RELEASE_PROVEN_MARKER);
        serial.write_line(PHASE_1F_PASS_MARKER);
        Ok(())
    }
}

fn release_source_after_error(
    kernel: ValidatedKernel,
    primary: LoadError,
) -> Result<(), LoadError> {
    match kernel.release() {
        Ok(()) => Err(primary),
        Err(_) => Err(LoadError::Free),
    }
}

fn map_work_error(error: &LoadWorkFailure<UefiTargetBackend>) -> LoadError {
    if *error.error() == BackendError::Free {
        return LoadError::Free;
    }
    match error.kind() {
        LoadWorkKind::Allocate => LoadError::Alloc,
        LoadWorkKind::Zero => LoadError::Zero,
        LoadWorkKind::Copy => LoadError::Copy,
        LoadWorkKind::Verify => LoadError::Verify,
    }
}

fn metadata_matches_plan(loaded: &LoadedKernel<UefiTargetBackend>, plan: &LoadPlan) -> bool {
    let range = loaded.load_range();
    let first = plan.items().map(|item| item.page_start).min();
    let last = plan
        .items()
        .filter_map(|item| item.page_start.checked_add(item.page_length()))
        .max();
    loaded.entry_point() == plan.entry()
        && loaded.segment_count() == plan.len()
        && loaded.owned_page_count().checked_mul(4096) == Some(plan.total_page_bytes())
        && Some(range.start) == first
        && Some(range.end) == last
        && loaded
            .segment_metadata()
            .zip(plan.items())
            .all(|(metadata, item)| {
                metadata.memory_start == item.target_start
                    && metadata.memory_end == item.target_end
                    && metadata.allocation_start == item.page_start
                    && metadata.page_count == item.page_count
                    && metadata.flags == item.flags
                    && metadata.file_size == item.file_size
                    && metadata.memory_size == item.memory_size
            })
}

#[cfg(feature = "qemu-test")]
fn reference_metadata_matches(loaded: &LoadedKernel<UefiTargetBackend>) -> bool {
    loaded.entry_point() == 0x0020_0000
        && loaded.load_range().start == 0x0020_0000
        && loaded.load_range().end == 0x0020_7000
        && loaded.segment_count() == 3
        && loaded.owned_page_count() == 7
        && loaded.executable_entry_segment_index() == 0
}

#[cfg(feature = "qemu-test")]
fn ownership_probe(page_start: u64) -> bool {
    prove_page_owned(&mut UefiAddressProbe, page_start).is_ok()
}

#[cfg(all(feature = "qemu-test", not(feature = "exit-boot-services-test")))]
fn release_probe(page_start: u64) -> bool {
    prove_page_released(&mut UefiAddressProbe, page_start).is_ok()
}

#[cfg(feature = "qemu-test")]
struct UefiAddressProbe;

#[cfg(feature = "qemu-test")]
impl AddressProbe for UefiAddressProbe {
    type Error = BackendError;

    fn allocate_one_at(&mut self, page_start: u64) -> Result<u64, Self::Error> {
        boot::allocate_pages(
            AllocateType::Address(page_start),
            MemoryType::LOADER_DATA,
            1,
        )
        .map(|pointer| pointer.as_ptr() as u64)
        .map_err(|_| BackendError::Firmware)
    }

    fn free_one(&mut self, page_start: u64) -> Result<(), Self::Error> {
        let pointer = NonNull::new(page_start as *mut u8).ok_or(BackendError::Range)?;
        // SAFETY: the probe releases only the exact one-page allocation it
        // received from `allocate_one_at`, without constructing a reference.
        unsafe { boot::free_pages(pointer, 1) }.map_err(|_| BackendError::Free)
    }
}

fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackendError {
    Firmware,
    Free,
    AddressMismatch,
    Range,
    Memory(MemoryError),
}

struct UefiTargetBackend;

impl PageBackend for UefiTargetBackend {
    type Error = BackendError;

    fn allocate_at(&mut self, page_start: u64, page_count: u64) -> Result<(), Self::Error> {
        let count = usize::try_from(page_count).map_err(|_| BackendError::Range)?;
        let pointer = boot::allocate_pages(
            AllocateType::Address(page_start),
            MemoryType::LOADER_DATA,
            count,
        )
        .map_err(|_| BackendError::Firmware)?;
        if pointer.as_ptr() as u64 != page_start {
            // SAFETY: `pointer` and `count` are the allocation just returned by
            // UEFI. No reference was created, so immediate release is valid.
            // A failed immediate release is a cleanup failure, not an
            // allocation-policy failure, and must remain observable.
            unsafe { boot::free_pages(pointer, count) }.map_err(|_| BackendError::Free)?;
            return Err(BackendError::AddressMismatch);
        }
        Ok(())
    }

    fn free(&mut self, page_start: u64, page_count: u64) -> Result<(), Self::Error> {
        let count = usize::try_from(page_count).map_err(|_| BackendError::Range)?;
        let pointer = NonNull::new(page_start as *mut u8).ok_or(BackendError::Range)?;
        // SAFETY: `TargetOwnership` calls this once, in reverse order, for the
        // exact fixed-address allocation created by `allocate_at`. No target
        // slice is live across this call.
        unsafe { boot::free_pages(pointer, count) }.map_err(|_| BackendError::Free)
    }
}

impl SegmentBackend for UefiTargetBackend {
    fn zero(&mut self, item: LoadItem) -> Result<(), Self::Error> {
        self.target_mut(item)?.fill(0);
        Ok(())
    }

    fn copy(&mut self, item: LoadItem, source: &[u8]) -> Result<(), Self::Error> {
        copy_target(item, source, self.target_mut(item)?).map_err(BackendError::Memory)
    }

    fn verify(&mut self, item: LoadItem, source: &[u8]) -> Result<(), Self::Error> {
        verify_target(item, source, self.target(item)?).map_err(BackendError::Memory)
    }
}

impl UefiTargetBackend {
    fn target_mut(&mut self, item: LoadItem) -> Result<&mut [u8], BackendError> {
        let length = usize::try_from(item.page_length()).map_err(|_| BackendError::Range)?;
        // SAFETY: `TargetOwnership` owns the exact fixed-address UEFI pages for
        // this item. The plan proves the length, non-overlap with the source and
        // other items, and window bounds. The temporary slice is exclusive and
        // cannot outlive this backend call or the allocation.
        Ok(unsafe { core::slice::from_raw_parts_mut(item.page_start as *mut u8, length) })
    }

    fn target(&self, item: LoadItem) -> Result<&[u8], BackendError> {
        let length = usize::try_from(item.page_length()).map_err(|_| BackendError::Range)?;
        // SAFETY: the corresponding fixed-address pages remain owned, were
        // initialized before this call, do not alias the source, and the shared
        // slice ends before any release or subsequent mutable access.
        Ok(unsafe { core::slice::from_raw_parts(item.page_start as *const u8, length) })
    }
}
