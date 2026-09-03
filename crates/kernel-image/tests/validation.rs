use kernel_image::{
    BOOTSTRAP_LINK_ADDRESS, BOOTSTRAP_PAGE_SIZE, BootstrapValidationError, MAX_PROGRAM_HEADERS,
    ValidatedImage, ValidationError, validate_bootstrap_image,
};

const ELF_HEADER_SIZE: usize = 64;
const PROGRAM_HEADER_SIZE: usize = 56;
const FIRST_SEGMENT_OFFSET: u64 = 0x1000;

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn set_program_u32(bytes: &mut [u8], index: usize, field: usize, value: u32) {
    write_u32(
        bytes,
        ELF_HEADER_SIZE + index * PROGRAM_HEADER_SIZE + field,
        value,
    );
}

fn set_program_u64(bytes: &mut [u8], index: usize, field: usize, value: u64) {
    write_u64(
        bytes,
        ELF_HEADER_SIZE + index * PROGRAM_HEADER_SIZE + field,
        value,
    );
}

fn base_image() -> Vec<u8> {
    let mut bytes = vec![0_u8; 0x1010];
    bytes[..16].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    write_u16(&mut bytes, 16, 2);
    write_u16(&mut bytes, 18, 62);
    write_u32(&mut bytes, 20, 1);
    write_u64(&mut bytes, 24, BOOTSTRAP_LINK_ADDRESS);
    write_u64(&mut bytes, 32, ELF_HEADER_SIZE as u64);
    write_u64(&mut bytes, 40, 0);
    write_u16(&mut bytes, 52, ELF_HEADER_SIZE as u16);
    write_u16(&mut bytes, 54, PROGRAM_HEADER_SIZE as u16);
    write_u16(&mut bytes, 56, 1);
    write_u16(&mut bytes, 58, 0);
    write_u16(&mut bytes, 60, 0);
    write_u16(&mut bytes, 62, 0);

    set_program_u32(&mut bytes, 0, 0, 1);
    set_program_u32(&mut bytes, 0, 4, 5);
    set_program_u64(&mut bytes, 0, 8, FIRST_SEGMENT_OFFSET);
    set_program_u64(&mut bytes, 0, 16, BOOTSTRAP_LINK_ADDRESS);
    set_program_u64(&mut bytes, 0, 24, BOOTSTRAP_LINK_ADDRESS);
    set_program_u64(&mut bytes, 0, 32, 16);
    set_program_u64(&mut bytes, 0, 40, 32);
    set_program_u64(&mut bytes, 0, 48, BOOTSTRAP_PAGE_SIZE);
    bytes[0x1000..0x1010].fill(0x90);
    bytes
}

fn two_segment_image() -> Vec<u8> {
    let mut bytes = base_image();
    bytes.resize(0x2010, 0);
    write_u16(&mut bytes, 56, 2);
    set_program_u32(&mut bytes, 1, 0, 1);
    set_program_u32(&mut bytes, 1, 4, 6);
    set_program_u64(&mut bytes, 1, 8, 0x2000);
    set_program_u64(&mut bytes, 1, 16, 0x202000);
    set_program_u64(&mut bytes, 1, 24, 0x202000);
    set_program_u64(&mut bytes, 1, 32, 16);
    set_program_u64(&mut bytes, 1, 40, 0x1000);
    set_program_u64(&mut bytes, 1, 48, BOOTSTRAP_PAGE_SIZE);
    bytes[0x2000..0x2010].fill(0xa5);
    bytes
}

fn bootstrap_contract_image() -> Vec<u8> {
    let mut bytes = base_image();
    bytes.resize(0x3010, 0);
    write_u16(&mut bytes, 56, 3);

    set_program_u64(&mut bytes, 0, 40, 16);
    for (index, offset, address, flags, memory_size) in [
        (1, 0x2000, 0x201000, 4, 16),
        (2, 0x3000, 0x202000, 6, 0x2000),
    ] {
        set_program_u32(&mut bytes, index, 0, 1);
        set_program_u32(&mut bytes, index, 4, flags);
        set_program_u64(&mut bytes, index, 8, offset);
        set_program_u64(&mut bytes, index, 16, address);
        set_program_u64(&mut bytes, index, 24, address);
        set_program_u64(&mut bytes, index, 32, 16);
        set_program_u64(&mut bytes, index, 40, memory_size);
        set_program_u64(&mut bytes, index, 48, BOOTSTRAP_PAGE_SIZE);
    }
    bytes[0x2000..0x2010].fill(0x52);
    bytes[0x3000..0x3010].fill(0x57);
    bytes
}

