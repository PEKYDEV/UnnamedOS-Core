use kernel_image::ValidatedHigherHalfImage;
use memory_layout::{
    CachePolicy, LayoutError, MappingKind, MappingPermissions, MappingPlan, PhysicalRange,
    VirtualAddress, VirtualRange,
};

use crate::{
    LOAD_PAGE_SIZE, LoadItem, LoadPlan, MAX_LOAD_ITEMS, PlanError, SegmentSpec, SourceAllocation,
    round_up,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HigherHalfPlanError {
    Load(PlanError),
    Layout(LayoutError),
    InvalidPermissions,
    TranslationOverflow,
    EntryTranslationMismatch,
    EntryWithoutExecutableMapping,
}

pub struct HigherHalfLoadPlan {
    load: LoadPlan,
    mappings: MappingPlan<MAX_LOAD_ITEMS>,
    virtual_entry: u64,
    physical_entry: u64,
    translation_offset: u64,
    virtual_start: u64,
    virtual_end: u64,
}

impl HigherHalfLoadPlan {
    pub fn from_validated(
        image: &ValidatedHigherHalfImage<'_>,
        source: SourceAllocation,
    ) -> Result<Self, HigherHalfPlanError> {
        let load = LoadPlan::build(
            image.physical_entry(),
            image.load_segments().map(|segment| SegmentSpec {
                file_offset: segment.file_offset(),
                file_size: segment.file_size(),
                memory_size: segment.memory_size(),
                target: segment.physical_address(),
                flags: segment.flags(),
            }),
            source,
        )
        .map_err(HigherHalfPlanError::Load)?;

        let mut mappings = MappingPlan::new();
        for segment in image.load_segments() {
            let physical_end = round_up(
                segment
                    .physical_address()
                    .checked_add(segment.memory_size())
                    .ok_or(HigherHalfPlanError::TranslationOverflow)?,
                LOAD_PAGE_SIZE,
            )
            .map_err(HigherHalfPlanError::Load)?;
            let virtual_end = round_up(
                segment
                    .virtual_address()
                    .checked_add(segment.memory_size())
                    .ok_or(HigherHalfPlanError::TranslationOverflow)?,
                LOAD_PAGE_SIZE,
            )
            .map_err(HigherHalfPlanError::Load)?;
            let physical = PhysicalRange::new(segment.physical_address(), physical_end)
                .map_err(HigherHalfPlanError::Layout)?;
            let virtual_range = VirtualRange::new(segment.virtual_address(), virtual_end)
                .map_err(HigherHalfPlanError::Layout)?;
            let (permissions, kind) = mapping_policy(segment.flags())?;
            mappings
                .insert_mapping(
                    virtual_range,
                    physical,
                    permissions,
                    CachePolicy::WriteBack,
                    kind,
                )
                .map_err(HigherHalfPlanError::Layout)?;
        }

        let translated = mappings
            .translate(
                VirtualAddress::new(image.virtual_entry()).map_err(HigherHalfPlanError::Layout)?,
            )
            .map_err(HigherHalfPlanError::Layout)?;
        if translated.get() != image.physical_entry() {
            return Err(HigherHalfPlanError::EntryTranslationMismatch);
        }
        let executable = mappings.entries().iter().any(|entry| {
            entry.virtual_range().contains(
                VirtualAddress::new(image.virtual_entry()).expect("validated canonical entry"),
            ) && entry.permissions().is_some_and(|value| value.executable())
        });
        if !executable {
            return Err(HigherHalfPlanError::EntryWithoutExecutableMapping);
        }

        let (virtual_start, virtual_end) = image.virtual_load_range();
        Ok(Self {
            load,
            mappings,
            virtual_entry: image.virtual_entry(),
            physical_entry: image.physical_entry(),
            translation_offset: image.translation_offset(),
            virtual_start,
            virtual_end,
        })
    }

    pub const fn load_plan(&self) -> &LoadPlan {
        &self.load
    }

    pub const fn mapping_plan(&self) -> &MappingPlan<MAX_LOAD_ITEMS> {
        &self.mappings
    }

    pub fn load_items(&self) -> impl Iterator<Item = LoadItem> + '_ {
        self.load.items()
    }

    pub const fn virtual_entry(&self) -> u64 {
        self.virtual_entry
    }

    pub const fn physical_entry(&self) -> u64 {
        self.physical_entry
    }

    pub const fn translation_offset(&self) -> u64 {
        self.translation_offset
    }

    pub const fn virtual_span(&self) -> (u64, u64) {
        (self.virtual_start, self.virtual_end)
    }
}

