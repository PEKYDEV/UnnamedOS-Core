use std::mem::{align_of, offset_of, size_of};

use boot_protocol::{
    ABI_MAGIC, ABI_MAJOR, ABI_MINOR, BOOT_INFO_HEADER_SIZE, BOOT_INFO_SIZE, BootInfo,
    BootInfoHeader, FramebufferInfo, MEMORY_DESCRIPTOR_SIZE, MEMORY_PAGE_SIZE, MemoryDescriptor,
    MemoryMapInfo, PIXEL_FORMAT_RGBX8888, ReservedField, ValidationError,
};

fn valid_boot_info() -> BootInfo {
    BootInfo::new(
        MemoryMapInfo {
            physical_address: 0x10_0000,
            descriptor_count: 2,
            descriptor_stride: MEMORY_DESCRIPTOR_SIZE,
            descriptor_version: 1,
            reserved0: 0,
            byte_length: 64,
            reserved1: 0,
        },
        FramebufferInfo {
            physical_address: 0x20_0000,
            byte_length: 800 * 600 * 4,
            width: 800,
            height: 600,
            pixels_per_scanline: 800,
            pixel_format: PIXEL_FORMAT_RGBX8888,
            reserved0: 0,
        },
    )
}

#[test]
fn abi_type_sizes_and_alignments_are_fixed() {
    assert_eq!(size_of::<BootInfoHeader>(), 32);
    assert_eq!(align_of::<BootInfoHeader>(), 8);
    assert_eq!(size_of::<MemoryMapInfo>(), 40);
    assert_eq!(align_of::<MemoryMapInfo>(), 8);
    assert_eq!(size_of::<MemoryDescriptor>(), 32);
    assert_eq!(align_of::<MemoryDescriptor>(), 8);
    assert_eq!(size_of::<FramebufferInfo>(), 40);
    assert_eq!(align_of::<FramebufferInfo>(), 8);
    assert_eq!(size_of::<BootInfo>(), 128);
    assert_eq!(align_of::<BootInfo>(), 8);
}

#[test]
fn abi_field_offsets_are_fixed() {
    assert_eq!(offset_of!(BootInfoHeader, magic), 0);
    assert_eq!(offset_of!(BootInfoHeader, abi_major), 8);
    assert_eq!(offset_of!(BootInfoHeader, abi_minor), 10);
    assert_eq!(offset_of!(BootInfoHeader, header_size), 12);
    assert_eq!(offset_of!(BootInfoHeader, reserved0), 14);
    assert_eq!(offset_of!(BootInfoHeader, total_size), 16);
    assert_eq!(offset_of!(BootInfoHeader, reserved1), 20);
    assert_eq!(offset_of!(BootInfoHeader, flags), 24);

    assert_eq!(offset_of!(MemoryMapInfo, physical_address), 0);
    assert_eq!(offset_of!(MemoryMapInfo, descriptor_count), 8);
    assert_eq!(offset_of!(MemoryMapInfo, descriptor_stride), 16);
    assert_eq!(offset_of!(MemoryMapInfo, descriptor_version), 20);
    assert_eq!(offset_of!(MemoryMapInfo, reserved0), 22);
    assert_eq!(offset_of!(MemoryMapInfo, byte_length), 24);
    assert_eq!(offset_of!(MemoryMapInfo, reserved1), 32);

    assert_eq!(offset_of!(MemoryDescriptor, kind), 0);
    assert_eq!(offset_of!(MemoryDescriptor, reserved0), 4);
    assert_eq!(offset_of!(MemoryDescriptor, physical_start), 8);
    assert_eq!(offset_of!(MemoryDescriptor, page_count), 16);
    assert_eq!(offset_of!(MemoryDescriptor, attributes), 24);

    assert_eq!(offset_of!(FramebufferInfo, physical_address), 0);
    assert_eq!(offset_of!(FramebufferInfo, byte_length), 8);
    assert_eq!(offset_of!(FramebufferInfo, width), 16);
    assert_eq!(offset_of!(FramebufferInfo, height), 20);
    assert_eq!(offset_of!(FramebufferInfo, pixels_per_scanline), 24);
    assert_eq!(offset_of!(FramebufferInfo, pixel_format), 28);
    assert_eq!(offset_of!(FramebufferInfo, reserved0), 32);

    assert_eq!(offset_of!(BootInfo, header), 0);
    assert_eq!(offset_of!(BootInfo, memory_map), 32);
    assert_eq!(offset_of!(BootInfo, framebuffer), 72);
    assert_eq!(offset_of!(BootInfo, reserved0), 112);
    assert_eq!(offset_of!(BootInfo, reserved1), 120);
}

