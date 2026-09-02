use core::mem::{align_of, size_of};

use memory_layout::{
    AddressRegion, BOOTSTRAP_PHYSICAL_END, BOOTSTRAP_PHYSICAL_START, BOOTSTRAP_STACK_BYTES,
    CachePolicy, DECLARED_REGIONS, DIRECT_MAP_END, DIRECT_MAP_START, EntryBacking, FRAMEBUFFER_END,
    FRAMEBUFFER_START, HIGH_CANONICAL_START, KERNEL_IMAGE_END, KERNEL_IMAGE_START,
    KERNEL_LOCAL_END, KERNEL_LOCAL_START, KERNEL_SERVICES_END, KERNEL_SERVICES_START,
    LOW_CANONICAL_END, LayoutError, MAX_TRANSITION_IDENTITY_BYTES, MMIO_END, MMIO_START,
    MappingKind, MappingPermissions, MappingPlan, PAGE_SIZE, PhysicalAddress, PhysicalRange,
    SUPPORTED_PHYSICAL_END, TRANSITION_IDENTITY_END, USER_SPACE_END, USER_SPACE_START,
    VirtualAddress, VirtualRange, align_down, align_up, direct_map_address, is_canonical,
    validate_declared_regions,
};

fn virtual_range(start: u64, end: u64) -> VirtualRange {
    VirtualRange::new(start, end).expect("test virtual range must be valid")
}

fn physical_range(start: u64, end: u64) -> PhysicalRange {
    PhysicalRange::new(start, end).expect("test physical range must be valid")
}

#[test]
fn public_scalar_layout_is_fixed() {
    assert_eq!(size_of::<PhysicalAddress>(), 8);
    assert_eq!(align_of::<PhysicalAddress>(), 8);
    assert_eq!(size_of::<VirtualAddress>(), 8);
    assert_eq!(align_of::<VirtualAddress>(), 8);
    assert_eq!(size_of::<PhysicalRange>(), 16);
    assert_eq!(align_of::<PhysicalRange>(), 8);
    assert_eq!(size_of::<VirtualRange>(), 16);
    assert_eq!(align_of::<VirtualRange>(), 8);
    assert_eq!(size_of::<MappingPermissions>(), 1);
    assert_eq!(align_of::<MappingPermissions>(), 1);
}

#[test]
fn canonical_boundaries_cover_both_four_level_halves() {
    assert!(is_canonical(0));
    assert!(is_canonical(LOW_CANONICAL_END - 1));
    assert!(!is_canonical(LOW_CANONICAL_END));
    assert!(!is_canonical(HIGH_CANONICAL_START - 1));
    assert!(is_canonical(HIGH_CANONICAL_START));
    assert!(is_canonical(u64::MAX));
    assert_eq!(
        VirtualAddress::new(LOW_CANONICAL_END),
        Err(LayoutError::NonCanonicalVirtualAddress)
    );
    assert_eq!(
        VirtualAddress::new(HIGH_CANONICAL_START - 1),
        Err(LayoutError::NonCanonicalVirtualAddress)
    );
}

#[test]
fn alignment_is_checked_and_overflow_safe() {
    assert_eq!(align_down(0x2345, PAGE_SIZE), Ok(0x2000));
    assert_eq!(align_up(0x2000, PAGE_SIZE), Ok(0x2000));
    assert_eq!(align_up(0x2001, PAGE_SIZE), Ok(0x3000));
    assert_eq!(align_up(1, 0), Err(LayoutError::InvalidAlignment));
    assert_eq!(align_down(1, 3), Err(LayoutError::InvalidAlignment));
    assert_eq!(
        align_up(u64::MAX, PAGE_SIZE),
        Err(LayoutError::AlignmentOverflow)
    );
}

#[test]
fn physical_ranges_reject_empty_reversed_truncated_overflow_and_unsupported() {
    assert_eq!(
        PhysicalRange::new(0x1000, 0x1000),
        Err(LayoutError::EmptyRange)
    );
    assert_eq!(
        PhysicalRange::new(0x2000, 0x1000),
        Err(LayoutError::ReversedRange)
    );
    assert_eq!(
        PhysicalRange::new(0x1000, 0x1800),
        Err(LayoutError::RangeNotPageAligned)
    );
    assert_eq!(
        PhysicalRange::from_start_and_length(u64::MAX - 0xfff, 0x2000),
        Err(LayoutError::RangeOverflow)
    );
    assert_eq!(
        PhysicalAddress::new(SUPPORTED_PHYSICAL_END),
        Err(LayoutError::UnsupportedPhysicalAddress)
    );
    assert_eq!(
        PhysicalRange::new(SUPPORTED_PHYSICAL_END - PAGE_SIZE, SUPPORTED_PHYSICAL_END),
        Ok(PhysicalRange::new(SUPPORTED_PHYSICAL_END - PAGE_SIZE, SUPPORTED_PHYSICAL_END).unwrap())
    );
}

