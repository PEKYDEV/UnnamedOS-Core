use memory_layout::{
    BOOTSTRAP_STACK_BYTES, CachePolicy, ConstructionPlan, DIRECT_MAP_START, EntryFlags,
    EntryTargetKind, FrameBackend, FrameOwnerCause, MappingKind, MappingPermissions, MappingPlan,
    PAGE_SIZE, PageTableFrameOwner, PageTablePlanError, PhysicalFrame, PhysicalRange, PlanMode,
    VirtualRange,
};
use std::{cell::RefCell, rc::Rc, vec, vec::Vec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackendError {
    Allocate,
    Zero,
    Free,
}

#[derive(Default)]
struct State {
    addresses: Vec<u64>,
    next: usize,
    fail_allocate_at: Option<usize>,
    fail_zero_at: Option<usize>,
    zero_calls: Vec<u64>,
    free_calls: Vec<u64>,
    fail_free_once: Option<u64>,
}

#[derive(Clone)]
struct FakeBackend(Rc<RefCell<State>>);

impl FrameBackend for FakeBackend {
    type Error = BackendError;

    fn allocate_frame(&mut self) -> Result<u64, Self::Error> {
        let mut state = self.0.borrow_mut();
        let index = state.next;
        if state.fail_allocate_at == Some(index) {
            return Err(BackendError::Allocate);
        }
        let address = state.addresses[index];
        state.next += 1;
        Ok(address)
    }

    fn zero_frame(&mut self, frame: PhysicalFrame) -> Result<(), Self::Error> {
        let mut state = self.0.borrow_mut();
        let index = state.zero_calls.len();
        state.zero_calls.push(frame.address());
        if state.fail_zero_at == Some(index) {
            return Err(BackendError::Zero);
        }
        Ok(())
    }

    fn free_frame(&mut self, address: u64) -> Result<(), Self::Error> {
        let mut state = self.0.borrow_mut();
        state.free_calls.push(address);
        if state.fail_free_once == Some(address) {
            state.fail_free_once = None;
            return Err(BackendError::Free);
        }
        Ok(())
    }
}

fn construction_plan() -> ConstructionPlan<8, 8, 1> {
    let mut mappings = MappingPlan::<1>::new();
    mappings
        .insert_mapping(
            VirtualRange::from_start_and_length(DIRECT_MAP_START + 0x1000, PAGE_SIZE).unwrap(),
            PhysicalRange::from_start_and_length(0x1000, PAGE_SIZE).unwrap(),
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
            MappingKind::DirectMap,
        )
        .unwrap();
    ConstructionPlan::build(&mappings, PlanMode::Final).unwrap()
}

fn backend(addresses: Vec<u64>) -> (FakeBackend, Rc<RefCell<State>>) {
    let state = Rc::new(RefCell::new(State {
        addresses,
        ..State::default()
    }));
    (FakeBackend(state.clone()), state)
}

#[test]
fn exact_frame_count_zeroing_root_and_encoded_entries_are_deterministic() {
    let plan = construction_plan();
    assert_eq!(plan.table_count(), 4);
    let addresses = vec![0x100000, 0x101000, 0x102000, 0x103000];
    let (backend, state) = backend(addresses.clone());
    let owner = PageTableFrameOwner::<_, 8>::allocate(&plan, backend)
        .unwrap_or_else(|_| panic!("deterministic backend must construct the complete hierarchy"));
    assert_eq!(owner.frame_count(), plan.table_count());
    assert_eq!(owner.root_frame().address(), addresses[0]);
    assert_eq!(&state.borrow().zero_calls, &addresses);
    assert_eq!(
        owner
            .frames()
            .iter()
            .map(|frame| frame.address())
            .collect::<Vec<_>>(),
        addresses
    );

    let mut encoded = [0_u64; 8];
    let count = plan
        .encoded_entries(&owner.assignments(), &mut encoded)
        .unwrap();
    assert_eq!(count, plan.entry_count());
    for (entry, value) in plan.entries().iter().zip(encoded) {
        if entry.target().kind() == EntryTargetKind::TableSlot {
            let child = owner
                .assignments()
                .frame(entry.target().frame_slot().unwrap())
                .unwrap();
            assert_eq!(value, child.address() | EntryFlags::INTERMEDIATE.bits());
        } else {
            assert_eq!(Some(value), entry.encoded_leaf_value());
        }
    }
    drop(owner);
    assert_eq!(
        state.borrow().free_calls,
        vec![0x103000, 0x102000, 0x101000, 0x100000]
    );
}

#[test]
fn allocation_failure_at_every_slot_rolls_back_in_reverse_order() {
    let plan = construction_plan();
    for failure in 0..plan.table_count() {
        let addresses = vec![0x100000, 0x101000, 0x102000, 0x103000];
        let (backend, state) = backend(addresses.clone());
        state.borrow_mut().fail_allocate_at = Some(failure);
        let mut error = match PageTableFrameOwner::<_, 8>::allocate(&plan, backend) {
            Ok(_) => panic!("allocation failure must reject construction"),
            Err(error) => error,
        };
        assert!(matches!(
            error.cause(),
            FrameOwnerCause::Allocation {
                source: BackendError::Allocate,
                ..
            }
        ));
        assert_eq!(error.remaining_frames(), 0);
        error.try_release().unwrap();
        let expected = addresses[..failure]
            .iter()
            .rev()
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(state.borrow().free_calls, expected);
    }
}

#[test]
fn zero_failure_at_every_slot_rolls_back_in_reverse_order() {
    let plan = construction_plan();
    for failure in 0..plan.table_count() {
        let addresses = vec![0x200000, 0x201000, 0x202000, 0x203000];
        let (backend, state) = backend(addresses.clone());
        state.borrow_mut().fail_zero_at = Some(failure);
        let error = match PageTableFrameOwner::<_, 8>::allocate(&plan, backend) {
            Ok(_) => panic!("zeroing failure must reject construction"),
            Err(error) => error,
        };
        assert!(matches!(
            error.cause(),
            FrameOwnerCause::Zeroing {
                source: BackendError::Zero,
                ..
            }
        ));
        assert_eq!(error.remaining_frames(), 0);
        let expected = addresses[..=failure]
            .iter()
            .rev()
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(state.borrow().free_calls, expected);
    }
}

#[test]
fn failed_partial_rollback_retains_frames_for_explicit_retry() {
    let plan = construction_plan();
    let (backend, state) = backend(vec![0x180000, 0x181000, 0x182000, 0x183000]);
    {
        let mut state = state.borrow_mut();
        state.fail_allocate_at = Some(2);
        state.fail_free_once = Some(0x181000);
    }
    let mut error = match PageTableFrameOwner::<_, 8>::allocate(&plan, backend) {
        Ok(_) => panic!("partial allocation must fail"),
        Err(error) => error,
    };
    assert_eq!(error.remaining_frames(), 2);
    assert_eq!(state.borrow().free_calls, vec![0x181000]);
    error.try_release().unwrap();
    assert_eq!(error.remaining_frames(), 0);
    assert_eq!(
        state.borrow().free_calls,
        vec![0x181000, 0x181000, 0x180000]
    );
}

#[test]
fn invalid_and_duplicate_backend_frames_are_rejected() {
    let plan = construction_plan();
    for addresses in [
        vec![0x100001, 0x101000, 0x102000, 0x103000],
        vec![0x100000, 0x100000, 0x102000, 0x103000],
        vec![
            memory_layout::SUPPORTED_PHYSICAL_END,
            0x101000,
            0x102000,
            0x103000,
        ],
    ] {
        let duplicate_address = addresses.get(1) == addresses.first();
        let (backend, state) = backend(addresses);
        let error = match PageTableFrameOwner::<_, 8>::allocate(&plan, backend) {
            Ok(_) => panic!("invalid backend frame must reject construction"),
            Err(error) => error,
        };
        assert!(matches!(
            error.cause(),
            FrameOwnerCause::InvalidFrame { .. } | FrameOwnerCause::DuplicateFrame { .. }
        ));
        assert!(state.borrow().zero_calls.len() < plan.table_count());
        if duplicate_address {
            assert_eq!(state.borrow().free_calls, vec![0x100000]);
        }
    }
}

#[test]
fn failed_release_retains_ownership_and_retry_never_double_frees() {
    let plan = construction_plan();
    let addresses = vec![0x300000, 0x301000, 0x302000, 0x303000];
    let (backend, state) = backend(addresses);
    state.borrow_mut().fail_free_once = Some(0x302000);
    let mut owner = PageTableFrameOwner::<_, 8>::allocate(&plan, backend)
        .unwrap_or_else(|_| panic!("deterministic backend must construct the complete hierarchy"));
    let error = owner.try_release().unwrap_err();
    assert_eq!(error.source, BackendError::Free);
    assert_eq!(error.remaining_frames, 3);
    assert_eq!(owner.frame_count(), 3);
    assert_eq!(state.borrow().free_calls, vec![0x303000, 0x302000]);

    owner.try_release().unwrap();
    assert_eq!(owner.frame_count(), 0);
    owner.try_release().unwrap();
    assert_eq!(
        state.borrow().free_calls,
        vec![0x303000, 0x302000, 0x302000, 0x301000, 0x300000]
    );
}

#[test]
fn transfer_disarms_drop_and_retains_plan_independent_metadata() {
    let (owner, state) = {
        let plan = construction_plan();
        let (backend, state) = backend(vec![0x400000, 0x401000, 0x402000, 0x403000]);
        let owner = PageTableFrameOwner::<_, 4>::allocate(&plan, backend).unwrap_or_else(|_| {
            panic!("deterministic backend must construct the complete hierarchy")
        });
        (owner, state)
    };
    assert_eq!(owner.root_frame().address(), 0x400000);
    {
        let transferred = owner.transfer();
        assert_eq!(transferred.frame_count(), 4);
        assert_eq!(transferred.root_frame().address(), 0x400000);
        assert_eq!(transferred.frames()[3].address(), 0x403000);
    }
    assert!(state.borrow().free_calls.is_empty());
}

#[test]
fn owner_capacity_rejects_before_allocating() {
    let plan = construction_plan();
    let (backend, state) = backend(vec![0x500000, 0x501000, 0x502000, 0x503000]);
    let error = match PageTableFrameOwner::<_, 3>::allocate(&plan, backend) {
        Ok(_) => panic!("undersized owner must fail"),
        Err(error) => error,
    };
    assert_eq!(
        error.cause(),
        &FrameOwnerCause::PlannedFrameCountExceedsCapacity
    );
    assert_eq!(state.borrow().next, 0);
}

#[test]
fn frame_assignment_count_and_output_capacity_are_checked() {
    let plan = construction_plan();
    let (backend, _) = backend(vec![0x600000, 0x601000, 0x602000, 0x603000]);
    let owner = PageTableFrameOwner::<_, 8>::allocate(&plan, backend)
        .unwrap_or_else(|_| panic!("deterministic backend must construct the complete hierarchy"));
    let mut short = [0_u64; 1];
    assert_eq!(
        plan.encoded_entries(&owner.assignments(), &mut short),
        Err(PageTablePlanError::EntryCapacityExhausted)
    );
}

#[test]
fn transition_removal_encodes_the_exact_root_entry_to_clear() {
    let mut mappings = MappingPlan::<2>::new();
    for (start, pages, permissions) in [
        (0x100000, 1, MappingPermissions::KERNEL_RX),
        (
            0x200000,
            BOOTSTRAP_STACK_BYTES / PAGE_SIZE,
            MappingPermissions::KERNEL_RW,
        ),
    ] {
        mappings
            .insert_mapping(
                VirtualRange::from_start_and_length(start, pages * PAGE_SIZE).unwrap(),
                PhysicalRange::from_start_and_length(start, pages * PAGE_SIZE).unwrap(),
                permissions,
                CachePolicy::WriteBack,
                MappingKind::TransitionIdentity,
            )
            .unwrap();
    }
    let plan = ConstructionPlan::<8, 32, 1>::build(&mappings, PlanMode::Transitional).unwrap();
    let addresses = (0..plan.table_count())
        .map(|index| 0x700000 + index as u64 * PAGE_SIZE)
        .collect::<Vec<_>>();
    let (backend, _) = backend(addresses);
    let owner = PageTableFrameOwner::<_, 8>::allocate(&plan, backend)
        .unwrap_or_else(|_| panic!("transition hierarchy allocation must succeed"));
    assert_eq!(
        plan.transition_removals()[0]
            .expected_value(&owner.assignments())
            .unwrap(),
        0x701000 | EntryFlags::INTERMEDIATE.bits()
    );
}
