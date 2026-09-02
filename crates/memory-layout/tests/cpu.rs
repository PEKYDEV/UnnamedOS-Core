use core::mem::{align_of, size_of};

use memory_layout::{CpuCapabilities, CpuCapabilityError, SUPPORTED_PHYSICAL_END};

fn supported() -> CpuCapabilities {
    CpuCapabilities {
        long_mode_active: 1,
        paging_level_count: 4,
        nx_supported: 1,
        nxe_enabled: 0,
        write_protect_supported: 1,
        write_protect_enabled: 0,
        physical_address_bits: 39,
        linear_address_bits: 48,
        la57_enabled: 0,
        reserved: [0; 7],
    }
}

#[test]
fn capability_layout_and_planning_state_are_explicit() {
    assert_eq!(size_of::<CpuCapabilities>(), 16);
    assert_eq!(align_of::<CpuCapabilities>(), 1);
    assert_eq!(supported().validate_for_planning(1_u64 << 38), Ok(()));
    assert_eq!(
        supported().validate_for_activation(1_u64 << 38),
        Err(CpuCapabilityError::NxeNotEnabled)
    );
    let mut active = supported();
    active.nxe_enabled = 1;
    active.write_protect_enabled = 1;
    assert_eq!(active.validate_for_activation(1_u64 << 38), Ok(()));
}

#[test]
fn every_required_capability_rejects_fail_closed() {
    let cases = [
        (
            {
                let mut value = supported();
                value.long_mode_active = 0;
                value
            },
            CpuCapabilityError::LongModeInactive,
        ),
        (
            {
                let mut value = supported();
                value.paging_level_count = 5;
                value
            },
            CpuCapabilityError::UnsupportedPagingLevelCount,
        ),
        (
            {
                let mut value = supported();
                value.nx_supported = 0;
                value
            },
            CpuCapabilityError::NxUnsupported,
        ),
        (
            {
                let mut value = supported();
                value.write_protect_supported = 0;
                value
            },
            CpuCapabilityError::WriteProtectUnsupported,
        ),
        (
            {
                let mut value = supported();
                value.physical_address_bits = 31;
                value
            },
            CpuCapabilityError::InvalidPhysicalAddressWidth,
        ),
        (
            {
                let mut value = supported();
                value.linear_address_bits = 57;
                value
            },
            CpuCapabilityError::InvalidLinearAddressWidth,
        ),
        (
            {
                let mut value = supported();
                value.la57_enabled = 1;
                value
            },
            CpuCapabilityError::La57Enabled,
        ),
        (
            {
                let mut value = supported();
                value.reserved[0] = 1;
                value
            },
            CpuCapabilityError::ReservedNotZero,
        ),
    ];
    for (capabilities, expected) in cases {
        assert_eq!(capabilities.validate_for_planning(0x1000), Err(expected));
    }
}

#[test]
fn boolean_width_and_physical_range_errors_are_distinct() {
    let mut invalid_boolean = supported();
    invalid_boolean.nx_supported = 2;
    assert_eq!(
        invalid_boolean.validate_for_planning(0x1000),
        Err(CpuCapabilityError::InvalidBooleanValue)
    );
    assert_eq!(
        supported().validate_for_planning((1_u64 << 39) + 1),
        Err(CpuCapabilityError::RequiredPhysicalRangeUnsupported)
    );
    let mut wide = supported();
    wide.physical_address_bits = 46;
    assert_eq!(
        wide.validate_for_planning(SUPPORTED_PHYSICAL_END + 1),
        Err(CpuCapabilityError::ArchitecturePhysicalCapExceeded)
    );
}

#[test]
fn activation_requires_both_nxe_and_write_protect_state() {
    let mut capabilities = supported();
    capabilities.nxe_enabled = 1;
    assert_eq!(
        capabilities.validate_for_activation(0x1000),
        Err(CpuCapabilityError::WriteProtectNotEnabled)
    );
    capabilities.write_protect_enabled = 1;
    capabilities.nxe_enabled = 0;
    assert_eq!(
        capabilities.validate_for_activation(0x1000),
        Err(CpuCapabilityError::NxeNotEnabled)
    );
}