#[test]
fn virtual_ranges_reject_empty_reversed_truncated_overflow_and_canonical_hole() {
    assert_eq!(
        VirtualRange::new(0x1000, 0x1000),
        Err(LayoutError::EmptyRange)
    );
    assert_eq!(
        VirtualRange::new(0x2000, 0x1000),
        Err(LayoutError::ReversedRange)
    );
    assert_eq!(
        VirtualRange::new(0x1000, 0x1800),
        Err(LayoutError::RangeNotPageAligned)
    );
    assert_eq!(
        VirtualRange::from_start_and_length(u64::MAX - 0xfff, 0x2000),
        Err(LayoutError::RangeOverflow)
    );
    assert_eq!(
        VirtualRange::new(
            LOW_CANONICAL_END - PAGE_SIZE,
            HIGH_CANONICAL_START + PAGE_SIZE
        ),
        Err(LayoutError::CrossesCanonicalHole)
    );
}

#[test]
fn declared_regions_have_exact_non_overlapping_canonical_bounds() {
    assert_eq!(validate_declared_regions(), Ok(()));
    let expected = [
        (AddressRegion::UserSpace, USER_SPACE_START, USER_SPACE_END),
        (AddressRegion::DirectMap, DIRECT_MAP_START, DIRECT_MAP_END),
        (
            AddressRegion::KernelServices,
            KERNEL_SERVICES_START,
            KERNEL_SERVICES_END,
        ),
        (AddressRegion::Mmio, MMIO_START, MMIO_END),
        (
            AddressRegion::Framebuffer,
            FRAMEBUFFER_START,
            FRAMEBUFFER_END,
        ),
        (AddressRegion::Reserved, FRAMEBUFFER_END, KERNEL_IMAGE_START),
        (
            AddressRegion::KernelImage,
            KERNEL_IMAGE_START,
            KERNEL_IMAGE_END,
        ),
        (
            AddressRegion::KernelLocal,
            KERNEL_LOCAL_START,
            KERNEL_LOCAL_END,
        ),
    ];
    for (actual, expected) in DECLARED_REGIONS.iter().zip(expected) {
        assert_eq!(actual.kind, expected.0);
        assert_eq!(actual.range.start().get(), expected.1);
        assert_eq!(actual.range.end(), expected.2);
    }
}

#[test]
fn direct_map_translation_is_bounded_to_64_tib() {
    assert_eq!(
        direct_map_address(PhysicalAddress::new(0).unwrap())
            .unwrap()
            .get(),
        DIRECT_MAP_START
    );
    assert_eq!(
        direct_map_address(PhysicalAddress::new(SUPPORTED_PHYSICAL_END - 1).unwrap())
            .unwrap()
            .get(),
        DIRECT_MAP_END - 1
    );
}

#[test]
fn permissions_reject_unknown_missing_read_writable_executable_and_user() {
    assert_eq!(
        MappingPermissions::from_bits(0x80),
        Err(LayoutError::UnknownPermissionBits)
    );
    assert_eq!(
        MappingPermissions::from_bits(MappingPermissions::WRITE),
        Err(LayoutError::ReadPermissionRequired)
    );
    assert_eq!(
        MappingPermissions::from_bits(
            MappingPermissions::READ | MappingPermissions::WRITE | MappingPermissions::EXECUTE,
        ),
        Err(LayoutError::WritableExecutable)
    );
    let user =
        MappingPermissions::from_bits(MappingPermissions::READ | MappingPermissions::USER).unwrap();
    let mut plan = MappingPlan::<1>::new();
    assert_eq!(
        plan.insert_mapping(
            virtual_range(KERNEL_IMAGE_START, KERNEL_IMAGE_START + PAGE_SIZE),
            physical_range(
                BOOTSTRAP_PHYSICAL_START,
                BOOTSTRAP_PHYSICAL_START + PAGE_SIZE
            ),
            user,
            CachePolicy::WriteBack,
            MappingKind::KernelText,
        ),
        Err(LayoutError::UserMappingForbidden)
    );
}

