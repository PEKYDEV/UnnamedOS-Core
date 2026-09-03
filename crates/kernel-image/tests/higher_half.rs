use kernel_image::{
    BOOTSTRAP_LINK_ADDRESS, BOOTSTRAP_PAGE_SIZE, HIGHER_HALF_LINK_ADDRESS,
    HIGHER_HALF_VIRTUAL_OFFSET, HigherHalfValidationError, KERNEL_IMAGE_VIRTUAL_END,
    ValidatedImage, ValidationError, validate_higher_half_image,
};

const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn ph_u32(bytes: &mut [u8], index: usize, field: usize, value: u32) {
    put_u32(
        bytes,
        ELF_HEADER_SIZE + index * PROGRAM_HEADER_SIZE + field,
        value,
    );
}

fn ph_u64(bytes: &mut [u8], index: usize, field: usize, value: u64) {
    put_u64(
        bytes,
        ELF_HEADER_SIZE + index * PROGRAM_HEADER_SIZE + field,
        value,
    );
}

fn higher_image() -> Vec<u8> {
    let mut bytes = vec![0_u8; 0x3010];
    bytes[..16].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    put_u16(&mut bytes, 16, 2);
    put_u16(&mut bytes, 18, 62);
    put_u32(&mut bytes, 20, 1);
    put_u64(&mut bytes, 24, HIGHER_HALF_LINK_ADDRESS);
    put_u64(&mut bytes, 32, ELF_HEADER_SIZE as u64);
    put_u16(&mut bytes, 52, ELF_HEADER_SIZE as u16);
    put_u16(&mut bytes, 54, PROGRAM_HEADER_SIZE as u16);
    put_u16(&mut bytes, 56, 3);
    for (index, file, physical, flags, file_size, memory_size) in [
        (0, 0x1000, 0x200000, 5, 16, 16),
        (1, 0x2000, 0x201000, 4, 16, 16),
        (2, 0x3000, 0x202000, 6, 16, 0x2000),
    ] {
        ph_u32(&mut bytes, index, 0, 1);
        ph_u32(&mut bytes, index, 4, flags);
        ph_u64(&mut bytes, index, 8, file);
        ph_u64(&mut bytes, index, 16, HIGHER_HALF_VIRTUAL_OFFSET + physical);
        ph_u64(&mut bytes, index, 24, physical);
        ph_u64(&mut bytes, index, 32, file_size);
        ph_u64(&mut bytes, index, 40, memory_size);
        ph_u64(&mut bytes, index, 48, BOOTSTRAP_PAGE_SIZE);
    }
    bytes[0x1000..0x1010].fill(0x90);
    bytes[0x2000..0x2010].fill(0x52);
    bytes[0x3000..0x3010].fill(0x57);
    bytes
}

#[test]
fn exact_higher_half_layout_is_accepted() {
    let bytes = higher_image();
    let image = validate_higher_half_image(&bytes).expect("higher-half image");
    assert_eq!(image.virtual_entry(), 0xffff_ffff_8020_0000);
    assert_eq!(image.physical_entry(), 0x0020_0000);
    assert_eq!(image.translation_offset(), 0xffff_ffff_8000_0000);
    assert_eq!(image.physical_load_range(), (0x0020_0000, 0x0020_4000));
    assert_eq!(
        image.virtual_load_range(),
        (0xffff_ffff_8020_0000, 0xffff_ffff_8020_4000)
    );
    assert_eq!(image.total_physical_load_size(), Ok(0x2020));
    let segments: Vec<_> = image.load_segments().collect();
    assert_eq!(
        segments.iter().map(|s| s.flags()).collect::<Vec<_>>(),
        [5, 4, 6]
    );
    assert!(segments[2].memory_size() > segments[2].file_size());
}

