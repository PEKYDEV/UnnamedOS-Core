use core::mem::{align_of, size_of};

use memory_layout::{
    ActivationReadiness, CpuCapabilityError, HardeningState, PcidState, PgeState, PhysicalFrame,
    RawCpuSnapshot, SUPPORTED_PHYSICAL_END,
};

const MSR: u32 = 1 << 5;
const PAE: u32 = 1 << 6;
const NX: u32 = 1 << 20;
const LM: u32 = 1 << 29;
const LA57: u32 = 1 << 16;

fn supported() -> RawCpuSnapshot {
    RawCpuSnapshot {
        maximum_basic_leaf: 7,
        maximum_extended_leaf: 0x8000_0008,
        basic_feature_edx: MSR | PAE,
        structured_feature_ecx: 0,
        extended_feature_edx: NX | LM,
        address_width_eax: 39 | (48 << 8),
        cr0: (1 << 31) | (1 << 16),
        cr3: 0x1234_5000,
        cr4: 1 << 5,
        efer: (1 << 8) | (1 << 10) | (1 << 11),
    }
}

fn readiness(raw: RawCpuSnapshot) -> Result<ActivationReadiness, CpuCapabilityError> {
    let frames = [
        PhysicalFrame::new(0x20_0000).unwrap(),
        PhysicalFrame::new(0x31_0000).unwrap(),
    ];
    raw.validate()?
        .classify_for_hierarchy(&frames, frames[0], 0x4000_0000)
}

#[test]
fn stable_layout_and_ready_classification_are_explicit() {
    assert_eq!(
        (size_of::<RawCpuSnapshot>(), align_of::<RawCpuSnapshot>()),
        (56, 8)
    );
    assert_eq!(
        (
            size_of::<ActivationReadiness>(),
            align_of::<ActivationReadiness>()
        ),
        (48, 8)
    );
    let result = readiness(supported()).unwrap();
    assert_eq!(result.nxe(), HardeningState::Enabled);
    assert_eq!(result.write_protect(), HardeningState::Enabled);
    assert_eq!(result.pge(), PgeState::Disabled);
    assert_eq!(result.pcid(), PcidState::Disabled);
    assert_eq!(result.effective_linear_address_bits(), 48);
    assert!(result.transition_permitted());
}

#[test]
fn missing_leaf_and_feature_order_is_deterministic() {
    let mut raw = supported();
    raw.maximum_basic_leaf = 0;
    raw.maximum_extended_leaf = 0;
    raw.extended_feature_edx = 0;
    assert_eq!(
        raw.validate(),
        Err(CpuCapabilityError::MissingBasicFeatureLeaf)
    );
    raw.maximum_basic_leaf = 1;
    assert_eq!(
        raw.validate(),
        Err(CpuCapabilityError::MissingExtendedFeatureLeaf)
    );
    raw.maximum_extended_leaf = 0x8000_0001;
    assert_eq!(
        raw.validate(),
        Err(CpuCapabilityError::MissingAddressWidthLeaf)
    );
    for (basic, extended, expected) in [
        (0, LM, CpuCapabilityError::LongModeUnsupported),
        (0, NX, CpuCapabilityError::NxUnsupported),
        (MSR, 0, CpuCapabilityError::MsrUnsupported),
        (PAE, 0, CpuCapabilityError::PaeUnsupported),
    ] {
        let mut raw = supported();
        raw.basic_feature_edx &= !basic;
        raw.extended_feature_edx &= !extended;
        assert_eq!(raw.validate(), Err(expected));
    }
}

#[test]
fn mandatory_current_state_and_contradictions_fail_closed() {
    for (case, expected) in [
        (1, CpuCapabilityError::PagingInactive),
        (2, CpuCapabilityError::LongModeInactive),
        (3, CpuCapabilityError::ContradictoryLongModeState),
        (4, CpuCapabilityError::PaeInactive),
        (5, CpuCapabilityError::ContradictoryLa57State),
        (6, CpuCapabilityError::La57Enabled),
    ] {
        let mut raw = supported();
        match case {
            1 => raw.cr0 &= !(1 << 31),
            2 => raw.efer &= !(1 << 10),
            3 => raw.efer &= !(1 << 8),
            4 => raw.cr4 &= !(1 << 5),
            5 => raw.cr4 |= 1 << 12,
            6 => {
                raw.cr4 |= 1 << 12;
                raw.structured_feature_ecx |= LA57;
            }
            _ => unreachable!(),
        }
        assert_eq!(raw.validate(), Err(expected));
    }
}

#[test]
fn la57_supported_but_disabled_retains_effective_four_level_mode() {
    let mut raw = supported();
    raw.structured_feature_ecx = LA57;
    raw.address_width_eax = 46 | (57 << 8);
    let validated = raw.validate().unwrap();
    assert!(validated.la57_supported());
    assert_eq!(validated.reported_linear_address_bits(), 57);
    assert_eq!(readiness(raw).unwrap().effective_linear_address_bits(), 48);
    raw.structured_feature_ecx = 0;
    assert_eq!(
        raw.validate(),
        Err(CpuCapabilityError::ContradictoryLinearAddressWidth)
    );
}

