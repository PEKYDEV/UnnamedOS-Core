//! Inactive page-table construction, verification, and ownership typestate.

use boot_protocol::{MEMORY_KIND_PAGE_TABLE, MEMORY_PAGE_SIZE, MemoryDescriptor};
use memory_layout::{
    BOOTSTRAP_STACK_BYTES, CachePolicy, ConstructionPlan, EntryFlags, EntryTargetKind,
    FrameBackend, FrameOwnerBuildError, FrameSlot, MappingKind, MappingPermissions, MappingPlan,
    PAGE_SIZE, PageTableFrameOwner, PageTablePlanError, PhysicalFrame, PhysicalRange, PlanMode,
    PlannedEntry, TableIndex, TableLevel, TransferredPageTableFrames, VirtualRange,
};

use crate::{MapBuildError, Reservation, ReservationList, ReservationSource};

pub const PAGE_TABLE_FRAME_CAPACITY: usize = 8;
pub const PAGE_TABLE_ENTRY_CAPACITY: usize = 32;
pub const PAGE_TABLE_REMOVAL_CAPACITY: usize = 1;
pub const PAGE_TABLE_ENTRIES_PER_FRAME: usize = 512;

pub type RuntimePageTablePlan = ConstructionPlan<
    PAGE_TABLE_FRAME_CAPACITY,
    PAGE_TABLE_ENTRY_CAPACITY,
    PAGE_TABLE_REMOVAL_CAPACITY,
>;

/// Safe scalar interface implemented by the narrowly scoped physical-memory
/// adapter. Callers provide only frames retained by the page-table owner.
pub trait PageTableMemory {
    type Error;

    fn write_entry(
        &mut self,
        frame: PhysicalFrame,
        index: TableIndex,
        value: u64,
    ) -> Result<(), Self::Error>;
    fn read_entry(&self, frame: PhysicalFrame, index: TableIndex) -> Result<u64, Self::Error>;
}

#[derive(Debug, Eq, PartialEq)]
pub enum MaterializationError<E> {
    Plan(PageTablePlanError),
    Access {
        table_slot: FrameSlot,
        entry_index: TableIndex,
        source: E,
    },
}

