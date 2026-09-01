#![no_std]
#![forbid(unsafe_code)]

//! Dependency-free validation for the UnnamedOS Phase 1D ELF64 kernel image.

pub const ELF_HEADER_SIZE: u16 = 64;
pub const PROGRAM_HEADER_SIZE: u16 = 56;
pub const SECTION_HEADER_SIZE: u16 = 64;
pub const MAX_PROGRAM_HEADERS: u16 = 128;
pub const BOOTSTRAP_LINK_ADDRESS: u64 = 0x0020_0000;
pub const BOOTSTRAP_PAGE_SIZE: u64 = 4096;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_TLS: u32 = 7;
const PF_EXECUTE: u32 = 1;
const PF_WRITE: u32 = 2;
const SHT_RELA: u32 = 4;
const SHT_REL: u32 = 9;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ValidationError {
    HeaderTooSmall,
    BadMagic,
    UnsupportedClass,
    UnsupportedEndianness,
    UnsupportedIdentVersion,
    UnsupportedOsAbi,
    UnsupportedObjectType,
    UnsupportedMachine,
    UnsupportedElfVersion,
    InvalidHeaderSize,
    InvalidProgramHeaderSize,
    TooManyProgramHeaders,
    ProgramHeaderTableOverflow,
    ProgramHeaderTableOutsideFile,
    InvalidSectionHeaderSize,
    SectionHeaderTableOverflow,
    SectionHeaderTableOutsideFile,
    RuntimeRelocations,
    InterpreterSegment,
    DynamicSegment,
    TlsSegment,
    NoLoadSegments,
    ZeroLengthLoadSegment,
    FileSizeExceedsMemorySize,
    SegmentFileRangeOverflow,
    SegmentOutsideFile,
    SegmentMemoryRangeOverflow,
    InvalidAlignment,
    InvalidOffsetAddressCongruence,
    WritableExecutableSegment,
    NonCanonicalAddress,
    PhysicalVirtualAddressMismatch,
    OverlappingLoadSegments,
    EntryOutsideExecutableSegment,
    InvalidPageSize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapValidationError {
    Elf(ValidationError),
    UnexpectedEntry,
    UnexpectedLoadStart,
    MisalignedLoadSegment,
    MissingExecuteSegment,
    MissingReadOnlySegment,
    MissingWritableSegment,
    MissingBss,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawHeader {
    pub class: u8,
    pub endianness: u8,
    pub ident_version: u8,
    pub os_abi: u8,
    pub object_type: u16,
    pub machine: u16,
    pub elf_version: u32,
    pub entry: u64,
    pub program_header_offset: u64,
    pub section_header_offset: u64,
    pub header_size: u16,
    pub program_header_size: u16,
    pub program_header_count: u16,
    pub section_header_size: u16,
    pub section_header_count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawProgramHeader {
    pub segment_type: u32,
    pub flags: u32,
    pub file_offset: u64,
    pub virtual_address: u64,
    pub physical_address: u64,
    pub file_size: u64,
    pub memory_size: u64,
    pub alignment: u64,
}

#[derive(Clone, Copy)]
pub struct RawImage<'a> {
    bytes: &'a [u8],
    header: RawHeader,
}

impl<'a> RawImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ValidationError> {
        if bytes.len() < usize::from(ELF_HEADER_SIZE) {
            return Err(ValidationError::HeaderTooSmall);
        }
        let header = RawHeader {
            class: bytes[4],
            endianness: bytes[5],
            ident_version: bytes[6],
            os_abi: bytes[7],
            object_type: read_u16(bytes, 16).ok_or(ValidationError::HeaderTooSmall)?,
            machine: read_u16(bytes, 18).ok_or(ValidationError::HeaderTooSmall)?,
            elf_version: read_u32(bytes, 20).ok_or(ValidationError::HeaderTooSmall)?,
            entry: read_u64(bytes, 24).ok_or(ValidationError::HeaderTooSmall)?,
            program_header_offset: read_u64(bytes, 32).ok_or(ValidationError::HeaderTooSmall)?,
            section_header_offset: read_u64(bytes, 40).ok_or(ValidationError::HeaderTooSmall)?,
            header_size: read_u16(bytes, 52).ok_or(ValidationError::HeaderTooSmall)?,
            program_header_size: read_u16(bytes, 54).ok_or(ValidationError::HeaderTooSmall)?,
            program_header_count: read_u16(bytes, 56).ok_or(ValidationError::HeaderTooSmall)?,
            section_header_size: read_u16(bytes, 58).ok_or(ValidationError::HeaderTooSmall)?,
            section_header_count: read_u16(bytes, 60).ok_or(ValidationError::HeaderTooSmall)?,
        };
        Ok(Self { bytes, header })
    }

    pub const fn header(&self) -> RawHeader {
        self.header
    }

    pub fn program_header(&self, index: u16) -> Option<RawProgramHeader> {
        if index >= self.header.program_header_count {
            return None;
        }
        let relative = u64::from(index).checked_mul(u64::from(self.header.program_header_size))?;
        let offset = self.header.program_header_offset.checked_add(relative)?;
        parse_program_header(self.bytes, offset)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadSegment {
    file_offset: u64,
    file_size: u64,
    memory_size: u64,
    address: u64,
    alignment: u64,
    flags: u32,
}

impl LoadSegment {
    pub const fn file_offset(self) -> u64 {
        self.file_offset
    }

    pub const fn file_size(self) -> u64 {
        self.file_size
    }

    pub const fn memory_size(self) -> u64 {
        self.memory_size
    }

    pub const fn address(self) -> u64 {
        self.address
    }

    pub const fn alignment(self) -> u64 {
        self.alignment
    }

    pub const fn flags(self) -> u32 {
        self.flags
    }

    pub const fn is_readable(self) -> bool {
        self.flags & 4 != 0
    }

    pub const fn is_writable(self) -> bool {
        self.flags & PF_WRITE != 0
    }

    pub const fn is_executable(self) -> bool {
        self.flags & PF_EXECUTE != 0
    }

    pub fn page_count(self, page_size: u64) -> Result<u64, ValidationError> {
        if page_size == 0 || !page_size.is_power_of_two() {
            return Err(ValidationError::InvalidPageSize);
        }
        let end = self
            .address
            .checked_add(self.memory_size)
            .ok_or(ValidationError::SegmentMemoryRangeOverflow)?;
        let first_page = self.address / page_size;
        let last_page = (end - 1) / page_size;
        Ok(last_page - first_page + 1)
    }
}

#[derive(Clone, Copy)]
pub struct ValidatedImage<'a> {
    raw: RawImage<'a>,
    load_count: u16,
    load_start: u64,
    load_end: u64,
}

impl<'a> ValidatedImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ValidationError> {
        validate_magic(bytes)?;
        let raw = RawImage::parse(bytes)?;
        validate_header(raw.header)?;
        validate_program_table(&raw)?;
        validate_section_table(&raw)?;

        let mut load_count = 0_u16;
        let mut load_start = u64::MAX;
        let mut load_end = 0_u64;
        let mut entry_is_executable = false;

        for index in 0..raw.header.program_header_count {
            let program = raw
                .program_header(index)
                .ok_or(ValidationError::ProgramHeaderTableOutsideFile)?;
            match program.segment_type {
                PT_INTERP => return Err(ValidationError::InterpreterSegment),
                PT_DYNAMIC => return Err(ValidationError::DynamicSegment),
                PT_TLS => return Err(ValidationError::TlsSegment),
                PT_LOAD => {
                    let end = validate_load_segment(bytes, program)?;
                    for previous_index in 0..index {
                        let previous = raw
                            .program_header(previous_index)
                            .ok_or(ValidationError::ProgramHeaderTableOutsideFile)?;
                        if previous.segment_type == PT_LOAD {
                            let previous_end = previous
                                .physical_address
                                .checked_add(previous.memory_size)
                                .ok_or(ValidationError::SegmentMemoryRangeOverflow)?;
                            if ranges_overlap(
                                previous.physical_address,
                                previous_end,
                                program.physical_address,
                                end,
                            ) {
                                return Err(ValidationError::OverlappingLoadSegments);
                            }
                        }
                    }
                    load_count += 1;
                    load_start = load_start.min(program.physical_address);
                    load_end = load_end.max(end);
                    if program.flags & PF_EXECUTE != 0
                        && raw.header.entry >= program.virtual_address
                        && raw.header.entry < end
                    {
                        entry_is_executable = true;
                    }
                }
                _ => {}
            }
        }

        if load_count == 0 {
            return Err(ValidationError::NoLoadSegments);
        }
        if !is_canonical(raw.header.entry) || !entry_is_executable {
            return Err(ValidationError::EntryOutsideExecutableSegment);
        }

        Ok(Self {
            raw,
            load_count,
            load_start,
            load_end,
        })
    }

    pub const fn raw_header(&self) -> RawHeader {
        self.raw.header
    }

    pub const fn entry(&self) -> u64 {
        self.raw.header.entry
    }

    pub const fn program_header_count(&self) -> u16 {
        self.raw.header.program_header_count
    }

    pub const fn load_segment_count(&self) -> u16 {
        self.load_count
    }

    pub const fn load_address_range(&self) -> (u64, u64) {
        (self.load_start, self.load_end)
    }

    pub fn load_segments(&self) -> LoadSegments<'a> {
        LoadSegments {
            raw: self.raw,
            next_index: 0,
        }
    }
}

