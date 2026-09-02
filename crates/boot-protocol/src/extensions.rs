//! Future major-2 linear extension envelope; the v1.0 emitter does not use it.

use core::mem::{align_of, size_of};

use crate::{
    ABI_MAJOR, BOOT_INFO_SIZE, MEMORY_KIND_PAGE_TABLE, MEMORY_PAGE_SIZE, MemoryDescriptor,
};

pub const BOOT_ENVELOPE_ABI_MAJOR: u16 = 2;
pub const BOOT_ENVELOPE_ABI_MINOR: u16 = 0;
pub const EXTENSION_HEADER_SIZE: u16 = 16;
pub const EXTENSION_VERSION_1: u16 = 1;
pub const EXTENSION_FLAG_REQUIRED: u32 = 1;
pub const EXTENSION_KIND_PAGE_TABLE_OWNERSHIP: u32 = 1;
pub const PAGE_TABLE_OWNERSHIP_SIZE: u32 = 80;
pub const OWNED_PAGE_TABLE_FRAME_SIZE: u32 = 16;
pub const PAGE_TABLE_HIERARCHY_VERSION: u16 = 1;
pub const PAGE_TABLE_STATE_TRANSITIONAL: u8 = 1;
pub const PAGE_TABLE_STATE_FINAL: u8 = 2;
pub const PAGE_TABLE_PHYSICAL_CAP: u64 = 0x0000_4000_0000_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct ExtensionHeader {
    pub kind: u32,
    pub version: u16,
    pub header_size: u16,
    pub total_size: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct PageTableOwnership {
    pub hierarchy_version: u16,
    pub paging_level_count: u8,
    pub state: u8,
    pub page_size: u32,
    pub root_physical_frame: u64,
    pub owned_frame_list_physical_address: u64,
    pub owned_frame_count: u32,
    pub descriptor_stride: u32,
    pub physical_address_cap: u64,
    pub reserved0: u64,
    pub reserved1: u64,
    pub reserved2: u64,
}

impl PageTableOwnership {
    pub fn validate(self) -> Result<ValidatedPageTableOwnership, ExtensionError> {
        if self.hierarchy_version != PAGE_TABLE_HIERARCHY_VERSION {
            return Err(ExtensionError::UnsupportedHierarchyVersion);
        }
        if self.paging_level_count != 4 {
            return Err(ExtensionError::UnsupportedPagingLevelCount);
        }
        if self.state != PAGE_TABLE_STATE_TRANSITIONAL && self.state != PAGE_TABLE_STATE_FINAL {
            return Err(ExtensionError::InvalidPageTableState);
        }
        if self.page_size != MEMORY_PAGE_SIZE as u32 {
            return Err(ExtensionError::UnsupportedPageSize);
        }
        if self.physical_address_cap != PAGE_TABLE_PHYSICAL_CAP {
            return Err(ExtensionError::InvalidPhysicalAddressCap);
        }
        validate_frame_address(self.root_physical_frame, self.physical_address_cap)?;
        if self.owned_frame_list_physical_address == 0
            || !self
                .owned_frame_list_physical_address
                .is_multiple_of(align_of::<OwnedPageTableFrame>() as u64)
        {
            return Err(ExtensionError::InvalidOwnedFrameListAddress);
        }
        if self.owned_frame_count == 0 {
            return Err(ExtensionError::EmptyOwnedFrameList);
        }
        if self.descriptor_stride < OWNED_PAGE_TABLE_FRAME_SIZE
            || !self.descriptor_stride.is_multiple_of(8)
        {
            return Err(ExtensionError::InvalidOwnedFrameStride);
        }
        let list_byte_length = u64::from(self.owned_frame_count)
            .checked_mul(u64::from(self.descriptor_stride))
            .ok_or(ExtensionError::OwnedFrameListSizeOverflow)?;
        let list_end = self
            .owned_frame_list_physical_address
            .checked_add(list_byte_length)
            .ok_or(ExtensionError::OwnedFrameListRangeOverflow)?;
        if list_end > self.physical_address_cap {
            return Err(ExtensionError::InvalidOwnedFrameListAddress);
        }
        if self.reserved0 != 0 || self.reserved1 != 0 || self.reserved2 != 0 {
            return Err(ExtensionError::ReservedNotZero);
        }
        Ok(ValidatedPageTableOwnership {
            raw: self,
            list_byte_length,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidatedPageTableOwnership {
    raw: PageTableOwnership,
    list_byte_length: u64,
}

impl ValidatedPageTableOwnership {
    pub const fn raw(self) -> PageTableOwnership {
        self.raw
    }

    pub const fn list_byte_length(self) -> u64 {
        self.list_byte_length
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct OwnedPageTableFrame {
    pub physical_frame: u64,
    pub reserved0: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExtensionSummary {
    page_table_ownership: Option<ValidatedPageTableOwnership>,
    extension_count: u32,
}

impl ExtensionSummary {
    pub const fn page_table_ownership(self) -> Option<ValidatedPageTableOwnership> {
        self.page_table_ownership
    }

    pub const fn extension_count(self) -> u32 {
        self.extension_count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionError {
    UnsupportedEnvelopeVersion,
    ExtensionsForbiddenForV1,
    TotalSizeMismatch,
    ExtensionAreaMisaligned,
    TruncatedExtensionHeader,
    InvalidExtensionHeaderSize,
    InvalidExtensionSize,
    UnsupportedExtensionFlags,
    UnknownRequiredExtension,
    UnsupportedPageTableExtensionVersion,
    DuplicatePageTableOwnership,
    UnsupportedHierarchyVersion,
    UnsupportedPagingLevelCount,
    InvalidPageTableState,
    UnsupportedPageSize,
    InvalidPhysicalAddressCap,
    InvalidRootFrame,
    InvalidOwnedFrameListAddress,
    EmptyOwnedFrameList,
    InvalidOwnedFrameStride,
    OwnedFrameListSizeOverflow,
    OwnedFrameListRangeOverflow,
    ExtensionCountOverflow,
    TruncatedPageTableOwnership,
    ReservedNotZero,
    OwnedFrameCountMismatch,
    OwnedFrameReservedNotZero,
    UnalignedOwnedFrame,
    OwnedFrameOutsidePhysicalCap,
    DuplicateOwnedFrame,
    RootFrameMissingOrDuplicated,
    OwnedFrameNotReserved,
}

pub fn validate_extension_area(
    abi_major: u16,
    _abi_minor: u16,
    declared_total_size: u32,
    bytes: &[u8],
) -> Result<ExtensionSummary, ExtensionError> {
    let expected_total = BOOT_INFO_SIZE
        .checked_add(u32::try_from(bytes.len()).map_err(|_| ExtensionError::TotalSizeMismatch)?)
        .ok_or(ExtensionError::TotalSizeMismatch)?;
    if declared_total_size != expected_total {
        return Err(ExtensionError::TotalSizeMismatch);
    }
    if abi_major == ABI_MAJOR {
        if declared_total_size != BOOT_INFO_SIZE || !bytes.is_empty() {
            return Err(ExtensionError::ExtensionsForbiddenForV1);
        }
        return Ok(ExtensionSummary {
            page_table_ownership: None,
            extension_count: 0,
        });
    }
    if abi_major != BOOT_ENVELOPE_ABI_MAJOR {
        return Err(ExtensionError::UnsupportedEnvelopeVersion);
    }
    if !bytes.len().is_multiple_of(8) {
        return Err(ExtensionError::ExtensionAreaMisaligned);
    }

    let mut offset = 0_usize;
    let mut extension_count = 0_u32;
    let mut page_table_ownership = None;
    while offset < bytes.len() {
        let header_end = offset
            .checked_add(EXTENSION_HEADER_SIZE as usize)
            .ok_or(ExtensionError::InvalidExtensionSize)?;
        if header_end > bytes.len() {
            return Err(ExtensionError::TruncatedExtensionHeader);
        }
        let header = parse_extension_header(&bytes[offset..header_end]);
        if header.header_size != EXTENSION_HEADER_SIZE {
            return Err(ExtensionError::InvalidExtensionHeaderSize);
        }
        if header.total_size < u32::from(header.header_size) || !header.total_size.is_multiple_of(8)
        {
            return Err(ExtensionError::InvalidExtensionSize);
        }
        if header.flags & !EXTENSION_FLAG_REQUIRED != 0 {
            return Err(ExtensionError::UnsupportedExtensionFlags);
        }
        let record_end = offset
            .checked_add(
                usize::try_from(header.total_size)
                    .map_err(|_| ExtensionError::InvalidExtensionSize)?,
            )
            .ok_or(ExtensionError::InvalidExtensionSize)?;
        if record_end > bytes.len() {
            return Err(ExtensionError::InvalidExtensionSize);
        }

        if header.kind == EXTENSION_KIND_PAGE_TABLE_OWNERSHIP {
            if header.version != EXTENSION_VERSION_1 {
                if header.flags & EXTENSION_FLAG_REQUIRED != 0 {
                    return Err(ExtensionError::UnsupportedPageTableExtensionVersion);
                }
            } else if header.total_size != PAGE_TABLE_OWNERSHIP_SIZE {
                return Err(ExtensionError::TruncatedPageTableOwnership);
            } else if page_table_ownership.is_some() {
                return Err(ExtensionError::DuplicatePageTableOwnership);
            } else {
                page_table_ownership =
                    Some(parse_page_table_ownership(&bytes[header_end..record_end]).validate()?);
            }
        } else if header.flags & EXTENSION_FLAG_REQUIRED != 0 {
            return Err(ExtensionError::UnknownRequiredExtension);
        }

        extension_count = extension_count
            .checked_add(1)
            .ok_or(ExtensionError::ExtensionCountOverflow)?;
        offset = record_end;
    }

    Ok(ExtensionSummary {
        page_table_ownership,
        extension_count,
    })
}

pub fn validate_page_table_frames(
    ownership: ValidatedPageTableOwnership,
    frames: &[OwnedPageTableFrame],
    memory_map: &[MemoryDescriptor],
) -> Result<(), ExtensionError> {
    let expected_count = usize::try_from(ownership.raw.owned_frame_count)
        .map_err(|_| ExtensionError::OwnedFrameCountMismatch)?;
    if frames.len() != expected_count {
        return Err(ExtensionError::OwnedFrameCountMismatch);
    }
    let mut root_count = 0_u32;
    for (index, frame) in frames.iter().enumerate() {
        if frame.reserved0 != 0 {
            return Err(ExtensionError::OwnedFrameReservedNotZero);
        }
        validate_frame_address(frame.physical_frame, ownership.raw.physical_address_cap)?;
        if frames[..index]
            .iter()
            .any(|previous| previous.physical_frame == frame.physical_frame)
        {
            return Err(ExtensionError::DuplicateOwnedFrame);
        }
        if frame.physical_frame == ownership.raw.root_physical_frame {
            root_count += 1;
        }
        let reserved = memory_map.iter().any(|descriptor| {
            if descriptor.kind != MEMORY_KIND_PAGE_TABLE || descriptor.validate().is_err() {
                return false;
            }
            let length = descriptor.page_count * MEMORY_PAGE_SIZE;
            descriptor.physical_start <= frame.physical_frame
                && frame.physical_frame < descriptor.physical_start + length
        });
        if !reserved {
            return Err(ExtensionError::OwnedFrameNotReserved);
        }
    }
    if root_count != 1 {
        return Err(ExtensionError::RootFrameMissingOrDuplicated);
    }
    Ok(())
}

fn validate_frame_address(address: u64, cap: u64) -> Result<(), ExtensionError> {
    if address == 0 || !address.is_multiple_of(MEMORY_PAGE_SIZE) {
        return Err(if address.is_multiple_of(MEMORY_PAGE_SIZE) {
            ExtensionError::InvalidRootFrame
        } else {
            ExtensionError::UnalignedOwnedFrame
        });
    }
    let end = address
        .checked_add(MEMORY_PAGE_SIZE)
        .ok_or(ExtensionError::OwnedFrameOutsidePhysicalCap)?;
    if end > cap {
        return Err(ExtensionError::OwnedFrameOutsidePhysicalCap);
    }
    Ok(())
}

fn parse_extension_header(bytes: &[u8]) -> ExtensionHeader {
    ExtensionHeader {
        kind: read_u32(bytes, 0),
        version: read_u16(bytes, 4),
        header_size: read_u16(bytes, 6),
        total_size: read_u32(bytes, 8),
        flags: read_u32(bytes, 12),
    }
}

fn parse_page_table_ownership(bytes: &[u8]) -> PageTableOwnership {
    PageTableOwnership {
        hierarchy_version: read_u16(bytes, 0),
        paging_level_count: bytes[2],
        state: bytes[3],
        page_size: read_u32(bytes, 4),
        root_physical_frame: read_u64(bytes, 8),
        owned_frame_list_physical_address: read_u64(bytes, 16),
        owned_frame_count: read_u32(bytes, 24),
        descriptor_stride: read_u32(bytes, 28),
        physical_address_cap: read_u64(bytes, 32),
        reserved0: read_u64(bytes, 40),
        reserved1: read_u64(bytes, 48),
        reserved2: read_u64(bytes, 56),
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

const _: () = assert!(size_of::<ExtensionHeader>() == 16);
const _: () = assert!(align_of::<ExtensionHeader>() == 4);
const _: () = assert!(size_of::<PageTableOwnership>() == 64);
const _: () = assert!(align_of::<PageTableOwnership>() == 8);
const _: () = assert!(size_of::<OwnedPageTableFrame>() == 16);
const _: () = assert!(align_of::<OwnedPageTableFrame>() == 8);