#[test]
fn planning_order_and_translation_are_deterministic() {
    let mut plan = MappingPlan::<3>::new();
    plan.insert_mapping(
        virtual_range(
            KERNEL_IMAGE_START + 2 * PAGE_SIZE,
            KERNEL_IMAGE_START + 3 * PAGE_SIZE,
        ),
        physical_range(0x202000, 0x203000),
        MappingPermissions::KERNEL_RW,
        CachePolicy::WriteBack,
        MappingKind::KernelData,
    )
    .unwrap();
    plan.insert_mapping(
        virtual_range(KERNEL_IMAGE_START, KERNEL_IMAGE_START + PAGE_SIZE),
        physical_range(0x200000, 0x201000),
        MappingPermissions::KERNEL_RX,
        CachePolicy::WriteBack,
        MappingKind::KernelText,
    )
    .unwrap();
    plan.insert_mapping(
        virtual_range(
            KERNEL_IMAGE_START + PAGE_SIZE,
            KERNEL_IMAGE_START + 2 * PAGE_SIZE,
        ),
        physical_range(0x201000, 0x202000),
        MappingPermissions::KERNEL_R,
        CachePolicy::WriteBack,
        MappingKind::KernelRodata,
    )
    .unwrap();

    assert_eq!(
        plan.entries()
            .iter()
            .map(|entry| entry.virtual_range().start().get())
            .collect::<std::vec::Vec<_>>(),
        [
            KERNEL_IMAGE_START,
            KERNEL_IMAGE_START + PAGE_SIZE,
            KERNEL_IMAGE_START + 2 * PAGE_SIZE,
        ]
    );
    assert_eq!(
        plan.translate(VirtualAddress::new(KERNEL_IMAGE_START + 0x123).unwrap())
            .unwrap()
            .get(),
        0x200123
    );
}

#[test]
fn plan_rejects_overlap_length_mismatch_and_exhaustion() {
    let mut plan = MappingPlan::<1>::new();
    plan.insert_mapping(
        virtual_range(KERNEL_IMAGE_START, KERNEL_IMAGE_START + PAGE_SIZE),
        physical_range(0x200000, 0x201000),
        MappingPermissions::KERNEL_RX,
        CachePolicy::WriteBack,
        MappingKind::KernelText,
    )
    .unwrap();
    assert_eq!(
        plan.insert_guard(virtual_range(
            KERNEL_LOCAL_START,
            KERNEL_LOCAL_START + PAGE_SIZE
        )),
        Err(LayoutError::PlanExhausted)
    );

    let mut overlap = MappingPlan::<2>::new();
    overlap
        .insert_guard(virtual_range(
            KERNEL_LOCAL_START,
            KERNEL_LOCAL_START + PAGE_SIZE,
        ))
        .unwrap();
    assert_eq!(
        overlap.insert_guard(virtual_range(
            KERNEL_LOCAL_START,
            KERNEL_LOCAL_START + 2 * PAGE_SIZE,
        )),
        Err(LayoutError::VirtualOverlap)
    );

    let mut mismatch = MappingPlan::<1>::new();
    assert_eq!(
        mismatch.insert_mapping(
            virtual_range(KERNEL_IMAGE_START, KERNEL_IMAGE_START + 2 * PAGE_SIZE),
            physical_range(0x200000, 0x201000),
            MappingPermissions::KERNEL_RX,
            CachePolicy::WriteBack,
            MappingKind::KernelText,
        ),
        Err(LayoutError::RangeLengthMismatch)
    );
}