pub fn validate_bootstrap_image(
    bytes: &[u8],
) -> Result<ValidatedImage<'_>, BootstrapValidationError> {
    let image = ValidatedImage::parse(bytes).map_err(BootstrapValidationError::Elf)?;
    if image.entry() != BOOTSTRAP_LINK_ADDRESS {
        return Err(BootstrapValidationError::UnexpectedEntry);
    }
    if image.load_address_range().0 != BOOTSTRAP_LINK_ADDRESS {
        return Err(BootstrapValidationError::UnexpectedLoadStart);
    }

    let mut has_execute = false;
    let mut has_read_only = false;
    let mut has_writable = false;
    let mut has_bss = false;
    for segment in image.load_segments() {
        if segment.alignment() != BOOTSTRAP_PAGE_SIZE
            || segment.address() % BOOTSTRAP_PAGE_SIZE != 0
            || segment.file_offset() % BOOTSTRAP_PAGE_SIZE != 0
        {
            return Err(BootstrapValidationError::MisalignedLoadSegment);
        }
        has_execute |= segment.is_executable();
        has_read_only |=
            segment.is_readable() && !segment.is_writable() && !segment.is_executable();
        has_writable |= segment.is_writable() && !segment.is_executable();
        has_bss |= segment.memory_size() > segment.file_size();
    }
    if !has_execute {
        return Err(BootstrapValidationError::MissingExecuteSegment);
    }
    if !has_read_only {
        return Err(BootstrapValidationError::MissingReadOnlySegment);
    }
    if !has_writable {
        return Err(BootstrapValidationError::MissingWritableSegment);
    }
    if !has_bss {
        return Err(BootstrapValidationError::MissingBss);
    }
    Ok(image)
}

