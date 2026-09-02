use core::mem::{align_of, size_of};

use memory_layout::{
    BOOTSTRAP_PHYSICAL_START, BOOTSTRAP_STACK_BYTES, CachePolicy, ConstructionPlan,
    DIRECT_MAP_START, EntryFlags, EntryTargetKind, FRAMEBUFFER_START, FrameSlot,
    KERNEL_IMAGE_START, KERNEL_LOCAL_START, LayoutError, MMIO_START, MappingKind,
    MappingPermissions, MappingPlan, PAGE_SIZE, PageTablePlanError, PhysicalFrame, PhysicalRange,
    PlanMode, SUPPORTED_PHYSICAL_END, TableIndex, TableLevel, VirtualAddress, VirtualRange,
    virtual_address_indices,
};

fn physical(start: u64, pages: u64) -> PhysicalRange {
    PhysicalRange::from_start_and_length(start, pages * PAGE_SIZE).unwrap()
}

fn virtual_range(start: u64, pages: u64) -> VirtualRange {
    VirtualRange::from_start_and_length(start, pages * PAGE_SIZE).unwrap()
}

fn add_mapping<const CAPACITY: usize>(
    plan: &mut MappingPlan<CAPACITY>,
    virtual_start: u64,
    physical_start: u64,
    pages: u64,
    permissions: MappingPermissions,
    cache: CachePolicy,
    kind: MappingKind,
) {
    plan.insert_mapping(
        virtual_range(virtual_start, pages),
        physical(physical_start, pages),
        permissions,
        cache,
        kind,
    )
    .unwrap();
}

fn direct_mapping_plan(addresses: &[u64]) -> MappingPlan<8> {
    let mut plan = MappingPlan::new();
    for physical_start in addresses.iter().copied().rev() {
        add_mapping(
            &mut plan,
            DIRECT_MAP_START + physical_start,
            physical_start,
            1,
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
            MappingKind::DirectMap,
        );
    }
    plan
}

#[test]
fn public_scalar_sizes_and_alignments_are_stable() {
    assert_eq!(size_of::<TableIndex>(), 2);
    assert_eq!(align_of::<TableIndex>(), 2);
    assert_eq!(size_of::<FrameSlot>(), 4);
    assert_eq!(align_of::<FrameSlot>(), 4);
    assert_eq!(size_of::<PhysicalFrame>(), 8);
    assert_eq!(align_of::<PhysicalFrame>(), 8);
    assert_eq!(size_of::<EntryFlags>(), 8);
    assert_eq!(align_of::<EntryFlags>(), 8);
}

#[test]
fn every_level_index_boundary_and_both_canonical_halves_are_exact() {
    let cases = [
        (0x0000_0000_0000_0000, [0, 0, 0, 0]),
        (0x0000_0000_001f_f000, [0, 0, 0, 511]),
        (0x0000_0000_0020_0000, [0, 0, 1, 0]),
        (0x0000_0000_3fff_f000, [0, 0, 511, 511]),
        (0x0000_0000_4000_0000, [0, 1, 0, 0]),
        (0x0000_007f_ffff_f000, [0, 511, 511, 511]),
        (0x0000_0080_0000_0000, [1, 0, 0, 0]),
        (0x0000_7fff_ffff_f000, [255, 511, 511, 511]),
        (0xffff_8000_0000_0000, [256, 0, 0, 0]),
        (0xffff_ffff_ffff_f000, [511, 511, 511, 511]),
    ];
    for (address, expected) in cases {
        let indices = virtual_address_indices(VirtualAddress::new(address).unwrap());
        assert_eq!(
            indices.map(TableIndex::get),
            expected,
            "address {address:#018x}"
        );
    }
    assert_eq!(
        VirtualAddress::new(0x0000_8000_0000_0000),
        Err(LayoutError::NonCanonicalVirtualAddress)
    );
    assert_eq!(
        TableIndex::new(512),
        Err(PageTablePlanError::InvalidTableIndex)
    );
}

#[test]
fn table_deduplication_is_deterministic_at_every_parent_level() {
    for (addresses, expected_tables) in [
        (&[0x1000, 0x2000][..], 4),
        (&[0x1000, 0x20_0000][..], 5),
        (&[0x1000, 0x4000_0000][..], 6),
        (&[0x1000, 0x80_0000_0000][..], 7),
    ] {
        let input = direct_mapping_plan(addresses);
        let first = ConstructionPlan::<16, 32, 1>::build(&input, PlanMode::Final).unwrap();
        let second = ConstructionPlan::<16, 32, 1>::build(&input, PlanMode::Final).unwrap();
        assert_eq!(first.table_count(), expected_tables);
        assert_eq!(first.tables(), second.tables());
        assert_eq!(first.entries(), second.entries());
        assert_eq!(first.root_frame_slot(), FrameSlot::ROOT);
        assert_eq!(first.tables()[0].level(), TableLevel::Pml4);
        for (index, table) in first.tables().iter().enumerate() {
            assert_eq!(table.frame_slot().get(), index as u32);
        }
    }
}