#[test]
fn identity_and_higher_half_contracts_do_not_confuse_policy() {
    let higher = higher_image();
    assert!(validate_higher_half_image(&higher).is_ok());
    assert!(kernel_image::validate_bootstrap_image(&higher).is_err());

    let mut identity = higher;
    put_u64(&mut identity, 24, BOOTSTRAP_LINK_ADDRESS);
    for index in 0..3 {
        let physical = 0x200000 + index as u64 * 0x1000;
        ph_u64(&mut identity, index, 16, physical);
    }
    assert!(kernel_image::validate_bootstrap_image(&identity).is_ok());
    assert!(validate_higher_half_image(&identity).is_err());
}

#[test]
fn lower_and_upper_canonical_boundaries_are_explicit() {
    let mut bytes = higher_image();
    put_u64(&mut bytes, 24, 0x0000_7fff_ffff_ffff);
    ph_u64(&mut bytes, 0, 16, 0x0000_7fff_ffff_ffff);
    ph_u64(&mut bytes, 0, 32, 1);
    ph_u64(&mut bytes, 0, 40, 1);
    ph_u64(&mut bytes, 0, 48, 0);
    assert!(ValidatedImage::parse(&bytes).is_ok());

    let mut bytes = higher_image();
    put_u64(&mut bytes, 24, 0xffff_8000_0000_0000);
    ph_u64(&mut bytes, 0, 16, 0xffff_8000_0000_0000);
    assert!(ValidatedImage::parse(&bytes).is_ok());

    let mut bytes = higher_image();
    put_u64(&mut bytes, 24, 0x0000_8000_0000_0000);
    ph_u64(&mut bytes, 0, 16, 0x0000_8000_0000_0000);
    assert_eq!(
        ValidatedImage::parse(&bytes).err(),
        Some(ValidationError::NonCanonicalAddress)
    );
}

#[test]
fn every_load_permission_combination_is_classified() {
    for flags in 0..=7 {
        let mut bytes = higher_image();
        ph_u32(&mut bytes, 0, 4, flags);
        let result = validate_higher_half_image(&bytes);
        match flags {
            5 => assert!(result.is_ok()),
            0 | 2 | 4 | 6 => assert_eq!(
                result.err(),
                Some(HigherHalfValidationError::Elf(
                    ValidationError::EntryOutsideExecutableSegment
                ))
            ),
            3 | 7 => assert_eq!(
                result.err(),
                Some(HigherHalfValidationError::Elf(
                    ValidationError::WritableExecutableSegment
                ))
            ),
            1 => assert_eq!(
                result.err(),
                Some(HigherHalfValidationError::InvalidSegmentPermissions)
            ),
            _ => unreachable!(),
        }
    }
}

#[test]
fn inconsistent_translation_offset_is_rejected() {
    let mut bytes = higher_image();
    ph_u64(&mut bytes, 1, 24, 0x205000);
    assert_eq!(
        validate_higher_half_image(&bytes).err(),
        Some(HigherHalfValidationError::InconsistentTranslationOffset)
    );
}

#[test]
fn physical_and_virtual_overlaps_are_independent() {
    let mut physical = higher_image();
    ph_u64(&mut physical, 1, 24, 0x200000);
    assert_eq!(
        ValidatedImage::parse(&physical).err(),
        Some(ValidationError::OverlappingLoadSegments)
    );

    let mut virtual_image = higher_image();
    ph_u64(&mut virtual_image, 1, 16, HIGHER_HALF_LINK_ADDRESS);
    assert_eq!(
        ValidatedImage::parse(&virtual_image).err(),
        Some(ValidationError::OverlappingVirtualLoadSegments)
    );
}

#[test]
fn entry_must_be_higher_half_rx_and_translate_to_physical_backing() {
    let mut outside = higher_image();
    put_u64(&mut outside, 24, HIGHER_HALF_LINK_ADDRESS + 0x1000);
    assert_eq!(
        ValidatedImage::parse(&outside).err(),
        Some(ValidationError::EntryOutsideExecutableSegment)
    );

    let mut unexpected = higher_image();
    put_u64(&mut unexpected, 24, HIGHER_HALF_LINK_ADDRESS + 1);
    assert_eq!(
        validate_higher_half_image(&unexpected).err(),
        Some(HigherHalfValidationError::UnexpectedEntry)
    );
}

