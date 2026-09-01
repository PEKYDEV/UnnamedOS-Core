use core::{convert::Infallible, mem::ManuallyDrop};

use boot_protocol::{
    BOOT_INFO_SIZE, BootInfo, FramebufferInfo, MEMORY_DESCRIPTOR_SIZE, MemoryDescriptor,
};

use crate::{
    BootDataAllocations, BootstrapStack, LoadRange, LoadedKernel, MAX_LOAD_ITEMS, PageBackend,
    PreparedBootInfo, SegmentMetadata, TransferredBootstrapStack,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitPreparationError {
    KernelReleased,
    BootInformationReleased,
    EmptyMemoryMap,
    InvalidBootInformation,
}

pub const EXIT_BOOT_SERVICES_ATTEMPT_LIMIT: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExitRetryPolicy {
    attempts: u8,
}

impl ExitRetryPolicy {
    pub const fn new() -> Self {
        Self { attempts: 0 }
    }

    pub fn begin_attempt(&mut self) -> bool {
        if self.attempts == EXIT_BOOT_SERVICES_ATTEMPT_LIMIT {
            return false;
        }
        self.attempts += 1;
        true
    }

    pub const fn retry_invalid_key(&self, invalid_key: bool) -> bool {
        invalid_key && self.attempts < EXIT_BOOT_SERVICES_ATTEMPT_LIMIT
    }

    pub const fn attempts(&self) -> u8 {
        self.attempts
    }
}

impl Default for ExitRetryPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Initial typestate while firmware boot services are still available.
pub struct BootServicesState;

/// Typestate proving that the validated kernel ownership is present.
pub struct KernelOwned<B: PageBackend> {
    kernel: LoadedKernel<B>,
}

/// Complete pre-exit state. Dropping this value before transfer rolls back both
/// owners through their normal release safety nets.
#[must_use]
pub struct ExitReady<KB: PageBackend, BB: PageBackend> {
    kernel: Option<LoadedKernel<KB>>,
    boot_info: Option<PreparedBootInfo<BB>>,
}

/// Complete handoff state. It cannot be constructed without all three live
/// owners, and pre-exit Drop still rolls each allocation back.
#[must_use]
pub struct HandoffReady<KB: PageBackend, BB: PageBackend, SB: PageBackend> {
    exit: ExitReady<KB, BB>,
    stack: Option<BootstrapStack<SB>>,
}

impl BootServicesState {
    pub const fn new() -> Self {
        Self
    }

    pub fn with_kernel<B: PageBackend>(
        self,
        kernel: LoadedKernel<B>,
    ) -> Result<KernelOwned<B>, ExitPreparationError> {
        if kernel.is_released() || kernel.segment_count() == 0 {
            return Err(ExitPreparationError::KernelReleased);
        }
        Ok(KernelOwned { kernel })
    }
}

impl Default for BootServicesState {
    fn default() -> Self {
        Self::new()
    }
}

impl<KB: PageBackend> KernelOwned<KB> {
    pub fn with_boot_information<BB: PageBackend>(
        self,
        boot_info: PreparedBootInfo<BB>,
    ) -> Result<ExitReady<KB, BB>, ExitPreparationError> {
        if boot_info.is_released() {
            return Err(ExitPreparationError::BootInformationReleased);
        }
        if boot_info.descriptor_count() == 0 {
            return Err(ExitPreparationError::EmptyMemoryMap);
        }
        if boot_info.wire_size() != BOOT_INFO_SIZE
            || boot_info.descriptor_stride() != MEMORY_DESCRIPTOR_SIZE
        {
            return Err(ExitPreparationError::InvalidBootInformation);
        }
        Ok(ExitReady {
            kernel: Some(self.kernel),
            boot_info: Some(boot_info),
        })
    }
}

impl<KB: PageBackend, BB: PageBackend> ExitReady<KB, BB> {
    pub fn boot_allocations(&self) -> BootDataAllocations {
        self.boot_info
            .as_ref()
            .expect("ExitReady always owns boot information")
            .allocations()
    }

    pub fn with_bootstrap_stack<SB: PageBackend>(
        self,
        stack: BootstrapStack<SB>,
    ) -> Result<HandoffReady<KB, BB, SB>, ExitPreparationError> {
        if stack.is_released() {
            return Err(ExitPreparationError::BootInformationReleased);
        }
        Ok(HandoffReady {
            exit: self,
            stack: Some(stack),
        })
    }

    /// Performs the small ownership-disarm immediately before the caller's
    /// non-returning atomic exit operation. The disarm itself is private; the
    /// callback receives only copied scalar metadata and cannot free firmware
    /// allocations.
    pub fn cross_exit_boundary(
        self,
        operation: impl FnOnce(TransferredBootState) -> Infallible,
    ) -> ! {
        self.cross_exit_boundary_with_stack(None, operation)
    }

    fn cross_exit_boundary_with_stack(
        mut self,
        bootstrap_stack: Option<TransferredBootstrapStack>,
        operation: impl FnOnce(TransferredBootState) -> Infallible,
    ) -> ! {
        let kernel = ManuallyDrop::new(self.kernel.take().expect("ExitReady always owns a kernel"));
        let boot_info = ManuallyDrop::new(
            self.boot_info
                .take()
                .expect("ExitReady always owns boot information"),
        );

        let mut segments = [empty_segment(); MAX_LOAD_ITEMS];
        for (index, segment) in kernel.segment_metadata().enumerate() {
            segments[index] = segment;
        }
        let state = TransferredBootState {
            kernel_entry: kernel.entry_point(),
            kernel_range: kernel.load_range(),
            segments,
            segment_count: kernel.segment_count(),
            allocations: boot_info.allocations(),
            framebuffer: boot_info.framebuffer(),
            boot_info_address: boot_info.boot_info_physical_address(),
            bootstrap_stack,
        };
        match operation(state) {}
    }
}

impl<KB: PageBackend, BB: PageBackend, SB: PageBackend> HandoffReady<KB, BB, SB> {
    pub fn boot_allocations(&self) -> BootDataAllocations {
        self.exit.boot_allocations()
    }
    pub fn stack_allocation(&self) -> crate::PageAllocation {
        self.stack
            .as_ref()
            .expect("HandoffReady owns stack")
            .allocation()
    }
    pub fn cross_exit_boundary(
        mut self,
        operation: impl FnOnce(TransferredBootState) -> Infallible,
    ) -> ! {
        let stack = self
            .stack
            .take()
            .expect("HandoffReady owns stack")
            .transfer();
        self.exit
            .cross_exit_boundary_with_stack(Some(stack), operation)
    }
}

const fn empty_segment() -> SegmentMetadata {
    SegmentMetadata {
        memory_start: 0,
        memory_end: 0,
        allocation_start: 0,
        allocation_end: 0,
        page_count: 0,
        flags: 0,
        file_size: 0,
        memory_size: 0,
    }
}

/// Raw, allocator-free ownership record valid after boot services are gone.
/// It intentionally has no `Drop` implementation and contains no backend,
/// protocol guard, reference, or firmware-owned Rust object.
#[must_use]
pub struct TransferredBootState {
    kernel_entry: u64,
    kernel_range: LoadRange,
    segments: [SegmentMetadata; MAX_LOAD_ITEMS],
    segment_count: usize,
    allocations: BootDataAllocations,
    framebuffer: FramebufferInfo,
    boot_info_address: u64,
    bootstrap_stack: Option<TransferredBootstrapStack>,
}

impl TransferredBootState {
    pub const fn kernel_entry(&self) -> u64 {
        self.kernel_entry
    }
    pub const fn kernel_range(&self) -> LoadRange {
        self.kernel_range
    }
    pub fn kernel_segments(&self) -> impl Iterator<Item = SegmentMetadata> + '_ {
        self.segments[..self.segment_count].iter().copied()
    }
    pub const fn boot_allocations(&self) -> BootDataAllocations {
        self.allocations
    }
    pub const fn framebuffer(&self) -> FramebufferInfo {
        self.framebuffer
    }
    pub const fn boot_info_address(&self) -> u64 {
        self.boot_info_address
    }
    pub const fn bootstrap_stack(&self) -> Option<TransferredBootstrapStack> {
        self.bootstrap_stack
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FinalMapMetadata {
    pub map_size: u64,
    pub descriptor_stride: u64,
    pub descriptor_version: u32,
    pub map_key: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostExitError {
    InvalidBootInformation,
    InvalidDescriptors,
    WrongBootInformationAddress,
}

/// Fully validated post-UEFI state. It owns no Rust value whose destructor can
/// call firmware; all retained data is scalar or fixed-capacity copied metadata.
#[must_use]
pub struct PostExitState {
    transferred: TransferredBootState,
    boot_info: BootInfo,
    final_map: FinalMapMetadata,
}

impl PostExitState {
    pub fn from_final_map(
        transferred: TransferredBootState,
        boot_info: BootInfo,
        descriptors: &[MemoryDescriptor],
        final_map: FinalMapMetadata,
    ) -> Result<Self, PostExitError> {
        if boot_info.validate().is_err()
            || u64::try_from(descriptors.len()).ok() != Some(boot_info.memory_map.descriptor_count)
            || boot_info.memory_map.descriptor_stride != MEMORY_DESCRIPTOR_SIZE
        {
            return Err(PostExitError::InvalidBootInformation);
        }
        if descriptors
            .iter()
            .any(|descriptor| descriptor.validate().is_err())
        {
            return Err(PostExitError::InvalidDescriptors);
        }
        if transferred.boot_info_address != transferred.allocations.boot_info.page_start
            || boot_info.memory_map.physical_address
                != transferred.allocations.converted_map.page_start
        {
            return Err(PostExitError::WrongBootInformationAddress);
        }
        Ok(Self {
            transferred,
            boot_info,
            final_map,
        })
    }

    pub const fn kernel_entry(&self) -> u64 {
        self.transferred.kernel_entry
    }
    pub const fn boot_info_address(&self) -> u64 {
        self.transferred.boot_info_address
    }
    pub const fn descriptor_count(&self) -> u64 {
        self.boot_info.memory_map.descriptor_count
    }
    pub const fn final_map_metadata(&self) -> FinalMapMetadata {
        self.final_map
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::{
        GopFramebuffer, GopPixelFormat, LoadPlan, MapKeyStatus, PageAllocation,
        ProvisionalMapMetadata, SegmentBackend, SegmentSpec, SourceAllocation, build_boot_info,
        convert_framebuffer, prepare_targets,
    };
    use boot_protocol::{MEMORY_KIND_USABLE, MemoryDescriptor};
    use std::{cell::RefCell, panic::AssertUnwindSafe, rc::Rc, vec::Vec};

    #[derive(Clone)]
    struct FakeBackend(Rc<RefCell<Vec<u64>>>);

    impl PageBackend for FakeBackend {
        type Error = ();
        fn allocate_at(&mut self, _: u64, _: u64) -> Result<(), Self::Error> {
            Ok(())
        }
        fn free(&mut self, start: u64, _: u64) -> Result<(), Self::Error> {
            self.0.borrow_mut().push(start);
            Ok(())
        }
    }

    impl SegmentBackend for FakeBackend {
        fn zero(&mut self, _: crate::LoadItem) -> Result<(), Self::Error> {
            Ok(())
        }
        fn copy(&mut self, _: crate::LoadItem, _: &[u8]) -> Result<(), Self::Error> {
            Ok(())
        }
        fn verify(&mut self, _: crate::LoadItem, _: &[u8]) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn owners() -> (
        LoadedKernel<FakeBackend>,
        PreparedBootInfo<FakeBackend>,
        Rc<RefCell<Vec<u64>>>,
    ) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let plan = LoadPlan::build(
            0x20_0000,
            [SegmentSpec {
                file_offset: 0,
                file_size: 1,
                memory_size: 1,
                target: 0x20_0000,
                flags: 1,
            }],
            SourceAllocation {
                page_start: 0x10_0000,
                page_count: 1,
                file_length: 1,
            },
        )
        .unwrap();
        let mut verified = match prepare_targets(&plan, &[1], FakeBackend(calls.clone()), |_| {}) {
            Ok(verified) => verified,
            Err(_) => panic!("fake target preparation must succeed"),
        };
        let kernel = verified.into_loaded_kernel(&plan).unwrap();
        let descriptors = [MemoryDescriptor {
            kind: MEMORY_KIND_USABLE,
            reserved0: 0,
            physical_start: 0x1000,
            page_count: 1,
            attributes: 0,
        }];
        let framebuffer = convert_framebuffer(GopFramebuffer {
            physical_address: 0x8000_0000,
            byte_length: 4,
            width: 1,
            height: 1,
            pixels_per_scanline: 1,
            pixel_format: GopPixelFormat::Rgb,
        })
        .unwrap();
        let allocations = BootDataAllocations {
            raw_map: PageAllocation {
                page_start: 0x10000,
                page_count: 1,
            },
            conversion_scratch: PageAllocation {
                page_start: 0x11000,
                page_count: 1,
            },
            converted_map: PageAllocation {
                page_start: 0x12000,
                page_count: 1,
            },
            boot_info: PageAllocation {
                page_start: 0x13000,
                page_count: 1,
            },
        };
        let wire = build_boot_info(0x12000, &descriptors, 1, framebuffer).unwrap();
        let boot = PreparedBootInfo::from_validated(
            FakeBackend(calls.clone()),
            allocations,
            wire,
            &descriptors,
            ProvisionalMapMetadata {
                map_size: 48,
                descriptor_stride: 48,
                descriptor_version: 1,
                map_key: 7,
                status: MapKeyStatus::Provisional,
            },
        )
        .unwrap();
        (kernel, boot, calls)
    }

    #[test]
    fn typestate_requires_complete_valid_owners_and_pre_exit_drop_rolls_back() {
        let (kernel, boot, calls) = owners();
        let ready = BootServicesState::new()
            .with_kernel(kernel)
            .unwrap()
            .with_boot_information(boot)
            .unwrap();
        drop(ready);
        assert_eq!(calls.borrow().len(), 5);
    }

    #[test]
    fn released_kernel_cannot_become_exit_ready() {
        let (mut kernel, boot, calls) = owners();
        kernel.try_release().unwrap();
        assert!(matches!(
            BootServicesState::new().with_kernel(kernel),
            Err(ExitPreparationError::KernelReleased)
        ));
        drop(boot);
        assert_eq!(calls.borrow().len(), 5);
    }

    #[test]
    fn transfer_disarms_both_owners_and_post_exit_state_needs_no_drop() {
        let (kernel, boot, calls) = owners();
        let ready = BootServicesState::new()
            .with_kernel(kernel)
            .unwrap()
            .with_boot_information(boot)
            .unwrap();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            ready.cross_exit_boundary(|state| {
                assert_eq!(state.kernel_entry(), 0x20_0000);
                std::panic!("stand in for the non-returning firmware boundary")
            })
        }));
        assert!(result.is_err());
        assert!(calls.borrow().is_empty());
        assert!(!core::mem::needs_drop::<PostExitState>());
    }

    #[test]
    fn handoff_typestate_rolls_back_stack_and_transfers_all_owners_once() {
        let (kernel, boot, calls) = owners();
        let stack = BootstrapStack::from_initialized(
            FakeBackend(calls.clone()),
            PageAllocation {
                page_start: 0x40000,
                page_count: crate::BOOTSTRAP_STACK_PAGES,
            },
            &[],
        )
        .unwrap();
        let ready = BootServicesState::new()
            .with_kernel(kernel)
            .unwrap()
            .with_boot_information(boot)
            .unwrap()
            .with_bootstrap_stack(stack)
            .unwrap();
        drop(ready);
        assert_eq!(calls.borrow().len(), 6);

        let (kernel, boot, calls) = owners();
        let stack = BootstrapStack::from_initialized(
            FakeBackend(calls.clone()),
            PageAllocation {
                page_start: 0x40000,
                page_count: crate::BOOTSTRAP_STACK_PAGES,
            },
            &[],
        )
        .unwrap();
        let ready = BootServicesState::new()
            .with_kernel(kernel)
            .unwrap()
            .with_boot_information(boot)
            .unwrap()
            .with_bootstrap_stack(stack)
            .unwrap();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            ready.cross_exit_boundary(|state| {
                assert_eq!(state.bootstrap_stack().unwrap().top, 0x50000);
                std::panic!("stand in for handoff")
            })
        }));
        assert!(result.is_err());
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn fake_exit_backend_gets_only_one_invalid_key_retry() {
        let fake_statuses = [true, false];
        let mut policy = ExitRetryPolicy::new();
        let mut calls = 0;
        for invalid_key in fake_statuses {
            assert!(policy.begin_attempt());
            calls += 1;
            if !policy.retry_invalid_key(invalid_key) {
                break;
            }
        }
        assert_eq!(calls, 2);
        assert!(!policy.begin_attempt());
        assert_eq!(policy.attempts(), 2);

        let mut fatal = ExitRetryPolicy::new();
        assert!(fatal.begin_attempt());
        assert!(!fatal.retry_invalid_key(false));
    }

    #[test]
    fn final_boot_information_creates_drop_free_post_exit_state() {
        let descriptors = [MemoryDescriptor {
            kind: MEMORY_KIND_USABLE,
            reserved0: 0,
            physical_start: 0x1000,
            page_count: 1,
            attributes: 0,
        }];
        let framebuffer = convert_framebuffer(GopFramebuffer {
            physical_address: 0x8000_0000,
            byte_length: 4,
            width: 1,
            height: 1,
            pixels_per_scanline: 1,
            pixel_format: GopPixelFormat::Rgb,
        })
        .unwrap();
        let allocations = BootDataAllocations {
            raw_map: PageAllocation {
                page_start: 0x10000,
                page_count: 1,
            },
            conversion_scratch: PageAllocation {
                page_start: 0x11000,
                page_count: 1,
            },
            converted_map: PageAllocation {
                page_start: 0x12000,
                page_count: 1,
            },
            boot_info: PageAllocation {
                page_start: 0x13000,
                page_count: 1,
            },
        };
        let transferred = TransferredBootState {
            kernel_entry: 0x20_0000,
            kernel_range: LoadRange {
                start: 0x20_0000,
                end: 0x20_1000,
            },
            segments: [empty_segment(); MAX_LOAD_ITEMS],
            segment_count: 1,
            allocations,
            framebuffer,
            boot_info_address: allocations.boot_info.page_start,
            bootstrap_stack: None,
        };
        let boot_info = build_boot_info(
            allocations.converted_map.page_start,
            &descriptors,
            1,
            framebuffer,
        )
        .unwrap();
        let post = PostExitState::from_final_map(
            transferred,
            boot_info,
            &descriptors,
            FinalMapMetadata {
                map_size: 48,
                descriptor_stride: 48,
                descriptor_version: 1,
                map_key: 9,
            },
        )
        .unwrap();
        assert_eq!(post.kernel_entry(), 0x20_0000);
        assert_eq!(post.boot_info_address(), 0x13000);
        assert_eq!(post.descriptor_count(), 1);
        assert_eq!(post.final_map_metadata().map_key, 9);
        assert!(!core::mem::needs_drop::<PostExitState>());
    }
}