#[test]
fn physical_width_and_hierarchy_ranges_are_checked() {
    for bits in [35_u8, 53] {
        let mut raw = supported();
        raw.address_width_eax = u32::from(bits) | (48 << 8);
        assert_eq!(
            raw.validate(),
            Err(CpuCapabilityError::InvalidPhysicalAddressWidth)
        );
    }
    for bits in [36_u8, 52] {
        let mut raw = supported();
        raw.address_width_eax = u32::from(bits) | (48 << 8);
        assert!(raw.validate().is_ok());
    }
    let validated = supported().validate().unwrap();
    let root = PhysicalFrame::new(0x20_0000).unwrap();
    assert_eq!(
        validated.classify_for_hierarchy(&[root], root, SUPPORTED_PHYSICAL_END + 1),
        Err(CpuCapabilityError::ArchitecturePhysicalCapExceeded)
    );
    assert_eq!(
        validated.classify_for_hierarchy(&[root], root, 1_u64 << 40),
        Err(CpuCapabilityError::MappedPhysicalRangeUnsupported)
    );

    let mut narrow = supported();
    narrow.address_width_eax = 36 | (48 << 8);
    let narrow = narrow.validate().unwrap();
    let high_root = PhysicalFrame::new(1_u64 << 36).unwrap();
    assert_eq!(
        narrow.classify_for_hierarchy(&[high_root], high_root, 0x1000),
        Err(CpuCapabilityError::ProposedRootUnsupported)
    );
    assert_eq!(
        narrow.classify_for_hierarchy(&[root, high_root], root, 0x1000),
        Err(CpuCapabilityError::OwnedFrameUnsupported)
    );
}

#[test]
fn cr3_pcid_and_legacy_flag_interpretations_are_separate() {
    let mut raw = supported();
    raw.cr3 |= (1 << 3) | (1 << 4);
    let legacy = raw.validate().unwrap().current_cr3();
    assert_eq!(
        (legacy.root_address(), legacy.context_or_flags()),
        (0x1234_5000, 0x18)
    );
    assert_eq!(legacy.pcid_state(), PcidState::Disabled);
    raw.cr4 |= 1 << 17;
    raw.cr3 = 0x1234_5abc;
    let pcid = raw.validate().unwrap().current_cr3();
    assert_eq!(
        (pcid.root_address(), pcid.context_or_flags()),
        (0x1234_5000, 0xabc)
    );
    assert_eq!(pcid.pcid_state(), PcidState::EnabledMustRemainUnused);
    raw.cr4 &= !(1 << 17);
    assert_eq!(
        raw.validate(),
        Err(CpuCapabilityError::UnsupportedCr3Encoding)
    );
    raw.cr3 = 0;
    assert_eq!(
        raw.validate(),
        Err(CpuCapabilityError::InvalidCurrentCr3Root)
    );
    raw.cr3 = (1_u64 << 39) | 0x1000;
    assert_eq!(
        raw.validate(),
        Err(CpuCapabilityError::UnsupportedCr3Encoding)
    );
}

#[test]
fn every_nxe_wp_combination_and_pge_is_classified() {
    for (nxe, wp) in [(false, false), (false, true), (true, false), (true, true)] {
        let mut raw = supported();
        if !nxe {
            raw.efer &= !(1 << 11);
        }
        if !wp {
            raw.cr0 &= !(1 << 16);
        }
        raw.cr4 |= 1 << 7;
        let result = readiness(raw).unwrap();
        assert_eq!(result.nxe() == HardeningState::Enabled, nxe);
        assert_eq!(result.write_protect() == HardeningState::Enabled, wp);
        assert_eq!(result.pge(), PgeState::Enabled);
    }
}

#[test]
fn proposed_root_and_cr3_stability_are_proven() {
    let validated = supported().validate().unwrap();
    let root = PhysicalFrame::new(0x20_0000).unwrap();
    let other = PhysicalFrame::new(0x21_0000).unwrap();
    assert_eq!(
        validated.classify_for_hierarchy(&[], root, 0x1000),
        Err(CpuCapabilityError::InvalidProposedRoot)
    );
    assert_eq!(
        validated.classify_for_hierarchy(&[other, root], root, 0x1000),
        Err(CpuCapabilityError::InvalidProposedRoot)
    );
    let token = validated.cr3_stability_token();
    assert_eq!(token.verify(supported().cr3), Ok(()));
    assert_eq!(
        token.verify(supported().cr3 + 0x1000),
        Err(CpuCapabilityError::InheritedCr3Changed)
    );
}