pub struct LoadSegments<'a> {
    raw: RawImage<'a>,
    next_index: u16,
}

impl Iterator for LoadSegments<'_> {
    type Item = LoadSegment;

    fn next(&mut self) -> Option<Self::Item> {
        while self.next_index < self.raw.header.program_header_count {
            let index = self.next_index;
            self.next_index += 1;
            let raw = self.raw.program_header(index)?;
            if raw.segment_type == PT_LOAD {
                return Some(LoadSegment {
                    file_offset: raw.file_offset,
                    file_size: raw.file_size,
                    memory_size: raw.memory_size,
                    address: raw.physical_address,
                    alignment: raw.alignment,
                    flags: raw.flags,
                });
            }
        }
        None
    }
}

fn validate_magic(bytes: &[u8]) -> Result<(), ValidationError> {
    if bytes.len() < 4 {
        return Err(ValidationError::HeaderTooSmall);
    }
    if bytes[..4] != [0x7f, b'E', b'L', b'F'] {
        return Err(ValidationError::BadMagic);
    }
    Ok(())
}

fn validate_header(header: RawHeader) -> Result<(), ValidationError> {
    if header.class != 2 {
        return Err(ValidationError::UnsupportedClass);
    }
    if header.endianness != 1 {
        return Err(ValidationError::UnsupportedEndianness);
    }
    if header.ident_version != 1 {
        return Err(ValidationError::UnsupportedIdentVersion);
    }
    if header.os_abi != 0 {
        return Err(ValidationError::UnsupportedOsAbi);
    }
    if header.object_type != 2 {
        return Err(ValidationError::UnsupportedObjectType);
    }
    if header.machine != 62 {
        return Err(ValidationError::UnsupportedMachine);
    }
    if header.elf_version != 1 {
        return Err(ValidationError::UnsupportedElfVersion);
    }
    if header.header_size != ELF_HEADER_SIZE {
        return Err(ValidationError::InvalidHeaderSize);
    }
    if header.program_header_size != PROGRAM_HEADER_SIZE {
        return Err(ValidationError::InvalidProgramHeaderSize);
    }
    if header.program_header_count > MAX_PROGRAM_HEADERS {
        return Err(ValidationError::TooManyProgramHeaders);
    }
    Ok(())
}

fn validate_program_table(raw: &RawImage<'_>) -> Result<(), ValidationError> {
    let table_size = u64::from(raw.header.program_header_size)
        .checked_mul(u64::from(raw.header.program_header_count))
        .ok_or(ValidationError::ProgramHeaderTableOverflow)?;
    let table_end = raw
        .header
        .program_header_offset
        .checked_add(table_size)
        .ok_or(ValidationError::ProgramHeaderTableOverflow)?;
    if table_end > raw.bytes.len() as u64 {
        return Err(ValidationError::ProgramHeaderTableOutsideFile);
    }
    Ok(())
}