#[test]
fn kernel_segment_stack_guard_and_boot_data_policies_are_exact() {
    let mut plan = MappingPlan::<8>::new();
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
        plan.insert_mapping(
            virtual_range(
                KERNEL_IMAGE_START + offset,
                KERNEL_IMAGE_START + offset + PAGE_SIZE,
            ),
            physical_range(0x200000 + offset, 0x200000 + offset + PAGE_SIZE),
            permissions,
            CachePolicy::WriteBack,
            kind,
        )
        .unwrap();
    }
    let guard = virtual_range(KERNEL_LOCAL_START, KERNEL_LOCAL_START + PAGE_SIZE);
    plan.insert_guard(guard).unwrap();
    plan.insert_mapping(
        virtual_range(
            KERNEL_LOCAL_START + PAGE_SIZE,
            KERNEL_LOCAL_START + 17 * PAGE_SIZE,
        ),
        physical_range(0x400000, 0x410000),
        MappingPermissions::KERNEL_RW,
        CachePolicy::WriteBack,
        MappingKind::BootstrapStack,
    )
    .unwrap();
    for (offset, kind) in [
        (0x12000, MappingKind::BootInfo),
        (0x13000, MappingKind::BootMemoryMap),
    ] {
        let virtual_start = DIRECT_MAP_START + offset;
        plan.insert_mapping(
            virtual_range(virtual_start, virtual_start + PAGE_SIZE),
            physical_range(offset, offset + PAGE_SIZE),
            MappingPermissions::KERNEL_R,
            CachePolicy::WriteBack,
            kind,
        )
        .unwrap();
    }
    assert!(plan.entries().iter().any(|entry| entry.is_guard()));
    assert_eq!(
        plan.translate(VirtualAddress::new(KERNEL_LOCAL_START).unwrap()),
        Err(LayoutError::Unmapped)
    );
    assert_eq!(plan.validate_final(), Ok(()));
}

#[test]
fn page_table_frame_is_rw_nx_and_directly_accessible() {
    let physical = 0x300000;
    let virtual_start = DIRECT_MAP_START + physical;
    let mut plan = MappingPlan::<1>::new();
    plan.insert_mapping(
        virtual_range(virtual_start, virtual_start + PAGE_SIZE),
        physical_range(physical, physical + PAGE_SIZE),
        MappingPermissions::KERNEL_RW,
        CachePolicy::WriteBack,
        MappingKind::PageTable,
    )
    .unwrap();
    assert_eq!(
        plan.translate(VirtualAddress::new(virtual_start + 8).unwrap())
            .unwrap()
            .get(),
        physical + 8
    );
    assert!(!plan.entries()[0].permissions().unwrap().executable());
}

#[test]
fn framebuffer_and_mmio_require_dedicated_uncached_regions() {
    let mut plan = MappingPlan::<2>::new();
    plan.insert_mapping(
        virtual_range(FRAMEBUFFER_START, FRAMEBUFFER_START + PAGE_SIZE),
        physical_range(0x80000000, 0x80001000),
        MappingPermissions::KERNEL_RW,
        CachePolicy::Uncached,
        MappingKind::Framebuffer,
    )
    .unwrap();
    plan.insert_mapping(
        virtual_range(MMIO_START, MMIO_START + PAGE_SIZE),
        physical_range(0xfec00000, 0xfec01000),
        MappingPermissions::KERNEL_RW,
        CachePolicy::Uncached,
        MappingKind::Mmio,
    )
    .unwrap();

    let mut wrong_cache = MappingPlan::<1>::new();
    assert_eq!(
        wrong_cache.insert_mapping(
            virtual_range(FRAMEBUFFER_START, FRAMEBUFFER_START + PAGE_SIZE),
            physical_range(0x80000000, 0x80001000),
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
            MappingKind::Framebuffer,
        ),
        Err(LayoutError::InvalidCachePolicy)
    );
}

#[test]
fn writable_alias_of_executable_physical_memory_is_rejected() {
    let mut plan = MappingPlan::<2>::new();
    plan.insert_mapping(
        virtual_range(KERNEL_IMAGE_START, KERNEL_IMAGE_START + PAGE_SIZE),
        physical_range(
            BOOTSTRAP_PHYSICAL_START,
            BOOTSTRAP_PHYSICAL_START + PAGE_SIZE,
        ),
        MappingPermissions::KERNEL_RX,
        CachePolicy::WriteBack,
        MappingKind::KernelText,
    )
    .unwrap();
    assert_eq!(
        plan.insert_mapping(
            virtual_range(
                DIRECT_MAP_START + BOOTSTRAP_PHYSICAL_START,
                DIRECT_MAP_START + BOOTSTRAP_PHYSICAL_START + PAGE_SIZE,
            ),
            physical_range(
                BOOTSTRAP_PHYSICAL_START,
                BOOTSTRAP_PHYSICAL_START + PAGE_SIZE
            ),
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
            MappingKind::DirectMap,
        ),
        Err(LayoutError::PhysicalAliasWriteExecute)
    );
}

