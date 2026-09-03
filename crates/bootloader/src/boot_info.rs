use boot_protocol::{
    BOOT_INFO_SIZE, BootInfo, FramebufferInfo, MEMORY_DESCRIPTOR_SIZE, MEMORY_KIND_ACPI_RECLAIM,
    MEMORY_KIND_LOADER, MEMORY_KIND_PERSISTENT, MEMORY_KIND_RESERVED, MEMORY_KIND_RUNTIME,
    MEMORY_KIND_UNUSABLE, MEMORY_KIND_USABLE, MEMORY_PAGE_SIZE, MemoryDescriptor, MemoryMapInfo,
    PIXEL_FORMAT_BGRX8888, PIXEL_FORMAT_RGBX8888,
};
#[cfg(test)]
use boot_protocol::{MEMORY_KIND_BOOT_INFO, MEMORY_KIND_BOOT_MEMORY_MAP, MEMORY_KIND_KERNEL_IMAGE};

use crate::PageBackend;

pub const RAW_MEMORY_MAP_MAX_BYTES: usize = 256 * 1024;
pub const RAW_MEMORY_MAP_RETRY_LIMIT: usize = 3;
pub const CONVERTED_DESCRIPTOR_CAPACITY: usize = 2048;
pub const RESERVATION_CAPACITY: usize = 64;

pub const UEFI_RESERVED: u32 = 0;
pub const UEFI_LOADER_CODE: u32 = 1;
pub const UEFI_LOADER_DATA: u32 = 2;
pub const UEFI_BOOT_SERVICES_CODE: u32 = 3;
pub const UEFI_BOOT_SERVICES_DATA: u32 = 4;
pub const UEFI_RUNTIME_SERVICES_CODE: u32 = 5;
pub const UEFI_RUNTIME_SERVICES_DATA: u32 = 6;
pub const UEFI_CONVENTIONAL: u32 = 7;
pub const UEFI_UNUSABLE: u32 = 8;
pub const UEFI_ACPI_RECLAIM: u32 = 9;
pub const UEFI_ACPI_NVS: u32 = 10;
pub const UEFI_MMIO: u32 = 11;
pub const UEFI_MMIO_PORT_SPACE: u32 = 12;
pub const UEFI_PAL_CODE: u32 = 13;
pub const UEFI_PERSISTENT_MEMORY: u32 = 14;
pub const UEFI_UNACCEPTED: u32 = 15;

pub const fn map_uefi_memory_kind(memory_type: u32) -> u32 {
    match memory_type {
        UEFI_CONVENTIONAL => MEMORY_KIND_USABLE,
        UEFI_LOADER_CODE | UEFI_LOADER_DATA => MEMORY_KIND_LOADER,
        UEFI_RUNTIME_SERVICES_CODE | UEFI_RUNTIME_SERVICES_DATA => MEMORY_KIND_RUNTIME,
        UEFI_ACPI_RECLAIM => MEMORY_KIND_ACPI_RECLAIM,
        UEFI_UNUSABLE | UEFI_UNACCEPTED => MEMORY_KIND_UNUSABLE,
        UEFI_PERSISTENT_MEMORY => MEMORY_KIND_PERSISTENT,
        UEFI_RESERVED
        | UEFI_BOOT_SERVICES_CODE
        | UEFI_BOOT_SERVICES_DATA
        | UEFI_ACPI_NVS
        | UEFI_MMIO
        | UEFI_MMIO_PORT_SPACE
        | UEFI_PAL_CODE => MEMORY_KIND_RESERVED,
        _ => MEMORY_KIND_RESERVED,
    }
}

