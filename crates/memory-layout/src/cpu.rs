//! Pure CPU capability and required-state gates for later runtime probing.

use core::mem::{align_of, size_of};

use crate::SUPPORTED_PHYSICAL_END;

pub const MIN_PHYSICAL_ADDRESS_BITS: u8 = 36;
pub const MAX_PHYSICAL_ADDRESS_BITS: u8 = 52;
pub const FOUR_LEVEL_LINEAR_ADDRESS_BITS: u8 = 48;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct CpuCapabilities {
    pub long_mode_active: u8,
    pub paging_level_count: u8,
    pub nx_supported: u8,
    pub nxe_enabled: u8,
    pub write_protect_supported: u8,
    pub write_protect_enabled: u8,
    pub physical_address_bits: u8,
    pub linear_address_bits: u8,
    pub la57_enabled: u8,
    pub reserved: [u8; 7],
}

impl CpuCapabilities {
    pub fn validate_for_planning(
        &self,
        highest_required_physical_end: u64,
    ) -> Result<(), CpuCapabilityError> {
        self.validate_boolean_fields()?;
        if self.long_mode_active == 0 {
            return Err(CpuCapabilityError::LongModeInactive);
        }
        if self.paging_level_count != 4 {
            return Err(CpuCapabilityError::UnsupportedPagingLevelCount);
        }
        if self.nx_supported == 0 {
            return Err(CpuCapabilityError::NxUnsupported);
        }
        if self.write_protect_supported == 0 {
            return Err(CpuCapabilityError::WriteProtectUnsupported);
        }
        if !(MIN_PHYSICAL_ADDRESS_BITS..=MAX_PHYSICAL_ADDRESS_BITS)
            .contains(&self.physical_address_bits)
        {
            return Err(CpuCapabilityError::InvalidPhysicalAddressWidth);
        }
        if self.linear_address_bits != FOUR_LEVEL_LINEAR_ADDRESS_BITS {
            return Err(CpuCapabilityError::InvalidLinearAddressWidth);
        }
        if self.la57_enabled != 0 {
            return Err(CpuCapabilityError::La57Enabled);
        }
        if self.reserved != [0; 7] {
            return Err(CpuCapabilityError::ReservedNotZero);
        }
        if highest_required_physical_end > SUPPORTED_PHYSICAL_END {
            return Err(CpuCapabilityError::ArchitecturePhysicalCapExceeded);
        }
        let cpu_end = 1_u64
            .checked_shl(u32::from(self.physical_address_bits))
            .ok_or(CpuCapabilityError::InvalidPhysicalAddressWidth)?;
        if highest_required_physical_end > cpu_end {
            return Err(CpuCapabilityError::RequiredPhysicalRangeUnsupported);
        }
        Ok(())
    }

    pub fn validate_for_activation(
        &self,
        highest_required_physical_end: u64,
    ) -> Result<(), CpuCapabilityError> {
        self.validate_for_planning(highest_required_physical_end)?;
        if self.nxe_enabled == 0 {
            return Err(CpuCapabilityError::NxeNotEnabled);
        }
        if self.write_protect_enabled == 0 {
            return Err(CpuCapabilityError::WriteProtectNotEnabled);
        }
        Ok(())
    }

    fn validate_boolean_fields(&self) -> Result<(), CpuCapabilityError> {
        for value in [
            self.long_mode_active,
            self.nx_supported,
            self.nxe_enabled,
            self.write_protect_supported,
            self.write_protect_enabled,
            self.la57_enabled,
        ] {
            if value > 1 {
                return Err(CpuCapabilityError::InvalidBooleanValue);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuCapabilityError {
    InvalidBooleanValue,
    LongModeInactive,
    UnsupportedPagingLevelCount,
    NxUnsupported,
    NxeNotEnabled,
    WriteProtectUnsupported,
    WriteProtectNotEnabled,
    InvalidPhysicalAddressWidth,
    InvalidLinearAddressWidth,
    La57Enabled,
    ReservedNotZero,
    ArchitecturePhysicalCapExceeded,
    RequiredPhysicalRangeUnsupported,
}

const _: () = assert!(size_of::<CpuCapabilities>() == 16);
const _: () = assert!(align_of::<CpuCapabilities>() == 1);
