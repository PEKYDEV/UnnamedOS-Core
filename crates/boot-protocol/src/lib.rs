#![no_std]
#![deny(unsafe_code)]
#![doc = "UnnamedOS loader-to-kernel boot wire protocol."]
#![doc = ""]
#![doc = "The wire format is little-endian and is currently defined only for"]
#![doc = "x86-64. Raw `repr(C)` structures contain only fixed-width integers;"]
#![doc = "physical addresses are opaque `u64` values and validation never"]
#![doc = "dereferences them. Consumers must validate `BootInfo` before use."]

/// ASCII `UNOSBOOT` interpreted as a little-endian `u64`.
pub const ABI_MAGIC: u64 = u64::from_le_bytes(*b"UNOSBOOT");
pub const ABI_MAJOR: u16 = 1;
pub const ABI_MINOR: u16 = 0;

pub const BOOT_INFO_HEADER_SIZE: u16 = 32;
pub const BOOT_INFO_SIZE: u32 = 128;
pub const MEMORY_DESCRIPTOR_SIZE: u32 = 32;
pub const MEMORY_DESCRIPTOR_ALIGNMENT: u32 = 8;
pub const MEMORY_PAGE_SIZE: u64 = 4096;

pub const PIXEL_FORMAT_RGBX8888: u32 = 1;
pub const PIXEL_FORMAT_BGRX8888: u32 = 2;
pub const BYTES_PER_PIXEL: u64 = 4;

/// Normalized memory kinds used by `MemoryDescriptor::kind`.
pub const MEMORY_KIND_USABLE: u32 = 1;
pub const MEMORY_KIND_RESERVED: u32 = 2;
pub const MEMORY_KIND_ACPI_RECLAIM: u32 = 3;
pub const MEMORY_KIND_RUNTIME: u32 = 4;
pub const MEMORY_KIND_KERNEL_IMAGE: u32 = 5;
pub const MEMORY_KIND_BOOT_INFO: u32 = 6;
pub const MEMORY_KIND_BOOT_MEMORY_MAP: u32 = 7;
pub const MEMORY_KIND_LOADER: u32 = 8;
pub const MEMORY_KIND_FRAMEBUFFER: u32 = 9;
pub const MEMORY_KIND_UNUSABLE: u32 = 10;
pub const MEMORY_KIND_PERSISTENT: u32 = 11;
pub const MEMORY_KIND_BOOTSTRAP_STACK: u32 = 12;

/// Raw wire header. Reserved fields and all currently undefined flags are zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct BootInfoHeader {
    pub magic: u64,
    pub abi_major: u16,
    pub abi_minor: u16,
    pub header_size: u16,
    pub reserved0: u16,
    pub total_size: u32,
    pub reserved1: u32,
    pub flags: u64,
}

/// Raw descriptor for the normalized memory-map array.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct MemoryMapInfo {
    pub physical_address: u64,
    pub descriptor_count: u64,
    pub descriptor_stride: u32,
    pub descriptor_version: u16,
    pub reserved0: u16,
    pub byte_length: u64,
    pub reserved1: u64,
}

/// Minimum descriptor layout. Later compatible minors may append fields by
/// increasing `MemoryMapInfo::descriptor_stride`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct MemoryDescriptor {
    pub kind: u32,
    pub reserved0: u32,
    pub physical_start: u64,
    pub page_count: u64,
    pub attributes: u64,
}

impl MemoryDescriptor {
    /// Validates one already-accessible descriptor without following any
    /// address stored in it.
    pub fn validate(&self) -> Result<ValidatedMemoryDescriptor<'_>, ValidationError> {
        if self.reserved0 != 0 {
            return Err(ValidationError::ReservedNotZero(
                ReservedField::MemoryDescriptor0,
            ));
        }
        if self.page_count == 0 || !self.physical_start.is_multiple_of(MEMORY_PAGE_SIZE) {
            return Err(ValidationError::InvalidMemoryDescriptorRange);
        }

        let byte_length = self
            .page_count
            .checked_mul(MEMORY_PAGE_SIZE)
            .ok_or(ValidationError::MemoryDescriptorSizeOverflow)?;
        self.physical_start
            .checked_add(byte_length)
            .ok_or(ValidationError::MemoryDescriptorRangeOverflow)?;