#[test]
fn writable_alias_of_read_only_physical_memory_is_rejected() {
    let physical = 0x300000;
    let mut plan = MappingPlan::<2>::new();
    plan.insert_mapping(
        virtual_range(KERNEL_IMAGE_START, KERNEL_IMAGE_START + PAGE_SIZE),
        physical_range(physical, physical + PAGE_SIZE),
        MappingPermissions::KERNEL_R,
        CachePolicy::WriteBack,
        MappingKind::KernelRodata,
    )
    .unwrap();
    assert_eq!(
        plan.insert_mapping(
            virtual_range(
                DIRECT_MAP_START + physical,
                DIRECT_MAP_START + physical + PAGE_SIZE,
            ),
            physical_range(physical, physical + PAGE_SIZE),
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
            MappingKind::DirectMap,
        ),
        Err(LayoutError::PhysicalAliasPermissionEscalation)
    );

    let mut equal_permissions = MappingPlan::<2>::new();
    equal_permissions
        .insert_mapping(
            virtual_range(KERNEL_IMAGE_START, KERNEL_IMAGE_START + PAGE_SIZE),
            physical_range(physical, physical + PAGE_SIZE),
            MappingPermissions::KERNEL_R,
            CachePolicy::WriteBack,
            MappingKind::KernelRodata,
        )
        .unwrap();
    assert_eq!(
        equal_permissions.insert_mapping(
            virtual_range(
                DIRECT_MAP_START + physical,
                DIRECT_MAP_START + physical + PAGE_SIZE,
            ),
            physical_range(physical, physical + PAGE_SIZE),
            MappingPermissions::KERNEL_R,
            CachePolicy::WriteBack,
            MappingKind::DirectMap,
        ),
        Ok(())
    );
}

#[test]
fn transition_identity_is_exact_bounded_and_absent_from_final_plan() {
    let mut exact = MappingPlan::<2>::new();
    exact
        .insert_mapping(
            virtual_range(0x100000, 0x100000 + PAGE_SIZE),
            physical_range(0x100000, 0x100000 + PAGE_SIZE),
            MappingPermissions::KERNEL_RX,
            CachePolicy::WriteBack,
            MappingKind::TransitionIdentity,
        )
        .unwrap();
    exact
        .insert_mapping(
            virtual_range(0x200000, 0x200000 + BOOTSTRAP_STACK_BYTES),
            physical_range(0x200000, 0x200000 + BOOTSTRAP_STACK_BYTES),
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
            MappingKind::TransitionIdentity,
        )
        .unwrap();
    assert_eq!(exact.validate_transition(), Ok(()));
    assert_eq!(
        exact.validate_final(),
        Err(LayoutError::TransitionMappingInFinalPlan)
    );

    let mut excessive = MappingPlan::<1>::new();
    excessive
        .insert_mapping(
            virtual_range(
                0x100000,
                0x100000 + MAX_TRANSITION_IDENTITY_BYTES + PAGE_SIZE,
            ),
            physical_range(
                0x100000,
                0x100000 + MAX_TRANSITION_IDENTITY_BYTES + PAGE_SIZE,
            ),
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
            MappingKind::TransitionIdentity,
        )
        .unwrap();
    assert_eq!(
        excessive.validate_transition(),
        Err(LayoutError::TransitionIdentityTooLarge)
    );

    let mut split_stack = MappingPlan::<3>::new();
    split_stack
        .insert_mapping(
            virtual_range(0x100000, 0x100000 + PAGE_SIZE),
            physical_range(0x100000, 0x100000 + PAGE_SIZE),
            MappingPermissions::KERNEL_RX,
            CachePolicy::WriteBack,
            MappingKind::TransitionIdentity,
        )
        .unwrap();
    for start in [0x200000, 0x208000] {
        split_stack
            .insert_mapping(
                virtual_range(start, start + BOOTSTRAP_STACK_BYTES / 2),
                physical_range(start, start + BOOTSTRAP_STACK_BYTES / 2),
                MappingPermissions::KERNEL_RW,
                CachePolicy::WriteBack,
                MappingKind::TransitionIdentity,
            )
            .unwrap();
    }
    assert_eq!(
        split_stack.validate_transition(),
        Err(LayoutError::InvalidTransitionComposition)
    );

    let mut mismatch = MappingPlan::<1>::new();
    assert_eq!(
        mismatch.insert_mapping(
            virtual_range(0x100000, 0x101000),
            physical_range(0x200000, 0x201000),
            MappingPermissions::KERNEL_RX,
            CachePolicy::WriteBack,
            MappingKind::TransitionIdentity,
        ),
        Err(LayoutError::IdentityAddressMismatch)
    );
    assert_eq!(
        BOOTSTRAP_PHYSICAL_END - BOOTSTRAP_PHYSICAL_START,
        64 * 1024 * 1024
    );
    assert_eq!(TRANSITION_IDENTITY_END, 1_u64 << 32);
    assert_eq!(
        MAX_TRANSITION_IDENTITY_BYTES,
        PAGE_SIZE + BOOTSTRAP_STACK_BYTES
    );
}