#[test]
fn canonical_abstract_encoding_is_byte_for_byte_deterministic() {
    let input = direct_mapping_plan(&[0x80_0000_0000, 0x20_0000, 0x1000]);
    let first = ConstructionPlan::<16, 32, 1>::build(&input, PlanMode::Final).unwrap();
    let second = ConstructionPlan::<16, 32, 1>::build(&input, PlanMode::Final).unwrap();
    let mut first_bytes = [0xaa; 1024];
    let mut second_bytes = [0x55; 1024];
    let first_length = first.encode_abstract(&mut first_bytes).unwrap();
    let second_length = second.encode_abstract(&mut second_bytes).unwrap();
    assert_eq!(first_length, first.abstract_byte_len().unwrap());
    assert_eq!(first_length, second_length);
    assert_eq!(&first_bytes[..first_length], &second_bytes[..second_length]);
    assert_eq!(
        first.encode_abstract(&mut [0; 1]),
        Err(PageTablePlanError::OutputTooSmall)
    );
}

#[test]
fn rx_ro_nx_and_rw_nx_leaf_encodings_are_exact() {
    let mut input = MappingPlan::<3>::new();
    for (offset, permissions, kind) in [
        (0, MappingPermissions::KERNEL_RX, MappingKind::KernelText),
        (
            PAGE_SIZE,
            MappingPermissions::KERNEL_R,
            MappingKind::KernelRodata,
        ),
        (
            2 * PAGE_SIZE,
            MappingPermissions::KERNEL_RW,
            MappingKind::KernelData,
        ),
    ] {
        add_mapping(
            &mut input,
            KERNEL_IMAGE_START + offset,
            BOOTSTRAP_PHYSICAL_START + offset,
            1,
            permissions,
            CachePolicy::WriteBack,
            kind,
        );
    }
    let plan = ConstructionPlan::<8, 16, 1>::build(&input, PlanMode::Final).unwrap();
    let rx = plan
        .leaf_entry(VirtualAddress::new(KERNEL_IMAGE_START).unwrap())
        .unwrap();
    let ro = plan
        .leaf_entry(VirtualAddress::new(KERNEL_IMAGE_START + PAGE_SIZE).unwrap())
        .unwrap();
    let rw = plan
        .leaf_entry(VirtualAddress::new(KERNEL_IMAGE_START + 2 * PAGE_SIZE).unwrap())
        .unwrap();
    assert_eq!(rx.target().kind(), EntryTargetKind::PhysicalFrame);
    assert_eq!(rx.flags().bits(), EntryFlags::PRESENT | EntryFlags::GLOBAL);
    assert_eq!(
        ro.flags().bits(),
        EntryFlags::PRESENT | EntryFlags::GLOBAL | EntryFlags::NO_EXECUTE
    );
    assert_eq!(
        rw.flags().bits(),
        EntryFlags::PRESENT | EntryFlags::WRITABLE | EntryFlags::GLOBAL | EntryFlags::NO_EXECUTE
    );
    assert_eq!(
        rx.encoded_leaf_value(),
        Some(BOOTSTRAP_PHYSICAL_START | EntryFlags::PRESENT | EntryFlags::GLOBAL)
    );
}

#[test]
fn mixed_executable_descendants_keep_intermediates_traversable() {
    let mut input = MappingPlan::<2>::new();
    add_mapping(
        &mut input,
        KERNEL_IMAGE_START,
        BOOTSTRAP_PHYSICAL_START,
        1,
        MappingPermissions::KERNEL_RX,
        CachePolicy::WriteBack,
        MappingKind::KernelText,
    );
    add_mapping(
        &mut input,
        KERNEL_IMAGE_START + PAGE_SIZE,
        BOOTSTRAP_PHYSICAL_START + PAGE_SIZE,
        1,
        MappingPermissions::KERNEL_R,
        CachePolicy::WriteBack,
        MappingKind::KernelRodata,
    );
    let plan = ConstructionPlan::<8, 16, 1>::build(&input, PlanMode::Final).unwrap();
    for entry in plan.entries() {
        if entry.target().kind() == EntryTargetKind::TableSlot {
            assert_eq!(entry.flags(), EntryFlags::INTERMEDIATE);
            assert!(entry.flags().writable());
            assert!(entry.flags().executable());
        }
    }
}

#[test]
fn guard_is_absent_and_stack_leafs_are_rw_nx() {
    let mut input = MappingPlan::<2>::new();
    input
        .insert_guard(virtual_range(KERNEL_LOCAL_START, 1))
        .unwrap();
    add_mapping(
        &mut input,
        KERNEL_LOCAL_START + PAGE_SIZE,
        0x400000,
        BOOTSTRAP_STACK_BYTES / PAGE_SIZE,
        MappingPermissions::KERNEL_RW,
        CachePolicy::WriteBack,
        MappingKind::BootstrapStack,
    );
    let plan = ConstructionPlan::<8, 32, 1>::build(&input, PlanMode::Final).unwrap();
    assert!(
        plan.leaf_entry(VirtualAddress::new(KERNEL_LOCAL_START).unwrap())
            .is_none()
    );
    let stack = plan
        .leaf_entry(VirtualAddress::new(KERNEL_LOCAL_START + PAGE_SIZE).unwrap())
        .unwrap();
    assert!(stack.flags().writable());
    assert!(!stack.flags().executable());
}