/// Converts the final map after `ExitBootServices` has succeeded.
///
/// Boot-services code and data become reclaimable only on this post-exit path.
/// Loader data remains conservatively loader-owned because it can contain the
/// kernel, boot information, map buffers, or the loader image itself.
pub const fn map_uefi_memory_kind_post_exit(memory_type: u32) -> u32 {
    match memory_type {
        UEFI_BOOT_SERVICES_CODE | UEFI_BOOT_SERVICES_DATA => MEMORY_KIND_USABLE,
        _ => map_uefi_memory_kind(memory_type),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawMemoryDescriptor {
    pub memory_type: u32,
    pub physical_start: u64,
    pub page_count: u64,
    pub attributes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapBuildError {
    InvalidRawStride,
    RawCapacityExceeded,
    ZeroPages,
    RangeOverflow,
    Unaligned,
    DescriptorOverlap,
    ReservationCapacity,
    ReservationOverlap,
    ReservationOutsideMap,
    OutputCapacity,
}

pub fn validate_raw_map_capacity(required: usize, stride: usize) -> Result<usize, MapBuildError> {
    if stride < core::mem::size_of::<uefi::mem::memory_map::MemoryDescriptor>() {
        return Err(MapBuildError::InvalidRawStride);
    }
    let rounded = required
        .checked_add(stride.checked_mul(8).ok_or(MapBuildError::RangeOverflow)?)
        .ok_or(MapBuildError::RangeOverflow)?;
    if rounded > RAW_MEMORY_MAP_MAX_BYTES {
        return Err(MapBuildError::RawCapacityExceeded);
    }
    Ok(rounded)
}

pub const fn retry_is_allowed(attempt: usize) -> bool {
    attempt < RAW_MEMORY_MAP_RETRY_LIMIT
}

pub fn normalize_memory_map(
    raw: &[RawMemoryDescriptor],
    output: &mut [MemoryDescriptor],
) -> Result<usize, MapBuildError> {
    if raw.len() > output.len() {
        return Err(MapBuildError::OutputCapacity);
    }
    for (index, descriptor) in raw.iter().copied().enumerate() {
        validate_range(descriptor.physical_start, descriptor.page_count)?;
        output[index] = MemoryDescriptor {
            kind: map_uefi_memory_kind(descriptor.memory_type),
            reserved0: 0,
            physical_start: descriptor.physical_start,
            page_count: descriptor.page_count,
            attributes: descriptor.attributes,
        };
    }
    normalize_mapped_memory_map(output, raw.len())
}

pub fn normalize_mapped_memory_map(
    output: &mut [MemoryDescriptor],
    mut len: usize,
) -> Result<usize, MapBuildError> {
    if len > output.len() {
        return Err(MapBuildError::OutputCapacity);
    }
    for descriptor in &output[..len] {
        validate_range(descriptor.physical_start, descriptor.page_count)?;
    }
    insertion_sort(&mut output[..len]);
    validate_non_overlapping(&output[..len])?;
    len = merge_base_descriptors(output, len)?;
    output[len..].fill(empty_descriptor());
    Ok(len)
}

fn insertion_sort(descriptors: &mut [MemoryDescriptor]) {
    for index in 1..descriptors.len() {
        let value = descriptors[index];
        let mut position = index;
        while position > 0 && descriptors[position - 1].physical_start > value.physical_start {
            descriptors[position] = descriptors[position - 1];
            position -= 1;
        }
        descriptors[position] = value;
    }
}

fn validate_non_overlapping(descriptors: &[MemoryDescriptor]) -> Result<(), MapBuildError> {
    for pair in descriptors.windows(2) {
        let left_end = range_end(pair[0].physical_start, pair[0].page_count)?;
        if left_end > pair[1].physical_start {
            return Err(MapBuildError::DescriptorOverlap);
        }
    }
    Ok(())
}

fn merge_base_descriptors(
    descriptors: &mut [MemoryDescriptor],
    len: usize,
) -> Result<usize, MapBuildError> {
    if len == 0 {
        return Ok(0);
    }
    let mut write = 1;
    for read in 1..len {
        let previous = descriptors[write - 1];
        let current = descriptors[read];
        let previous_end = range_end(previous.physical_start, previous.page_count)?;
        if previous_end == current.physical_start
            && previous.kind == current.kind
            && previous.attributes == current.attributes
        {
            descriptors[write - 1].page_count = previous
                .page_count
                .checked_add(current.page_count)
                .ok_or(MapBuildError::RangeOverflow)?;
        } else {
            descriptors[write] = current;
            write += 1;
        }
    }
    Ok(write)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReservationSource {
    KernelImage,
    BootInfo,
    RawMemoryMap,
    ConversionScratch,
    ConvertedMemoryMap,
    Framebuffer,
    BootstrapStack,
    PageTable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reservation {
    pub physical_start: u64,
    pub page_count: u64,
    pub kind: u32,
    pub source: ReservationSource,
}

impl Reservation {
    const EMPTY: Self = Self {
        physical_start: 0,
        page_count: 0,
        kind: MEMORY_KIND_RESERVED,
        source: ReservationSource::KernelImage,
    };
}

pub struct ReservationList {
    items: [Reservation; RESERVATION_CAPACITY],
    len: usize,
}

impl ReservationList {
    pub const fn new() -> Self {
        Self {
            items: [Reservation::EMPTY; RESERVATION_CAPACITY],
            len: 0,
        }
    }

    pub fn push(&mut self, reservation: Reservation) -> Result<(), MapBuildError> {
        validate_range(reservation.physical_start, reservation.page_count)?;
        if self.len == RESERVATION_CAPACITY {
            return Err(MapBuildError::ReservationCapacity);
        }
        self.items[self.len] = reservation;
        self.len += 1;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), MapBuildError> {
        for index in 1..self.len {
            let value = self.items[index];
            let mut position = index;
            while position > 0 && self.items[position - 1].physical_start > value.physical_start {
                self.items[position] = self.items[position - 1];
                position -= 1;
            }
            self.items[position] = value;
        }
        for pair in self.items[..self.len].windows(2) {
            if range_end(pair[0].physical_start, pair[0].page_count)? > pair[1].physical_start {
                return Err(MapBuildError::ReservationOverlap);
            }
        }
        Ok(())
    }

    pub fn items(&self) -> impl Iterator<Item = Reservation> + '_ {
        self.items[..self.len].iter().copied()
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for ReservationList {
    fn default() -> Self {
        Self::new()
    }
}

pub fn apply_reservations(
    base: &[MemoryDescriptor],
    reservations: &ReservationList,
    output: &mut [MemoryDescriptor],
) -> Result<usize, MapBuildError> {
    validate_non_overlapping(base)?;
    let mut covered = [0_u64; RESERVATION_CAPACITY];
    let reservation_items: [Reservation; RESERVATION_CAPACITY] = reservations.items;
    let mut len = 0;
    for descriptor in base {
        descriptor
            .validate()
            .map_err(|_| MapBuildError::RangeOverflow)?;
        let descriptor_end = range_end(descriptor.physical_start, descriptor.page_count)?;
        let mut cursor = descriptor.physical_start;
        for (reservation_index, reservation) in
            reservation_items[..reservations.len].iter().enumerate()
        {
            let reservation_end = range_end(reservation.physical_start, reservation.page_count)?;
            let overlap_start = cursor.max(reservation.physical_start);
            let overlap_end = descriptor_end.min(reservation_end);
            if overlap_start >= overlap_end {
                continue;
            }
            if cursor < overlap_start {
                append_descriptor(
                    output,
                    &mut len,
                    MemoryDescriptor {
                        kind: descriptor.kind,
                        reserved0: 0,
                        physical_start: cursor,
                        page_count: (overlap_start - cursor) / MEMORY_PAGE_SIZE,
                        attributes: descriptor.attributes,
                    },
                    true,
                )?;
            }
            let pages = (overlap_end - overlap_start) / MEMORY_PAGE_SIZE;
            append_descriptor(
                output,
                &mut len,
                MemoryDescriptor {
                    kind: reservation.kind,
                    reserved0: 0,
                    physical_start: overlap_start,
                    page_count: pages,
                    attributes: 0,
                },
                false,
            )?;
            covered[reservation_index] = covered[reservation_index]
                .checked_add(pages)
                .ok_or(MapBuildError::RangeOverflow)?;
            cursor = overlap_end;
        }
        if cursor < descriptor_end {
            append_descriptor(
                output,
                &mut len,
                MemoryDescriptor {
                    kind: descriptor.kind,
                    reserved0: 0,
                    physical_start: cursor,
                    page_count: (descriptor_end - cursor) / MEMORY_PAGE_SIZE,
                    attributes: descriptor.attributes,
                },
                true,
            )?;
        }
    }
    for (index, reservation) in reservation_items[..reservations.len].iter().enumerate() {
        if reservation.source != ReservationSource::Framebuffer
            && covered[index] != reservation.page_count
        {
            return Err(MapBuildError::ReservationOutsideMap);
        }
    }
    output[len..].fill(empty_descriptor());
    validate_non_overlapping(&output[..len])?;
    Ok(len)
}

fn append_descriptor(
    output: &mut [MemoryDescriptor],
    len: &mut usize,
    descriptor: MemoryDescriptor,
    allow_merge: bool,
) -> Result<(), MapBuildError> {
    if descriptor.page_count == 0 {
        return Ok(());
    }
    if allow_merge && *len != 0 {
        let previous = output[*len - 1];
        if range_end(previous.physical_start, previous.page_count)? == descriptor.physical_start
            && previous.kind == descriptor.kind
            && previous.attributes == descriptor.attributes
        {
            output[*len - 1].page_count = previous
                .page_count
                .checked_add(descriptor.page_count)
                .ok_or(MapBuildError::RangeOverflow)?;
            return Ok(());
        }
    }
    if *len == output.len() {
        return Err(MapBuildError::OutputCapacity);
    }
    output[*len] = descriptor;
    *len += 1;
    Ok(())
}

const fn empty_descriptor() -> MemoryDescriptor {
    MemoryDescriptor {
        kind: 0,
        reserved0: 0,
        physical_start: 0,
        page_count: 0,
        attributes: 0,
    }
}

fn validate_range(start: u64, pages: u64) -> Result<(), MapBuildError> {
    if pages == 0 {
        return Err(MapBuildError::ZeroPages);
    }
    if !start.is_multiple_of(MEMORY_PAGE_SIZE) {
        return Err(MapBuildError::Unaligned);
    }
    range_end(start, pages).map(|_| ())
}

fn range_end(start: u64, pages: u64) -> Result<u64, MapBuildError> {
    start
        .checked_add(
            pages
                .checked_mul(MEMORY_PAGE_SIZE)
                .ok_or(MapBuildError::RangeOverflow)?,
        )
        .ok_or(MapBuildError::RangeOverflow)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GopPixelFormat {
    Rgb,
    Bgr,
    Bitmask {
        red: u32,
        green: u32,
        blue: u32,
        reserved: u32,
    },
    BltOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GopFramebuffer {
    pub physical_address: u64,
    pub byte_length: u64,
    pub width: u32,
    pub height: u32,
    pub pixels_per_scanline: u32,
    pub pixel_format: GopPixelFormat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FramebufferError {
    BltOnly,
    UnsupportedBitmask,
    InvalidAddress,
    InvalidDimensions,
    InvalidStride,
    SizeOverflow,
    TooSmall,
    RangeOverflow,
}

pub fn convert_framebuffer(gop: GopFramebuffer) -> Result<FramebufferInfo, FramebufferError> {
    if gop.physical_address == 0 || gop.byte_length == 0 {
        return Err(FramebufferError::InvalidAddress);
    }
    if gop.width == 0 || gop.height == 0 {
        return Err(FramebufferError::InvalidDimensions);
    }
    if gop.pixels_per_scanline < gop.width {
        return Err(FramebufferError::InvalidStride);
    }
    let format = match gop.pixel_format {
        GopPixelFormat::Rgb => PIXEL_FORMAT_RGBX8888,
        GopPixelFormat::Bgr => PIXEL_FORMAT_BGRX8888,
        GopPixelFormat::Bitmask {
            red: 0x0000_00ff,
            green: 0x0000_ff00,
            blue: 0x00ff_0000,
            reserved: 0xff00_0000,
        } => PIXEL_FORMAT_RGBX8888,
        GopPixelFormat::Bitmask {
            red: 0x00ff_0000,
            green: 0x0000_ff00,
            blue: 0x0000_00ff,
            reserved: 0xff00_0000,
        } => PIXEL_FORMAT_BGRX8888,
        GopPixelFormat::Bitmask { .. } => return Err(FramebufferError::UnsupportedBitmask),
        GopPixelFormat::BltOnly => return Err(FramebufferError::BltOnly),
    };
    let required = u64::from(gop.pixels_per_scanline)
        .checked_mul(u64::from(gop.height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(FramebufferError::SizeOverflow)?;
    if gop.byte_length < required {
        return Err(FramebufferError::TooSmall);
    }
    gop.physical_address
        .checked_add(gop.byte_length)
        .ok_or(FramebufferError::RangeOverflow)?;
    Ok(FramebufferInfo {
        physical_address: gop.physical_address,
        byte_length: gop.byte_length,
        width: gop.width,
        height: gop.height,
        pixels_per_scanline: gop.pixels_per_scanline,
        pixel_format: format,
        reserved0: 0,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootInfoBuildError {
    CountOverflow,
    LengthOverflow,
    DescriptorInvalid,
    BootInfoInvalid,
}

pub fn build_boot_info(
    descriptor_address: u64,
    descriptors: &[MemoryDescriptor],
    descriptor_version: u16,
    framebuffer: FramebufferInfo,
) -> Result<BootInfo, BootInfoBuildError> {
    for descriptor in descriptors {
        descriptor
            .validate()
            .map_err(|_| BootInfoBuildError::DescriptorInvalid)?;
    }
    let count = u64::try_from(descriptors.len()).map_err(|_| BootInfoBuildError::CountOverflow)?;
    let byte_length = count
        .checked_mul(u64::from(MEMORY_DESCRIPTOR_SIZE))
        .ok_or(BootInfoBuildError::LengthOverflow)?;
    let boot_info = BootInfo::new(
        MemoryMapInfo {
            physical_address: descriptor_address,
            descriptor_count: count,
            descriptor_stride: MEMORY_DESCRIPTOR_SIZE,
            descriptor_version,
            reserved0: 0,
            byte_length,
            reserved1: 0,
        },
        framebuffer,
    );
    boot_info
        .validate()
        .map_err(|_| BootInfoBuildError::BootInfoInvalid)?;
    Ok(boot_info)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageAllocation {
    pub page_start: u64,
    pub page_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootDataAllocations {
    pub raw_map: PageAllocation,
    pub conversion_scratch: PageAllocation,
    pub converted_map: PageAllocation,
    pub boot_info: PageAllocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MapKeyStatus {
    Provisional,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProvisionalMapMetadata {
    pub map_size: u64,
    pub descriptor_stride: u64,
    pub descriptor_version: u32,
    pub map_key: u64,
    pub status: MapKeyStatus,
}

#[derive(Debug, Eq, PartialEq)]
pub struct BootInfoReleaseError<E> {
    pub allocation_index: usize,
    pub remaining_allocations: usize,
    pub source: E,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedBootInfoError {
    InvalidBootInfo,
    InvalidAllocation,
    MapOutsideAllocation,
}

#[must_use]
pub struct PreparedBootInfo<B: PageBackend> {
    backend: B,
    allocations: [PageAllocation; 4],
    owned: [bool; 4],
    boot_info: BootInfo,
    boot_info_address: u64,
    provisional_map: ProvisionalMapMetadata,
}

impl<B: PageBackend> PreparedBootInfo<B> {
    pub fn from_validated(
        backend: B,
        allocations: BootDataAllocations,
        boot_info: BootInfo,
        descriptors: &[MemoryDescriptor],
        provisional_map: ProvisionalMapMetadata,
    ) -> Result<Self, PreparedBootInfoError> {
        boot_info
            .validate()
            .map_err(|_| PreparedBootInfoError::InvalidBootInfo)?;
        if u64::try_from(descriptors.len()).ok() != Some(boot_info.memory_map.descriptor_count)
            || descriptors
                .iter()
                .any(|descriptor| descriptor.validate().is_err())
        {
            return Err(PreparedBootInfoError::InvalidBootInfo);
        }
        let ordered = [
            allocations.raw_map,
            allocations.conversion_scratch,
            allocations.converted_map,
            allocations.boot_info,
        ];
        for allocation in ordered {
            validate_range(allocation.page_start, allocation.page_count)
                .map_err(|_| PreparedBootInfoError::InvalidAllocation)?;
        }
        let converted_end = range_end(
            allocations.converted_map.page_start,
            allocations.converted_map.page_count,
        )
        .map_err(|_| PreparedBootInfoError::InvalidAllocation)?;
        let map_end = boot_info
            .memory_map
            .physical_address
            .checked_add(boot_info.memory_map.byte_length)
            .ok_or(PreparedBootInfoError::MapOutsideAllocation)?;
        if boot_info.memory_map.physical_address < allocations.converted_map.page_start
            || map_end > converted_end
        {
            return Err(PreparedBootInfoError::MapOutsideAllocation);
        }
        Ok(Self {
            backend,
            allocations: ordered,
            owned: [true; 4],
            boot_info,
            boot_info_address: allocations.boot_info.page_start,
            provisional_map,
        })
    }

    pub const fn boot_info_physical_address(&self) -> u64 {
        self.boot_info_address
    }
    pub const fn wire_size(&self) -> u32 {
        BOOT_INFO_SIZE
    }
    pub const fn descriptor_count(&self) -> u64 {
        self.boot_info.memory_map.descriptor_count
    }
    pub const fn descriptor_stride(&self) -> u32 {
        self.boot_info.memory_map.descriptor_stride
    }
    pub const fn framebuffer(&self) -> FramebufferInfo {
        self.boot_info.framebuffer
    }
    pub const fn provisional_map_key(&self) -> (u64, MapKeyStatus) {
        (self.provisional_map.map_key, self.provisional_map.status)
    }
    pub const fn provisional_map_metadata(&self) -> ProvisionalMapMetadata {
        self.provisional_map
    }
    pub(crate) const fn allocations(&self) -> BootDataAllocations {
        BootDataAllocations {
            raw_map: self.allocations[0],
            conversion_scratch: self.allocations[1],
            converted_map: self.allocations[2],
            boot_info: self.allocations[3],
        }
    }
    pub fn remaining_allocation_count(&self) -> usize {
        self.owned.iter().filter(|owned| **owned).count()
    }
    pub fn is_released(&self) -> bool {
        self.remaining_allocation_count() == 0
    }

    pub fn try_release(&mut self) -> Result<(), BootInfoReleaseError<B::Error>> {
        for index in (0..self.allocations.len()).rev() {
            if self.owned[index] {
                let allocation = self.allocations[index];
                if let Err(source) = self
                    .backend
                    .free(allocation.page_start, allocation.page_count)
                {
                    return Err(BootInfoReleaseError {
                        allocation_index: index,
                        remaining_allocations: self.remaining_allocation_count(),
                        source,
                    });
                }
                self.owned[index] = false;
            }
        }
        Ok(())
    }
}

impl<B: PageBackend> Drop for PreparedBootInfo<B> {
    fn drop(&mut self) {
        for index in (0..self.allocations.len()).rev() {
            if self.owned[index] {
                let allocation = self.allocations[index];
                if self
                    .backend
                    .free(allocation.page_start, allocation.page_count)
                    .is_ok()
                {
                    self.owned[index] = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{cell::RefCell, rc::Rc, vec, vec::Vec};

    fn raw(ty: u32, start: u64, pages: u64, attributes: u64) -> RawMemoryDescriptor {
        RawMemoryDescriptor {
            memory_type: ty,
            physical_start: start,
            page_count: pages,
            attributes,
        }
    }

    fn fb(format: GopPixelFormat) -> GopFramebuffer {
        GopFramebuffer {
            physical_address: 0x8000_0000,
            byte_length: 800 * 600 * 4,
            width: 800,
            height: 600,
            pixels_per_scanline: 800,
            pixel_format: format,
        }
    }

    #[test]
    fn every_uefi_memory_type_has_conservative_mapping() {
        assert_eq!(map_uefi_memory_kind(UEFI_CONVENTIONAL), MEMORY_KIND_USABLE);
        for ty in [UEFI_LOADER_CODE, UEFI_LOADER_DATA] {
            assert_eq!(map_uefi_memory_kind(ty), MEMORY_KIND_LOADER);
        }
        for ty in [
            UEFI_BOOT_SERVICES_CODE,
            UEFI_BOOT_SERVICES_DATA,
            UEFI_RESERVED,
            UEFI_ACPI_NVS,
            UEFI_MMIO,
            UEFI_MMIO_PORT_SPACE,
            UEFI_PAL_CODE,
        ] {
            assert_eq!(map_uefi_memory_kind(ty), MEMORY_KIND_RESERVED);
        }
        for ty in [UEFI_RUNTIME_SERVICES_CODE, UEFI_RUNTIME_SERVICES_DATA] {
            assert_eq!(map_uefi_memory_kind(ty), MEMORY_KIND_RUNTIME);
        }
        assert_eq!(
            map_uefi_memory_kind(UEFI_ACPI_RECLAIM),
            MEMORY_KIND_ACPI_RECLAIM
        );
        assert_eq!(map_uefi_memory_kind(UEFI_UNUSABLE), MEMORY_KIND_UNUSABLE);
        assert_eq!(map_uefi_memory_kind(UEFI_UNACCEPTED), MEMORY_KIND_UNUSABLE);
        assert_eq!(
            map_uefi_memory_kind(UEFI_PERSISTENT_MEMORY),
            MEMORY_KIND_PERSISTENT
        );
        assert_eq!(map_uefi_memory_kind(0x7000_0042), MEMORY_KIND_RESERVED);
    }

    #[test]
    fn post_exit_mapping_reclaims_only_boot_services_memory() {
        assert_eq!(
            map_uefi_memory_kind_post_exit(UEFI_BOOT_SERVICES_CODE),
            MEMORY_KIND_USABLE
        );
        assert_eq!(
            map_uefi_memory_kind_post_exit(UEFI_BOOT_SERVICES_DATA),
            MEMORY_KIND_USABLE
        );
        assert_eq!(
            map_uefi_memory_kind_post_exit(UEFI_LOADER_DATA),
            MEMORY_KIND_LOADER
        );
        assert_eq!(
            map_uefi_memory_kind_post_exit(UEFI_RUNTIME_SERVICES_DATA),
            MEMORY_KIND_RUNTIME
        );
        assert_eq!(
            map_uefi_memory_kind_post_exit(UEFI_ACPI_NVS),
            MEMORY_KIND_RESERVED
        );
        assert_eq!(
            map_uefi_memory_kind_post_exit(UEFI_MMIO),
            MEMORY_KIND_RESERVED
        );
        assert_eq!(
            map_uefi_memory_kind_post_exit(0x7000_0042),
            MEMORY_KIND_RESERVED
        );
    }

    #[test]
    fn raw_capacity_stride_and_retry_are_bounded() {
        assert_eq!(validate_raw_map_capacity(1024, 48), Ok(1408));
        assert_eq!(
            validate_raw_map_capacity(1024, 8),
            Err(MapBuildError::InvalidRawStride)
        );
        assert_eq!(
            validate_raw_map_capacity(RAW_MEMORY_MAP_MAX_BYTES, 48),
            Err(MapBuildError::RawCapacityExceeded)
        );
        assert!(retry_is_allowed(0));
        assert!(retry_is_allowed(2));
        assert!(!retry_is_allowed(3));
    }

    #[test]
    fn normalization_rejects_ranges_sorts_and_merges_only_matching_neighbors() {
        let mut output = [empty_descriptor(); 8];
        assert_eq!(
            normalize_memory_map(&[raw(7, 0x1000, 0, 0)], &mut output),
            Err(MapBuildError::ZeroPages)
        );
        assert_eq!(
            normalize_memory_map(&[raw(7, u64::MAX & !0xfff, 2, 0)], &mut output),
            Err(MapBuildError::RangeOverflow)
        );
        let len = normalize_memory_map(
            &[
                raw(7, 0x4000, 1, 1),
                raw(7, 0x1000, 2, 1),
                raw(7, 0x3000, 1, 2),
            ],
            &mut output,
        )
        .expect("normalize");
        assert_eq!(len, 3);
        assert_eq!(output[0].physical_start, 0x1000);
        let mut overlap = [empty_descriptor(); 4];
        assert_eq!(
            normalize_memory_map(&[raw(7, 0x1000, 2, 0), raw(7, 0x2000, 1, 0)], &mut overlap),
            Err(MapBuildError::DescriptorOverlap)
        );
        let mut merged = [empty_descriptor(); 4];
        assert_eq!(
            normalize_memory_map(&[raw(7, 0x1000, 1, 1), raw(7, 0x2000, 2, 1)], &mut merged),
            Ok(1)
        );
        assert_eq!(merged[0].page_count, 3);
    }

    #[test]
    fn reservations_sort_reject_overlap_and_split_without_losing_coverage() {
        let mut list = ReservationList::new();
        list.push(Reservation {
            physical_start: 0x5000,
            page_count: 1,
            kind: MEMORY_KIND_BOOT_INFO,
            source: ReservationSource::BootInfo,
        })
        .unwrap();
        list.push(Reservation {
            physical_start: 0x2000,
            page_count: 1,
            kind: MEMORY_KIND_KERNEL_IMAGE,
            source: ReservationSource::KernelImage,
        })
        .unwrap();
        list.finish().unwrap();
        assert_eq!(list.items().next().unwrap().physical_start, 0x2000);
        let base = [MemoryDescriptor {
            kind: MEMORY_KIND_USABLE,
            reserved0: 0,
            physical_start: 0x1000,
            page_count: 6,
            attributes: 1,
        }];
        let mut output = [empty_descriptor(); 8];
        let len = apply_reservations(&base, &list, &mut output).unwrap();
        assert_eq!(len, 5);
        assert_eq!(output[..len].iter().map(|d| d.page_count).sum::<u64>(), 6);
        assert_eq!(output[1].kind, MEMORY_KIND_KERNEL_IMAGE);
        assert_eq!(output[3].kind, MEMORY_KIND_BOOT_INFO);

        let mut overlap = ReservationList::new();
        overlap
            .push(Reservation {
                physical_start: 0x2000,
                page_count: 2,
                kind: MEMORY_KIND_RESERVED,
                source: ReservationSource::RawMemoryMap,
            })
            .unwrap();
        overlap
            .push(Reservation {
                physical_start: 0x3000,
                page_count: 1,
                kind: MEMORY_KIND_RESERVED,
                source: ReservationSource::ConvertedMemoryMap,
            })
            .unwrap();
        assert_eq!(overlap.finish(), Err(MapBuildError::ReservationOverlap));

        let mut adjacent = ReservationList::new();
        adjacent
            .push(Reservation {
                physical_start: 0x2000,
                page_count: 1,
                kind: MEMORY_KIND_KERNEL_IMAGE,
                source: ReservationSource::KernelImage,
            })
            .unwrap();
        adjacent
            .push(Reservation {
                physical_start: 0x3000,
                page_count: 1,
                kind: MEMORY_KIND_KERNEL_IMAGE,
                source: ReservationSource::KernelImage,
            })
            .unwrap();
        adjacent.finish().unwrap();
        let mut explicit = [empty_descriptor(); 8];
        assert_eq!(apply_reservations(&base, &adjacent, &mut explicit), Ok(4));
        assert_eq!(explicit[1].physical_start, 0x2000);
        assert_eq!(explicit[2].physical_start, 0x3000);

        let mut outside = ReservationList::new();
        outside
            .push(Reservation {
                physical_start: 0x9000,
                page_count: 1,
                kind: MEMORY_KIND_BOOT_INFO,
                source: ReservationSource::BootInfo,
            })
            .unwrap();
        outside.finish().unwrap();
        assert_eq!(
            apply_reservations(&base, &outside, &mut explicit),
            Err(MapBuildError::ReservationOutsideMap)
        );
    }

    #[test]
    fn overlay_covers_kernel_and_all_boot_buffers_and_reports_capacity() {
        let base = [MemoryDescriptor {
            kind: MEMORY_KIND_LOADER,
            reserved0: 0,
            physical_start: 0x1000,
            page_count: 12,
            attributes: 0,
        }];
        let sources = [
            (
                0x2000,
                MEMORY_KIND_KERNEL_IMAGE,
                ReservationSource::KernelImage,
            ),
            (0x4000, MEMORY_KIND_BOOT_INFO, ReservationSource::BootInfo),
            (
                0x6000,
                MEMORY_KIND_BOOT_MEMORY_MAP,
                ReservationSource::RawMemoryMap,
            ),
            (
                0x8000,
                MEMORY_KIND_BOOT_MEMORY_MAP,
                ReservationSource::ConversionScratch,
            ),
            (
                0xa000,
                MEMORY_KIND_BOOT_MEMORY_MAP,
                ReservationSource::ConvertedMemoryMap,
            ),
        ];
        let mut list = ReservationList::new();
        for (start, kind, source) in sources {
            list.push(Reservation {
                physical_start: start,
                page_count: 1,
                kind,
                source,
            })
            .unwrap();
        }
        list.finish().unwrap();
        let mut output = [empty_descriptor(); 16];
        let len = apply_reservations(&base, &list, &mut output).unwrap();
        assert_eq!(output[..len].iter().map(|d| d.page_count).sum::<u64>(), 12);
        assert_eq!(
            output[..len]
                .iter()
                .filter(|d| d.kind == MEMORY_KIND_BOOT_MEMORY_MAP)
                .count(),
            3
        );
        assert_eq!(
            apply_reservations(&base, &list, &mut [empty_descriptor(); 2]),
            Err(MapBuildError::OutputCapacity)
        );
    }

    #[test]
    fn framebuffer_formats_and_failures_are_explicit() {
        assert_eq!(
            convert_framebuffer(fb(GopPixelFormat::Rgb))
                .unwrap()
                .pixel_format,
            PIXEL_FORMAT_RGBX8888
        );
        assert_eq!(
            convert_framebuffer(fb(GopPixelFormat::Bgr))
                .unwrap()
                .pixel_format,
            PIXEL_FORMAT_BGRX8888
        );
        assert_eq!(
            convert_framebuffer(fb(GopPixelFormat::Bitmask {
                red: 0xff,
                green: 0xff00,
                blue: 0xff0000,
                reserved: 0xff000000
            }))
            .unwrap()
            .pixel_format,
            PIXEL_FORMAT_RGBX8888
        );
        assert_eq!(
            convert_framebuffer(fb(GopPixelFormat::Bitmask {
                red: 1,
                green: 2,
                blue: 4,
                reserved: 8
            })),
            Err(FramebufferError::UnsupportedBitmask)
        );
        assert_eq!(
            convert_framebuffer(fb(GopPixelFormat::BltOnly)),
            Err(FramebufferError::BltOnly)
        );
        let mut invalid = fb(GopPixelFormat::Rgb);
        invalid.width = 0;
        assert_eq!(
            convert_framebuffer(invalid),
            Err(FramebufferError::InvalidDimensions)
        );
        let mut invalid = fb(GopPixelFormat::Rgb);
        invalid.pixels_per_scanline = 799;
        assert_eq!(
            convert_framebuffer(invalid),
            Err(FramebufferError::InvalidStride)
        );
        let mut invalid = fb(GopPixelFormat::Rgb);
        invalid.byte_length = 1;
        assert_eq!(
            convert_framebuffer(invalid),
            Err(FramebufferError::TooSmall)
        );
        let mut invalid = fb(GopPixelFormat::Rgb);
        invalid.physical_address = u64::MAX - 1;
        assert_eq!(
            convert_framebuffer(invalid),
            Err(FramebufferError::RangeOverflow)
        );
        let mut invalid = fb(GopPixelFormat::Rgb);
        invalid.width = u32::MAX;
        invalid.height = u32::MAX;
        invalid.pixels_per_scanline = u32::MAX;
        invalid.byte_length = u64::MAX;
        assert_eq!(
            convert_framebuffer(invalid),
            Err(FramebufferError::SizeOverflow)
        );
    }

    #[test]
    fn boot_info_builder_uses_wire_layout_and_zeroes_reserved_fields() {
        let descriptors = [MemoryDescriptor {
            kind: MEMORY_KIND_USABLE,
            reserved0: 0,
            physical_start: 0x1000,
            page_count: 1,
            attributes: 0,
        }];
        let framebuffer = convert_framebuffer(fb(GopPixelFormat::Rgb)).unwrap();
        let info = build_boot_info(0x4000, &descriptors, 1, framebuffer).unwrap();
        assert_eq!(info.memory_map.descriptor_count, 1);
        assert_eq!(info.memory_map.descriptor_stride, 32);
        assert_eq!(info.memory_map.byte_length, 32);
        assert_eq!(info.header.reserved0, 0);
        assert_eq!(info.header.reserved1, 0);
        assert_eq!(info.memory_map.reserved0, 0);
        assert_eq!(info.memory_map.reserved1, 0);
        assert_eq!(info.framebuffer.reserved0, 0);
        assert_eq!(info.reserved0, 0);
        assert_eq!(info.reserved1, 0);
        assert!(info.validate().is_ok());
    }

    #[derive(Clone)]
    struct FakeBackend {
        calls: Rc<RefCell<Vec<u64>>>,
        fail_at: Option<usize>,
        attempts: usize,
    }
    impl PageBackend for FakeBackend {
        type Error = &'static str;
        fn allocate_at(&mut self, _: u64, _: u64) -> Result<(), Self::Error> {
            Ok(())
        }
        fn free(&mut self, start: u64, _: u64) -> Result<(), Self::Error> {
            let attempt = self.attempts;
            self.attempts += 1;
            self.calls.borrow_mut().push(start);
            if self.fail_at == Some(attempt) {
                Err("free")
            } else {
                Ok(())
            }
        }
    }

    fn allocations() -> BootDataAllocations {
        BootDataAllocations {
            raw_map: PageAllocation {
                page_start: 0x10000,
                page_count: 1,
            },
            conversion_scratch: PageAllocation {
                page_start: 0x11000,
                page_count: 1,
            },
            converted_map: PageAllocation {
                page_start: 0x12000,
                page_count: 1,
            },
            boot_info: PageAllocation {
                page_start: 0x13000,
                page_count: 1,
            },
        }
    }

    #[test]
    fn prepared_boot_info_release_retry_and_provisional_key_are_exact() {
        let descriptors = [MemoryDescriptor {
            kind: MEMORY_KIND_USABLE,
            reserved0: 0,
            physical_start: 0x1000,
            page_count: 1,
            attributes: 0,
        }];
        let info = build_boot_info(
            0x12000,
            &descriptors,
            1,
            convert_framebuffer(fb(GopPixelFormat::Rgb)).unwrap(),
        )
        .unwrap();
        let calls = Rc::new(RefCell::new(vec![]));
        let backend = FakeBackend {
            calls: calls.clone(),
            fail_at: Some(1),
            attempts: 0,
        };
        let mut prepared = PreparedBootInfo::from_validated(
            backend,
            allocations(),
            info,
            &descriptors,
            ProvisionalMapMetadata {
                map_size: 48,
                descriptor_stride: 48,
                descriptor_version: 1,
                map_key: 77,
                status: MapKeyStatus::Provisional,
            },
        )
        .unwrap();
        assert_eq!(prepared.boot_info_physical_address(), 0x13000);
        assert_eq!(prepared.wire_size(), 128);
        assert_eq!(prepared.descriptor_count(), 1);
        assert_eq!(
            prepared.provisional_map_key(),
            (77, MapKeyStatus::Provisional)
        );
        let error = prepared.try_release().expect_err("second reverse free");
        assert_eq!(error.allocation_index, 2);
        assert_eq!(error.remaining_allocations, 3);
        prepared.try_release().unwrap();
        assert!(prepared.is_released());
        assert_eq!(
            *calls.borrow(),
            [0x13000, 0x12000, 0x12000, 0x11000, 0x10000]
        );
    }

    #[test]
    fn prepared_boot_info_drop_is_best_effort_reverse_fallback() {
        let descriptors = [MemoryDescriptor {
            kind: MEMORY_KIND_USABLE,
            reserved0: 0,
            physical_start: 0x1000,
            page_count: 1,
            attributes: 0,
        }];
        let info = build_boot_info(
            0x12000,
            &descriptors,
            1,
            convert_framebuffer(fb(GopPixelFormat::Rgb)).unwrap(),
        )
        .unwrap();
        let calls = Rc::new(RefCell::new(vec![]));
        let backend = FakeBackend {
            calls: calls.clone(),
            fail_at: None,
            attempts: 0,
        };
        let prepared = PreparedBootInfo::from_validated(
            backend,
            allocations(),
            info,
            &descriptors,
            ProvisionalMapMetadata {
                map_size: 48,
                descriptor_stride: 48,
                descriptor_version: 1,
                map_key: 1,
                status: MapKeyStatus::Provisional,
            },
        )
        .unwrap();
        drop(prepared);
        assert_eq!(*calls.borrow(), [0x13000, 0x12000, 0x11000, 0x10000]);
    }
}
