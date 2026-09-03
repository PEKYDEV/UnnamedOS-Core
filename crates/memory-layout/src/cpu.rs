//! Pure validation for read-only x86-64 CPU and control-state snapshots.

use core::mem::{align_of, size_of};

use crate::{PAGE_SIZE, PhysicalFrame, SUPPORTED_PHYSICAL_END};

pub const MIN_PHYSICAL_ADDRESS_BITS: u8 = 36;
pub const MAX_PHYSICAL_ADDRESS_BITS: u8 = 52;
pub const FOUR_LEVEL_LINEAR_ADDRESS_BITS: u8 = 48;
pub const FIVE_LEVEL_LINEAR_ADDRESS_BITS: u8 = 57;

const CPUID_MSR: u32 = 1 << 5;
const CPUID_PAE: u32 = 1 << 6;
const CPUID_NX: u32 = 1 << 20;
const CPUID_LONG_MODE: u32 = 1 << 29;
const CPUID_LA57: u32 = 1 << 16;
const CR0_WP: u64 = 1 << 16;
const CR0_PG: u64 = 1 << 31;
const CR4_PAE: u64 = 1 << 5;
const CR4_PGE: u64 = 1 << 7;
const CR4_LA57: u64 = 1 << 12;
const CR4_PCIDE: u64 = 1 << 17;
const EFER_LME: u64 = 1 << 8;
const EFER_LMA: u64 = 1 << 10;
const EFER_NXE: u64 = 1 << 11;
const CR3_PWT_PCD: u64 = (1 << 3) | (1 << 4);