fn assert_error(bytes: &[u8], expected: ValidationError) {
    assert_eq!(ValidatedImage::parse(bytes).err(), Some(expected));
}

#[test]
fn accepts_minimal_valid_elf64_image() {
    let bytes = base_image();
    let image = ValidatedImage::parse(&bytes).expect("valid image");
    assert_eq!(image.entry(), BOOTSTRAP_LINK_ADDRESS);
    assert_eq!(image.program_header_count(), 1);
    assert_eq!(image.load_segment_count(), 1);
    let segment = image.load_segments().next().expect("load segment");
    assert_eq!(segment.page_count(4096), Ok(1));
}

#[test]
fn accepts_complete_bootstrap_contract() {
    let bytes = bootstrap_contract_image();
    let image = validate_bootstrap_image(&bytes).expect("bootstrap contract");
    assert_eq!(image.entry(), BOOTSTRAP_LINK_ADDRESS);
    assert_eq!(image.load_segment_count(), 3);
}

#[test]
fn bootstrap_contract_requires_all_segment_kinds_and_bss() {
    assert_eq!(
        validate_bootstrap_image(&base_image()).err(),
        Some(BootstrapValidationError::MissingReadOnlySegment)
    );
    let mut bytes = bootstrap_contract_image();
    set_program_u64(&mut bytes, 2, 40, 16);
    assert_eq!(
        validate_bootstrap_image(&bytes).err(),
        Some(BootstrapValidationError::MissingBss)
    );
}

#[test]
fn rejects_too_small_header_and_bad_magic() {
    assert_error(&[0x7f, b'E', b'L'], ValidationError::HeaderTooSmall);
    let mut bytes = base_image();
    bytes[1] = b'X';
    assert_error(&bytes, ValidationError::BadMagic);
}

#[test]
fn rejects_32_bit_and_big_endian_images() {
    let mut bytes = base_image();
    bytes[4] = 1;
    assert_error(&bytes, ValidationError::UnsupportedClass);
    let mut bytes = base_image();
    bytes[5] = 2;
    assert_error(&bytes, ValidationError::UnsupportedEndianness);
}

#[test]
fn rejects_ident_and_elf_versions() {
    let mut bytes = base_image();
    bytes[6] = 2;
    assert_error(&bytes, ValidationError::UnsupportedIdentVersion);
    let mut bytes = base_image();
    write_u32(&mut bytes, 20, 2);
    assert_error(&bytes, ValidationError::UnsupportedElfVersion);
}

#[test]
fn rejects_non_none_os_abi() {
    let mut bytes = base_image();
    bytes[7] = 3;
    assert_error(&bytes, ValidationError::UnsupportedOsAbi);
}

#[test]
fn rejects_wrong_object_type_and_machine() {
    let mut bytes = base_image();
    write_u16(&mut bytes, 16, 3);
    assert_error(&bytes, ValidationError::UnsupportedObjectType);
    let mut bytes = base_image();
    write_u16(&mut bytes, 18, 3);
    assert_error(&bytes, ValidationError::UnsupportedMachine);
}

#[test]
fn rejects_wrong_elf_and_program_header_sizes() {
    let mut bytes = base_image();
    write_u16(&mut bytes, 52, 63);
    assert_error(&bytes, ValidationError::InvalidHeaderSize);
    let mut bytes = base_image();
    write_u16(&mut bytes, 54, 55);
    assert_error(&bytes, ValidationError::InvalidProgramHeaderSize);
}

#[test]
fn rejects_program_header_table_outside_file_and_overflow() {
    let mut bytes = base_image();
    let near_end = bytes.len() as u64 - 8;
    write_u64(&mut bytes, 32, near_end);
    assert_error(&bytes, ValidationError::ProgramHeaderTableOutsideFile);
    let mut bytes = base_image();
    write_u64(&mut bytes, 32, u64::MAX - 8);
    assert_error(&bytes, ValidationError::ProgramHeaderTableOverflow);
}

