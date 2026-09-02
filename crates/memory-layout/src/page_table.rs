//! Deterministic planning for an inactive four-level x86-64 hierarchy.
//!
//! This module emits scalar construction records only. It never dereferences
//! a physical address, writes table memory, or executes privileged code.

use core::mem::{align_of, size_of};

use crate::{
    CachePolicy, EntryBacking, HIGH_CANONICAL_START, LayoutError, MappingKind, MappingPermissions,
    MappingPlan, PAGE_SIZE, PhysicalAddress, PlanEntry, VirtualAddress,
};

pub const DEFAULT_TABLE_CAPACITY: usize = 256;
pub const DEFAULT_ENTRY_CAPACITY: usize = 4096;
pub const DEFAULT_REMOVAL_CAPACITY: usize = 8;
pub const ABSTRACT_PLAN_HEADER_SIZE: usize = 32;
pub const ABSTRACT_TABLE_SIZE: usize = 16;
pub const ABSTRACT_ENTRY_SIZE: usize = 32;
pub const ABSTRACT_REMOVAL_SIZE: usize = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TableLevel {
    Pt = 1,
    Pd = 2,
    Pdpt = 3,
    Pml4 = 4,
}

impl TableLevel {
    const fn child(self) -> Option<Self> {
        match self {
            Self::Pml4 => Some(Self::Pdpt),
            Self::Pdpt => Some(Self::Pd),
            Self::Pd => Some(Self::Pt),
            Self::Pt => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct TableIndex(u16);

impl TableIndex {
    pub const COUNT: u16 = 512;

    pub const fn new(value: u16) -> Result<Self, PageTablePlanError> {
        if value < Self::COUNT {
            Ok(Self(value))
        } else {
            Err(PageTablePlanError::InvalidTableIndex)
        }
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct FrameSlot(u32);

impl FrameSlot {
    pub const ROOT: Self = Self(0);
    const NONE: Self = Self(u32::MAX);

    pub const fn get(self) -> u32 {
        self.0
    }

    pub fn from_index(index: usize) -> Result<Self, PageTablePlanError> {
        Ok(Self(
            u32::try_from(index).map_err(|_| PageTablePlanError::FrameSlotOverflow)?,
        ))
    }

    pub fn as_index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct PhysicalFrame(u64);

impl PhysicalFrame {
    pub(crate) const EMPTY: Self = Self(0);

    pub fn new(address: u64) -> Result<Self, PageTablePlanError> {
        PhysicalAddress::new(address).map_err(PageTablePlanError::Layout)?;
        if !address.is_multiple_of(PAGE_SIZE) {
            return Err(PageTablePlanError::UnalignedPhysicalFrame);
        }
        Ok(Self(address))
    }

    pub const fn address(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct EntryFlags(u64);

impl EntryFlags {
    pub const PRESENT: u64 = 1 << 0;
    pub const WRITABLE: u64 = 1 << 1;
    pub const USER: u64 = 1 << 2;
    pub const WRITE_THROUGH: u64 = 1 << 3;
    pub const CACHE_DISABLE: u64 = 1 << 4;
    pub const HUGE_PAGE: u64 = 1 << 7;
    pub const GLOBAL: u64 = 1 << 8;
    pub const NO_EXECUTE: u64 = 1 << 63;
    const KNOWN: u64 = Self::PRESENT
        | Self::WRITABLE
        | Self::USER
        | Self::WRITE_THROUGH
        | Self::CACHE_DISABLE
        | Self::GLOBAL
        | Self::NO_EXECUTE;

    pub const INTERMEDIATE: Self = Self(Self::PRESENT | Self::WRITABLE);

    pub const fn from_bits(bits: u64) -> Result<Self, PageTablePlanError> {
        if bits & !Self::KNOWN != 0 || bits & Self::PRESENT == 0 {
            return Err(PageTablePlanError::InvalidEntryFlags);
        }
        if bits & Self::USER != 0 || bits & Self::HUGE_PAGE != 0 {
            return Err(PageTablePlanError::InvalidEntryFlags);
        }
        Ok(Self(bits))
    }

    fn for_leaf(
        permissions: MappingPermissions,
        cache: CachePolicy,
    ) -> Result<Self, PageTablePlanError> {
        if permissions.user() {
            return Err(PageTablePlanError::UserMappingForbidden);
        }
        if permissions.writable() && permissions.executable() {
            return Err(PageTablePlanError::WritableExecutableLeaf);
        }
        let mut bits = Self::PRESENT | Self::GLOBAL;
        if permissions.writable() {
            bits |= Self::WRITABLE;
        }
        if !permissions.executable() {
            bits |= Self::NO_EXECUTE;
        }
        match cache {
            CachePolicy::WriteBack => {}
            CachePolicy::Uncached => bits |= Self::WRITE_THROUGH | Self::CACHE_DISABLE,
        }
        Self::from_bits(bits)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn writable(self) -> bool {
        self.0 & Self::WRITABLE != 0
    }

    pub const fn executable(self) -> bool {
        self.0 & Self::NO_EXECUTE == 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum EntryTargetKind {
    TableSlot = 1,
    PhysicalFrame = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct EntryTarget {
    kind: EntryTargetKind,
    value: u64,
}

impl EntryTarget {
    const fn table(slot: FrameSlot) -> Self {
        Self {
            kind: EntryTargetKind::TableSlot,
            value: slot.0 as u64,
        }
    }

    const fn physical(frame: PhysicalFrame) -> Self {
        Self {
            kind: EntryTargetKind::PhysicalFrame,
            value: frame.0,
        }
    }

    pub const fn kind(self) -> EntryTargetKind {
        self.kind
    }

    pub const fn frame_slot(self) -> Option<FrameSlot> {
        if matches!(self.kind, EntryTargetKind::TableSlot) {
            Some(FrameSlot(self.value as u32))
        } else {
            None
        }
    }

    pub const fn physical_frame(self) -> Option<PhysicalFrame> {
        if matches!(self.kind, EntryTargetKind::PhysicalFrame) {
            Some(PhysicalFrame(self.value))
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct PlannedTable {
    level: TableLevel,
    frame_slot: FrameSlot,
    parent_slot: FrameSlot,
    parent_index: TableIndex,
}

impl PlannedTable {
    const EMPTY: Self = Self {
        level: TableLevel::Pt,
        frame_slot: FrameSlot::NONE,
        parent_slot: FrameSlot::NONE,
        parent_index: TableIndex(0),
    };

    pub const fn level(self) -> TableLevel {
        self.level
    }

    pub const fn frame_slot(self) -> FrameSlot {
        self.frame_slot
    }

    pub const fn parent(self) -> Option<(FrameSlot, TableIndex)> {
        if self.parent_slot.0 == FrameSlot::NONE.0 {
            None
        } else {
            Some((self.parent_slot, self.parent_index))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct PlannedEntry {
    table_slot: FrameSlot,
    index: TableIndex,
    target: EntryTarget,
    flags: EntryFlags,
}

impl PlannedEntry {
    const EMPTY: Self = Self {
        table_slot: FrameSlot::NONE,
        index: TableIndex(0),
        target: EntryTarget::table(FrameSlot::NONE),
        flags: EntryFlags(0),
    };

    pub const fn table_slot(self) -> FrameSlot {
        self.table_slot
    }

    pub const fn index(self) -> TableIndex {
        self.index
    }

    pub const fn target(self) -> EntryTarget {
        self.target
    }

    pub const fn flags(self) -> EntryFlags {
        self.flags
    }

    pub fn encoded_value<const CAPACITY: usize>(
        self,
        assignments: &FrameAssignments<CAPACITY>,
    ) -> Result<u64, PageTablePlanError> {
        let address = match self.target.kind {
            EntryTargetKind::PhysicalFrame => self.target.value,
            EntryTargetKind::TableSlot => assignments
                .frame(FrameSlot(self.target.value as u32))?
                .address(),
        };
        Ok(address | self.flags.bits())
    }

    pub const fn encoded_leaf_value(self) -> Option<u64> {
        if matches!(self.target.kind, EntryTargetKind::PhysicalFrame) {
            Some(self.target.value | self.flags.bits())
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct TransitionRemoval {
    table_slot: FrameSlot,
    index: TableIndex,
    expected_target: FrameSlot,
    expected_flags: EntryFlags,
}

impl TransitionRemoval {
    const EMPTY: Self = Self {
        table_slot: FrameSlot::NONE,
        index: TableIndex(0),
        expected_target: FrameSlot::NONE,
        expected_flags: EntryFlags(0),
    };

    pub const fn table_slot(self) -> FrameSlot {
        self.table_slot
    }

    pub const fn index(self) -> TableIndex {
        self.index
    }

    pub fn expected_value<const CAPACITY: usize>(
        self,
        assignments: &FrameAssignments<CAPACITY>,
    ) -> Result<u64, PageTablePlanError> {
        Ok(assignments.frame(self.expected_target)?.address() | self.expected_flags.bits())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PlanMode {
    Transitional = 1,
    Final = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageTablePlanError {
    Layout(LayoutError),
    InvalidTableIndex,
    UnalignedPhysicalFrame,
    InvalidEntryFlags,
    UserMappingForbidden,
    WritableExecutableLeaf,
    UnsupportedGuardLocation,
    DuplicateLeaf,
    IncompatibleRemap,
    MissingParent,
    TableCapacityExhausted,
    EntryCapacityExhausted,
    RemovalCapacityExhausted,
    FrameSlotOverflow,
    MissingFrameAssignment,
    FrameAssignmentCountMismatch,
    OutputTooSmall,
    ArithmeticOverflow,
}

#[derive(Clone, Copy)]
pub struct FrameAssignments<const CAPACITY: usize> {
    frames: [PhysicalFrame; CAPACITY],
    len: usize,
}

impl<const CAPACITY: usize> FrameAssignments<CAPACITY> {
    pub(crate) const fn from_parts(frames: [PhysicalFrame; CAPACITY], len: usize) -> Self {
        Self { frames, len }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn frame(&self, slot: FrameSlot) -> Result<PhysicalFrame, PageTablePlanError> {
        let index =
            usize::try_from(slot.0).map_err(|_| PageTablePlanError::MissingFrameAssignment)?;
        if index >= self.len {
            return Err(PageTablePlanError::MissingFrameAssignment);
        }
        Ok(self.frames[index])
    }
}

pub struct ConstructionPlan<
    const TABLES: usize = DEFAULT_TABLE_CAPACITY,
    const ENTRIES: usize = DEFAULT_ENTRY_CAPACITY,
    const REMOVALS: usize = DEFAULT_REMOVAL_CAPACITY,
> {
    mode: PlanMode,
    tables: [PlannedTable; TABLES],
    table_count: usize,
    entries: [PlannedEntry; ENTRIES],
    entry_count: usize,
    removals: [TransitionRemoval; REMOVALS],
    removal_count: usize,
}

impl<const TABLES: usize, const ENTRIES: usize, const REMOVALS: usize>
    ConstructionPlan<TABLES, ENTRIES, REMOVALS>
{
    pub fn build<const INPUT_CAPACITY: usize>(
        mappings: &MappingPlan<INPUT_CAPACITY>,
        mode: PlanMode,
    ) -> Result<Self, PageTablePlanError> {
        match mode {
            PlanMode::Transitional => mappings.validate_transition(),
            PlanMode::Final => mappings.validate_final(),
        }
        .map_err(PageTablePlanError::Layout)?;

        let mut plan = Self::begin(mode)?;
        for mapping in mappings.entries() {
            plan.add_mapping(*mapping)?;
        }
        plan.finish()
    }

    fn begin(mode: PlanMode) -> Result<Self, PageTablePlanError> {
        if TABLES == 0 {
            return Err(PageTablePlanError::TableCapacityExhausted);
        }
        let mut result = Self {
            mode,
            tables: [PlannedTable::EMPTY; TABLES],
            table_count: 1,
            entries: [PlannedEntry::EMPTY; ENTRIES],
            entry_count: 0,
            removals: [TransitionRemoval::EMPTY; REMOVALS],
            removal_count: 0,
        };
        result.tables[0] = PlannedTable {
            level: TableLevel::Pml4,
            frame_slot: FrameSlot::ROOT,
            parent_slot: FrameSlot::NONE,
            parent_index: TableIndex(0),
        };
        Ok(result)
    }

    fn add_mapping(&mut self, mapping: PlanEntry) -> Result<(), PageTablePlanError> {
        if self.mode == PlanMode::Final
            && (mapping.virtual_range().start().get() < HIGH_CANONICAL_START
                || mapping.kind() == MappingKind::TransitionIdentity)
        {
            return Err(PageTablePlanError::Layout(
                LayoutError::LowMappingInFinalPlan,
            ));
        }
        let EntryBacking::Mapped(physical) = mapping.backing() else {
            if mapping.kind() != MappingKind::Guard {
                return Err(PageTablePlanError::UnsupportedGuardLocation);
            }
            return Ok(());
        };
        let permissions = mapping
            .permissions()
            .ok_or(PageTablePlanError::InvalidEntryFlags)?;
        let cache = mapping
            .cache_policy()
            .ok_or(PageTablePlanError::InvalidEntryFlags)?;
        let page_count = mapping.virtual_range().length() / PAGE_SIZE;
        for page in 0..page_count {
            let offset = page
                .checked_mul(PAGE_SIZE)
                .ok_or(PageTablePlanError::ArithmeticOverflow)?;
            let virtual_address = mapping
                .virtual_range()
                .start()
                .get()
                .checked_add(offset)
                .ok_or(PageTablePlanError::ArithmeticOverflow)?;
            let physical_address = physical
                .start()
                .get()
                .checked_add(offset)
                .ok_or(PageTablePlanError::ArithmeticOverflow)?;
            self.insert_page(
                VirtualAddress::new(virtual_address).map_err(PageTablePlanError::Layout)?,
                PhysicalFrame::new(physical_address)?,
                permissions,
                cache,
            )?;
        }
        Ok(())
    }

    fn finish(self) -> Result<Self, PageTablePlanError> {
        self.validate_structure()?;
        Ok(self)
    }

    fn insert_page(
        &mut self,
        virtual_address: VirtualAddress,
        physical_frame: PhysicalFrame,
        permissions: MappingPermissions,
        cache: CachePolicy,
    ) -> Result<(), PageTablePlanError> {
        let indices = virtual_address_indices(virtual_address);
        let mut table_slot = FrameSlot::ROOT;
        for (depth, index) in indices[..3].iter().copied().enumerate() {
            let level = match depth {
                0 => TableLevel::Pml4,
                1 => TableLevel::Pdpt,
                _ => TableLevel::Pd,
            };
            table_slot = self.ensure_child(table_slot, level, index, virtual_address)?;
        }

        let leaf = PlannedEntry {
            table_slot,
            index: indices[3],
            target: EntryTarget::physical(physical_frame),
            flags: EntryFlags::for_leaf(permissions, cache)?,
        };
        if let Some(existing) = self.find_entry(table_slot, indices[3]) {
            return if existing == leaf {
                Err(PageTablePlanError::DuplicateLeaf)
            } else {
                Err(PageTablePlanError::IncompatibleRemap)
            };
        }
        self.push_entry(leaf)
    }

    fn ensure_child(
        &mut self,
        parent_slot: FrameSlot,
        parent_level: TableLevel,
        index: TableIndex,
        virtual_address: VirtualAddress,
    ) -> Result<FrameSlot, PageTablePlanError> {
        if let Some(existing) = self.find_entry(parent_slot, index) {
            return existing
                .target
                .frame_slot()
                .ok_or(PageTablePlanError::IncompatibleRemap);
        }
        if self.table_count == TABLES {
            return Err(PageTablePlanError::TableCapacityExhausted);
        }
        let slot_value =
            u32::try_from(self.table_count).map_err(|_| PageTablePlanError::FrameSlotOverflow)?;
        let child_slot = FrameSlot(slot_value);
        let child_level = parent_level
            .child()
            .ok_or(PageTablePlanError::MissingParent)?;
        self.tables[self.table_count] = PlannedTable {
            level: child_level,
            frame_slot: child_slot,
            parent_slot,
            parent_index: index,
        };
        self.table_count += 1;
        let intermediate = PlannedEntry {
            table_slot: parent_slot,
            index,
            target: EntryTarget::table(child_slot),
            flags: EntryFlags::INTERMEDIATE,
        };
        self.push_entry(intermediate)?;
        if parent_level == TableLevel::Pml4 && virtual_address.get() < HIGH_CANONICAL_START {
            self.push_removal(intermediate)?;
        }
        Ok(child_slot)
    }

    fn find_entry(&self, table_slot: FrameSlot, index: TableIndex) -> Option<PlannedEntry> {
        self.entries()
            .iter()
            .copied()
            .find(|entry| entry.table_slot == table_slot && entry.index == index)
    }

    fn push_entry(&mut self, entry: PlannedEntry) -> Result<(), PageTablePlanError> {
        if self.entry_count == ENTRIES {
            return Err(PageTablePlanError::EntryCapacityExhausted);
        }
        self.entries[self.entry_count] = entry;
        self.entry_count += 1;
        Ok(())
    }

    fn push_removal(&mut self, entry: PlannedEntry) -> Result<(), PageTablePlanError> {
        if self.removal_count == REMOVALS {
            return Err(PageTablePlanError::RemovalCapacityExhausted);
        }
        self.removals[self.removal_count] = TransitionRemoval {
            table_slot: entry.table_slot,
            index: entry.index,
            expected_target: entry
                .target
                .frame_slot()
                .ok_or(PageTablePlanError::MissingParent)?,
            expected_flags: entry.flags,
        };
        self.removal_count += 1;
        Ok(())
    }

    fn validate_structure(&self) -> Result<(), PageTablePlanError> {
        for table in &self.tables()[1..] {
            let (parent, index) = table.parent().ok_or(PageTablePlanError::MissingParent)?;
            let entry = self
                .find_entry(parent, index)
                .ok_or(PageTablePlanError::MissingParent)?;
            if entry.target.frame_slot() != Some(table.frame_slot)
                || entry.flags != EntryFlags::INTERMEDIATE
            {
                return Err(PageTablePlanError::MissingParent);
            }
        }
        Ok(())
    }

    pub const fn mode(&self) -> PlanMode {
        self.mode
    }

    pub const fn root_frame_slot(&self) -> FrameSlot {
        FrameSlot::ROOT
    }

    pub const fn table_count(&self) -> usize {
        self.table_count
    }

    pub const fn entry_count(&self) -> usize {
        self.entry_count
    }

    pub const fn removal_count(&self) -> usize {
        self.removal_count
    }

    pub fn tables(&self) -> &[PlannedTable] {
        &self.tables[..self.table_count]
    }

    pub fn entries(&self) -> &[PlannedEntry] {
        &self.entries[..self.entry_count]
    }

    pub fn transition_removals(&self) -> &[TransitionRemoval] {
        &self.removals[..self.removal_count]
    }

    pub fn leaf_entry(&self, address: VirtualAddress) -> Option<PlannedEntry> {
        let indices = virtual_address_indices(address);
        let mut slot = FrameSlot::ROOT;
        for index in indices[..3].iter().copied() {
            slot = self.find_entry(slot, index)?.target.frame_slot()?;
        }
        let entry = self.find_entry(slot, indices[3])?;
        if entry.target.kind == EntryTargetKind::PhysicalFrame {
            Some(entry)
        } else {
            None
        }
    }

    pub fn encoded_entries<const CAPACITY: usize>(
        &self,
        assignments: &FrameAssignments<CAPACITY>,
        output: &mut [u64],
    ) -> Result<usize, PageTablePlanError> {
        if assignments.len() != self.table_count {
            return Err(PageTablePlanError::FrameAssignmentCountMismatch);
        }
        if output.len() < self.entry_count {
            return Err(PageTablePlanError::EntryCapacityExhausted);
        }
        for (encoded, entry) in output.iter_mut().zip(self.entries()) {
            *encoded = entry.encoded_value(assignments)?;
        }
        Ok(self.entry_count)
    }

    pub fn abstract_byte_len(&self) -> Result<usize, PageTablePlanError> {
        ABSTRACT_PLAN_HEADER_SIZE
            .checked_add(
                self.table_count
                    .checked_mul(ABSTRACT_TABLE_SIZE)
                    .ok_or(PageTablePlanError::ArithmeticOverflow)?,
            )
            .and_then(|value| {
                self.entry_count
                    .checked_mul(ABSTRACT_ENTRY_SIZE)
                    .and_then(|size| value.checked_add(size))
            })
            .and_then(|value| {
                self.removal_count
                    .checked_mul(ABSTRACT_REMOVAL_SIZE)
                    .and_then(|size| value.checked_add(size))
            })
            .ok_or(PageTablePlanError::ArithmeticOverflow)
    }

    pub fn encode_abstract(&self, output: &mut [u8]) -> Result<usize, PageTablePlanError> {
        let required = self.abstract_byte_len()?;
        if output.len() < required {
            return Err(PageTablePlanError::OutputTooSmall);
        }
        output[..required].fill(0);
        put_u16(output, 0, 1);
        output[2] = self.mode as u8;
        put_u32(
            output,
            4,
            u32::try_from(self.table_count).map_err(|_| PageTablePlanError::ArithmeticOverflow)?,
        );
        put_u32(
            output,
            8,
            u32::try_from(self.entry_count).map_err(|_| PageTablePlanError::ArithmeticOverflow)?,
        );
        put_u32(
            output,
            12,
            u32::try_from(self.removal_count)
                .map_err(|_| PageTablePlanError::ArithmeticOverflow)?,
        );
        put_u32(output, 16, FrameSlot::ROOT.get());

        let mut offset = ABSTRACT_PLAN_HEADER_SIZE;
        for table in self.tables() {
            output[offset] = table.level as u8;
            put_u32(output, offset + 4, table.frame_slot.get());
            put_u32(output, offset + 8, table.parent_slot.get());
            put_u16(output, offset + 12, table.parent_index.get());
            offset += ABSTRACT_TABLE_SIZE;
        }
        for entry in self.entries() {
            put_u32(output, offset, entry.table_slot.get());
            put_u16(output, offset + 4, entry.index.get());
            output[offset + 6] = entry.target.kind as u8;
            put_u64(output, offset + 8, entry.target.value);
            put_u64(output, offset + 16, entry.flags.bits());
            offset += ABSTRACT_ENTRY_SIZE;
        }
        for removal in self.transition_removals() {
            put_u32(output, offset, removal.table_slot.get());
            put_u16(output, offset + 4, removal.index.get());
            put_u32(output, offset + 8, removal.expected_target.get());
            put_u64(output, offset + 16, removal.expected_flags.bits());
            offset += ABSTRACT_REMOVAL_SIZE;
        }
        Ok(required)
    }
}

pub fn virtual_address_indices(address: VirtualAddress) -> [TableIndex; 4] {
    [
        TableIndex(((address.get() >> 39) & 0x1ff) as u16),
        TableIndex(((address.get() >> 30) & 0x1ff) as u16),
        TableIndex(((address.get() >> 21) & 0x1ff) as u16),
        TableIndex(((address.get() >> 12) & 0x1ff) as u16),
    ]
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

const _: () = assert!(size_of::<TableIndex>() == 2);
const _: () = assert!(align_of::<TableIndex>() == 2);
const _: () = assert!(size_of::<FrameSlot>() == 4);
const _: () = assert!(align_of::<FrameSlot>() == 4);
const _: () = assert!(size_of::<PhysicalFrame>() == 8);
const _: () = assert!(align_of::<PhysicalFrame>() == 8);
const _: () = assert!(size_of::<EntryFlags>() == 8);
const _: () = assert!(align_of::<EntryFlags>() == 8);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BOOTSTRAP_PHYSICAL_START, KERNEL_IMAGE_START, PhysicalRange, VirtualRange};

    fn source_entry(physical: u64) -> PlanEntry {
        let mut mappings = MappingPlan::<1>::new();
        mappings
            .insert_mapping(
                VirtualRange::from_start_and_length(KERNEL_IMAGE_START, PAGE_SIZE).unwrap(),
                PhysicalRange::from_start_and_length(physical, PAGE_SIZE).unwrap(),
                MappingPermissions::KERNEL_RX,
                CachePolicy::WriteBack,
                MappingKind::KernelText,
            )
            .unwrap();
        mappings.entries()[0]
    }

    #[test]
    fn duplicate_and_incompatible_remaps_are_rejected_defensively() {
        let first = source_entry(BOOTSTRAP_PHYSICAL_START);
        let mut duplicate = ConstructionPlan::<8, 8, 1>::begin(PlanMode::Final).unwrap();
        duplicate.add_mapping(first).unwrap();
        assert_eq!(
            duplicate.add_mapping(first),
            Err(PageTablePlanError::DuplicateLeaf)
        );

        let mut incompatible = ConstructionPlan::<8, 8, 1>::begin(PlanMode::Final).unwrap();
        incompatible.add_mapping(first).unwrap();
        assert_eq!(
            incompatible.add_mapping(source_entry(BOOTSTRAP_PHYSICAL_START + PAGE_SIZE)),
            Err(PageTablePlanError::IncompatibleRemap)
        );
    }
}