#[derive(Debug, Eq, PartialEq)]
pub enum VerificationError<E> {
    Plan(PageTablePlanError),
    OwnershipCount,
    RootFrame,
    Access {
        table_slot: FrameSlot,
        entry_index: TableIndex,
        source: E,
    },
    InvalidBits {
        table_slot: FrameSlot,
        entry_index: TableIndex,
    },
    UnexpectedNonZero {
        table_slot: FrameSlot,
        entry_index: TableIndex,
    },
    EntryMismatch {
        table_slot: FrameSlot,
        entry_index: TableIndex,
    },
    UnownedChild {
        table_slot: FrameSlot,
        entry_index: TableIndex,
    },
    Cycle {
        table_slot: FrameSlot,
        entry_index: TableIndex,
    },
    WrongChild {
        table_slot: FrameSlot,
        entry_index: TableIndex,
    },
    WrongTableLevel {
        table_slot: FrameSlot,
        entry_index: TableIndex,
    },
    UnreachableTable {
        table_slot: FrameSlot,
    },
    RemovalMismatch {
        table_slot: FrameSlot,
        entry_index: TableIndex,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageTableReservationError {
    Map(MapBuildError),
    IncompleteCoverage { frame_slot: FrameSlot },
    UsableFrame { frame_slot: FrameSlot },
    WrongKind { frame_slot: FrameSlot },
    RangeOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageTableReservationProof {
    frame_count: usize,
    root_frame: u64,
    fingerprint: u64,
}

pub struct PlannedPageTables {
    plan: RuntimePageTablePlan,
}

pub struct AllocatedPageTables<B: FrameBackend> {
    plan: RuntimePageTablePlan,
    owner: PageTableFrameOwner<B, PAGE_TABLE_FRAME_CAPACITY>,
}

pub struct MaterializedPageTables<B: FrameBackend> {
    plan: RuntimePageTablePlan,
    owner: PageTableFrameOwner<B, PAGE_TABLE_FRAME_CAPACITY>,
}

pub struct VerifiedInactivePageTables<B: FrameBackend> {
    planned_table_count: usize,
    owner: PageTableFrameOwner<B, PAGE_TABLE_FRAME_CAPACITY>,
}

pub struct FinalMapReservedPageTables<B: FrameBackend> {
    planned_table_count: usize,
    owner: PageTableFrameOwner<B, PAGE_TABLE_FRAME_CAPACITY>,
}

#[must_use]
pub struct TransferredInactivePageTables {
    frames: TransferredPageTableFrames<PAGE_TABLE_FRAME_CAPACITY>,
}

impl PlannedPageTables {
    pub fn for_transition(
        trampoline_page: u64,
        stack_start: u64,
    ) -> Result<Self, PageTablePlanError> {
        let mut mappings = MappingPlan::<2>::new();
        mappings
            .insert_mapping(
                VirtualRange::from_start_and_length(trampoline_page, PAGE_SIZE)
                    .map_err(PageTablePlanError::Layout)?,
                PhysicalRange::from_start_and_length(trampoline_page, PAGE_SIZE)
                    .map_err(PageTablePlanError::Layout)?,
                MappingPermissions::KERNEL_RX,
                CachePolicy::WriteBack,
                MappingKind::TransitionIdentity,
            )
            .map_err(PageTablePlanError::Layout)?;
        mappings
            .insert_mapping(
                VirtualRange::from_start_and_length(stack_start, BOOTSTRAP_STACK_BYTES)
                    .map_err(PageTablePlanError::Layout)?,
                PhysicalRange::from_start_and_length(stack_start, BOOTSTRAP_STACK_BYTES)
                    .map_err(PageTablePlanError::Layout)?,
                MappingPermissions::KERNEL_RW,
                CachePolicy::WriteBack,
                MappingKind::TransitionIdentity,
            )
            .map_err(PageTablePlanError::Layout)?;
        Ok(Self {
            plan: RuntimePageTablePlan::build(&mappings, PlanMode::Transitional)?,
        })
    }

    pub const fn table_count(&self) -> usize {
        self.plan.table_count()
    }

    pub fn allocate<B: FrameBackend>(
        self,
        backend: B,
    ) -> Result<AllocatedPageTables<B>, FrameOwnerBuildError<B, PAGE_TABLE_FRAME_CAPACITY>> {
        let owner = PageTableFrameOwner::allocate(&self.plan, backend)?;
        Ok(AllocatedPageTables {
            plan: self.plan,
            owner,
        })
    }
}

impl<B: FrameBackend> AllocatedPageTables<B> {
    pub fn materialize<M: PageTableMemory>(
        self,
        memory: &mut M,
    ) -> Result<MaterializedPageTables<B>, MaterializationError<M::Error>> {
        let assignments = self.owner.assignments();
        for table in self.plan.tables() {
            let frame = assignments
                .frame(table.frame_slot())
                .map_err(MaterializationError::Plan)?;
            for raw_index in 0..PAGE_TABLE_ENTRIES_PER_FRAME {
                let index = TableIndex::new(raw_index as u16)
                    .expect("the fixed page-table bound is 512 entries");
                let value = expected_entry(&self.plan, table.frame_slot(), index)
                    .map(|entry| entry.encoded_value(&assignments))
                    .transpose()
                    .map_err(MaterializationError::Plan)?
                    .unwrap_or(0)
                    .to_le();
                memory.write_entry(frame, index, value).map_err(|source| {
                    MaterializationError::Access {
                        table_slot: table.frame_slot(),
                        entry_index: index,
                        source,
                    }
                })?;
            }
        }
        Ok(MaterializedPageTables {
            plan: self.plan,
            owner: self.owner,
        })
    }
}

impl<B: FrameBackend> MaterializedPageTables<B> {
    pub fn verify<M: PageTableMemory>(
        self,
        memory: &M,
    ) -> Result<VerifiedInactivePageTables<B>, VerificationError<M::Error>> {
        verify_materialized(&self.plan, &self.owner, memory)?;
        Ok(VerifiedInactivePageTables {
            planned_table_count: self.plan.table_count(),
            owner: self.owner,
        })
    }
}

impl<B: FrameBackend> VerifiedInactivePageTables<B> {
    pub const fn frame_count(&self) -> usize {
        self.owner.frame_count()
    }

    pub const fn root_frame(&self) -> PhysicalFrame {
        self.owner.root_frame()
    }

    pub fn frames(&self) -> &[PhysicalFrame] {
        self.owner.frames()
    }

    pub fn append_reservations(
        &self,
        reservations: &mut ReservationList,
    ) -> Result<(), PageTableReservationError> {
        append_page_table_reservations(self.owner.frames(), reservations)
    }

    pub fn confirm_final_map_reservation(
        self,
        proof: PageTableReservationProof,
    ) -> Result<FinalMapReservedPageTables<B>, PageTableReservationError> {
        if proof.frame_count != self.owner.frame_count()
            || proof.root_frame != self.owner.root_frame().address()
            || proof.fingerprint != frame_fingerprint(self.owner.frames())
        {
            return Err(PageTableReservationError::IncompleteCoverage {
                frame_slot: FrameSlot::ROOT,
            });
        }
        Ok(FinalMapReservedPageTables {
            planned_table_count: self.planned_table_count,
            owner: self.owner,
        })
    }
}

impl<B: FrameBackend> FinalMapReservedPageTables<B> {
    pub const fn frame_count(&self) -> usize {
        self.owner.frame_count()
    }

    pub const fn root_frame(&self) -> PhysicalFrame {
        self.owner.root_frame()
    }

    pub fn frames(&self) -> &[PhysicalFrame] {
        self.owner.frames()
    }

    pub const fn planned_table_count(&self) -> usize {
        self.planned_table_count
    }

    pub fn transfer(self) -> TransferredInactivePageTables {
        TransferredInactivePageTables {
            frames: self.owner.transfer(),
        }
    }
}

impl TransferredInactivePageTables {
    pub const fn frame_count(&self) -> usize {
        self.frames.frame_count()
    }

    pub const fn root_frame(&self) -> PhysicalFrame {
        self.frames.root_frame()
    }

    pub fn frames(&self) -> &[PhysicalFrame] {
        self.frames.frames()
    }
}

fn expected_entry(
    plan: &RuntimePageTablePlan,
    table_slot: FrameSlot,
    entry_index: TableIndex,
) -> Option<PlannedEntry> {
    plan.entries()
        .iter()
        .copied()
        .find(|entry| entry.table_slot() == table_slot && entry.index() == entry_index)
}

fn verify_materialized<B: FrameBackend, M: PageTableMemory>(
    plan: &RuntimePageTablePlan,
    owner: &PageTableFrameOwner<B, PAGE_TABLE_FRAME_CAPACITY>,
    memory: &M,
) -> Result<(), VerificationError<M::Error>> {
    if owner.frame_count() != plan.table_count() {
        return Err(VerificationError::OwnershipCount);
    }
    let assignments = owner.assignments();
    if assignments
        .frame(plan.root_frame_slot())
        .map_err(VerificationError::Plan)?
        != owner.root_frame()
    {
        return Err(VerificationError::RootFrame);
    }

    let mut reachable = [false; PAGE_TABLE_FRAME_CAPACITY];
    reachable[plan.root_frame_slot().as_index()] = true;
    for table in plan.tables() {
        let slot = table.frame_slot();
        let frame = assignments.frame(slot).map_err(VerificationError::Plan)?;
        for raw_index in 0..PAGE_TABLE_ENTRIES_PER_FRAME {
            let index = TableIndex::new(raw_index as u16)
                .expect("the fixed page-table bound is 512 entries");
            let actual = memory
                .read_entry(frame, index)
                .map_err(|source| VerificationError::Access {
                    table_slot: slot,
                    entry_index: index,
                    source,
                })?
                .to_le();
            let expected = expected_entry(plan, slot, index);
            if actual == 0 {
                if expected
                    .is_some_and(|entry| entry.target().kind() == EntryTargetKind::PhysicalFrame)
                {
                    return Err(VerificationError::EntryMismatch {
                        table_slot: slot,
                        entry_index: index,
                    });
                }
                continue;
            }
            validate_entry_bits(actual, expected, slot, index)?;
            let Some(expected) = expected else {
                return Err(VerificationError::UnexpectedNonZero {
                    table_slot: slot,
                    entry_index: index,
                });
            };
            if expected.target().kind() == EntryTargetKind::TableSlot {
                let address = actual & ENTRY_ADDRESS_MASK;
                let Some(actual_child) = owner
                    .frames()
                    .iter()
                    .position(|candidate| candidate.address() == address)
                    .and_then(|position| FrameSlot::from_index(position).ok())
                else {
                    return Err(VerificationError::UnownedChild {
                        table_slot: slot,
                        entry_index: index,
                    });
                };
                let expected_child =
                    expected
                        .target()
                        .frame_slot()
                        .ok_or(VerificationError::WrongChild {
                            table_slot: slot,
                            entry_index: index,
                        })?;
                if actual_child != expected_child {
                    if actual_child == slot || is_ancestor(plan, actual_child, slot) {
                        return Err(VerificationError::Cycle {
                            table_slot: slot,
                            entry_index: index,
                        });
                    }
                    let actual_table = plan.tables().get(actual_child.as_index()).ok_or(
                        VerificationError::WrongChild {
                            table_slot: slot,
                            entry_index: index,
                        },
                    )?;
                    if !levels_are_parent_child(table.level(), actual_table.level()) {
                        return Err(VerificationError::WrongTableLevel {
                            table_slot: slot,
                            entry_index: index,
                        });
                    }
                    return Err(VerificationError::WrongChild {
                        table_slot: slot,
                        entry_index: index,
                    });
                }
                let child = plan.tables().get(actual_child.as_index()).ok_or(
                    VerificationError::WrongChild {
                        table_slot: slot,
                        entry_index: index,
                    },
                )?;
                if !levels_are_parent_child(table.level(), child.level()) {
                    return Err(VerificationError::WrongTableLevel {
                        table_slot: slot,
                        entry_index: index,
                    });
                }
                reachable[actual_child.as_index()] = true;
            }
            let expected_value = expected
                .encoded_value(&assignments)
                .map_err(VerificationError::Plan)?;
            if actual != expected_value {
                return Err(VerificationError::EntryMismatch {
                    table_slot: slot,
                    entry_index: index,
                });
            }
        }
    }
    for table in &plan.tables()[1..] {
        if !reachable[table.frame_slot().as_index()] {
            return Err(VerificationError::UnreachableTable {
                table_slot: table.frame_slot(),
            });
        }
    }
    for removal in plan.transition_removals() {
        let root = assignments
            .frame(removal.table_slot())
            .map_err(VerificationError::Plan)?;
        let actual = memory
            .read_entry(root, removal.index())
            .map_err(|source| VerificationError::Access {
                table_slot: removal.table_slot(),
                entry_index: removal.index(),
                source,
            })?
            .to_le();
        let expected = removal
            .expected_value(&assignments)
            .map_err(VerificationError::Plan)?;
        if actual != expected {
            return Err(VerificationError::RemovalMismatch {
                table_slot: removal.table_slot(),
                entry_index: removal.index(),
            });
        }
    }
    Ok(())
}

const ENTRY_ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;
const ENTRY_FLAG_MASK: u64 = EntryFlags::PRESENT
    | EntryFlags::WRITABLE
    | EntryFlags::WRITE_THROUGH
    | EntryFlags::CACHE_DISABLE
    | EntryFlags::GLOBAL
    | EntryFlags::NO_EXECUTE;

fn validate_entry_bits<E>(
    value: u64,
    expected: Option<PlannedEntry>,
    table_slot: FrameSlot,
    entry_index: TableIndex,
) -> Result<(), VerificationError<E>> {
    if value & EntryFlags::PRESENT == 0
        || value & EntryFlags::USER != 0
        || value & EntryFlags::HUGE_PAGE != 0
        || value & !(ENTRY_ADDRESS_MASK | ENTRY_FLAG_MASK) != 0
        || value & ENTRY_ADDRESS_MASK >= memory_layout::SUPPORTED_PHYSICAL_END
    {
        return Err(VerificationError::InvalidBits {
            table_slot,
            entry_index,
        });
    }
    if expected.is_some_and(|entry| entry.target().kind() == EntryTargetKind::PhysicalFrame)
        && value & EntryFlags::WRITABLE != 0
        && value & EntryFlags::NO_EXECUTE == 0
    {
        return Err(VerificationError::InvalidBits {
            table_slot,
            entry_index,
        });
    }
    Ok(())
}

fn levels_are_parent_child(parent: TableLevel, child: TableLevel) -> bool {
    matches!(
        (parent, child),
        (TableLevel::Pml4, TableLevel::Pdpt)
            | (TableLevel::Pdpt, TableLevel::Pd)
            | (TableLevel::Pd, TableLevel::Pt)
    )
}

fn is_ancestor(plan: &RuntimePageTablePlan, candidate: FrameSlot, mut slot: FrameSlot) -> bool {
    while let Some((parent, _)) = plan.tables()[slot.as_index()].parent() {
        if parent == candidate {
            return true;
        }
        slot = parent;
    }
    false
}

pub fn append_page_table_reservations(
    frames: &[PhysicalFrame],
    reservations: &mut ReservationList,
) -> Result<(), PageTableReservationError> {
    if frames.is_empty() || frames.len() > PAGE_TABLE_FRAME_CAPACITY {
        return Err(PageTableReservationError::IncompleteCoverage {
            frame_slot: FrameSlot::ROOT,
        });
    }
    let mut sorted = [frames[0]; PAGE_TABLE_FRAME_CAPACITY];
    sorted[..frames.len()].copy_from_slice(frames);
    for index in 1..frames.len() {
        let value = sorted[index];
        let mut position = index;
        while position > 0 && sorted[position - 1].address() > value.address() {
            sorted[position] = sorted[position - 1];
            position -= 1;
        }
        sorted[position] = value;
    }
    let mut start = sorted[0].address();
    let mut pages = 1_u64;
    for frame in &sorted[1..frames.len()] {
        let expected = start
            .checked_add(
                pages
                    .checked_mul(MEMORY_PAGE_SIZE)
                    .ok_or(PageTableReservationError::RangeOverflow)?,
            )
            .ok_or(PageTableReservationError::RangeOverflow)?;
        if frame.address() == expected {
            pages = pages
                .checked_add(1)
                .ok_or(PageTableReservationError::RangeOverflow)?;
        } else {
            reservations
                .push(Reservation {
                    physical_start: start,
                    page_count: pages,
                    kind: MEMORY_KIND_PAGE_TABLE,
                    source: ReservationSource::PageTable,
                })
                .map_err(PageTableReservationError::Map)?;
            start = frame.address();
            pages = 1;
        }
    }
    reservations
        .push(Reservation {
            physical_start: start,
            page_count: pages,
            kind: MEMORY_KIND_PAGE_TABLE,
            source: ReservationSource::PageTable,
        })
        .map_err(PageTableReservationError::Map)
}

pub fn verify_page_table_reservations(
    frames: &[PhysicalFrame],
    descriptors: &[MemoryDescriptor],
) -> Result<PageTableReservationProof, PageTableReservationError> {
    if frames.is_empty() {
        return Err(PageTableReservationError::IncompleteCoverage {
            frame_slot: FrameSlot::ROOT,
        });
    }
    for (position, frame) in frames.iter().enumerate() {
        let slot = FrameSlot::from_index(position).map_err(|_| {
            PageTableReservationError::IncompleteCoverage {
                frame_slot: FrameSlot::ROOT,
            }
        })?;
        let mut covering = None;
        for descriptor in descriptors {
            let length = descriptor
                .page_count
                .checked_mul(MEMORY_PAGE_SIZE)
                .ok_or(PageTableReservationError::RangeOverflow)?;
            let end = descriptor
                .physical_start
                .checked_add(length)
                .ok_or(PageTableReservationError::RangeOverflow)?;
            if descriptor.physical_start <= frame.address() && frame.address() < end {
                covering = Some(descriptor.kind);
                break;
            }
        }
        let Some(kind) = covering else {
            return Err(PageTableReservationError::IncompleteCoverage { frame_slot: slot });
        };
        if kind == boot_protocol::MEMORY_KIND_USABLE {
            return Err(PageTableReservationError::UsableFrame { frame_slot: slot });
        }
        if kind != MEMORY_KIND_PAGE_TABLE {
            return Err(PageTableReservationError::WrongKind { frame_slot: slot });
        }
    }
    Ok(PageTableReservationProof {
        frame_count: frames.len(),
        root_frame: frames[0].address(),
        fingerprint: frame_fingerprint(frames),
    })
}

fn frame_fingerprint(frames: &[PhysicalFrame]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for frame in frames {
        for byte in frame.address().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::apply_reservations;
    use boot_protocol::{MEMORY_KIND_USABLE, MemoryDescriptor};
    use std::{cell::RefCell, rc::Rc, vec, vec::Vec};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum FakeError {
        Allocate,
        Zero,
        Free,
        Access,
    }

    #[derive(Clone)]
    struct FakeState {
        available: Vec<u64>,
        pages: Vec<(u64, Vec<u64>)>,
        allocated: Vec<u64>,
        freed: Vec<u64>,
        fail_allocate: Option<usize>,
        fail_zero: Option<usize>,
        fail_free_once: Option<u64>,
        writes: usize,
        fail_write: Option<usize>,
    }

    impl FakeState {
        fn new(available: &[u64]) -> Self {
            Self {
                available: available.to_vec(),
                pages: Vec::new(),
                allocated: Vec::new(),
                freed: Vec::new(),
                fail_allocate: None,
                fail_zero: None,
                fail_free_once: None,
                writes: 0,
                fail_write: None,
            }
        }

        fn page(&self, address: u64) -> Option<&[u64]> {
            self.pages
                .iter()
                .find(|(candidate, _)| *candidate == address)
                .map(|(_, page)| page.as_slice())
        }

        fn page_mut(&mut self, address: u64) -> Option<&mut [u64]> {
            self.pages
                .iter_mut()
                .find(|(candidate, _)| *candidate == address)
                .map(|(_, page)| page.as_mut_slice())
        }
    }

    #[derive(Clone)]
    struct FakeBackend(Rc<RefCell<FakeState>>);

    impl FrameBackend for FakeBackend {
        type Error = FakeError;

        fn allocate_frame(&mut self) -> Result<u64, Self::Error> {
            let mut state = self.0.borrow_mut();
            if state.fail_allocate == Some(state.allocated.len()) {
                return Err(FakeError::Allocate);
            }
            let index = state.allocated.len();
            let address = *state.available.get(index).ok_or(FakeError::Allocate)?;
            state.allocated.push(address);
            state.pages.push((address, vec![u64::MAX; 512]));
            Ok(address)
        }

        fn zero_frame(&mut self, frame: PhysicalFrame) -> Result<(), Self::Error> {
            let mut state = self.0.borrow_mut();
            if state.fail_zero == Some(state.allocated.len() - 1) {
                return Err(FakeError::Zero);
            }
            state
                .page_mut(frame.address())
                .ok_or(FakeError::Access)?
                .fill(0);
            Ok(())
        }

        fn free_frame(&mut self, address: u64) -> Result<(), Self::Error> {
            let mut state = self.0.borrow_mut();
            if state.fail_free_once == Some(address) {
                state.fail_free_once = None;
                return Err(FakeError::Free);
            }
            state.freed.push(address);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct FakeMemory(Rc<RefCell<FakeState>>);

    impl PageTableMemory for FakeMemory {
        type Error = FakeError;

        fn write_entry(
            &mut self,
            frame: PhysicalFrame,
            index: TableIndex,
            value: u64,
        ) -> Result<(), Self::Error> {
            let mut state = self.0.borrow_mut();
            if state.fail_write == Some(state.writes) {
                return Err(FakeError::Access);
            }
            state.writes += 1;
            *state
                .page_mut(frame.address())
                .and_then(|page| page.get_mut(usize::from(index.get())))
                .ok_or(FakeError::Access)? = value;
            Ok(())
        }

        fn read_entry(&self, frame: PhysicalFrame, index: TableIndex) -> Result<u64, Self::Error> {
            self.0
                .borrow()
                .page(frame.address())
                .and_then(|page| page.get(usize::from(index.get())))
                .copied()
                .ok_or(FakeError::Access)
        }
    }

    fn addresses() -> [u64; PAGE_TABLE_FRAME_CAPACITY] {
        [
            0x10_0000, 0x18_0000, 0x11_0000, 0x1f_0000, 0x14_0000, 0x21_0000, 0x16_0000, 0x23_0000,
        ]
    }

    fn setup() -> (
        AllocatedPageTables<FakeBackend>,
        FakeMemory,
        Rc<RefCell<FakeState>>,
    ) {
        let state = Rc::new(RefCell::new(FakeState::new(&addresses())));
        let plan = PlannedPageTables::for_transition(0x200000, 0x400000).unwrap();
        assert_eq!(plan.table_count(), 5);
        let allocated = plan
            .allocate(FakeBackend(state.clone()))
            .unwrap_or_else(|_| panic!("test frame allocation must succeed"));
        (allocated, FakeMemory(state.clone()), state)
    }

    #[test]
    fn materializes_all_entries_and_verifies_non_contiguous_frames() {
        let (allocated, mut memory, state) = setup();
        let materialized = allocated.materialize(&mut memory).unwrap();
        assert_eq!(state.borrow().writes, 5 * PAGE_TABLE_ENTRIES_PER_FRAME);
        assert!(
            state
                .borrow()
                .pages
                .iter()
                .all(|(_, page)| page.len() == 512)
        );
        let verified = materialized.verify(&memory).unwrap();
        assert_eq!(verified.frame_count(), 5);
        assert_eq!(verified.root_frame().address(), addresses()[0]);
        assert_eq!(verified.frames()[1].address(), addresses()[1]);
    }

    #[test]
    fn leaf_permissions_parent_resolution_guard_and_removal_are_exact() {
        let (allocated, mut memory, _) = setup();
        let materialized = allocated.materialize(&mut memory).unwrap();
        let assignments = materialized.owner.assignments();
        for entry in materialized.plan.entries() {
            let frame = assignments.frame(entry.table_slot()).unwrap();
            let actual = memory.read_entry(frame, entry.index()).unwrap();
            assert_eq!(actual, entry.encoded_value(&assignments).unwrap());
            assert_eq!(actual & EntryFlags::USER, 0);
            assert_eq!(actual & EntryFlags::HUGE_PAGE, 0);
            if entry.target().kind() == EntryTargetKind::PhysicalFrame {
                assert_ne!(actual & EntryFlags::PRESENT, 0);
                assert!(
                    !(actual & EntryFlags::WRITABLE != 0 && actual & EntryFlags::NO_EXECUTE == 0)
                );
            }
        }
        let removal = materialized.plan.transition_removals()[0];
        let root = assignments.frame(FrameSlot::ROOT).unwrap();
        assert_eq!(
            memory.read_entry(root, removal.index()).unwrap(),
            removal.expected_value(&assignments).unwrap()
        );
        materialized.verify(&memory).unwrap();
    }

    #[test]
    fn write_failure_at_first_middle_and_last_position_rolls_back() {
        for failure in [0, 512, 5 * 512 - 1] {
            let (allocated, mut memory, state) = setup();
            state.borrow_mut().fail_write = Some(failure);
            assert!(matches!(
                allocated.materialize(&mut memory),
                Err(MaterializationError::Access { .. })
            ));
            assert_eq!(
                state.borrow().freed,
                addresses()[..5].iter().rev().copied().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn unexpected_nonzero_and_reserved_or_permission_bits_are_rejected() {
        let (allocated, mut memory, _) = setup();
        let materialized = allocated.materialize(&mut memory).unwrap();
        let root = materialized.owner.root_frame();
        memory.0.borrow_mut().page_mut(root.address()).unwrap()[511] = EntryFlags::PRESENT | 0x1000;
        assert!(matches!(
            materialized.verify(&memory),
            Err(VerificationError::UnexpectedNonZero { .. })
        ));

        let (allocated, mut memory, _) = setup();
        let materialized = allocated.materialize(&mut memory).unwrap();
        let root = materialized.owner.root_frame();
        memory.0.borrow_mut().page_mut(root.address()).unwrap()[0] |= 1 << 62;
        assert!(matches!(
            materialized.verify(&memory),
            Err(VerificationError::InvalidBits { .. })
        ));
    }

    #[test]
    fn incorrect_unowned_child_cycle_and_unreachable_table_are_distinct() {
        let (allocated, mut memory, _) = setup();
        let materialized = allocated.materialize(&mut memory).unwrap();
        let root = materialized.owner.root_frame();
        memory.0.borrow_mut().page_mut(root.address()).unwrap()[0] =
            0x30_0000 | EntryFlags::INTERMEDIATE.bits();
        assert!(matches!(
            materialized.verify(&memory),
            Err(VerificationError::UnownedChild { .. })
        ));

        let (allocated, mut memory, _) = setup();
        let materialized = allocated.materialize(&mut memory).unwrap();
        let root = materialized.owner.root_frame();
        memory.0.borrow_mut().page_mut(root.address()).unwrap()[0] =
            root.address() | EntryFlags::INTERMEDIATE.bits();
        assert!(matches!(
            materialized.verify(&memory),
            Err(VerificationError::Cycle { .. })
        ));

        let (allocated, mut memory, _) = setup();
        let materialized = allocated.materialize(&mut memory).unwrap();
        let root = materialized.owner.root_frame();
        memory.0.borrow_mut().page_mut(root.address()).unwrap()[0] = 0;
        assert!(matches!(
            materialized.verify(&memory),
            Err(VerificationError::UnreachableTable { .. })
        ));
    }

    #[test]
    fn wrong_child_level_and_wrong_flags_are_rejected() {
        let (allocated, mut memory, _) = setup();
        let materialized = allocated.materialize(&mut memory).unwrap();
        let root = materialized.owner.root_frame();
        let pt_frame = materialized.owner.frames()[3];
        memory.0.borrow_mut().page_mut(root.address()).unwrap()[0] =
            pt_frame.address() | EntryFlags::INTERMEDIATE.bits();
        assert!(matches!(
            materialized.verify(&memory),
            Err(VerificationError::WrongTableLevel { .. })
        ));

        let (allocated, mut memory, _) = setup();
        let materialized = allocated.materialize(&mut memory).unwrap();
        let assignments = materialized.owner.assignments();
        let leaf = materialized
            .plan
            .entries()
            .iter()
            .find(|entry| entry.target().kind() == EntryTargetKind::PhysicalFrame)
            .copied()
            .unwrap();
        let frame = assignments.frame(leaf.table_slot()).unwrap();
        memory.0.borrow_mut().page_mut(frame.address()).unwrap()
            [usize::from(leaf.index().get())] |= EntryFlags::WRITABLE;
        assert!(matches!(
            materialized.verify(&memory),
            Err(VerificationError::InvalidBits { .. })
        ));
    }

    #[test]
    fn all_bounded_single_entry_corruptions_are_panic_free() {
        for index in 0..512 {
            let (allocated, mut memory, _) = setup();
            let materialized = allocated.materialize(&mut memory).unwrap();
            let last = *materialized.owner.frames().last().unwrap();
            memory.0.borrow_mut().page_mut(last.address()).unwrap()[index] ^=
                EntryFlags::PRESENT | 0x1000;
            assert!(materialized.verify(&memory).is_err());
        }
    }

    #[test]
    fn reservation_coalesces_adjacent_frames_and_preserves_exact_coverage() {
        let frames = [
            PhysicalFrame::new(0x3000).unwrap(),
            PhysicalFrame::new(0x1000).unwrap(),
            PhysicalFrame::new(0x2000).unwrap(),
            PhysicalFrame::new(0x8000).unwrap(),
        ];
        let mut reservations = ReservationList::new();
        append_page_table_reservations(&frames, &mut reservations).unwrap();
        reservations.finish().unwrap();
        let items = reservations.items().collect::<Vec<_>>();
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].physical_start, items[0].page_count), (0x1000, 3));
        assert_eq!((items[1].physical_start, items[1].page_count), (0x8000, 1));

        let base = [MemoryDescriptor {
            kind: MEMORY_KIND_USABLE,
            reserved0: 0,
            physical_start: 0x1000,
            page_count: 8,
            attributes: 0,
        }];
        let mut output = [base[0]; 8];
        let count = apply_reservations(&base, &reservations, &mut output).unwrap();
        verify_page_table_reservations(&frames, &output[..count]).unwrap();
        assert!(
            output[..count]
                .iter()
                .filter(|descriptor| descriptor.kind == MEMORY_KIND_PAGE_TABLE)
                .map(|descriptor| descriptor.page_count)
                .sum::<u64>()
                == 4
        );
    }

    #[test]
    fn reservation_rejects_usable_wrong_missing_overlap_and_capacity() {
        let frame = PhysicalFrame::new(0x2000).unwrap();
        for (descriptors, expected) in [
            (
                vec![MemoryDescriptor {
                    kind: MEMORY_KIND_USABLE,
                    reserved0: 0,
                    physical_start: 0x1000,
                    page_count: 2,
                    attributes: 0,
                }],
                PageTableReservationError::UsableFrame {
                    frame_slot: FrameSlot::ROOT,
                },
            ),
            (
                vec![MemoryDescriptor {
                    kind: boot_protocol::MEMORY_KIND_RESERVED,
                    reserved0: 0,
                    physical_start: 0x1000,
                    page_count: 2,
                    attributes: 0,
                }],
                PageTableReservationError::WrongKind {
                    frame_slot: FrameSlot::ROOT,
                },
            ),
            (
                Vec::new(),
                PageTableReservationError::IncompleteCoverage {
                    frame_slot: FrameSlot::ROOT,
                },
            ),
        ] {
            assert_eq!(
                verify_page_table_reservations(&[frame], &descriptors),
                Err(expected)
            );
        }

        let mut full = ReservationList::new();
        for index in 0..crate::RESERVATION_CAPACITY {
            full.push(Reservation {
                physical_start: 0x1000 + (index as u64) * 0x2000,
                page_count: 1,
                kind: boot_protocol::MEMORY_KIND_RESERVED,
                source: ReservationSource::BootInfo,
            })
            .unwrap();
        }
        assert_eq!(
            append_page_table_reservations(&[frame], &mut full),
            Err(PageTableReservationError::Map(
                MapBuildError::ReservationCapacity
            ))
        );
    }

    #[test]
    fn transfer_disarms_firmware_release_and_retains_root_metadata() {
        let (allocated, mut memory, state) = setup();
        let verified = allocated
            .materialize(&mut memory)
            .unwrap()
            .verify(&memory)
            .unwrap();
        let mut reservations = ReservationList::new();
        verified.append_reservations(&mut reservations).unwrap();
        reservations.finish().unwrap();
        let base = [MemoryDescriptor {
            kind: MEMORY_KIND_USABLE,
            reserved0: 0,
            physical_start: 0x100000,
            page_count: 0x140,
            attributes: 0,
        }];
        let mut output = [base[0]; 16];
        let count = apply_reservations(&base, &reservations, &mut output).unwrap();
        let proof = verify_page_table_reservations(verified.frames(), &output[..count]).unwrap();
        let reserved = verified.confirm_final_map_reservation(proof).unwrap();
        let root = reserved.root_frame();
        let count = reserved.frame_count();
        let transferred = reserved.transfer();
        assert_eq!(transferred.root_frame(), root);
        assert_eq!(transferred.frame_count(), count);
        drop(transferred);
        assert!(state.borrow().freed.is_empty());
    }
}