        Ok(ValidatedMemoryDescriptor {
            raw: self,
            byte_length,
        })
    }
}

/// Raw linear framebuffer description. Pixel-format values are the
/// `PIXEL_FORMAT_*` constants; zero and unknown values are invalid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct FramebufferInfo {
    pub physical_address: u64,
    pub byte_length: u64,
    pub width: u32,
    pub height: u32,
    pub pixels_per_scanline: u32,
    pub pixel_format: u32,
    pub reserved0: u64,
}

/// Complete raw loader-to-kernel wire block for ABI major version 1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct BootInfo {
    pub header: BootInfoHeader,
    pub memory_map: MemoryMapInfo,
    pub framebuffer: FramebufferInfo,
    pub reserved0: u64,
    pub reserved1: u64,
}

impl BootInfo {
    /// Constructs a current-version raw value. The supplied child structures
    /// still require validation.
    pub const fn new(memory_map: MemoryMapInfo, framebuffer: FramebufferInfo) -> Self {
        Self {
            header: BootInfoHeader {
                magic: ABI_MAGIC,
                abi_major: ABI_MAJOR,
                abi_minor: ABI_MINOR,
                header_size: BOOT_INFO_HEADER_SIZE,
                reserved0: 0,
                total_size: BOOT_INFO_SIZE,
                reserved1: 0,
                flags: 0,
            },
            memory_map,
            framebuffer,
            reserved0: 0,
            reserved1: 0,
        }
    }

    /// Validates scalar metadata and address ranges without dereferencing any
    /// physical address.
    pub fn validate(&self) -> Result<ValidatedBootInfo<'_>, ValidationError> {
        validate_header(&self.header)?;
        let memory_map_byte_length = validate_memory_map(&self.memory_map)?;
        let framebuffer_required_bytes = validate_framebuffer(&self.framebuffer)?;

        if self.reserved0 != 0 {
            return Err(ValidationError::ReservedNotZero(ReservedField::BootInfo0));
        }
        if self.reserved1 != 0 {
            return Err(ValidationError::ReservedNotZero(ReservedField::BootInfo1));
        }

        Ok(ValidatedBootInfo {
            raw: self,
            memory_map_byte_length,
            framebuffer_required_bytes,
        })
    }
}

/// Safe interpretation produced only after all scalar invariants are checked.
/// This is an in-process Rust API, not part of the wire ABI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidatedBootInfo<'a> {
    raw: &'a BootInfo,
    memory_map_byte_length: u64,
    framebuffer_required_bytes: u64,
}

impl<'a> ValidatedBootInfo<'a> {
    pub const fn raw(self) -> &'a BootInfo {
        self.raw
    }

    pub const fn memory_map_byte_length(self) -> u64 {
        self.memory_map_byte_length
    }

    pub const fn framebuffer_required_bytes(self) -> u64 {
        self.framebuffer_required_bytes
    }
}

/// Safe scalar interpretation of one memory descriptor. This type is not part
/// of the wire ABI and does not dereference `physical_start`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidatedMemoryDescriptor<'a> {
    raw: &'a MemoryDescriptor,
    byte_length: u64,
}