#[test]
fn rejects_excessive_program_header_count() {
    let mut bytes = base_image();
    write_u16(&mut bytes, 56, MAX_PROGRAM_HEADERS + 1);
    assert_error(&bytes, ValidationError::TooManyProgramHeaders);
}

#[test]
fn rejects_image_without_load_segment() {
    let mut bytes = base_image();
    set_program_u32(&mut bytes, 0, 0, 4);
    assert_error(&bytes, ValidationError::NoLoadSegments);
}

#[test]
fn rejects_interp_dynamic_and_tls_segments() {
    for (segment_type, error) in [
        (3, ValidationError::InterpreterSegment),
        (2, ValidationError::DynamicSegment),
        (7, ValidationError::TlsSegment),
    ] {
        let mut bytes = base_image();
        set_program_u32(&mut bytes, 0, 0, segment_type);
        assert_error(&bytes, error);
    }
}

#[test]
fn rejects_runtime_relocation_sections() {
    let mut bytes = base_image();
    bytes.resize(0x1100, 0);
    write_u64(&mut bytes, 40, 0x10c0);
    write_u16(&mut bytes, 58, 64);
    write_u16(&mut bytes, 60, 1);
    write_u32(&mut bytes, 0x10c4, 4);
    write_u64(&mut bytes, 0x10e0, 24);
    assert_error(&bytes, ValidationError::RuntimeRelocations);
}

#[test]
fn rejects_invalid_section_table_range_and_entry_size() {
    let mut bytes = base_image();
    write_u64(&mut bytes, 40, 0x1000);
    write_u16(&mut bytes, 58, 63);
    write_u16(&mut bytes, 60, 1);
    assert_error(&bytes, ValidationError::InvalidSectionHeaderSize);
    let mut bytes = base_image();
    write_u64(&mut bytes, 40, u64::MAX - 8);
    write_u16(&mut bytes, 58, 64);
    write_u16(&mut bytes, 60, 1);
    assert_error(&bytes, ValidationError::SectionHeaderTableOverflow);
    let mut bytes = base_image();
    let near_end = bytes.len() as u64 - 8;
    write_u64(&mut bytes, 40, near_end);
    write_u16(&mut bytes, 58, 64);
    write_u16(&mut bytes, 60, 1);
    assert_error(&bytes, ValidationError::SectionHeaderTableOutsideFile);
}

#[test]
fn rejects_file_size_larger_than_memory_size() {
    let mut bytes = base_image();
    set_program_u64(&mut bytes, 0, 32, 33);
    set_program_u64(&mut bytes, 0, 40, 32);
    assert_error(&bytes, ValidationError::FileSizeExceedsMemorySize);
}

#[test]
fn rejects_file_range_overflow_and_segment_past_file() {
    let mut bytes = base_image();
    set_program_u64(&mut bytes, 0, 8, u64::MAX - 4);
    set_program_u64(&mut bytes, 0, 32, 8);
    assert_error(&bytes, ValidationError::SegmentFileRangeOverflow);
    let mut bytes = base_image();
    let near_end = bytes.len() as u64 - 4;
    set_program_u64(&mut bytes, 0, 8, near_end);
    set_program_u64(&mut bytes, 0, 32, 8);
    assert_error(&bytes, ValidationError::SegmentOutsideFile);
}

#[test]
fn rejects_memory_range_overflow() {
    let mut bytes = base_image();
    set_program_u64(&mut bytes, 0, 16, u64::MAX - 4);
    set_program_u64(&mut bytes, 0, 24, u64::MAX - 4);
    set_program_u64(&mut bytes, 0, 32, 0);
    set_program_u64(&mut bytes, 0, 40, 8);
    assert_error(&bytes, ValidationError::SegmentMemoryRangeOverflow);
}