#[test]
fn uncached_leaf_bits_are_explicit_for_mmio_and_framebuffer() {
    let mut input = MappingPlan::<2>::new();
    for (virtual_start, physical_start, kind) in [
        (MMIO_START, 0xfec0_0000, MappingKind::Mmio),
        (FRAMEBUFFER_START, 0x8000_0000, MappingKind::Framebuffer),
    ] {
        add_mapping(
            &mut input,
            virtual_start,
            physical_start,
            1,
            MappingPermissions::KERNEL_RW,
            CachePolicy::Uncached,
            kind,
        );
    }
    let plan = ConstructionPlan::<8, 16, 1>::build(&input, PlanMode::Final).unwrap();
    for address in [MMIO_START, FRAMEBUFFER_START] {
        let flags = plan
            .leaf_entry(VirtualAddress::new(address).unwrap())
            .unwrap()
            .flags()
            .bits();
        assert_ne!(flags & EntryFlags::WRITE_THROUGH, 0);
        assert_ne!(flags & EntryFlags::CACHE_DISABLE, 0);
        assert_ne!(flags & EntryFlags::NO_EXECUTE, 0);
    }
}

#[test]
fn transitional_aliases_have_one_exact_root_removal() {
    let mut input = MappingPlan::<2>::new();
    add_mapping(
        &mut input,
        0x100000,
        0x100000,
        1,
        MappingPermissions::KERNEL_RX,
        CachePolicy::WriteBack,
        MappingKind::TransitionIdentity,
    );
    add_mapping(
        &mut input,
        0x200000,
        0x200000,
        BOOTSTRAP_STACK_BYTES / PAGE_SIZE,
        MappingPermissions::KERNEL_RW,
        CachePolicy::WriteBack,
        MappingKind::TransitionIdentity,
    );
    let plan = ConstructionPlan::<8, 32, 1>::build(&input, PlanMode::Transitional).unwrap();
    assert_eq!(plan.removal_count(), 1);
    let removal = plan.transition_removals()[0];
    assert_eq!(removal.table_slot(), FrameSlot::ROOT);
    assert_eq!(removal.index().get(), 0);
    assert_eq!(plan.mode(), PlanMode::Transitional);

    let final_input = direct_mapping_plan(&[0x1000]);
    let final_plan = ConstructionPlan::<8, 8, 1>::build(&final_input, PlanMode::Final).unwrap();
    assert_eq!(final_plan.removal_count(), 0);
    assert!(final_plan.tables().iter().all(|table| {
        table
            .parent()
            .is_none_or(|(_, index)| index.get() >= 256 || table.level() != TableLevel::Pdpt)
    }));
}

#[test]
fn physical_frames_and_entry_bits_fail_closed() {
    assert_eq!(
        PhysicalFrame::new(1),
        Err(PageTablePlanError::UnalignedPhysicalFrame)
    );
    assert!(matches!(
        PhysicalFrame::new(SUPPORTED_PHYSICAL_END),
        Err(PageTablePlanError::Layout(
            LayoutError::UnsupportedPhysicalAddress
        ))
    ));
    assert_eq!(
        EntryFlags::from_bits(EntryFlags::PRESENT | EntryFlags::USER),
        Err(PageTablePlanError::InvalidEntryFlags)
    );
    assert_eq!(
        EntryFlags::from_bits(EntryFlags::PRESENT | EntryFlags::HUGE_PAGE),
        Err(PageTablePlanError::InvalidEntryFlags)
    );
    assert_eq!(
        EntryFlags::from_bits(EntryFlags::WRITABLE),
        Err(PageTablePlanError::InvalidEntryFlags)
    );
}

#[test]
fn fixed_capacities_fail_at_tables_entries_and_removals() {
    let final_input = direct_mapping_plan(&[0x1000]);
    assert!(matches!(
        ConstructionPlan::<1, 8, 1>::build(&final_input, PlanMode::Final),
        Err(PageTablePlanError::TableCapacityExhausted)
    ));
    assert!(matches!(
        ConstructionPlan::<8, 2, 1>::build(&final_input, PlanMode::Final),
        Err(PageTablePlanError::EntryCapacityExhausted)
    ));

    let mut transitional = MappingPlan::<2>::new();
    add_mapping(
        &mut transitional,
        0x100000,
        0x100000,
        1,
        MappingPermissions::KERNEL_RX,
        CachePolicy::WriteBack,
        MappingKind::TransitionIdentity,
    );
    add_mapping(
        &mut transitional,
        0x200000,
        0x200000,
        BOOTSTRAP_STACK_BYTES / PAGE_SIZE,
        MappingPermissions::KERNEL_RW,
        CachePolicy::WriteBack,
        MappingKind::TransitionIdentity,
    );
    assert!(matches!(
        ConstructionPlan::<8, 32, 0>::build(&transitional, PlanMode::Transitional),
        Err(PageTablePlanError::RemovalCapacityExhausted)
    ));
}