#[test]
fn current_magic_and_version_are_valid() {
    let boot_info = valid_boot_info();

    assert_eq!(boot_info.header.magic, ABI_MAGIC);
    assert_eq!(boot_info.header.abi_major, ABI_MAJOR);
    assert_eq!(boot_info.header.abi_minor, ABI_MINOR);
    assert_eq!(boot_info.header.header_size, BOOT_INFO_HEADER_SIZE);
    assert_eq!(boot_info.header.total_size, BOOT_INFO_SIZE);
    assert!(boot_info.validate().is_ok());
}

#[test]
fn bad_magic_is_rejected() {
    let mut boot_info = valid_boot_info();
    boot_info.header.magic ^= 1;

    assert_eq!(boot_info.validate(), Err(ValidationError::BadMagic));
}

#[test]
fn unsupported_major_version_is_rejected() {
    let mut boot_info = valid_boot_info();
    boot_info.header.abi_major = ABI_MAJOR + 1;

    assert_eq!(
        boot_info.validate(),
        Err(ValidationError::UnsupportedMajorVersion)
    );
}

#[test]
fn every_boot_info_reserved_field_must_be_zero() {
    let cases = [
        (ReservedField::Header0, 0),
        (ReservedField::Header1, 1),
        (ReservedField::MemoryMap0, 2),
        (ReservedField::MemoryMap1, 3),
        (ReservedField::Framebuffer0, 4),
        (ReservedField::BootInfo0, 5),
        (ReservedField::BootInfo1, 6),
    ];

    for (expected, field) in cases {
        let mut boot_info = valid_boot_info();
        match field {
            0 => boot_info.header.reserved0 = 1,
            1 => boot_info.header.reserved1 = 1,
            2 => boot_info.memory_map.reserved0 = 1,
            3 => boot_info.memory_map.reserved1 = 1,
            4 => boot_info.framebuffer.reserved0 = 1,
            5 => boot_info.reserved0 = 1,
            6 => boot_info.reserved1 = 1,
            _ => unreachable!(),
        }
        assert_eq!(
            boot_info.validate(),
            Err(ValidationError::ReservedNotZero(expected))
        );
    }
}

#[test]
fn zero_descriptor_stride_is_rejected() {
    let mut boot_info = valid_boot_info();
    boot_info.memory_map.descriptor_stride = 0;

    assert_eq!(
        boot_info.validate(),
        Err(ValidationError::InvalidDescriptorStride)
    );
}

#[test]
fn undersized_or_misaligned_descriptor_stride_is_rejected() {
    for stride in [MEMORY_DESCRIPTOR_SIZE - 8, MEMORY_DESCRIPTOR_SIZE + 4] {
        let mut boot_info = valid_boot_info();
        boot_info.memory_map.descriptor_stride = stride;
        assert_eq!(
            boot_info.validate(),
            Err(ValidationError::InvalidDescriptorStride)
        );
    }
}

#[test]
fn memory_map_size_overflow_is_rejected() {
    let mut boot_info = valid_boot_info();
    boot_info.memory_map.descriptor_count = u64::MAX;

    assert_eq!(
        boot_info.validate(),
        Err(ValidationError::MemoryMapSizeOverflow)
    );
}

#[test]
fn memory_map_address_range_overflow_is_rejected() {
    let mut boot_info = valid_boot_info();
    boot_info.memory_map.physical_address = u64::MAX - 31;

    assert_eq!(
        boot_info.validate(),
        Err(ValidationError::MemoryMapRangeOverflow)
    );
}

#[test]
fn memory_map_length_must_match_count_times_stride() {
    let mut boot_info = valid_boot_info();
    boot_info.memory_map.byte_length -= 1;

    assert_eq!(
        boot_info.validate(),
        Err(ValidationError::MemoryMapLengthMismatch)
    );
}

#[test]
fn framebuffer_address_range_overflow_is_rejected() {
    let mut boot_info = valid_boot_info();
    boot_info.framebuffer.physical_address = u64::MAX - 15;

    assert_eq!(
        boot_info.validate(),
        Err(ValidationError::FramebufferRangeOverflow)
    );
}