#[test]
fn rejects_bad_alignment_and_congruence() {
    let mut bytes = base_image();
    set_program_u64(&mut bytes, 0, 48, 3);
    assert_error(&bytes, ValidationError::InvalidAlignment);
    let mut bytes = base_image();
    set_program_u64(&mut bytes, 0, 8, FIRST_SEGMENT_OFFSET + 1);
    set_program_u64(&mut bytes, 0, 32, 15);
    assert_error(&bytes, ValidationError::InvalidOffsetAddressCongruence);
}

#[test]
fn accepts_zero_and_one_alignment() {
    for alignment in [0, 1] {
        let mut bytes = base_image();
        set_program_u64(&mut bytes, 0, 48, alignment);
        ValidatedImage::parse(&bytes).expect("zero and one alignment are legal");
    }
}

#[test]
fn rejects_writable_executable_segment() {
    let mut bytes = base_image();
    set_program_u32(&mut bytes, 0, 4, 7);
    assert_error(&bytes, ValidationError::WritableExecutableSegment);
}

#[test]
fn rejects_overlapping_load_address_ranges() {
    let mut bytes = two_segment_image();
    set_program_u64(&mut bytes, 1, 16, BOOTSTRAP_LINK_ADDRESS + 16);
    set_program_u64(&mut bytes, 1, 24, BOOTSTRAP_LINK_ADDRESS + 16);
    set_program_u64(&mut bytes, 1, 48, 0);
    assert_error(&bytes, ValidationError::OverlappingLoadSegments);
}

#[test]
fn rejects_non_canonical_segment_address() {
    let mut bytes = base_image();
    let address = 0x0000_8000_0000_0000;
    set_program_u64(&mut bytes, 0, 16, address);
    set_program_u64(&mut bytes, 0, 24, address);
    set_program_u64(&mut bytes, 0, 48, 0);
    write_u64(&mut bytes, 24, address);
    assert_error(&bytes, ValidationError::NonCanonicalAddress);
}

#[test]
fn rejects_different_physical_and_virtual_addresses() {
    let mut bytes = bootstrap_contract_image();
    set_program_u64(&mut bytes, 1, 24, BOOTSTRAP_LINK_ADDRESS + 0x5000);
    assert_eq!(
        validate_bootstrap_image(&bytes).err(),
        Some(BootstrapValidationError::Elf(
            ValidationError::PhysicalVirtualAddressMismatch
        ))
    );
}

#[test]
fn rejects_entry_outside_load_or_in_non_executable_load() {
    let mut bytes = base_image();
    write_u64(&mut bytes, 24, BOOTSTRAP_LINK_ADDRESS + 0x1000);
    assert_error(&bytes, ValidationError::EntryOutsideExecutableSegment);
    let mut bytes = base_image();
    set_program_u32(&mut bytes, 0, 4, 4);
    assert_error(&bytes, ValidationError::EntryOutsideExecutableSegment);
}

#[test]
fn accepts_bss_and_reports_required_pages() {
    let mut bytes = base_image();
    set_program_u64(&mut bytes, 0, 32, 8);
    set_program_u64(&mut bytes, 0, 40, 0x2001);
    let image = ValidatedImage::parse(&bytes).expect("BSS is valid");
    let segment = image.load_segments().next().expect("segment");
    assert_eq!(segment.file_size(), 8);
    assert_eq!(segment.memory_size(), 0x2001);
    assert_eq!(segment.page_count(4096), Ok(3));
    assert_eq!(segment.page_count(3), Err(ValidationError::InvalidPageSize));
}

#[test]
fn accepts_multiple_non_overlapping_load_segments() {
    let bytes = two_segment_image();
    let image = ValidatedImage::parse(&bytes).expect("two valid loads");
    assert_eq!(image.load_segment_count(), 2);
    assert_eq!(image.load_segments().count(), 2);
    assert_eq!(image.load_address_range(), (0x200000, 0x203000));
}

#[test]
fn rejects_zero_length_load_segment() {
    let mut bytes = base_image();
    set_program_u64(&mut bytes, 0, 32, 0);
    set_program_u64(&mut bytes, 0, 40, 0);
    assert_error(&bytes, ValidationError::ZeroLengthLoadSegment);
}

#[test]
fn every_truncated_prefix_is_handled_without_panic() {
    let bytes = two_segment_image();
    for length in 0..bytes.len() {
        let _ = ValidatedImage::parse(&bytes[..length]);
    }
}