fn mapping_policy(flags: u32) -> Result<(MappingPermissions, MappingKind), HigherHalfPlanError> {
    match flags {
        5 => Ok((MappingPermissions::KERNEL_RX, MappingKind::KernelText)),
        4 => Ok((MappingPermissions::KERNEL_R, MappingKind::KernelRodata)),
        6 => Ok((MappingPermissions::KERNEL_RW, MappingKind::KernelData)),
        _ => Err(HigherHalfPlanError::InvalidPermissions),
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use kernel_image::{
        HIGHER_HALF_LINK_ADDRESS, HIGHER_HALF_VIRTUAL_OFFSET, validate_higher_half_image,
    };
    use memory_layout::{
        ConstructionPlan, EntryBacking, EntryFlags, MappingKind, PlanMode, VirtualAddress,
    };

    use super::*;

    const SOURCE: SourceAllocation = SourceAllocation {
        page_start: 0x1000_0000,
        page_count: 4,
        file_length: 0x4000,
    };

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    fn fixture() -> std::vec::Vec<u8> {
        let mut bytes = std::vec![0_u8; 0x3010];
        bytes[..16].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        put_u16(&mut bytes, 16, 2);
        put_u16(&mut bytes, 18, 62);
        put_u32(&mut bytes, 20, 1);
        put_u64(&mut bytes, 24, HIGHER_HALF_LINK_ADDRESS);
        put_u64(&mut bytes, 32, 64);
        put_u16(&mut bytes, 52, 64);
        put_u16(&mut bytes, 54, 56);
        put_u16(&mut bytes, 56, 3);
        for (index, file, physical, flags, file_size, memory_size) in [
            (0, 0x1000, 0x200000, 5, 16, 16),
            (1, 0x2000, 0x201000, 4, 16, 16),
            (2, 0x3000, 0x202000, 6, 16, 0x2000),
        ] {
            let header = 64 + index * 56;
            put_u32(&mut bytes, header, 1);
            put_u32(&mut bytes, header + 4, flags);
            put_u64(&mut bytes, header + 8, file);
            put_u64(
                &mut bytes,
                header + 16,
                HIGHER_HALF_VIRTUAL_OFFSET + physical,
            );
            put_u64(&mut bytes, header + 24, physical);
            put_u64(&mut bytes, header + 32, file_size);
            put_u64(&mut bytes, header + 40, memory_size);
            put_u64(&mut bytes, header + 48, 4096);
        }
        bytes
    }

    fn plan() -> HigherHalfLoadPlan {
        let bytes = fixture();
        let image = validate_higher_half_image(&bytes).expect("image");
        HigherHalfLoadPlan::from_validated(&image, SOURCE).expect("plan")
    }

    #[test]
    fn copy_targets_and_virtual_mappings_remain_distinct() {
        let plan = plan();
        assert_eq!(plan.virtual_entry(), 0xffff_ffff_8020_0000);
        assert_eq!(plan.physical_entry(), 0x0020_0000);
        assert_eq!(plan.translation_offset(), 0xffff_ffff_8000_0000);
        assert_eq!(
            plan.load_items()
                .map(|item| item.page_start)
                .collect::<std::vec::Vec<_>>(),
            [0x200000, 0x201000, 0x202000]
        );
        let mappings = plan.mapping_plan().entries();
        assert_eq!(mappings.len(), 3);
        assert!(
            mappings
                .iter()
                .all(|entry| entry.virtual_range().start().get() >= HIGHER_HALF_LINK_ADDRESS)
        );
        assert_eq!(mappings[0].kind(), MappingKind::KernelText);
        assert_eq!(mappings[1].kind(), MappingKind::KernelRodata);
        assert_eq!(mappings[2].kind(), MappingKind::KernelData);
        assert_eq!(
            plan.mapping_plan()
                .translate(VirtualAddress::new(plan.virtual_entry()).unwrap())
                .unwrap()
                .get(),
            plan.physical_entry()
        );
    }

    #[test]
    fn higher_half_mappings_feed_the_final_page_table_planner_exactly() {
        let plan = plan();
        let first = ConstructionPlan::<8, 16, 1>::build(plan.mapping_plan(), PlanMode::Final)
            .expect("construction");
        let second = ConstructionPlan::<8, 16, 1>::build(plan.mapping_plan(), PlanMode::Final)
            .expect("repeat");
        assert_eq!(first.table_count(), 4);
        assert_eq!(first.entry_count(), 7);
        assert_eq!(first.removal_count(), 0);
        let mut first_bytes = [0_u8; 512];
        let mut second_bytes = [0_u8; 512];
        let first_len = first.encode_abstract(&mut first_bytes).unwrap();
        let second_len = second.encode_abstract(&mut second_bytes).unwrap();
        assert_eq!(first_len, second_len);
        assert_eq!(&first_bytes[..first_len], &second_bytes[..second_len]);
        let entry = first
            .leaf_entry(VirtualAddress::new(plan.virtual_entry()).unwrap())
            .expect("entry leaf");
        assert_eq!(entry.target().physical_frame().unwrap().address(), 0x200000);
        assert!(entry.flags().executable());
        for mapping in plan.mapping_plan().entries() {
            let EntryBacking::Mapped(physical) = mapping.backing() else {
                panic!("mapped segment")
            };
            let leaf = first
                .leaf_entry(mapping.virtual_range().start())
                .expect("leaf");
            assert_eq!(
                leaf.target().physical_frame().unwrap().address(),
                physical.start().get()
            );
            assert_ne!(leaf.flags().writable() && leaf.flags().executable(), true);
            if mapping.kind() != MappingKind::KernelText {
                assert_eq!(
                    leaf.flags().bits() & EntryFlags::NO_EXECUTE,
                    EntryFlags::NO_EXECUTE
                );
            }
        }
    }

    #[test]
    fn final_higher_half_plan_contains_no_low_or_transition_mapping() {
        let plan = plan();
        assert!(plan.mapping_plan().entries().iter().all(|entry| {
            entry.virtual_range().start().get() >= memory_layout::HIGH_CANONICAL_START
                && entry.kind() != MappingKind::TransitionIdentity
        }));
        plan.mapping_plan()
            .validate_final()
            .expect("final mapping policy");
    }
}