#[test]
fn framebuffer_size_calculation_overflow_is_rejected() {
    let mut boot_info = valid_boot_info();
    boot_info.framebuffer.width = u32::MAX;
    boot_info.framebuffer.height = u32::MAX;
    boot_info.framebuffer.pixels_per_scanline = u32::MAX;
    boot_info.framebuffer.byte_length = u64::MAX;

    assert_eq!(
        boot_info.validate(),
        Err(ValidationError::FramebufferSizeOverflow)
    );
}

#[test]
fn zero_or_undersized_framebuffer_length_is_rejected() {
    let mut zero = valid_boot_info();
    zero.framebuffer.byte_length = 0;
    assert_eq!(
        zero.validate(),
        Err(ValidationError::InvalidFramebufferLength)
    );

    let mut undersized = valid_boot_info();
    undersized.framebuffer.byte_length -= 1;
    assert_eq!(
        undersized.validate(),
        Err(ValidationError::FramebufferTooSmall)
    );
}

#[test]
fn zero_or_invalid_resolution_is_rejected() {
    for (width, height) in [(0, 600), (800, 0)] {
        let mut boot_info = valid_boot_info();
        boot_info.framebuffer.width = width;
        boot_info.framebuffer.height = height;
        assert_eq!(
            boot_info.validate(),
            Err(ValidationError::InvalidFramebufferDimensions)
        );
    }
}

#[test]
fn framebuffer_stride_smaller_than_width_is_rejected() {
    let mut boot_info = valid_boot_info();
    boot_info.framebuffer.pixels_per_scanline = 799;

    assert_eq!(
        boot_info.validate(),
        Err(ValidationError::InvalidFramebufferStride)
    );
}

#[test]
fn unknown_pixel_format_is_rejected() {
    let mut boot_info = valid_boot_info();
    boot_info.framebuffer.pixel_format = 99;

    assert_eq!(
        boot_info.validate(),
        Err(ValidationError::UnknownPixelFormat)
    );
}

#[test]
fn minimal_valid_boot_info_produces_safe_interpretation() {
    let boot_info = valid_boot_info();
    let validated = boot_info
        .validate()
        .expect("minimal boot info must validate");

    assert_eq!(validated.raw(), &boot_info);
    assert_eq!(validated.memory_map_byte_length(), 64);
    assert_eq!(validated.framebuffer_required_bytes(), 1_920_000);
}

#[test]
fn newer_compatible_minor_version_is_accepted() {
    let mut boot_info = valid_boot_info();
    boot_info.header.abi_minor = ABI_MINOR + 1;

    assert!(boot_info.validate().is_ok());
}

#[test]
fn minor_version_size_rules_are_enforced() {
    let mut current_with_extra_bytes = valid_boot_info();
    current_with_extra_bytes.header.total_size = BOOT_INFO_SIZE + 8;
    assert_eq!(
        current_with_extra_bytes.validate(),
        Err(ValidationError::InvalidTotalSize)
    );

    let mut newer_but_too_small = valid_boot_info();
    newer_but_too_small.header.abi_minor = ABI_MINOR + 1;
    newer_but_too_small.header.total_size = BOOT_INFO_SIZE - 1;
    assert_eq!(
        newer_but_too_small.validate(),
        Err(ValidationError::InvalidTotalSize)
    );
}

#[test]
fn memory_descriptor_reserved_and_ranges_are_validated_without_dereference() {
    let descriptor = MemoryDescriptor {
        kind: 1,
        reserved0: 0,
        physical_start: 0x40_0000,
        page_count: 2,
        attributes: 0,
    };
    let validated = descriptor.validate().expect("descriptor must validate");
    assert_eq!(validated.byte_length(), 2 * MEMORY_PAGE_SIZE);
    assert_eq!(validated.raw(), &descriptor);

    let mut reserved = descriptor;
    reserved.reserved0 = 1;
    assert_eq!(
        reserved.validate(),
        Err(ValidationError::ReservedNotZero(
            ReservedField::MemoryDescriptor0
        ))
    );

    let last_aligned_page = u64::MAX - (MEMORY_PAGE_SIZE - 1);
    let mut overflowing = descriptor;
    overflowing.physical_start = last_aligned_page + 1;
    assert_eq!(
        overflowing.validate(),
        Err(ValidationError::InvalidMemoryDescriptorRange)
    );

    overflowing.physical_start = last_aligned_page;
    assert_eq!(
        overflowing.validate(),
        Err(ValidationError::MemoryDescriptorRangeOverflow)
    );

    overflowing.physical_start = 0;
    overflowing.page_count = u64::MAX;
    assert_eq!(
        overflowing.validate(),
        Err(ValidationError::MemoryDescriptorSizeOverflow)
    );
}