/// Raw values captured by the architecture adapter. Policy code never probes
/// hardware and synthetic instances are safe to validate on a host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct RawCpuSnapshot {
    pub maximum_basic_leaf: u32,
    pub maximum_extended_leaf: u32,
    pub basic_feature_edx: u32,
    pub structured_feature_ecx: u32,
    pub extended_feature_edx: u32,
    pub address_width_eax: u32,
    pub cr0: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub efer: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum HardeningState {
    Enabled = 1,
    MustEnableBeforeActivation = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PgeState {
    Disabled = 0,
    Enabled = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PcidState {
    Disabled = 0,
    EnabledMustRemainUnused = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Cr3State {
    root_address: u64,
    context_or_flags: u16,
    pcid_state: PcidState,
    reserved: [u8; 5],
}

impl Cr3State {
    pub const fn root_address(self) -> u64 {
        self.root_address
    }
    pub const fn context_or_flags(self) -> u16 {
        self.context_or_flags
    }
    pub const fn pcid_state(self) -> PcidState {
        self.pcid_state
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidatedCpuSnapshot {
    physical_address_bits: u8,
    reported_linear_address_bits: u8,
    la57_supported: bool,
    current_cr3: Cr3State,
    nxe: HardeningState,
    write_protect: HardeningState,
    pge: PgeState,
    stability: Cr3StabilityToken,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cr3StabilityToken(u64);

impl Cr3StabilityToken {
    pub const fn verify(self, observed_cr3: u64) -> Result<(), CpuCapabilityError> {
        if self.0 == observed_cr3 {
            Ok(())
        } else {
            Err(CpuCapabilityError::InheritedCr3Changed)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct ActivationReadiness {
    current_cr3: Cr3State,
    proposed_root: u64,
    nxe: HardeningState,
    write_protect: HardeningState,
    pge: PgeState,
    pcid: PcidState,
    physical_address_bits: u8,
    effective_linear_address_bits: u8,
    transition_permitted: u8,
    reserved: [u8; 5],
    stability: Cr3StabilityToken,
}

impl ActivationReadiness {
    pub const fn current_cr3(self) -> Cr3State {
        self.current_cr3
    }
    pub const fn proposed_root(self) -> u64 {
        self.proposed_root
    }
    pub const fn nxe(self) -> HardeningState {
        self.nxe
    }
    pub const fn write_protect(self) -> HardeningState {
        self.write_protect
    }
    pub const fn pge(self) -> PgeState {
        self.pge
    }
    pub const fn pcid(self) -> PcidState {
        self.pcid
    }
    pub const fn physical_address_bits(self) -> u8 {
        self.physical_address_bits
    }
    pub const fn effective_linear_address_bits(self) -> u8 {
        self.effective_linear_address_bits
    }
    pub const fn transition_permitted(self) -> bool {
        self.transition_permitted == 1
    }
    pub const fn cr3_stability_token(self) -> Cr3StabilityToken {
        self.stability
    }
}

impl RawCpuSnapshot {
    pub fn validate(self) -> Result<ValidatedCpuSnapshot, CpuCapabilityError> {
        if self.maximum_basic_leaf < 1 {
            return Err(CpuCapabilityError::MissingBasicFeatureLeaf);
        }
        if self.maximum_extended_leaf < 0x8000_0001 {
            return Err(CpuCapabilityError::MissingExtendedFeatureLeaf);
        }
        if self.maximum_extended_leaf < 0x8000_0008 {
            return Err(CpuCapabilityError::MissingAddressWidthLeaf);
        }
        if self.extended_feature_edx & CPUID_LONG_MODE == 0 {
            return Err(CpuCapabilityError::LongModeUnsupported);
        }
        if self.extended_feature_edx & CPUID_NX == 0 {
            return Err(CpuCapabilityError::NxUnsupported);
        }
        if self.basic_feature_edx & CPUID_MSR == 0 {
            return Err(CpuCapabilityError::MsrUnsupported);
        }
        if self.basic_feature_edx & CPUID_PAE == 0 {
            return Err(CpuCapabilityError::PaeUnsupported);
        }
        if self.cr0 & CR0_PG == 0 {
            return Err(CpuCapabilityError::PagingInactive);
        }
        if self.efer & EFER_LMA == 0 {
            return Err(CpuCapabilityError::LongModeInactive);
        }
        if self.efer & EFER_LME == 0 {
            return Err(CpuCapabilityError::ContradictoryLongModeState);
        }
        if self.cr4 & CR4_PAE == 0 {
            return Err(CpuCapabilityError::PaeInactive);
        }

        let la57_supported =
            self.maximum_basic_leaf >= 7 && self.structured_feature_ecx & CPUID_LA57 != 0;
        if self.cr4 & CR4_LA57 != 0 {
            if !la57_supported {
                return Err(CpuCapabilityError::ContradictoryLa57State);
            }
            return Err(CpuCapabilityError::La57Enabled);
        }
        let physical_address_bits = (self.address_width_eax & 0xff) as u8;
        if !(MIN_PHYSICAL_ADDRESS_BITS..=MAX_PHYSICAL_ADDRESS_BITS).contains(&physical_address_bits)
        {
            return Err(CpuCapabilityError::InvalidPhysicalAddressWidth);
        }
        let reported_linear_address_bits = ((self.address_width_eax >> 8) & 0xff) as u8;
        if !matches!(
            reported_linear_address_bits,
            FOUR_LEVEL_LINEAR_ADDRESS_BITS | FIVE_LEVEL_LINEAR_ADDRESS_BITS
        ) {
            return Err(CpuCapabilityError::InvalidLinearAddressWidth);
        }
        if reported_linear_address_bits == FIVE_LEVEL_LINEAR_ADDRESS_BITS && !la57_supported {
            return Err(CpuCapabilityError::ContradictoryLinearAddressWidth);
        }

        let cpu_physical_end = 1_u64 << physical_address_bits;
        let address_mask = (cpu_physical_end - 1) & !(PAGE_SIZE - 1);
        let pcid = self.cr4 & CR4_PCIDE != 0;
        let allowed_low = if pcid { PAGE_SIZE - 1 } else { CR3_PWT_PCD };
        if self.cr3 & !(address_mask | allowed_low) != 0 {
            return Err(CpuCapabilityError::UnsupportedCr3Encoding);
        }
        let root_address = self.cr3 & address_mask;
        if root_address == 0 || !root_address.is_multiple_of(PAGE_SIZE) {
            return Err(CpuCapabilityError::InvalidCurrentCr3Root);
        }
        let pcid_state = if pcid {
            PcidState::EnabledMustRemainUnused
        } else {
            PcidState::Disabled
        };
        Ok(ValidatedCpuSnapshot {
            physical_address_bits,
            reported_linear_address_bits,
            la57_supported,
            current_cr3: Cr3State {
                root_address,
                context_or_flags: (self.cr3 & allowed_low) as u16,
                pcid_state,
                reserved: [0; 5],
            },
            nxe: if self.efer & EFER_NXE != 0 {
                HardeningState::Enabled
            } else {
                HardeningState::MustEnableBeforeActivation
            },
            write_protect: if self.cr0 & CR0_WP != 0 {
                HardeningState::Enabled
            } else {
                HardeningState::MustEnableBeforeActivation
            },
            pge: if self.cr4 & CR4_PGE != 0 {
                PgeState::Enabled
            } else {
                PgeState::Disabled
            },
            stability: Cr3StabilityToken(self.cr3),
        })
    }
}

impl ValidatedCpuSnapshot {
    pub const fn physical_address_bits(self) -> u8 {
        self.physical_address_bits
    }
    pub const fn reported_linear_address_bits(self) -> u8 {
        self.reported_linear_address_bits
    }
    pub const fn la57_supported(self) -> bool {
        self.la57_supported
    }
    pub const fn current_cr3(self) -> Cr3State {
        self.current_cr3
    }
    pub const fn cr3_stability_token(self) -> Cr3StabilityToken {
        self.stability
    }

    pub fn classify_for_hierarchy(
        self,
        owned_frames: &[PhysicalFrame],
        proposed_root: PhysicalFrame,
        highest_mapped_physical_end: u64,
    ) -> Result<ActivationReadiness, CpuCapabilityError> {
        if owned_frames.is_empty()
            || owned_frames[0] != proposed_root
            || proposed_root.address() == 0
        {
            return Err(CpuCapabilityError::InvalidProposedRoot);
        }
        if highest_mapped_physical_end == 0 {
            return Err(CpuCapabilityError::InvalidRequiredPhysicalRange);
        }
        if highest_mapped_physical_end > SUPPORTED_PHYSICAL_END {
            return Err(CpuCapabilityError::ArchitecturePhysicalCapExceeded);
        }
        let cpu_end = 1_u64 << self.physical_address_bits;
        if highest_mapped_physical_end > cpu_end {
            return Err(CpuCapabilityError::MappedPhysicalRangeUnsupported);
        }
        if proposed_root
            .address()
            .checked_add(PAGE_SIZE)
            .ok_or(CpuCapabilityError::ProposedRootUnsupported)?
            > cpu_end
        {
            return Err(CpuCapabilityError::ProposedRootUnsupported);
        }
        for frame in owned_frames {
            let end = frame
                .address()
                .checked_add(PAGE_SIZE)
                .ok_or(CpuCapabilityError::OwnedFrameUnsupported)?;
            if end > cpu_end {
                return Err(CpuCapabilityError::OwnedFrameUnsupported);
            }
        }
        Ok(ActivationReadiness {
            current_cr3: self.current_cr3,
            proposed_root: proposed_root.address(),
            nxe: self.nxe,
            write_protect: self.write_protect,
            pge: self.pge,
            pcid: self.current_cr3.pcid_state,
            physical_address_bits: self.physical_address_bits,
            effective_linear_address_bits: FOUR_LEVEL_LINEAR_ADDRESS_BITS,
            transition_permitted: 1,
            reserved: [0; 5],
            stability: self.stability,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuCapabilityError {
    MissingBasicFeatureLeaf,
    MissingExtendedFeatureLeaf,
    MissingAddressWidthLeaf,
    LongModeUnsupported,
    NxUnsupported,
    MsrUnsupported,
    PaeUnsupported,
    PagingInactive,
    LongModeInactive,
    ContradictoryLongModeState,
    PaeInactive,
    ContradictoryLa57State,
    La57Enabled,
    InvalidPhysicalAddressWidth,
    InvalidLinearAddressWidth,
    ContradictoryLinearAddressWidth,
    UnsupportedCr3Encoding,
    InvalidCurrentCr3Root,
    InvalidProposedRoot,
    InvalidRequiredPhysicalRange,
    ArchitecturePhysicalCapExceeded,
    MappedPhysicalRangeUnsupported,
    OwnedFrameUnsupported,
    ProposedRootUnsupported,
    InheritedCr3Changed,
}

const _: () = assert!(size_of::<RawCpuSnapshot>() == 56);
const _: () = assert!(align_of::<RawCpuSnapshot>() == 8);
const _: () = assert!(size_of::<Cr3State>() == 16);
const _: () = assert!(align_of::<Cr3State>() == 8);
const _: () = assert!(size_of::<ActivationReadiness>() == 48);
const _: () = assert!(align_of::<ActivationReadiness>() == 8);