fn validate_section_table(raw: &RawImage<'_>) -> Result<(), ValidationError> {
    if raw.header.section_header_count == 0 {
        return Ok(());
    }
    if raw.header.section_header_size != SECTION_HEADER_SIZE {
        return Err(ValidationError::InvalidSectionHeaderSize);
    }
    let table_size = u64::from(raw.header.section_header_size)
        .checked_mul(u64::from(raw.header.section_header_count))
        .ok_or(ValidationError::SectionHeaderTableOverflow)?;
    let table_end = raw
        .header
        .section_header_offset
        .checked_add(table_size)
        .ok_or(ValidationError::SectionHeaderTableOverflow)?;
    if table_end > raw.bytes.len() as u64 {
        return Err(ValidationError::SectionHeaderTableOutsideFile);
    }
    for index in 0..raw.header.section_header_count {
        let relative = u64::from(index)
            .checked_mul(u64::from(raw.header.section_header_size))
            .ok_or(ValidationError::SectionHeaderTableOverflow)?;
        let offset = raw
            .header
            .section_header_offset
            .checked_add(relative)
            .ok_or(ValidationError::SectionHeaderTableOverflow)?;
        let section_type = read_u32_at(raw.bytes, offset + 4)
            .ok_or(ValidationError::SectionHeaderTableOutsideFile)?;
        let section_size = read_u64_at(raw.bytes, offset + 32)
            .ok_or(ValidationError::SectionHeaderTableOutsideFile)?;
        if (section_type == SHT_REL || section_type == SHT_RELA) && section_size != 0 {
            return Err(ValidationError::RuntimeRelocations);
        }
    }
    Ok(())
}

fn validate_load_segment(bytes: &[u8], program: RawProgramHeader) -> Result<u64, ValidationError> {
    if program.memory_size == 0 {
        return Err(ValidationError::ZeroLengthLoadSegment);
    }
    if program.file_size > program.memory_size {
        return Err(ValidationError::FileSizeExceedsMemorySize);
    }
    let file_end = program
        .file_offset
        .checked_add(program.file_size)
        .ok_or(ValidationError::SegmentFileRangeOverflow)?;
    if file_end > bytes.len() as u64 {
        return Err(ValidationError::SegmentOutsideFile);
    }
    let memory_end = program
        .physical_address
        .checked_add(program.memory_size)
        .ok_or(ValidationError::SegmentMemoryRangeOverflow)?;
    if program.physical_address != program.virtual_address {
        return Err(ValidationError::PhysicalVirtualAddressMismatch);
    }
    if !is_canonical(program.virtual_address) || !is_canonical(memory_end - 1) {
        return Err(ValidationError::NonCanonicalAddress);
    }
    if program.alignment > 1 && !program.alignment.is_power_of_two() {
        return Err(ValidationError::InvalidAlignment);
    }
    if program.alignment > 1
        && program.file_offset % program.alignment != program.virtual_address % program.alignment
    {
        return Err(ValidationError::InvalidOffsetAddressCongruence);
    }
    if program.flags & (PF_WRITE | PF_EXECUTE) == (PF_WRITE | PF_EXECUTE) {
        return Err(ValidationError::WritableExecutableSegment);
    }
    Ok(memory_end)
}

fn parse_program_header(bytes: &[u8], offset: u64) -> Option<RawProgramHeader> {
    Some(RawProgramHeader {
        segment_type: read_u32_at(bytes, offset)?,
        flags: read_u32_at(bytes, offset.checked_add(4)?)?,
        file_offset: read_u64_at(bytes, offset.checked_add(8)?)?,
        virtual_address: read_u64_at(bytes, offset.checked_add(16)?)?,
        physical_address: read_u64_at(bytes, offset.checked_add(24)?)?,
        file_size: read_u64_at(bytes, offset.checked_add(32)?)?,
        memory_size: read_u64_at(bytes, offset.checked_add(40)?)?,
        alignment: read_u64_at(bytes, offset.checked_add(48)?)?,
    })
}

const fn ranges_overlap(a_start: u64, a_end: u64, b_start: u64, b_end: u64) -> bool {
    a_start < b_end && b_start < a_end
}

const fn is_canonical(address: u64) -> bool {
    address <= 0x0000_7fff_ffff_ffff || address >= 0xffff_8000_0000_0000
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    let value = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let value = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let value = bytes.get(offset..offset.checked_add(8)?)?;
    Some(u64::from_le_bytes([
        value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
    ]))
}

fn read_u32_at(bytes: &[u8], offset: u64) -> Option<u32> {
    read_u32(bytes, usize::try_from(offset).ok()?)
}

fn read_u64_at(bytes: &[u8], offset: u64) -> Option<u64> {
    read_u64(bytes, usize::try_from(offset).ok()?)
}