impl<'a> ValidatedMemoryDescriptor<'a> {
    pub const fn raw(self) -> &'a MemoryDescriptor {
        self.raw
    }

    pub const fn byte_length(self) -> u64 {
        self.byte_length
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReservedField {
    Header0,
    Header1,
    MemoryMap0,
    MemoryMap1,
    MemoryDescriptor0,
    Framebuffer0,
    BootInfo0,
    BootInfo1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationError {
    BadMagic,
    UnsupportedMajorVersion,
    InvalidHeaderSize,
    InvalidTotalSize,
    UnsupportedFlags,
    ReservedNotZero(ReservedField),
    InvalidMemoryMapAddress,
    InvalidDescriptorCount,
    InvalidDescriptorStride,
    MemoryMapSizeOverflow,
    MemoryMapLengthMismatch,
    MemoryMapRangeOverflow,
    InvalidMemoryDescriptorRange,
    MemoryDescriptorSizeOverflow,
    MemoryDescriptorRangeOverflow,
    InvalidFramebufferAddress,
    InvalidFramebufferLength,
    InvalidFramebufferDimensions,
    InvalidFramebufferStride,
    UnknownPixelFormat,
    FramebufferSizeOverflow,
    FramebufferTooSmall,
    FramebufferRangeOverflow,
}

fn validate_header(header: &BootInfoHeader) -> Result<(), ValidationError> {
    if header.magic != ABI_MAGIC {
        return Err(ValidationError::BadMagic);
    }
    if header.abi_major != ABI_MAJOR {
        return Err(ValidationError::UnsupportedMajorVersion);
    }
    if header.header_size != BOOT_INFO_HEADER_SIZE {
        return Err(ValidationError::InvalidHeaderSize);
    }

    let valid_total_size = if header.abi_minor == ABI_MINOR {
        header.total_size == BOOT_INFO_SIZE
    } else {
        header.total_size >= BOOT_INFO_SIZE
    };
    if !valid_total_size {
        return Err(ValidationError::InvalidTotalSize);
    }
    if header.reserved0 != 0 {
        return Err(ValidationError::ReservedNotZero(ReservedField::Header0));
    }
    if header.reserved1 != 0 {
        return Err(ValidationError::ReservedNotZero(ReservedField::Header1));
    }
    if header.flags != 0 {
        return Err(ValidationError::UnsupportedFlags);
    }

    Ok(())
}

fn validate_memory_map(memory_map: &MemoryMapInfo) -> Result<u64, ValidationError> {
    if memory_map.physical_address == 0 {
        return Err(ValidationError::InvalidMemoryMapAddress);
    }
    if memory_map.descriptor_count == 0 {
        return Err(ValidationError::InvalidDescriptorCount);
    }
    if memory_map.descriptor_stride < MEMORY_DESCRIPTOR_SIZE
        || !memory_map
            .descriptor_stride
            .is_multiple_of(MEMORY_DESCRIPTOR_ALIGNMENT)
    {
        return Err(ValidationError::InvalidDescriptorStride);
    }
    if memory_map.reserved0 != 0 {
        return Err(ValidationError::ReservedNotZero(ReservedField::MemoryMap0));
    }
    if memory_map.reserved1 != 0 {
        return Err(ValidationError::ReservedNotZero(ReservedField::MemoryMap1));
    }

    let byte_length = memory_map
        .descriptor_count
        .checked_mul(u64::from(memory_map.descriptor_stride))
        .ok_or(ValidationError::MemoryMapSizeOverflow)?;
    if byte_length != memory_map.byte_length {
        return Err(ValidationError::MemoryMapLengthMismatch);
    }
    memory_map
        .physical_address
        .checked_add(byte_length)
        .ok_or(ValidationError::MemoryMapRangeOverflow)?;

    Ok(byte_length)
}

fn validate_framebuffer(framebuffer: &FramebufferInfo) -> Result<u64, ValidationError> {
    if framebuffer.physical_address == 0 {
        return Err(ValidationError::InvalidFramebufferAddress);
    }
    if framebuffer.byte_length == 0 {
        return Err(ValidationError::InvalidFramebufferLength);
    }
    if framebuffer.width == 0 || framebuffer.height == 0 {
        return Err(ValidationError::InvalidFramebufferDimensions);
    }
    if framebuffer.pixels_per_scanline < framebuffer.width {
        return Err(ValidationError::InvalidFramebufferStride);
    }
    if !matches!(
        framebuffer.pixel_format,
        PIXEL_FORMAT_RGBX8888 | PIXEL_FORMAT_BGRX8888
    ) {
        return Err(ValidationError::UnknownPixelFormat);
    }
    if framebuffer.reserved0 != 0 {
        return Err(ValidationError::ReservedNotZero(
            ReservedField::Framebuffer0,
        ));
    }

    let required_bytes = u64::from(framebuffer.pixels_per_scanline)
        .checked_mul(u64::from(framebuffer.height))
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
        .ok_or(ValidationError::FramebufferSizeOverflow)?;
    if framebuffer.byte_length < required_bytes {
        return Err(ValidationError::FramebufferTooSmall);
    }
    framebuffer
        .physical_address
        .checked_add(framebuffer.byte_length)
        .ok_or(ValidationError::FramebufferRangeOverflow)?;

    Ok(required_bytes)
}
