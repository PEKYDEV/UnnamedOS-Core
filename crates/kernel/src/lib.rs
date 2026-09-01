#![no_std]
#![forbid(unsafe_code)]

use boot_protocol::{BootInfo, MEMORY_DESCRIPTOR_SIZE, MEMORY_KIND_USABLE, MemoryDescriptor};
pub const STACK_BYTES: u64 = 64 * 1024;
pub const STACK_CANARY: u64 = 0x554e_4f53_5354_414b;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandoffInputs {
    pub boot_info_address: u64,
    pub stack_bottom: u64,
    pub stack_top: u64,
    pub entry_rsp: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalRange {
    pub start: u64,
    pub end: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KernelValidationError {
    Pointer,
    Stack,
    BootInfo,
    MapShape,
    Descriptor,
    UsableReservation,
}

pub fn validate_handoff_inputs(input: HandoffInputs) -> Result<(), KernelValidationError> {
    if input.boot_info_address == 0 || !input.boot_info_address.is_multiple_of(8) {
        return Err(KernelValidationError::Pointer);
    }
    if !input.stack_bottom.is_multiple_of(4096)
        || !input.stack_top.is_multiple_of(16)
        || input.entry_rsp != input.stack_top
    {
        return Err(KernelValidationError::Stack);
    }
    if input.stack_top.checked_sub(input.stack_bottom) != Some(STACK_BYTES) {
        return Err(KernelValidationError::Stack);
    }
    Ok(())
}
pub fn validate_canary(observed: u64) -> Result<(), KernelValidationError> {
    if observed == STACK_CANARY {
        Ok(())
    } else {
        Err(KernelValidationError::Stack)
    }
}
pub fn validate_boot_state(
    boot: &BootInfo,
    descriptors: &[MemoryDescriptor],
    reserved: &[PhysicalRange],
) -> Result<(), KernelValidationError> {
    boot.validate()
        .map_err(|_| KernelValidationError::BootInfo)?;
    if boot.memory_map.descriptor_stride != MEMORY_DESCRIPTOR_SIZE
        || usize::try_from(boot.memory_map.descriptor_count).ok() != Some(descriptors.len())
        || boot.memory_map.byte_length
            != boot
                .memory_map
                .descriptor_count
                .checked_mul(u64::from(MEMORY_DESCRIPTOR_SIZE))
                .ok_or(KernelValidationError::MapShape)?
    {
        return Err(KernelValidationError::MapShape);
    }
    for descriptor in descriptors {
        descriptor
            .validate()
            .map_err(|_| KernelValidationError::Descriptor)?;
    }
    for range in reserved {
        if range.start >= range.end || !covered_non_usable(*range, descriptors)? {
            return Err(KernelValidationError::UsableReservation);
        }
    }
    Ok(())
}
fn covered_non_usable(
    range: PhysicalRange,
    descriptors: &[MemoryDescriptor],
) -> Result<bool, KernelValidationError> {
    let mut cursor = range.start;
    while cursor < range.end {
        let descriptor = match descriptors.iter().find(|item| {
            let end = item
                .physical_start
                .checked_add(item.page_count.saturating_mul(4096))
                .unwrap_or(0);
            item.physical_start <= cursor && cursor < end
        }) {
            Some(value) => value,
            None => return Ok(false),
        };
        if descriptor.kind == MEMORY_KIND_USABLE {
            return Ok(false);
        }
        let end = descriptor
            .physical_start
            .checked_add(
                descriptor
                    .page_count
                    .checked_mul(4096)
                    .ok_or(KernelValidationError::Descriptor)?,
            )
            .ok_or(KernelValidationError::Descriptor)?;
        cursor = end.min(range.end);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use boot_protocol::{
        FramebufferInfo, MEMORY_KIND_BOOTSTRAP_STACK, MEMORY_KIND_KERNEL_IMAGE, MemoryMapInfo,
        PIXEL_FORMAT_RGBX8888,
    };
    fn boot(count: u64) -> BootInfo {
        BootInfo::new(
            MemoryMapInfo {
                physical_address: 0x3000,
                descriptor_count: count,
                descriptor_stride: 32,
                descriptor_version: 1,
                reserved0: 0,
                byte_length: count * 32,
                reserved1: 0,
            },
            FramebufferInfo {
                physical_address: 0x800000,
                byte_length: 4,
                width: 1,
                height: 1,
                pixels_per_scanline: 1,
                pixel_format: PIXEL_FORMAT_RGBX8888,
                reserved0: 0,
            },
        )
    }
    fn descriptor(kind: u32, start: u64, pages: u64) -> MemoryDescriptor {
        MemoryDescriptor {
            kind,
            reserved0: 0,
            physical_start: start,
            page_count: pages,
            attributes: 0,
        }
    }
    #[test]
    fn bad_pointer_and_stack_bounds_are_rejected() {
        let valid = HandoffInputs {
            boot_info_address: 0x4000,
            stack_bottom: 0x10000,
            stack_top: 0x20000,
            entry_rsp: 0x20000,
        };
        assert_eq!(validate_handoff_inputs(valid), Ok(()));
        assert_eq!(
            validate_handoff_inputs(HandoffInputs {
                boot_info_address: 0,
                ..valid
            }),
            Err(KernelValidationError::Pointer)
        );
        assert_eq!(validate_canary(STACK_CANARY), Ok(()));
        assert_eq!(validate_canary(0), Err(KernelValidationError::Stack));
        assert_eq!(
            validate_handoff_inputs(HandoffInputs {
                entry_rsp: 0x1fff0,
                ..valid
            }),
            Err(KernelValidationError::Stack)
        );
    }
    #[test]
    fn boot_info_and_reserved_map_validation_are_enforced() {
        let descriptors = [
            descriptor(MEMORY_KIND_KERNEL_IMAGE, 0x200000, 4),
            descriptor(MEMORY_KIND_BOOTSTRAP_STACK, 0x400000, 16),
        ];
        let ranges = [
            PhysicalRange {
                start: 0x200000,
                end: 0x204000,
            },
            PhysicalRange {
                start: 0x400000,
                end: 0x410000,
            },
        ];
        assert_eq!(validate_boot_state(&boot(2), &descriptors, &ranges), Ok(()));
        let usable = [descriptor(MEMORY_KIND_USABLE, 0x200000, 4), descriptors[1]];
        assert_eq!(
            validate_boot_state(&boot(2), &usable, &ranges),
            Err(KernelValidationError::UsableReservation)
        );
        let mut invalid = boot(2);
        invalid.header.magic = 0;
        assert_eq!(
            validate_boot_state(&invalid, &descriptors, &ranges),
            Err(KernelValidationError::BootInfo)
        );
    }
}
