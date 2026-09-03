#![no_std]
#![forbid(unsafe_code)]

//! Pure acceptance contract for the first UnnamedOS-owned x86-64 address space.
//! Planning validates scalar ranges and never dereferences an address or emits
//! privileged instructions.

use core::mem::{align_of, size_of};

mod cpu;
mod frame_owner;
mod page_table;

pub use cpu::{
    ActivationReadiness, CpuCapabilityError, Cr3StabilityToken, Cr3State, HardeningState,
    PcidState, PgeState, RawCpuSnapshot, ValidatedCpuSnapshot,
};
pub use frame_owner::{
    FrameBackend, FrameOwnerBuildError, FrameOwnerCause, FrameOwnerError, PageTableFrameOwner,
    TransferredPageTableFrames,
};
pub use page_table::{
    ConstructionPlan, EntryFlags, EntryTarget, EntryTargetKind, FrameAssignments, FrameSlot,
    PageTablePlanError, PhysicalFrame, PlanMode, PlannedEntry, PlannedTable, TableIndex,
    TableLevel, TransitionRemoval, virtual_address_indices,
};

pub const PAGE_SIZE: u64 = 4096;
pub const LOW_CANONICAL_END: u64 = 0x0000_8000_0000_0000;
pub const HIGH_CANONICAL_START: u64 = 0xffff_8000_0000_0000;
pub const SUPPORTED_PHYSICAL_END: u64 = 0x0000_4000_0000_0000;

pub const USER_SPACE_START: u64 = 0x0000_0000_0001_0000;
pub const USER_SPACE_END: u64 = 0x0000_7fff_ffff_f000;
pub const DIRECT_MAP_START: u64 = 0xffff_8000_0000_0000;
pub const DIRECT_MAP_END: u64 = 0xffff_c000_0000_0000;
pub const KERNEL_SERVICES_START: u64 = 0xffff_c000_0000_0000;
pub const KERNEL_SERVICES_END: u64 = 0xffff_e000_0000_0000;
pub const MMIO_START: u64 = 0xffff_e000_0000_0000;
pub const MMIO_END: u64 = 0xffff_e800_0000_0000;
pub const FRAMEBUFFER_START: u64 = 0xffff_e800_0000_0000;
pub const FRAMEBUFFER_END: u64 = 0xffff_e900_0000_0000;
pub const HIGH_RESERVED_START: u64 = FRAMEBUFFER_END;
pub const HIGH_RESERVED_END: u64 = 0xffff_ffff_8000_0000;
pub const KERNEL_IMAGE_START: u64 = 0xffff_ffff_8000_0000;
pub const KERNEL_IMAGE_END: u64 = 0xffff_ffff_c000_0000;
pub const KERNEL_LOCAL_START: u64 = KERNEL_IMAGE_END;
pub const KERNEL_LOCAL_END: u64 = 0xffff_ffff_ffff_f000;

