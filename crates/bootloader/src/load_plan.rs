use kernel_image::{MAX_PROGRAM_HEADERS, ValidatedImage};

pub const BOOTSTRAP_WINDOW_START: u64 = 0x0020_0000;
pub const BOOTSTRAP_WINDOW_END: u64 = 0x0420_0000;
pub const BOOTSTRAP_WINDOW_BYTES: u64 = 64 * 1024 * 1024;
pub const LOAD_PAGE_SIZE: u64 = 4096;
pub const MAX_LOAD_ITEMS: usize = MAX_PROGRAM_HEADERS as usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SourceAllocation {
    pub page_start: u64,
    pub page_count: u64,
    pub file_length: u64,
}

impl SourceAllocation {
    pub fn page_end(self) -> Result<u64, PlanError> {
        let bytes = self
            .page_count
            .checked_mul(LOAD_PAGE_SIZE)
            .ok_or(PlanError::RangeOverflow)?;
        self.page_start
            .checked_add(bytes)
            .ok_or(PlanError::RangeOverflow)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SegmentSpec {
    pub file_offset: u64,
    pub file_size: u64,
    pub memory_size: u64,
    pub target: u64,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadItem {
    pub source_offset: u64,
    pub file_size: u64,
    pub memory_size: u64,
    pub target_start: u64,
    pub target_end: u64,
    pub page_start: u64,
    pub page_count: u64,
    pub flags: u32,
    pub bss_start: u64,
    pub bss_length: u64,
    pub zero_offset: u64,
    pub zero_length: u64,
    pub copy_offset: u64,
    pub padding_start: u64,
    pub padding_length: u64,
}

impl LoadItem {
    const EMPTY: Self = Self {
        source_offset: 0,
        file_size: 0,
        memory_size: 0,
        target_start: 0,
        target_end: 0,
        page_start: 0,
        page_count: 0,
        flags: 0,
        bss_start: 0,
        bss_length: 0,
        zero_offset: 0,
        zero_length: 0,
        copy_offset: 0,
        padding_start: 0,
        padding_length: 0,
    };

    pub const fn page_length(self) -> u64 {
        self.zero_length
    }

    pub const fn is_executable(self) -> bool {
        self.flags & 1 != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanError {
    NoSegments,
    TooManySegments,
    ZeroMemorySize,
    UnalignedTarget,
    SourceRangeOverflow,
    RangeOverflow,
    PageRoundingOverflow,
    SourceOutsideFile,
    BelowWindow,
    AboveWindow,
    TotalSizeOverflow,
    TotalSizeExceeded,
    RoundedOverlap,
    SourceOverlap,
    EntryOutsideExecutableSegment,
}

pub struct LoadPlan {
    items: [LoadItem; MAX_LOAD_ITEMS],
    len: usize,
    total_page_bytes: u64,
    entry: u64,
}

impl LoadPlan {
    pub fn from_validated(
        image: &ValidatedImage<'_>,
        source: SourceAllocation,
    ) -> Result<Self, PlanError> {
        let segments = image.load_segments().map(|segment| SegmentSpec {
            file_offset: segment.file_offset(),
            file_size: segment.file_size(),
            memory_size: segment.memory_size(),
            target: segment.address(),
            flags: segment.flags(),
        });
        Self::build(image.entry(), segments, source)
    }

    pub fn build(
        entry: u64,
        segments: impl IntoIterator<Item = SegmentSpec>,
        source: SourceAllocation,
    ) -> Result<Self, PlanError> {
        let source_page_end = source.page_end()?;
        let mut plan = Self {
            items: [LoadItem::EMPTY; MAX_LOAD_ITEMS],
            len: 0,
            total_page_bytes: 0,
            entry,
        };

        for segment in segments {
            if plan.len == MAX_LOAD_ITEMS {
                return Err(PlanError::TooManySegments);
            }
            let item = make_item(segment, source.file_length)?;
            if item.page_start < BOOTSTRAP_WINDOW_START {
                return Err(PlanError::BelowWindow);
            }
            let page_end = item
                .page_start
                .checked_add(item.page_length())
                .ok_or(PlanError::RangeOverflow)?;
            if page_end > BOOTSTRAP_WINDOW_END {
                return Err(PlanError::AboveWindow);
            }
            for previous in &plan.items[..plan.len] {
                let previous_end = previous
                    .page_start
                    .checked_add(previous.page_length())
                    .ok_or(PlanError::RangeOverflow)?;
                if ranges_overlap(previous.page_start, previous_end, item.page_start, page_end) {
                    return Err(PlanError::RoundedOverlap);
                }
            }
            if ranges_overlap(
                source.page_start,
                source_page_end,
                item.page_start,
                page_end,
            ) {
                return Err(PlanError::SourceOverlap);
            }
            plan.total_page_bytes = plan
                .total_page_bytes
                .checked_add(item.page_length())
                .ok_or(PlanError::TotalSizeOverflow)?;
            if plan.total_page_bytes > BOOTSTRAP_WINDOW_BYTES {
                return Err(PlanError::TotalSizeExceeded);
            }
            plan.items[plan.len] = item;
            plan.len += 1;
        }

        if plan.len == 0 {
            return Err(PlanError::NoSegments);
        }
        if !plan.items().any(|item| {
            item.is_executable() && entry >= item.target_start && entry < item.target_end
        }) {
            return Err(PlanError::EntryOutsideExecutableSegment);
        }
        Ok(plan)
    }

    pub fn items(&self) -> impl Iterator<Item = LoadItem> + '_ {
        self.items[..self.len].iter().copied()
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn total_page_bytes(&self) -> u64 {
        self.total_page_bytes
    }

    pub const fn entry(&self) -> u64 {
        self.entry
    }
}

fn make_item(segment: SegmentSpec, source_length: u64) -> Result<LoadItem, PlanError> {
    if segment.memory_size == 0 {
        return Err(PlanError::ZeroMemorySize);
    }
    if !segment.target.is_multiple_of(LOAD_PAGE_SIZE) {
        return Err(PlanError::UnalignedTarget);
    }
    let source_end = segment
        .file_offset
        .checked_add(segment.file_size)
        .ok_or(PlanError::SourceRangeOverflow)?;
    if source_end > source_length {
        return Err(PlanError::SourceOutsideFile);
    }
    let target_end = segment
        .target
        .checked_add(segment.memory_size)
        .ok_or(PlanError::RangeOverflow)?;
    let page_start = round_down(segment.target, LOAD_PAGE_SIZE);
    let page_end = round_up(target_end, LOAD_PAGE_SIZE)?;
    let page_length = page_end
        .checked_sub(page_start)
        .ok_or(PlanError::RangeOverflow)?;
    let page_count = page_length / LOAD_PAGE_SIZE;
    if page_count == 0 {
        return Err(PlanError::ZeroMemorySize);
    }
    let bss_start = segment
        .target
        .checked_add(segment.file_size)
        .ok_or(PlanError::RangeOverflow)?;
    let bss_length = segment
        .memory_size
        .checked_sub(segment.file_size)
        .ok_or(PlanError::RangeOverflow)?;
    let copy_offset = segment
        .target
        .checked_sub(page_start)
        .ok_or(PlanError::RangeOverflow)?;
    let padding_length = page_end
        .checked_sub(target_end)
        .ok_or(PlanError::RangeOverflow)?;

    Ok(LoadItem {
        source_offset: segment.file_offset,
        file_size: segment.file_size,
        memory_size: segment.memory_size,
        target_start: segment.target,
        target_end,
        page_start,
        page_count,
        flags: segment.flags,
        bss_start,
        bss_length,
        zero_offset: 0,
        zero_length: page_length,
        copy_offset,
        padding_start: target_end,
        padding_length,
    })
}

pub fn round_down(value: u64, alignment: u64) -> u64 {
    value & !(alignment - 1)
}

pub fn round_up(value: u64, alignment: u64) -> Result<u64, PlanError> {
    value
        .checked_add(alignment - 1)
        .map(|rounded| rounded & !(alignment - 1))
        .ok_or(PlanError::PageRoundingOverflow)
}

fn ranges_overlap(left_start: u64, left_end: u64, right_start: u64, right_end: u64) -> bool {
    left_start < right_end && right_start < left_end
}

pub trait PageBackend {
    type Error;

    fn allocate_at(&mut self, page_start: u64, page_count: u64) -> Result<(), Self::Error>;
    fn free(&mut self, page_start: u64, page_count: u64) -> Result<(), Self::Error>;
}

pub trait SegmentBackend: PageBackend {
    fn zero(&mut self, item: LoadItem) -> Result<(), Self::Error>;
    fn copy(&mut self, item: LoadItem, source: &[u8]) -> Result<(), Self::Error>;
    fn verify(&mut self, item: LoadItem, source: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryError {
    Range,
    CopyMismatch,
    NonZeroBss,
    NonZeroPadding,
}

pub fn initialize_target(
    item: LoadItem,
    source: &[u8],
    target: &mut [u8],
) -> Result<(), MemoryError> {
    target.fill(0);
    copy_target(item, source, target)
}

pub fn copy_target(item: LoadItem, source: &[u8], target: &mut [u8]) -> Result<(), MemoryError> {
    let page_length = usize::try_from(item.page_length()).map_err(|_| MemoryError::Range)?;
    if target.len() != page_length {
        return Err(MemoryError::Range);
    }
    let source_start = usize::try_from(item.source_offset).map_err(|_| MemoryError::Range)?;
    let file_size = usize::try_from(item.file_size).map_err(|_| MemoryError::Range)?;
    let source_end = source_start
        .checked_add(file_size)
        .ok_or(MemoryError::Range)?;
    let copy_start = usize::try_from(item.copy_offset).map_err(|_| MemoryError::Range)?;
    let copy_end = copy_start
        .checked_add(file_size)
        .ok_or(MemoryError::Range)?;
    let source_bytes = source
        .get(source_start..source_end)
        .ok_or(MemoryError::Range)?;
    let target_bytes = target
        .get_mut(copy_start..copy_end)
        .ok_or(MemoryError::Range)?;
    target_bytes.copy_from_slice(source_bytes);
    Ok(())
}

pub fn verify_target(item: LoadItem, source: &[u8], target: &[u8]) -> Result<(), MemoryError> {
    let page_length = usize::try_from(item.page_length()).map_err(|_| MemoryError::Range)?;
    if target.len() != page_length {
        return Err(MemoryError::Range);
    }
    let source_start = usize::try_from(item.source_offset).map_err(|_| MemoryError::Range)?;
    let file_size = usize::try_from(item.file_size).map_err(|_| MemoryError::Range)?;
    let memory_size = usize::try_from(item.memory_size).map_err(|_| MemoryError::Range)?;
    let copy_start = usize::try_from(item.copy_offset).map_err(|_| MemoryError::Range)?;
    let source_end = source_start
        .checked_add(file_size)
        .ok_or(MemoryError::Range)?;
    let copy_end = copy_start
        .checked_add(file_size)
        .ok_or(MemoryError::Range)?;
    let memory_end = copy_start
        .checked_add(memory_size)
        .ok_or(MemoryError::Range)?;
    let source_bytes = source
        .get(source_start..source_end)
        .ok_or(MemoryError::Range)?;
    let copied = target.get(copy_start..copy_end).ok_or(MemoryError::Range)?;
    if copied != source_bytes {
        return Err(MemoryError::CopyMismatch);
    }
    if !target
        .get(copy_end..memory_end)
        .ok_or(MemoryError::Range)?
        .iter()
        .all(|byte| *byte == 0)
    {
        return Err(MemoryError::NonZeroBss);
    }
    if !target
        .get(memory_end..page_length)
        .ok_or(MemoryError::Range)?
        .iter()
        .all(|byte| *byte == 0)
    {
        return Err(MemoryError::NonZeroPadding);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadStage {
    Allocated,
    Zeroed,
    Copied,
    Verified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadWorkKind {
    Allocate,
    Zero,
    Copy,
    Verify,
}

#[must_use]
pub struct LoadWorkFailure<B: PageBackend> {
    kind: LoadWorkKind,
    error: B::Error,
    owner: TargetOwnership<B>,
}

impl<B: PageBackend> LoadWorkFailure<B> {
    pub const fn kind(&self) -> LoadWorkKind {
        self.kind
    }

    pub const fn error(&self) -> &B::Error {
        &self.error
    }

    pub fn try_release(&mut self) -> Result<(), ReleaseError<B::Error>> {
        self.owner.try_release()
    }
}

// The error retains the compact, allocator-free owner so partial allocations
// remain tracked and retryable. Boxing is unavailable in this no_std path.
#[allow(clippy::result_large_err)]
pub fn prepare_targets<B: SegmentBackend>(
    plan: &LoadPlan,
    source: &[u8],
    backend: B,
    mut stage: impl FnMut(LoadStage),
) -> Result<VerifiedTargets<B>, LoadWorkFailure<B>> {
    let mut owner = TargetOwnership::new(plan, backend);
    for index in 0..owner.len {
        let item = owner.items[index];
        if let Err(error) = owner.backend.allocate_at(item.page_start, item.page_count) {
            return Err(LoadWorkFailure {
                kind: LoadWorkKind::Allocate,
                error,
                owner,
            });
        }
        owner.owned[index] = true;
    }
    stage(LoadStage::Allocated);

    for item in plan.items() {
        if let Err(error) = owner.backend.zero(item) {
            return Err(LoadWorkFailure {
                kind: LoadWorkKind::Zero,
                error,
                owner,
            });
        }
    }
    stage(LoadStage::Zeroed);

    for item in plan.items() {
        if let Err(error) = owner.backend.copy(item, source) {
            return Err(LoadWorkFailure {
                kind: LoadWorkKind::Copy,
                error,
                owner,
            });
        }
    }
    stage(LoadStage::Copied);

    for item in plan.items() {
        if let Err(error) = owner.backend.verify(item, source) {
            return Err(LoadWorkFailure {
                kind: LoadWorkKind::Verify,
                error,
                owner,
            });
        }
    }
    stage(LoadStage::Verified);
    Ok(VerifiedTargets { owner: Some(owner) })
}

#[derive(Debug, Eq, PartialEq)]
pub struct ReleaseError<E> {
    pub segment_index: usize,
    pub remaining_segments: usize,
    pub source: E,
}

#[derive(Clone, Copy)]
struct OwnedAllocation {
    page_start: u64,
    page_count: u64,
}

impl OwnedAllocation {
    const EMPTY: Self = Self {
        page_start: 0,
        page_count: 0,
    };
}

struct TargetOwnership<B: PageBackend> {
    backend: B,
    items: [OwnedAllocation; MAX_LOAD_ITEMS],
    owned: [bool; MAX_LOAD_ITEMS],
    len: usize,
}

impl<B: PageBackend> TargetOwnership<B> {
    fn new(plan: &LoadPlan, backend: B) -> Self {
        let mut owner = Self {
            backend,
            items: [OwnedAllocation::EMPTY; MAX_LOAD_ITEMS],
            owned: [false; MAX_LOAD_ITEMS],
            len: plan.len,
        };
        for (index, item) in plan.items().enumerate() {
            owner.items[index] = OwnedAllocation {
                page_start: item.page_start,
                page_count: item.page_count,
            };
        }
        owner
    }

    fn try_release(&mut self) -> Result<(), ReleaseError<B::Error>> {
        for index in (0..self.len).rev() {
            if self.owned[index] {
                let item = self.items[index];
                if let Err(source) = self.backend.free(item.page_start, item.page_count) {
                    return Err(ReleaseError {
                        segment_index: index,
                        remaining_segments: self.remaining_segments(),
                        source,
                    });
                }
                self.owned[index] = false;
            }
        }
        Ok(())
    }

    fn remaining_segments(&self) -> usize {
        self.owned[..self.len]
            .iter()
            .filter(|owned| **owned)
            .count()
    }

    fn drop_release(&mut self) {
        for index in (0..self.len).rev() {
            if self.owned[index] {
                let item = self.items[index];
                if self.backend.free(item.page_start, item.page_count).is_ok() {
                    self.owned[index] = false;
                }
            }
        }
    }
}

impl<B: PageBackend> Drop for TargetOwnership<B> {
    fn drop(&mut self) {
        // Best effort only: Drop cannot report firmware release failures. The
        // normal path must call `try_release` and handle its error explicitly.
        self.drop_release();
    }
}

#[must_use]
pub struct VerifiedTargets<B: PageBackend> {
    owner: Option<TargetOwnership<B>>,
}

impl<B: PageBackend> VerifiedTargets<B> {
    pub fn into_loaded_kernel(&mut self, plan: &LoadPlan) -> Option<LoadedKernel<B>> {
        self.owner
            .take()
            .map(|owner| LoadedKernel::from_verified(plan, owner))
    }

    pub const fn is_empty(&self) -> bool {
        self.owner.is_none()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SegmentMetadata {
    pub memory_start: u64,
    pub memory_end: u64,
    pub allocation_start: u64,
    pub allocation_end: u64,
    pub page_count: u64,
    pub flags: u32,
    pub file_size: u64,
    pub memory_size: u64,
}

impl SegmentMetadata {
    const EMPTY: Self = Self {
        memory_start: 0,
        memory_end: 0,
        allocation_start: 0,
        allocation_end: 0,
        page_count: 0,
        flags: 0,
        file_size: 0,
        memory_size: 0,
    };
}

#[must_use]
pub struct LoadedKernel<B: PageBackend> {
    entry_point: u64,
    load_range: LoadRange,
    segments: [SegmentMetadata; MAX_LOAD_ITEMS],
    segment_count: usize,
    total_pages: u64,
    entry_segment_index: usize,
    owner: TargetOwnership<B>,
}

impl<B: PageBackend> LoadedKernel<B> {
    fn from_verified(plan: &LoadPlan, owner: TargetOwnership<B>) -> Self {
        let mut segments = [SegmentMetadata::EMPTY; MAX_LOAD_ITEMS];
        let mut load_start = u64::MAX;
        let mut load_end = 0;
        let mut total_pages = 0;
        let mut entry_segment_index = 0;
        for (index, item) in plan.items().enumerate() {
            let allocation_end = item.page_start + item.page_length();
            segments[index] = SegmentMetadata {
                memory_start: item.target_start,
                memory_end: item.target_end,
                allocation_start: item.page_start,
                allocation_end,
                page_count: item.page_count,
                flags: item.flags,
                file_size: item.file_size,
                memory_size: item.memory_size,
            };
            load_start = load_start.min(item.page_start);
            load_end = load_end.max(allocation_end);
            total_pages += item.page_count;
            if item.is_executable()
                && plan.entry() >= item.target_start
                && plan.entry() < item.target_end
            {
                entry_segment_index = index;
            }
        }
        Self {
            entry_point: plan.entry(),
            load_range: LoadRange {
                start: load_start,
                end: load_end,
            },
            segments,
            segment_count: plan.len(),
            total_pages,
            entry_segment_index,
            owner,
        }
    }

    pub const fn entry_point(&self) -> u64 {
        self.entry_point
    }
    pub const fn load_range(&self) -> LoadRange {
        self.load_range
    }
    pub const fn segment_count(&self) -> usize {
        self.segment_count
    }
    pub fn segment_metadata(&self) -> impl Iterator<Item = SegmentMetadata> + '_ {
        self.segments[..self.segment_count].iter().copied()
    }
    pub const fn owned_page_count(&self) -> u64 {
        self.total_pages
    }
    pub const fn executable_entry_segment_index(&self) -> usize {
        self.entry_segment_index
    }
    pub fn remaining_segment_count(&self) -> usize {
        self.owner.remaining_segments()
    }
    pub fn is_released(&self) -> bool {
        self.owner.remaining_segments() == 0
    }
    pub fn try_release(&mut self) -> Result<(), ReleaseError<B::Error>> {
        self.owner.try_release()
    }
}

pub trait AddressProbe {
    type Error;

    fn allocate_one_at(&mut self, page_start: u64) -> Result<u64, Self::Error>;
    fn free_one(&mut self, page_start: u64) -> Result<(), Self::Error>;
}

#[derive(Debug, Eq, PartialEq)]
pub enum OwnershipProbeError<E> {
    UnexpectedAvailability,
    Cleanup(E),
}

pub fn prove_page_owned<P: AddressProbe>(
    probe: &mut P,
    page_start: u64,
) -> Result<(), OwnershipProbeError<P::Error>> {
    match probe.allocate_one_at(page_start) {
        Err(_) => Ok(()),
        Ok(allocated) => {
            probe
                .free_one(allocated)
                .map_err(OwnershipProbeError::Cleanup)?;
            Err(OwnershipProbeError::UnexpectedAvailability)
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum ReleaseProbeError<E> {
    Allocate(E),
    AddressMismatch,
    Free(E),
}

pub fn prove_page_released<P: AddressProbe>(
    probe: &mut P,
    page_start: u64,
) -> Result<(), ReleaseProbeError<P::Error>> {
    let allocated = probe
        .allocate_one_at(page_start)
        .map_err(ReleaseProbeError::Allocate)?;
    if allocated != page_start {
        probe.free_one(allocated).map_err(ReleaseProbeError::Free)?;
        return Err(ReleaseProbeError::AddressMismatch);
    }
    probe.free_one(allocated).map_err(ReleaseProbeError::Free)
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    const SOURCE: SourceAllocation = SourceAllocation {
        page_start: 0x1000_0000,
        page_count: 4,
        file_length: 0x4000,
    };

    fn segment(
        target: u64,
        file_offset: u64,
        file_size: u64,
        memory_size: u64,
        flags: u32,
    ) -> SegmentSpec {
        SegmentSpec {
            file_offset,
            file_size,
            memory_size,
            target,
            flags,
        }
    }

    fn valid_specs() -> [SegmentSpec; 3] {
        [
            segment(0x200000, 0x1000, 0x15, 0x15, 5),
            segment(0x201000, 0x2000, 0x1c, 0x1c, 4),
            segment(0x202000, 0x3000, 8, 0x2000, 6),
        ]
    }

    #[test]
    fn valid_three_segment_plan_is_deterministic() {
        let plan = LoadPlan::build(0x200000, valid_specs(), SOURCE).expect("plan");
        let items: [LoadItem; 3] = plan.items().collect::<heapless_array::Array<_, 3>>().into();
        assert_eq!(plan.len(), 3);
        assert_eq!(items[0].source_offset, 0x1000);
        assert_eq!(items[2].page_count, 2);
        assert_eq!(items[2].bss_length, 0x1ff8);
        assert_eq!(items[2].padding_length, 0);
        assert_eq!(plan.total_page_bytes(), 0x4000);
        assert_eq!(plan.entry(), 0x200000);
    }

    #[test]
    fn page_rounding_and_overflow_are_explicit() {
        assert_eq!(round_down(0x201234, 4096), 0x201000);
        assert_eq!(round_up(0x201234, 4096), Ok(0x202000));
        assert_eq!(
            round_up(u64::MAX, 4096),
            Err(PlanError::PageRoundingOverflow)
        );
    }

    #[test]
    fn window_boundaries_are_enforced() {
        let low = [segment(BOOTSTRAP_WINDOW_START, 0, 1, 1, 5)];
        assert!(LoadPlan::build(BOOTSTRAP_WINDOW_START, low, SOURCE).is_ok());
        let high = [segment(BOOTSTRAP_WINDOW_END - 4096, 0, 1, 4096, 5)];
        assert!(LoadPlan::build(BOOTSTRAP_WINDOW_END - 4096, high, SOURCE).is_ok());
        let below = [segment(BOOTSTRAP_WINDOW_START - 4096, 0, 1, 1, 5)];
        assert_eq!(
            LoadPlan::build(BOOTSTRAP_WINDOW_START - 4096, below, SOURCE).err(),
            Some(PlanError::BelowWindow)
        );
        let above = [segment(BOOTSTRAP_WINDOW_END, 0, 1, 1, 5)];
        assert_eq!(
            LoadPlan::build(BOOTSTRAP_WINDOW_END, above, SOURCE).err(),
            Some(PlanError::AboveWindow)
        );
    }

    #[test]
    fn rejects_unaligned_rounded_overlap_and_source_overlap() {
        let unaligned = [segment(0x200001, 0, 1, 1, 5)];
        assert_eq!(
            LoadPlan::build(0x200001, unaligned, SOURCE).err(),
            Some(PlanError::UnalignedTarget)
        );

        let overlapping = [
            segment(0x200000, 0, 1, 0x1800, 5),
            segment(0x201000, 0x1000, 1, 1, 4),
        ];
        assert_eq!(
            LoadPlan::build(0x200000, overlapping, SOURCE).err(),
            Some(PlanError::RoundedOverlap)
        );

        let first_raw = (0x200000, 0x200801);
        let second_raw = (0x200900, 0x200a00);
        assert!(!ranges_overlap(
            first_raw.0,
            first_raw.1,
            second_raw.0,
            second_raw.1
        ));
        assert!(ranges_overlap(
            round_down(first_raw.0, 4096),
            round_up(first_raw.1, 4096).expect("first rounded end"),
            round_down(second_raw.0, 4096),
            round_up(second_raw.1, 4096).expect("second rounded end"),
        ));

        let source = SourceAllocation {
            page_start: 0x200000,
            page_count: 1,
            file_length: 1,
        };
        assert_eq!(
            LoadPlan::build(0x200000, [segment(0x200000, 0, 1, 1, 5)], source).err(),
            Some(PlanError::SourceOverlap)
        );
    }

    #[test]
    fn total_limit_and_entry_policy_are_enforced() {
        let exact = [segment(
            BOOTSTRAP_WINDOW_START,
            0,
            1,
            BOOTSTRAP_WINDOW_BYTES,
            5,
        )];
        assert_eq!(
            LoadPlan::build(BOOTSTRAP_WINDOW_START, exact, SOURCE)
                .expect("exact window")
                .total_page_bytes(),
            BOOTSTRAP_WINDOW_BYTES
        );
        let too_large = [segment(0x200000, 0, 1, BOOTSTRAP_WINDOW_BYTES + 1, 5)];
        assert!(matches!(
            LoadPlan::build(
                0x200000,
                too_large,
                SourceAllocation {
                    file_length: 1,
                    ..SOURCE
                }
            ),
            Err(PlanError::AboveWindow) | Err(PlanError::TotalSizeExceeded)
        ));
        let no_exec_entry = [segment(0x200000, 0, 1, 1, 4)];
        assert_eq!(
            LoadPlan::build(0x200000, no_exec_entry, SOURCE).err(),
            Some(PlanError::EntryOutsideExecutableSegment)
        );
    }

    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Clone, Default)]
    struct FakeBackend {
        calls: Rc<RefCell<std::vec::Vec<(&'static str, u64)>>>,
        fail_allocate_at: Option<usize>,
        allocations: usize,
        fail_stage: Option<&'static str>,
        fail_free_at: Option<usize>,
        free_attempts: usize,
    }

    impl PageBackend for FakeBackend {
        type Error = &'static str;

        fn allocate_at(&mut self, start: u64, _pages: u64) -> Result<(), Self::Error> {
            let index = self.allocations;
            self.allocations += 1;
            self.calls.borrow_mut().push(("alloc", start));
            if self.fail_allocate_at == Some(index) {
                Err("alloc")
            } else {
                Ok(())
            }
        }

        fn free(&mut self, start: u64, _pages: u64) -> Result<(), Self::Error> {
            let attempt = self.free_attempts;
            self.free_attempts += 1;
            self.calls.borrow_mut().push(("free", start));
            if self.fail_free_at == Some(attempt) {
                Err("free")
            } else {
                Ok(())
            }
        }
    }

    impl SegmentBackend for FakeBackend {
        fn zero(&mut self, start: LoadItem) -> Result<(), Self::Error> {
            self.calls.borrow_mut().push(("zero", start.page_start));
            if self.fail_stage == Some("zero") {
                Err("zero")
            } else {
                Ok(())
            }
        }

        fn copy(&mut self, item: LoadItem, _source: &[u8]) -> Result<(), Self::Error> {
            self.calls.borrow_mut().push(("copy", item.page_start));
            if self.fail_stage == Some("copy") {
                Err("copy")
            } else {
                Ok(())
            }
        }

        fn verify(&mut self, item: LoadItem, _source: &[u8]) -> Result<(), Self::Error> {
            self.calls.borrow_mut().push(("verify", item.page_start));
            if self.fail_stage == Some("verify") {
                Err("verify")
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn partial_allocation_rolls_back_in_reverse_order() {
        let plan = LoadPlan::build(0x200000, valid_specs(), SOURCE).expect("plan");
        let backend = FakeBackend {
            fail_allocate_at: Some(2),
            ..FakeBackend::default()
        };
        let calls = backend.calls.clone();
        let mut failure = match prepare_targets(&plan, &[0_u8; 0x4000], backend, |_| {}) {
            Ok(_) => panic!("allocation must fail"),
            Err(failure) => failure,
        };
        assert_eq!(failure.kind(), LoadWorkKind::Allocate);
        assert_eq!(failure.error(), &"alloc");
        failure.try_release().expect("rollback");
        assert_eq!(
            *calls.borrow(),
            [
                ("alloc", 0x200000),
                ("alloc", 0x201000),
                ("alloc", 0x202000),
                ("free", 0x201000),
                ("free", 0x200000)
            ]
        );
    }

    #[test]
    fn allocation_failure_preserves_failed_rollback_for_retry() {
        let plan = LoadPlan::build(0x200000, valid_specs(), SOURCE).expect("plan");
        let backend = FakeBackend {
            fail_allocate_at: Some(1),
            fail_free_at: Some(0),
            ..FakeBackend::default()
        };
        let mut failure = match prepare_targets(&plan, &[0_u8; 0x4000], backend, |_| {}) {
            Ok(_) => panic!("allocation must fail"),
            Err(failure) => failure,
        };
        let release = failure.try_release().expect_err("first free fails");
        assert_eq!(release.segment_index, 0);
        assert_eq!(release.remaining_segments, 1);
        failure.try_release().expect("retry succeeds");
    }

    #[test]
    fn loaded_kernel_metadata_transfer_and_release_are_exact() {
        let plan = LoadPlan::build(0x200000, valid_specs(), SOURCE).expect("plan");
        let backend = FakeBackend::default();
        let calls = backend.calls.clone();
        let source = std::vec![0_u8; 0x4000];
        let mut verified = match prepare_targets(&plan, &source, backend, |_| {}) {
            Ok(verified) => verified,
            Err(_) => panic!("prepared"),
        };
        let mut loaded = verified.into_loaded_kernel(&plan).expect("first transfer");
        assert!(verified.is_empty());
        assert!(verified.into_loaded_kernel(&plan).is_none());
        drop(source);
        assert_eq!(loaded.entry_point(), 0x200000);
        assert_eq!(
            loaded.load_range(),
            LoadRange {
                start: 0x200000,
                end: 0x204000
            }
        );
        assert_eq!(loaded.segment_count(), 3);
        assert_eq!(loaded.owned_page_count(), 4);
        assert_eq!(loaded.executable_entry_segment_index(), 0);
        let metadata: std::vec::Vec<_> = loaded.segment_metadata().collect();
        assert_eq!(metadata.len(), 3);
        assert_eq!(metadata[2].memory_start, 0x202000);
        assert_eq!(metadata[2].allocation_end, 0x204000);
        assert_eq!(metadata[2].page_count, 2);
        assert!(!loaded.is_released());
        loaded.try_release().expect("released");
        assert!(loaded.is_released());
        assert_eq!(loaded.remaining_segment_count(), 0);
        loaded.try_release().expect("empty release is idempotent");
        assert_eq!(
            &calls.borrow()[12..],
            &[("free", 0x202000), ("free", 0x201000), ("free", 0x200000)]
        );
    }

    #[test]
    fn partial_release_retains_ownership_and_retry_does_not_double_free() {
        let plan = LoadPlan::build(0x200000, valid_specs(), SOURCE).expect("plan");
        let backend = FakeBackend {
            fail_free_at: Some(1),
            ..FakeBackend::default()
        };
        let calls = backend.calls.clone();
        let mut verified = prepare_targets(&plan, &[0_u8; 0x4000], backend, |_| {})
            .unwrap_or_else(|_| panic!("prepared"));
        let mut loaded = verified.into_loaded_kernel(&plan).expect("first transfer");
        let error = loaded.try_release().expect_err("second reverse free fails");
        assert_eq!(error.segment_index, 1);
        assert_eq!(error.remaining_segments, 2);
        assert_eq!(loaded.remaining_segment_count(), 2);
        loaded.try_release().expect("retry");
        assert!(loaded.is_released());
        let frees: std::vec::Vec<_> = calls
            .borrow()
            .iter()
            .copied()
            .filter(|(kind, _)| *kind == "free")
            .collect();
        assert_eq!(
            frees,
            [
                ("free", 0x202000),
                ("free", 0x201000),
                ("free", 0x201000),
                ("free", 0x200000)
            ]
        );
    }

    #[test]
    fn drop_is_a_reverse_order_best_effort_fallback() {
        let plan = LoadPlan::build(0x200000, valid_specs(), SOURCE).expect("plan");
        let backend = FakeBackend::default();
        let calls = backend.calls.clone();
        let mut verified = prepare_targets(&plan, &[0_u8; 0x4000], backend, |_| {})
            .unwrap_or_else(|_| panic!("prepared"));
        let loaded = verified.into_loaded_kernel(&plan).expect("first transfer");
        drop(loaded);
        let frees: std::vec::Vec<_> = calls
            .borrow()
            .iter()
            .copied()
            .filter(|(kind, _)| *kind == "free")
            .collect();
        assert_eq!(
            frees,
            [("free", 0x202000), ("free", 0x201000), ("free", 0x200000)]
        );
    }

    #[test]
    fn exact_copy_bss_and_padding_are_verified() {
        let spec = segment(0x200000, 1, 3, 6, 5);
        let plan = LoadPlan::build(0x200000, [spec], SOURCE).expect("plan");
        let item = plan.items().next().expect("item");
        let source = [9, 1, 2, 3, 8];
        let mut target = [0xaa_u8; 4096];
        initialize_target(item, &source, &mut target).expect("initialize");
        assert_eq!(&target[..3], &[1, 2, 3]);
        assert!(target[3..6].iter().all(|byte| *byte == 0));
        assert!(target[6..].iter().all(|byte| *byte == 0));
        verify_target(item, &source, &target).expect("verify");

        target[0] ^= 1;
        assert_eq!(
            verify_target(item, &source, &target),
            Err(MemoryError::CopyMismatch)
        );
        target[0] ^= 1;
        target[4] = 1;
        assert_eq!(
            verify_target(item, &source, &target),
            Err(MemoryError::NonZeroBss)
        );
        target[4] = 0;
        target[100] = 1;
        assert_eq!(
            verify_target(item, &source, &target),
            Err(MemoryError::NonZeroPadding)
        );
    }

    #[test]
    fn copy_and_verify_failures_cleanup_in_reverse_order() {
        let plan = LoadPlan::build(0x200000, valid_specs(), SOURCE).expect("plan");
        for (stage, expected) in [
            ("copy", LoadWorkKind::Copy),
            ("verify", LoadWorkKind::Verify),
        ] {
            let backend = FakeBackend {
                fail_stage: Some(stage),
                ..FakeBackend::default()
            };
            let calls = backend.calls.clone();
            let mut error = match prepare_targets(&plan, &[0_u8; 0x4000], backend, |_| {}) {
                Ok(_) => panic!("failure expected"),
                Err(error) => error,
            };
            assert_eq!(error.kind(), expected);
            error.try_release().expect("cleanup");
            assert_eq!(
                &calls.borrow()[calls.borrow().len() - 3..],
                &[("free", 0x202000), ("free", 0x201000), ("free", 0x200000)]
            );
        }
    }

    #[test]
    fn stage_machine_reports_only_completed_stages_and_free_failure_wins() {
        let plan = LoadPlan::build(0x200000, valid_specs(), SOURCE).expect("plan");
        let mut stages = std::vec::Vec::new();
        let backend = FakeBackend::default();
        let mut verified =
            prepare_targets(&plan, &[0_u8; 0x4000], backend, |stage| stages.push(stage))
                .unwrap_or_else(|_| panic!("prepared"));
        assert_eq!(
            stages,
            [
                LoadStage::Allocated,
                LoadStage::Zeroed,
                LoadStage::Copied,
                LoadStage::Verified
            ]
        );
        let mut loaded = verified.into_loaded_kernel(&plan).expect("first transfer");
        loaded.try_release().expect("release");

        let failing = FakeBackend {
            fail_stage: Some("copy"),
            fail_free_at: Some(0),
            ..FakeBackend::default()
        };
        let mut failure = match prepare_targets(&plan, &[0_u8; 0x4000], failing, |_| {}) {
            Ok(_) => panic!("failure expected"),
            Err(failure) => failure,
        };
        assert_eq!(failure.kind(), LoadWorkKind::Copy);
        assert_eq!(
            failure.try_release().expect_err("free failure").source,
            "free"
        );
        failure.try_release().expect("retry cleanup");
    }

    struct FakeProbe {
        allocation: Result<u64, &'static str>,
        free: Result<(), &'static str>,
        freed: std::vec::Vec<u64>,
    }

    impl AddressProbe for FakeProbe {
        type Error = &'static str;

        fn allocate_one_at(&mut self, _page_start: u64) -> Result<u64, Self::Error> {
            self.allocation
        }

        fn free_one(&mut self, page_start: u64) -> Result<(), Self::Error> {
            self.freed.push(page_start);
            self.free
        }
    }

    #[test]
    fn ownership_probe_accepts_unavailable_and_cleans_unexpected_allocation() {
        let mut owned = FakeProbe {
            allocation: Err("occupied"),
            free: Ok(()),
            freed: std::vec::Vec::new(),
        };
        assert_eq!(prove_page_owned(&mut owned, 0x200000), Ok(()));

        let mut available = FakeProbe {
            allocation: Ok(0x200000),
            free: Ok(()),
            freed: std::vec::Vec::new(),
        };
        assert_eq!(
            prove_page_owned(&mut available, 0x200000),
            Err(OwnershipProbeError::UnexpectedAvailability)
        );
        assert_eq!(available.freed, [0x200000]);

        available.free = Err("cleanup");
        assert_eq!(
            prove_page_owned(&mut available, 0x200000),
            Err(OwnershipProbeError::Cleanup("cleanup"))
        );
    }

    #[test]
    fn release_probe_covers_allocate_address_and_free_failures() {
        let mut released = FakeProbe {
            allocation: Ok(0x200000),
            free: Ok(()),
            freed: std::vec::Vec::new(),
        };
        assert_eq!(prove_page_released(&mut released, 0x200000), Ok(()));
        assert_eq!(released.freed, [0x200000]);

        let mut unavailable = FakeProbe {
            allocation: Err("allocate"),
            free: Ok(()),
            freed: std::vec::Vec::new(),
        };
        assert_eq!(
            prove_page_released(&mut unavailable, 0x200000),
            Err(ReleaseProbeError::Allocate("allocate"))
        );

        let mut mismatch = FakeProbe {
            allocation: Ok(0x201000),
            free: Ok(()),
            freed: std::vec::Vec::new(),
        };
        assert_eq!(
            prove_page_released(&mut mismatch, 0x200000),
            Err(ReleaseProbeError::AddressMismatch)
        );
        assert_eq!(mismatch.freed, [0x201000]);

        let mut free_failure = FakeProbe {
            allocation: Ok(0x200000),
            free: Err("free"),
            freed: std::vec::Vec::new(),
        };
        assert_eq!(
            prove_page_released(&mut free_failure, 0x200000),
            Err(ReleaseProbeError::Free("free"))
        );
    }

    // Small fixed-capacity collector used only in tests, without a dependency.
    mod heapless_array {
        pub struct Array<T, const N: usize> {
            items: [Option<T>; N],
            len: usize,
        }

        impl<T, const N: usize> FromIterator<T> for Array<T, N> {
            fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
                let mut items = core::array::from_fn(|_| None);
                let mut len = 0;
                for item in iter {
                    items[len] = Some(item);
                    len += 1;
                }
                Self { items, len }
            }
        }

        impl<T, const N: usize> From<Array<T, N>> for [T; N] {
            fn from(mut value: Array<T, N>) -> Self {
                assert_eq!(value.len, N);
                core::array::from_fn(|index| value.items[index].take().expect("filled"))
            }
        }
    }
}