#[test]
fn physical_and_virtual_policy_boundaries_are_enforced() {
    let mut physical_upper = higher_image();
    ph_u64(&mut physical_upper, 2, 24, 0x041f_f000);
    ph_u64(
        &mut physical_upper,
        2,
        16,
        HIGHER_HALF_VIRTUAL_OFFSET + 0x041f_f000,
    );
    ph_u64(&mut physical_upper, 2, 40, 0x1000);
    assert!(validate_higher_half_image(&physical_upper).is_ok());

    let mut physical = higher_image();
    ph_u64(&mut physical, 2, 24, 0x0420_0000);
    ph_u64(
        &mut physical,
        2,
        16,
        HIGHER_HALF_VIRTUAL_OFFSET + 0x0420_0000,
    );
    assert_eq!(
        validate_higher_half_image(&physical).err(),
        Some(HigherHalfValidationError::PhysicalOutsideBootstrapWindow)
    );

    let mut virtual_image = higher_image();
    ph_u64(&mut virtual_image, 2, 16, KERNEL_IMAGE_VIRTUAL_END);
    assert_eq!(
        validate_higher_half_image(&virtual_image).err(),
        Some(HigherHalfValidationError::VirtualOutsideKernelRegion)
    );
}

#[test]
fn alignment_bss_and_arithmetic_fail_closed() {
    let mut alignment = higher_image();
    ph_u64(&mut alignment, 1, 48, 1);
    assert_eq!(
        validate_higher_half_image(&alignment).err(),
        Some(HigherHalfValidationError::MisalignedLoadSegment)
    );

    let mut no_bss = higher_image();
    ph_u64(&mut no_bss, 2, 40, 16);
    assert_eq!(
        validate_higher_half_image(&no_bss).err(),
        Some(HigherHalfValidationError::MissingBss)
    );

    let mut overflow = higher_image();
    ph_u64(&mut overflow, 2, 16, u64::MAX - 0x1000);
    assert!(matches!(
        validate_higher_half_image(&overflow),
        Err(HigherHalfValidationError::Elf(
            ValidationError::SegmentMemoryRangeOverflow | ValidationError::NonCanonicalAddress
        ))
    ));
}

#[test]
fn every_truncated_higher_half_prefix_is_panic_free() {
    let bytes = higher_image();
    for length in 0..bytes.len() {
        let _ = validate_higher_half_image(&bytes[..length]);
    }
}

#[test]
fn undefined_and_malformed_runtime_symbol_tables_are_rejected() {
    let mut undefined = higher_image();
    undefined.resize(0x3230, 0);
    put_u64(&mut undefined, 40, 0x3100);
    put_u16(&mut undefined, 58, 64);
    put_u16(&mut undefined, 60, 2);
    put_u32(&mut undefined, 0x3140 + 4, 2);
    put_u64(&mut undefined, 0x3140 + 24, 0x3200);
    put_u64(&mut undefined, 0x3140 + 32, 48);
    put_u64(&mut undefined, 0x3140 + 56, 24);
    assert_eq!(
        validate_higher_half_image(&undefined).err(),
        Some(HigherHalfValidationError::Elf(
            ValidationError::UndefinedRuntimeSymbol
        ))
    );

    put_u16(&mut undefined, 0x3200 + 24 + 6, 1);
    put_u64(&mut undefined, 0x3140 + 56, 16);
    assert_eq!(
        validate_higher_half_image(&undefined).err(),
        Some(HigherHalfValidationError::Elf(
            ValidationError::InvalidSymbolTable
        ))
    );
}
