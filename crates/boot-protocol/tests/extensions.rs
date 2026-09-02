use core::mem::{align_of, offset_of, size_of};

use boot_protocol::{
    ABI_MAJOR, ABI_MINOR, BOOT_ENVELOPE_ABI_MAJOR, BOOT_ENVELOPE_ABI_MINOR, BOOT_INFO_SIZE,
    EXTENSION_FLAG_REQUIRED, EXTENSION_HEADER_SIZE, EXTENSION_KIND_PAGE_TABLE_OWNERSHIP,
    EXTENSION_VERSION_1, ExtensionError, ExtensionHeader, MEMORY_KIND_PAGE_TABLE,
    MEMORY_KIND_USABLE, MEMORY_PAGE_SIZE, MemoryDescriptor, OWNED_PAGE_TABLE_FRAME_SIZE,
    OwnedPageTableFrame, PAGE_TABLE_HIERARCHY_VERSION, PAGE_TABLE_OWNERSHIP_SIZE,
    PAGE_TABLE_PHYSICAL_CAP, PAGE_TABLE_STATE_FINAL, PageTableOwnership, validate_extension_area,
    validate_page_table_frames,
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

fn page_table_record() -> [u8; PAGE_TABLE_OWNERSHIP_SIZE as usize] {
    let mut bytes = [0_u8; PAGE_TABLE_OWNERSHIP_SIZE as usize];
    put_u32(&mut bytes, 0, EXTENSION_KIND_PAGE_TABLE_OWNERSHIP);
    put_u16(&mut bytes, 4, EXTENSION_VERSION_1);
    put_u16(&mut bytes, 6, EXTENSION_HEADER_SIZE);
    put_u32(&mut bytes, 8, PAGE_TABLE_OWNERSHIP_SIZE);
    put_u32(&mut bytes, 12, EXTENSION_FLAG_REQUIRED);
    put_u16(&mut bytes, 16, PAGE_TABLE_HIERARCHY_VERSION);
    bytes[18] = 4;
    bytes[19] = PAGE_TABLE_STATE_FINAL;
    put_u32(&mut bytes, 20, MEMORY_PAGE_SIZE as u32);
    put_u64(&mut bytes, 24, 0x100000);
    put_u64(&mut bytes, 32, 0x200000);
    put_u32(&mut bytes, 40, 4);
    put_u32(&mut bytes, 44, OWNED_PAGE_TABLE_FRAME_SIZE);
    put_u64(&mut bytes, 48, PAGE_TABLE_PHYSICAL_CAP);
    bytes
}

fn ownership() -> PageTableOwnership {
    validate_extension_area(
        BOOT_ENVELOPE_ABI_MAJOR,
        BOOT_ENVELOPE_ABI_MINOR,
        BOOT_INFO_SIZE + PAGE_TABLE_OWNERSHIP_SIZE,
        &page_table_record(),
    )
    .unwrap()
    .page_table_ownership()
    .unwrap()
    .raw()
}

fn frames() -> [OwnedPageTableFrame; 4] {
    [0x100000, 0x101000, 0x102000, 0x103000].map(|physical_frame| OwnedPageTableFrame {
        physical_frame,
        reserved0: 0,
    })
}

fn reserved_map() -> [MemoryDescriptor; 1] {
    [MemoryDescriptor {
        kind: MEMORY_KIND_PAGE_TABLE,
        reserved0: 0,
        physical_start: 0x100000,
        page_count: 4,
        attributes: 0,
    }]
}

#[test]
fn extension_wire_layout_is_fixed() {
    assert_eq!(size_of::<ExtensionHeader>(), 16);
    assert_eq!(align_of::<ExtensionHeader>(), 4);
    assert_eq!(offset_of!(ExtensionHeader, kind), 0);
    assert_eq!(offset_of!(ExtensionHeader, version), 4);
    assert_eq!(offset_of!(ExtensionHeader, header_size), 6);
    assert_eq!(offset_of!(ExtensionHeader, total_size), 8);
    assert_eq!(offset_of!(ExtensionHeader, flags), 12);

    assert_eq!(size_of::<PageTableOwnership>(), 64);
    assert_eq!(align_of::<PageTableOwnership>(), 8);
    assert_eq!(offset_of!(PageTableOwnership, hierarchy_version), 0);
    assert_eq!(offset_of!(PageTableOwnership, root_physical_frame), 8);
    assert_eq!(
        offset_of!(PageTableOwnership, owned_frame_list_physical_address),
        16
    );
    assert_eq!(offset_of!(PageTableOwnership, owned_frame_count), 24);
    assert_eq!(offset_of!(PageTableOwnership, physical_address_cap), 32);
    assert_eq!(size_of::<OwnedPageTableFrame>(), 16);
    assert_eq!(align_of::<OwnedPageTableFrame>(), 8);
}

#[test]
fn current_v1_0_remains_extension_free() {
    assert_eq!(
        validate_extension_area(ABI_MAJOR, ABI_MINOR, BOOT_INFO_SIZE, &[])
            .unwrap()
            .extension_count(),
        0
    );
    assert!(validate_extension_area(ABI_MAJOR, ABI_MINOR + 1, BOOT_INFO_SIZE, &[]).is_ok());
    assert_eq!(
        validate_extension_area(ABI_MAJOR, ABI_MINOR, BOOT_INFO_SIZE + 8, &[0; 8]),
        Err(ExtensionError::ExtensionsForbiddenForV1)
    );
}

#[test]
fn known_page_table_extension_parses_and_validates() {
    let record = page_table_record();
    let summary = validate_extension_area(
        BOOT_ENVELOPE_ABI_MAJOR,
        BOOT_ENVELOPE_ABI_MINOR,
        BOOT_INFO_SIZE + PAGE_TABLE_OWNERSHIP_SIZE,
        &record,
    )
    .unwrap();
    assert_eq!(summary.extension_count(), 1);
    let metadata = summary.page_table_ownership().unwrap();
    assert_eq!(metadata.raw(), ownership());
    assert_eq!(metadata.list_byte_length(), 64);
    assert_eq!(
        validate_page_table_frames(metadata, &frames(), &reserved_map()),
        Ok(())
    );
}

#[test]
fn unknown_optional_is_skipped_and_unknown_required_is_rejected() {
    let mut optional = [0_u8; 16];
    put_u32(&mut optional, 0, 99);
    put_u16(&mut optional, 4, 7);
    put_u16(&mut optional, 6, EXTENSION_HEADER_SIZE);
    put_u32(&mut optional, 8, 16);
    let summary = validate_extension_area(
        BOOT_ENVELOPE_ABI_MAJOR,
        BOOT_ENVELOPE_ABI_MINOR,
        BOOT_INFO_SIZE + 16,
        &optional,
    )
    .unwrap();
    assert_eq!(summary.extension_count(), 1);
    assert_eq!(summary.page_table_ownership(), None);

    put_u32(&mut optional, 12, EXTENSION_FLAG_REQUIRED);
    assert_eq!(
        validate_extension_area(
            BOOT_ENVELOPE_ABI_MAJOR,
            BOOT_ENVELOPE_ABI_MINOR,
            BOOT_INFO_SIZE + 16,
            &optional,
        ),
        Err(ExtensionError::UnknownRequiredExtension)
    );
}

#[test]
fn unknown_optional_version_is_skipped_but_required_version_is_rejected() {
    let mut record = page_table_record();
    put_u16(&mut record, 4, EXTENSION_VERSION_1 + 1);
    put_u32(&mut record, 12, 0);
    let summary = validate_extension_area(
        BOOT_ENVELOPE_ABI_MAJOR,
        BOOT_ENVELOPE_ABI_MINOR,
        BOOT_INFO_SIZE + PAGE_TABLE_OWNERSHIP_SIZE,
        &record,
    )
    .unwrap();
    assert_eq!(summary.page_table_ownership(), None);

    put_u32(&mut record, 12, EXTENSION_FLAG_REQUIRED);
    assert_eq!(
        validate_extension_area(
            BOOT_ENVELOPE_ABI_MAJOR,
            BOOT_ENVELOPE_ABI_MINOR,
            BOOT_INFO_SIZE + PAGE_TABLE_OWNERSHIP_SIZE,
            &record,
        ),
        Err(ExtensionError::UnsupportedPageTableExtensionVersion)
    );
}

#[test]
fn linear_records_are_bounded_and_duplicate_ownership_is_rejected() {
    let record = page_table_record();
    let mut duplicate = [0_u8; 160];
    duplicate[..80].copy_from_slice(&record);
    duplicate[80..].copy_from_slice(&record);
    assert_eq!(
        validate_extension_area(
            BOOT_ENVELOPE_ABI_MAJOR,
            BOOT_ENVELOPE_ABI_MINOR,
            BOOT_INFO_SIZE + 160,
            &duplicate,
        ),
        Err(ExtensionError::DuplicatePageTableOwnership)
    );

    let mut zero_size = [0_u8; 16];
    put_u32(&mut zero_size, 0, 99);
    put_u16(&mut zero_size, 6, EXTENSION_HEADER_SIZE);
    assert_eq!(
        validate_extension_area(
            BOOT_ENVELOPE_ABI_MAJOR,
            BOOT_ENVELOPE_ABI_MINOR,
            BOOT_INFO_SIZE + 16,
            &zero_size,
        ),
        Err(ExtensionError::InvalidExtensionSize)
    );
}

#[test]
fn every_truncated_page_table_record_is_rejected_without_panic() {
    let record = page_table_record();
    for length in 1..record.len() {
        let total_size = BOOT_INFO_SIZE + u32::try_from(length).unwrap();
        assert!(
            validate_extension_area(
                BOOT_ENVELOPE_ABI_MAJOR,
                BOOT_ENVELOPE_ABI_MINOR,
                total_size,
                &record[..length],
            )
            .is_err(),
            "accepted truncated record at byte {length}"
        );
    }
}

#[test]
fn metadata_scalar_failures_are_structured() {
    let base = ownership();
    let cases = [
        (
            PageTableOwnership {
                hierarchy_version: 2,
                ..base
            },
            ExtensionError::UnsupportedHierarchyVersion,
        ),
        (
            PageTableOwnership {
                paging_level_count: 5,
                ..base
            },
            ExtensionError::UnsupportedPagingLevelCount,
        ),
        (
            PageTableOwnership {
                page_size: 2,
                ..base
            },
            ExtensionError::UnsupportedPageSize,
        ),
        (
            PageTableOwnership {
                root_physical_frame: 1,
                ..base
            },
            ExtensionError::UnalignedOwnedFrame,
        ),
        (
            PageTableOwnership {
                owned_frame_count: 0,
                ..base
            },
            ExtensionError::EmptyOwnedFrameList,
        ),
        (
            PageTableOwnership {
                descriptor_stride: 15,
                ..base
            },
            ExtensionError::InvalidOwnedFrameStride,
        ),
        (
            PageTableOwnership {
                reserved0: 1,
                ..base
            },
            ExtensionError::ReservedNotZero,
        ),
    ];
    for (metadata, expected) in cases {
        assert_eq!(metadata.validate(), Err(expected));
    }
}

#[test]
fn owned_frames_must_be_unique_aligned_reserved_and_contain_root_once() {
    let metadata = ownership().validate().unwrap();
    let valid_frames = frames();
    let map = reserved_map();

    assert_eq!(
        validate_page_table_frames(metadata, &valid_frames[..3], &map),
        Err(ExtensionError::OwnedFrameCountMismatch)
    );

    let mut duplicate = valid_frames;
    duplicate[3].physical_frame = duplicate[2].physical_frame;
    assert_eq!(
        validate_page_table_frames(metadata, &duplicate, &map),
        Err(ExtensionError::DuplicateOwnedFrame)
    );

    let mut unaligned = valid_frames;
    unaligned[3].physical_frame += 1;
    assert_eq!(
        validate_page_table_frames(metadata, &unaligned, &map),
        Err(ExtensionError::UnalignedOwnedFrame)
    );

    let mut no_root = valid_frames;
    no_root[0].physical_frame = 0x104000;
    let larger_map = [MemoryDescriptor {
        page_count: 5,
        ..map[0]
    }];
    assert_eq!(
        validate_page_table_frames(metadata, &no_root, &larger_map),
        Err(ExtensionError::RootFrameMissingOrDuplicated)
    );

    let usable_map = [MemoryDescriptor {
        kind: MEMORY_KIND_USABLE,
        ..map[0]
    }];
    assert_eq!(
        validate_page_table_frames(metadata, &valid_frames, &usable_map),
        Err(ExtensionError::OwnedFrameNotReserved)
    );
}