pub const BOOTSTRAP_PHYSICAL_START: u64 = 0x0020_0000;
pub const BOOTSTRAP_PHYSICAL_END: u64 = 0x0420_0000;
pub const TRANSITION_IDENTITY_END: u64 = 0x1_0000_0000;
pub const BOOTSTRAP_STACK_BYTES: u64 = 16 * PAGE_SIZE;
pub const MAX_TRANSITION_IDENTITY_BYTES: u64 = 17 * PAGE_SIZE;
pub const DEFAULT_PLAN_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LayoutError {
    InvalidAlignment,
    AlignmentOverflow,
    NonCanonicalVirtualAddress,
    UnsupportedPhysicalAddress,
    EmptyRange,
    ReversedRange,
    RangeOverflow,
    RangeNotPageAligned,
    CrossesCanonicalHole,
    RangeLengthMismatch,
    UnknownPermissionBits,
    ReadPermissionRequired,
    WritableExecutable,
    UserMappingForbidden,
    InvalidRegionPermissions,
    InvalidCachePolicy,
    OutsideDeclaredRegion,
    OutsideBootstrapPhysicalWindow,
    DirectMapAddressMismatch,
    InvalidBootstrapStack,
    MissingStackGuard,
    VirtualOverlap,
    PhysicalAliasWriteExecute,
    PhysicalAliasPermissionEscalation,
    IdentityAddressMismatch,
    TransitionIdentityAboveLimit,
    TransitionIdentityTooLarge,
    InvalidTransitionComposition,
    LowMappingInFinalPlan,
    TransitionMappingInFinalPlan,
    PlanExhausted,
    Unmapped,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct PhysicalAddress(u64);

impl PhysicalAddress {
    pub const fn new(value: u64) -> Result<Self, LayoutError> {
        if value < SUPPORTED_PHYSICAL_END {
            Ok(Self(value))
        } else {
            Err(LayoutError::UnsupportedPhysicalAddress)
        }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct VirtualAddress(u64);

impl VirtualAddress {
    pub const fn new(value: u64) -> Result<Self, LayoutError> {
        if is_canonical(value) {
            Ok(Self(value))
        } else {
            Err(LayoutError::NonCanonicalVirtualAddress)
        }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct PhysicalRange {
    start: PhysicalAddress,
    end: u64,
}

impl PhysicalRange {
    pub fn new(start: u64, end: u64) -> Result<Self, LayoutError> {
        validate_order(start, end)?;
        if start >= SUPPORTED_PHYSICAL_END || end > SUPPORTED_PHYSICAL_END {
            return Err(LayoutError::UnsupportedPhysicalAddress);
        }
        validate_page_boundaries(start, end)?;
        Ok(Self {
            start: PhysicalAddress(start),
            end,
        })
    }

    pub fn from_start_and_length(start: u64, length: u64) -> Result<Self, LayoutError> {
        if length == 0 {
            return Err(LayoutError::EmptyRange);
        }
        let end = match start.checked_add(length) {
            Some(value) => value,
            None => return Err(LayoutError::RangeOverflow),
        };
        Self::new(start, end)
    }

    pub const fn start(self) -> PhysicalAddress {
        self.start
    }

    pub const fn end(self) -> u64 {
        self.end
    }

    pub const fn length(self) -> u64 {
        self.end - self.start.0
    }

    pub const fn contains(self, address: PhysicalAddress) -> bool {
        self.start.0 <= address.0 && address.0 < self.end
    }

    pub const fn contains_range(self, other: Self) -> bool {
        self.start.0 <= other.start.0 && other.end <= self.end
    }

    pub const fn overlaps(self, other: Self) -> bool {
        self.start.0 < other.end && other.start.0 < self.end
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct VirtualRange {
    start: VirtualAddress,
    end: u64,
}

impl VirtualRange {
    pub fn new(start: u64, end: u64) -> Result<Self, LayoutError> {
        validate_order(start, end)?;
        if !is_canonical(start) || !is_canonical(end - 1) {
            return Err(LayoutError::NonCanonicalVirtualAddress);
        }
        if canonical_half(start) != canonical_half(end - 1) {
            return Err(LayoutError::CrossesCanonicalHole);
        }
        validate_page_boundaries(start, end)?;
        Ok(Self {
            start: VirtualAddress(start),
            end,
        })
    }

    pub fn from_start_and_length(start: u64, length: u64) -> Result<Self, LayoutError> {
        if length == 0 {
            return Err(LayoutError::EmptyRange);
        }
        let end = match start.checked_add(length) {
            Some(value) => value,
            None => return Err(LayoutError::RangeOverflow),
        };
        Self::new(start, end)
    }

    pub const fn start(self) -> VirtualAddress {
        self.start
    }

    pub const fn end(self) -> u64 {
        self.end
    }

    pub const fn length(self) -> u64 {
        self.end - self.start.0
    }

    pub const fn contains(self, address: VirtualAddress) -> bool {
        self.start.0 <= address.0 && address.0 < self.end
    }

    pub const fn contains_range(self, other: Self) -> bool {
        self.start.0 <= other.start.0 && other.end <= self.end
    }

    pub const fn overlaps(self, other: Self) -> bool {
        self.start.0 < other.end && other.start.0 < self.end
    }
}

pub const fn align_down(value: u64, alignment: u64) -> Result<u64, LayoutError> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(LayoutError::InvalidAlignment);
    }
    Ok(value & !(alignment - 1))
}

pub const fn align_up(value: u64, alignment: u64) -> Result<u64, LayoutError> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(LayoutError::InvalidAlignment);
    }
    match value.checked_add(alignment - 1) {
        Some(rounded) => Ok(rounded & !(alignment - 1)),
        None => Err(LayoutError::AlignmentOverflow),
    }
}

pub const fn is_canonical(address: u64) -> bool {
    address < LOW_CANONICAL_END || address >= HIGH_CANONICAL_START
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct MappingPermissions(u8);

impl MappingPermissions {
    pub const READ: u8 = 1 << 0;
    pub const WRITE: u8 = 1 << 1;
    pub const EXECUTE: u8 = 1 << 2;
    pub const USER: u8 = 1 << 3;
    pub const GLOBAL: u8 = 1 << 4;
    const KNOWN: u8 = Self::READ | Self::WRITE | Self::EXECUTE | Self::USER | Self::GLOBAL;

    pub const KERNEL_RX: Self = Self(Self::READ | Self::EXECUTE | Self::GLOBAL);
    pub const KERNEL_R: Self = Self(Self::READ | Self::GLOBAL);
    pub const KERNEL_RW: Self = Self(Self::READ | Self::WRITE | Self::GLOBAL);

    pub const fn from_bits(bits: u8) -> Result<Self, LayoutError> {
        if bits & !Self::KNOWN != 0 {
            return Err(LayoutError::UnknownPermissionBits);
        }
        if bits & Self::READ == 0 {
            return Err(LayoutError::ReadPermissionRequired);
        }
        if bits & (Self::WRITE | Self::EXECUTE) == (Self::WRITE | Self::EXECUTE) {
            return Err(LayoutError::WritableExecutable);
        }
        Ok(Self(bits))
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn writable(self) -> bool {
        self.0 & Self::WRITE != 0
    }

    pub const fn executable(self) -> bool {
        self.0 & Self::EXECUTE != 0
    }

    pub const fn user(self) -> bool {
        self.0 & Self::USER != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CachePolicy {
    WriteBack = 0,
    Uncached = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum AddressRegion {
    UserSpace,
    DirectMap,
    KernelServices,
    Mmio,
    Framebuffer,
    Reserved,
    KernelImage,
    KernelLocal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct DeclaredRegion {
    pub kind: AddressRegion,
    pub range: VirtualRange,
}

pub const DECLARED_REGIONS: [DeclaredRegion; 8] = [
    declared(AddressRegion::UserSpace, USER_SPACE_START, USER_SPACE_END),
    declared(AddressRegion::DirectMap, DIRECT_MAP_START, DIRECT_MAP_END),
    declared(
        AddressRegion::KernelServices,
        KERNEL_SERVICES_START,
        KERNEL_SERVICES_END,
    ),
    declared(AddressRegion::Mmio, MMIO_START, MMIO_END),
    declared(
        AddressRegion::Framebuffer,
        FRAMEBUFFER_START,
        FRAMEBUFFER_END,
    ),
    declared(
        AddressRegion::Reserved,
        HIGH_RESERVED_START,
        HIGH_RESERVED_END,
    ),
    declared(
        AddressRegion::KernelImage,
        KERNEL_IMAGE_START,
        KERNEL_IMAGE_END,
    ),
    declared(
        AddressRegion::KernelLocal,
        KERNEL_LOCAL_START,
        KERNEL_LOCAL_END,
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MappingKind {
    KernelText,
    KernelRodata,
    KernelData,
    BootstrapStack,
    BootInfo,
    BootMemoryMap,
    PageTable,
    DirectMap,
    Framebuffer,
    Mmio,
    TransitionIdentity,
    Guard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryBacking {
    Mapped(PhysicalRange),
    Unmapped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanEntry {
    virtual_range: VirtualRange,
    backing: EntryBacking,
    permissions: Option<MappingPermissions>,
    cache: Option<CachePolicy>,
    kind: MappingKind,
}

impl PlanEntry {
    const EMPTY: Self = Self {
        virtual_range: VirtualRange {
            start: VirtualAddress(0),
            end: PAGE_SIZE,
        },
        backing: EntryBacking::Unmapped,
        permissions: None,
        cache: None,
        kind: MappingKind::Guard,
    };

    pub const fn virtual_range(self) -> VirtualRange {
        self.virtual_range
    }

    pub const fn backing(self) -> EntryBacking {
        self.backing
    }

    pub const fn permissions(self) -> Option<MappingPermissions> {
        self.permissions
    }

    pub const fn cache_policy(self) -> Option<CachePolicy> {
        self.cache
    }

    pub const fn kind(self) -> MappingKind {
        self.kind
    }

    pub const fn is_guard(self) -> bool {
        matches!(self.backing, EntryBacking::Unmapped)
    }
}

pub struct MappingPlan<const CAPACITY: usize = DEFAULT_PLAN_CAPACITY> {
    entries: [PlanEntry; CAPACITY],
    len: usize,
}

impl<const CAPACITY: usize> MappingPlan<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            entries: [PlanEntry::EMPTY; CAPACITY],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn entries(&self) -> &[PlanEntry] {
        &self.entries[..self.len]
    }

    pub fn insert_mapping(
        &mut self,
        virtual_range: VirtualRange,
        physical_range: PhysicalRange,
        permissions: MappingPermissions,
        cache: CachePolicy,
        kind: MappingKind,
    ) -> Result<(), LayoutError> {
        if virtual_range.length() != physical_range.length() {
            return Err(LayoutError::RangeLengthMismatch);
        }
        validate_mapping_policy(virtual_range, physical_range, permissions, cache, kind)?;
        self.insert(PlanEntry {
            virtual_range,
            backing: EntryBacking::Mapped(physical_range),
            permissions: Some(permissions),
            cache: Some(cache),
            kind,
        })
    }

    pub fn insert_guard(&mut self, range: VirtualRange) -> Result<(), LayoutError> {
        if !region_range(AddressRegion::KernelLocal).contains_range(range) {
            return Err(LayoutError::OutsideDeclaredRegion);
        }
        self.insert(PlanEntry {
            virtual_range: range,
            backing: EntryBacking::Unmapped,
            permissions: None,
            cache: None,
            kind: MappingKind::Guard,
        })
    }

    pub fn translate(&self, address: VirtualAddress) -> Result<PhysicalAddress, LayoutError> {
        let entry = self
            .entries()
            .iter()
            .find(|entry| entry.virtual_range.contains(address))
            .ok_or(LayoutError::Unmapped)?;
        let EntryBacking::Mapped(physical) = entry.backing else {
            return Err(LayoutError::Unmapped);
        };
        let offset = address
            .0
            .checked_sub(entry.virtual_range.start.0)
            .ok_or(LayoutError::RangeOverflow)?;
        let translated = physical
            .start
            .0
            .checked_add(offset)
            .ok_or(LayoutError::RangeOverflow)?;
        PhysicalAddress::new(translated)
    }

    pub fn validate_transition(&self) -> Result<(), LayoutError> {
        let mut identity_bytes = 0_u64;
        let mut trampoline_count = 0_u8;
        let mut stack_count = 0_u8;
        for entry in self.entries() {
            if entry.virtual_range.start.0 < HIGH_CANONICAL_START {
                if entry.kind != MappingKind::TransitionIdentity {
                    return Err(LayoutError::IdentityAddressMismatch);
                }
                identity_bytes = identity_bytes
                    .checked_add(entry.virtual_range.length())
                    .ok_or(LayoutError::TransitionIdentityTooLarge)?;
                if identity_bytes > MAX_TRANSITION_IDENTITY_BYTES {
                    return Err(LayoutError::TransitionIdentityTooLarge);
                }
                let permissions = entry
                    .permissions
                    .expect("transition mappings have permissions");
                if permissions == MappingPermissions::KERNEL_RX
                    && entry.virtual_range.length() == PAGE_SIZE
                {
                    trampoline_count = trampoline_count
                        .checked_add(1)
                        .ok_or(LayoutError::InvalidTransitionComposition)?;
                } else if permissions == MappingPermissions::KERNEL_RW
                    && entry.virtual_range.length() == BOOTSTRAP_STACK_BYTES
                {
                    stack_count = stack_count
                        .checked_add(1)
                        .ok_or(LayoutError::InvalidTransitionComposition)?;
                } else {
                    return Err(LayoutError::InvalidTransitionComposition);
                }
            }
        }
        if trampoline_count != 1 || stack_count != 1 {
            return Err(LayoutError::InvalidTransitionComposition);
        }
        Ok(())
    }

    pub fn validate_final(&self) -> Result<(), LayoutError> {
        for entry in self.entries() {
            if entry.kind == MappingKind::TransitionIdentity {
                return Err(LayoutError::TransitionMappingInFinalPlan);
            }
            if entry.virtual_range.start.0 < HIGH_CANONICAL_START {
                return Err(LayoutError::LowMappingInFinalPlan);
            }
        }
        for entry in self.entries() {
            if entry.kind == MappingKind::BootstrapStack {
                let guard_start = entry
                    .virtual_range
                    .start
                    .0
                    .checked_sub(PAGE_SIZE)
                    .ok_or(LayoutError::MissingStackGuard)?;
                if !self.entries().iter().any(|candidate| {
                    candidate.kind == MappingKind::Guard
                        && candidate.virtual_range.start.0 == guard_start
                        && candidate.virtual_range.end == entry.virtual_range.start.0
                }) {
                    return Err(LayoutError::MissingStackGuard);
                }
            }
        }
        Ok(())
    }

    fn insert(&mut self, entry: PlanEntry) -> Result<(), LayoutError> {
        for existing in self.entries() {
            if existing.virtual_range.overlaps(entry.virtual_range) {
                return Err(LayoutError::VirtualOverlap);
            }
            if let (EntryBacking::Mapped(left), EntryBacking::Mapped(right)) =
                (existing.backing, entry.backing)
                && left.overlaps(right)
            {
                let left_permissions = existing
                    .permissions
                    .expect("mapped entries have permissions");
                let right_permissions = entry.permissions.expect("mapped entries have permissions");
                if (left_permissions.writable() && right_permissions.executable())
                    || (right_permissions.writable() && left_permissions.executable())
                {
                    return Err(LayoutError::PhysicalAliasWriteExecute);
                }
                if left_permissions.writable() != right_permissions.writable() {
                    return Err(LayoutError::PhysicalAliasPermissionEscalation);
                }
            }
        }
        if self.len == CAPACITY {
            return Err(LayoutError::PlanExhausted);
        }
        let mut position = 0;
        while position < self.len
            && self.entries[position].virtual_range.start.0 < entry.virtual_range.start.0
        {
            position += 1;
        }
        let mut index = self.len;
        while index > position {
            self.entries[index] = self.entries[index - 1];
            index -= 1;
        }
        self.entries[position] = entry;
        self.len += 1;
        Ok(())
    }
}

impl<const CAPACITY: usize> Default for MappingPlan<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn validate_declared_regions() -> Result<(), LayoutError> {
    for (index, region) in DECLARED_REGIONS.iter().enumerate() {
        VirtualRange::new(region.range.start.0, region.range.end)?;
        for previous in &DECLARED_REGIONS[..index] {
            if previous.range.overlaps(region.range) {
                return Err(LayoutError::VirtualOverlap);
            }
        }
    }
    let user = region_range(AddressRegion::UserSpace);
    if user.overlaps(region_range(AddressRegion::DirectMap))
        || user.overlaps(region_range(AddressRegion::KernelImage))
        || user.overlaps(region_range(AddressRegion::KernelLocal))
    {
        return Err(LayoutError::VirtualOverlap);
    }
    Ok(())
}

pub const fn direct_map_address(physical: PhysicalAddress) -> Result<VirtualAddress, LayoutError> {
    match DIRECT_MAP_START.checked_add(physical.0) {
        Some(value) if value < DIRECT_MAP_END => Ok(VirtualAddress(value)),
        _ => Err(LayoutError::UnsupportedPhysicalAddress),
    }
}

fn validate_mapping_policy(
    virtual_range: VirtualRange,
    physical_range: PhysicalRange,
    permissions: MappingPermissions,
    cache: CachePolicy,
    kind: MappingKind,
) -> Result<(), LayoutError> {
    MappingPermissions::from_bits(permissions.bits())?;
    if permissions.user() {
        return Err(LayoutError::UserMappingForbidden);
    }
    if matches!(
        kind,
        MappingKind::KernelText | MappingKind::KernelRodata | MappingKind::KernelData
    ) && (physical_range.start.0 < BOOTSTRAP_PHYSICAL_START
        || physical_range.end > BOOTSTRAP_PHYSICAL_END)
    {
        return Err(LayoutError::OutsideBootstrapPhysicalWindow);
    }
    if matches!(
        kind,
        MappingKind::DirectMap
            | MappingKind::BootInfo
            | MappingKind::BootMemoryMap
            | MappingKind::PageTable
    ) && (DIRECT_MAP_START.checked_add(physical_range.start.0) != Some(virtual_range.start.0)
        || DIRECT_MAP_START.checked_add(physical_range.end) != Some(virtual_range.end))
    {
        return Err(LayoutError::DirectMapAddressMismatch);
    }
    if kind == MappingKind::BootstrapStack
        && (physical_range.length() != BOOTSTRAP_STACK_BYTES
            || physical_range.end > TRANSITION_IDENTITY_END)
    {
        return Err(LayoutError::InvalidBootstrapStack);
    }
    let (region, expected_permissions, expected_cache) = match kind {
        MappingKind::KernelText => (
            AddressRegion::KernelImage,
            MappingPermissions::KERNEL_RX,
            CachePolicy::WriteBack,
        ),
        MappingKind::KernelRodata => (
            AddressRegion::KernelImage,
            MappingPermissions::KERNEL_R,
            CachePolicy::WriteBack,
        ),
        MappingKind::KernelData => (
            AddressRegion::KernelImage,
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
        ),
        MappingKind::BootstrapStack => (
            AddressRegion::KernelLocal,
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
        ),
        MappingKind::BootInfo | MappingKind::BootMemoryMap => (
            AddressRegion::DirectMap,
            MappingPermissions::KERNEL_R,
            CachePolicy::WriteBack,
        ),
        MappingKind::PageTable => (
            AddressRegion::DirectMap,
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
        ),
        MappingKind::DirectMap => {
            if !matches!(
                permissions,
                MappingPermissions::KERNEL_RX
                    | MappingPermissions::KERNEL_R
                    | MappingPermissions::KERNEL_RW
            ) {
                return Err(LayoutError::InvalidRegionPermissions);
            }
            if cache != CachePolicy::WriteBack {
                return Err(LayoutError::InvalidCachePolicy);
            }
            if !region_range(AddressRegion::DirectMap).contains_range(virtual_range) {
                return Err(LayoutError::OutsideDeclaredRegion);
            }
            return Ok(());
        }
        MappingKind::Framebuffer => (
            AddressRegion::Framebuffer,
            MappingPermissions::KERNEL_RW,
            CachePolicy::Uncached,
        ),
        MappingKind::Mmio => (
            AddressRegion::Mmio,
            MappingPermissions::KERNEL_RW,
            CachePolicy::Uncached,
        ),
        MappingKind::TransitionIdentity => {
            if virtual_range.start.0 != physical_range.start.0
                || virtual_range.end != physical_range.end
            {
                return Err(LayoutError::IdentityAddressMismatch);
            }
            if virtual_range.end > TRANSITION_IDENTITY_END {
                return Err(LayoutError::TransitionIdentityAboveLimit);
            }
            if !matches!(
                permissions,
                MappingPermissions::KERNEL_RX
                    | MappingPermissions::KERNEL_R
                    | MappingPermissions::KERNEL_RW
            ) {
                return Err(LayoutError::InvalidRegionPermissions);
            }
            if cache != CachePolicy::WriteBack {
                return Err(LayoutError::InvalidCachePolicy);
            }
            return Ok(());
        }
        MappingKind::Guard => return Err(LayoutError::InvalidRegionPermissions),
    };
    if !region_range(region).contains_range(virtual_range) {
        return Err(LayoutError::OutsideDeclaredRegion);
    }
    if permissions != expected_permissions {
        return Err(LayoutError::InvalidRegionPermissions);
    }
    if cache != expected_cache {
        return Err(LayoutError::InvalidCachePolicy);
    }
    Ok(())
}

const fn validate_order(start: u64, end: u64) -> Result<(), LayoutError> {
    if start == end {
        Err(LayoutError::EmptyRange)
    } else if start > end {
        Err(LayoutError::ReversedRange)
    } else {
        Ok(())
    }
}

const fn validate_page_boundaries(start: u64, end: u64) -> Result<(), LayoutError> {
    if !start.is_multiple_of(PAGE_SIZE) || !end.is_multiple_of(PAGE_SIZE) {
        Err(LayoutError::RangeNotPageAligned)
    } else {
        Ok(())
    }
}

const fn canonical_half(address: u64) -> u8 {
    if address < LOW_CANONICAL_END { 0 } else { 1 }
}

const fn declared(kind: AddressRegion, start: u64, end: u64) -> DeclaredRegion {
    DeclaredRegion {
        kind,
        range: VirtualRange {
            start: VirtualAddress(start),
            end,
        },
    }
}

const fn region_range(kind: AddressRegion) -> VirtualRange {
    match kind {
        AddressRegion::UserSpace => {
            declared(AddressRegion::UserSpace, USER_SPACE_START, USER_SPACE_END).range
        }
        AddressRegion::DirectMap => {
            declared(AddressRegion::DirectMap, DIRECT_MAP_START, DIRECT_MAP_END).range
        }
        AddressRegion::KernelServices => {
            declared(
                AddressRegion::KernelServices,
                KERNEL_SERVICES_START,
                KERNEL_SERVICES_END,
            )
            .range
        }
        AddressRegion::Mmio => declared(AddressRegion::Mmio, MMIO_START, MMIO_END).range,
        AddressRegion::Framebuffer => {
            declared(
                AddressRegion::Framebuffer,
                FRAMEBUFFER_START,
                FRAMEBUFFER_END,
            )
            .range
        }
        AddressRegion::Reserved => {
            declared(
                AddressRegion::Reserved,
                HIGH_RESERVED_START,
                HIGH_RESERVED_END,
            )
            .range
        }
        AddressRegion::KernelImage => {
            declared(
                AddressRegion::KernelImage,
                KERNEL_IMAGE_START,
                KERNEL_IMAGE_END,
            )
            .range
        }
        AddressRegion::KernelLocal => {
            declared(
                AddressRegion::KernelLocal,
                KERNEL_LOCAL_START,
                KERNEL_LOCAL_END,
            )
            .range
        }
    }
}

const _: () = assert!(size_of::<PhysicalAddress>() == 8);
const _: () = assert!(align_of::<PhysicalAddress>() == 8);
const _: () = assert!(size_of::<VirtualAddress>() == 8);
const _: () = assert!(align_of::<VirtualAddress>() == 8);
const _: () = assert!(size_of::<PhysicalRange>() == 16);
const _: () = assert!(align_of::<PhysicalRange>() == 8);
const _: () = assert!(size_of::<VirtualRange>() == 16);
const _: () = assert!(align_of::<VirtualRange>() == 8);
const _: () = assert!(size_of::<MappingPermissions>() == 1);
const _: () = assert!(align_of::<MappingPermissions>() == 1);