#[test]
fn region_and_mapping_policy_failures_are_structured() {
    let mut wrong_region = MappingPlan::<1>::new();
    assert_eq!(
        wrong_region.insert_mapping(
            virtual_range(KERNEL_LOCAL_START, KERNEL_LOCAL_START + PAGE_SIZE),
            physical_range(0x200000, 0x201000),
            MappingPermissions::KERNEL_RX,
            CachePolicy::WriteBack,
            MappingKind::KernelText,
        ),
        Err(LayoutError::OutsideDeclaredRegion)
    );

    let mut wrong_permissions = MappingPlan::<1>::new();
    assert_eq!(
        wrong_permissions.insert_mapping(
            virtual_range(KERNEL_IMAGE_START, KERNEL_IMAGE_START + PAGE_SIZE),
            physical_range(0x200000, 0x201000),
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
            MappingKind::KernelText,
        ),
        Err(LayoutError::InvalidRegionPermissions)
    );

    let mut physical_window = MappingPlan::<1>::new();
    assert_eq!(
        physical_window.insert_mapping(
            virtual_range(KERNEL_IMAGE_START, KERNEL_IMAGE_START + PAGE_SIZE),
            physical_range(0x100000, 0x101000),
            MappingPermissions::KERNEL_RX,
            CachePolicy::WriteBack,
            MappingKind::KernelText,
        ),
        Err(LayoutError::OutsideBootstrapPhysicalWindow)
    );

    let mut direct_relation = MappingPlan::<1>::new();
    assert_eq!(
        direct_relation.insert_mapping(
            virtual_range(DIRECT_MAP_START + 0x2000, DIRECT_MAP_START + 0x3000),
            physical_range(0x1000, 0x2000),
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
            MappingKind::DirectMap,
        ),
        Err(LayoutError::DirectMapAddressMismatch)
    );
}

#[test]
fn final_stack_requires_exact_size_low_physical_memory_and_guard() {
    let stack_virtual = virtual_range(
        KERNEL_LOCAL_START + PAGE_SIZE,
        KERNEL_LOCAL_START + PAGE_SIZE + BOOTSTRAP_STACK_BYTES,
    );
    let stack_physical = physical_range(0x400000, 0x400000 + BOOTSTRAP_STACK_BYTES);
    let mut missing_guard = MappingPlan::<1>::new();
    missing_guard
        .insert_mapping(
            stack_virtual,
            stack_physical,
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
            MappingKind::BootstrapStack,
        )
        .unwrap();
    assert_eq!(
        missing_guard.validate_final(),
        Err(LayoutError::MissingStackGuard)
    );

    let mut wrong_size = MappingPlan::<1>::new();
    assert_eq!(
        wrong_size.insert_mapping(
            virtual_range(KERNEL_LOCAL_START, KERNEL_LOCAL_START + PAGE_SIZE),
            physical_range(0x400000, 0x401000),
            MappingPermissions::KERNEL_RW,
            CachePolicy::WriteBack,
            MappingKind::BootstrapStack,
        ),
        Err(LayoutError::InvalidBootstrapStack)
    );
}

#[test]
fn range_containment_overlap_and_backing_are_explicit() {
    let outer = physical_range(0x1000, 0x5000);
    let inner = physical_range(0x2000, 0x3000);
    let adjacent = physical_range(0x5000, 0x6000);
    assert!(outer.contains(PhysicalAddress::new(0x1000).unwrap()));
    assert!(outer.contains_range(inner));
    assert!(outer.overlaps(inner));
    assert!(!outer.overlaps(adjacent));

    let mut plan = MappingPlan::<1>::new();
    plan.insert_guard(virtual_range(
        KERNEL_LOCAL_START,
        KERNEL_LOCAL_START + PAGE_SIZE,
    ))
    .unwrap();
    assert_eq!(plan.entries()[0].backing(), EntryBacking::Unmapped);
    assert_eq!(plan.entries()[0].cache_policy(), None);
}
