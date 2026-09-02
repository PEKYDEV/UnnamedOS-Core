//! Rollback-safe ownership for page-table frames supplied by a backend.

use crate::{ConstructionPlan, FrameAssignments, FrameSlot, PageTablePlanError, PhysicalFrame};

pub trait FrameBackend {
    type Error;

    /// Allocates one frame but does not imply that its contents are initialized.
    fn allocate_frame(&mut self) -> Result<u64, Self::Error>;
    /// Proves complete zero-initialization before the frame may join a hierarchy.
    fn zero_frame(&mut self, frame: PhysicalFrame) -> Result<(), Self::Error>;
    /// Releases exactly one previously returned allocation.
    fn free_frame(&mut self, address: u64) -> Result<(), Self::Error>;
}

#[derive(Debug, Eq, PartialEq)]
pub enum FrameOwnerCause<E> {
    Allocation {
        slot: FrameSlot,
        source: E,
    },
    Zeroing {
        slot: FrameSlot,
        source: E,
    },
    InvalidFrame {
        slot: FrameSlot,
        source: PageTablePlanError,
    },
    DuplicateFrame {
        slot: FrameSlot,
        earlier_slot: FrameSlot,
    },
    PlannedFrameCountExceedsCapacity,
}

#[derive(Debug, Eq, PartialEq)]
pub struct FrameOwnerError<E> {
    pub source: E,
    pub remaining_frames: usize,
}

struct IncompleteFrameOwner<B: FrameBackend, const CAPACITY: usize> {
    backend: Option<B>,
    frames: [u64; CAPACITY],
    len: usize,
}

impl<B: FrameBackend, const CAPACITY: usize> IncompleteFrameOwner<B, CAPACITY> {
    fn new(backend: B) -> Self {
        Self {
            backend: Some(backend),
            frames: [0; CAPACITY],
            len: 0,
        }
    }

    fn push(&mut self, address: u64) {
        self.frames[self.len] = address;
        self.len += 1;
    }

    fn try_release(&mut self) -> Result<(), FrameOwnerError<B::Error>> {
        while self.len != 0 {
            let address = self.frames[self.len - 1];
            if let Err(source) = self
                .backend
                .as_mut()
                .expect("incomplete owner retains its backend")
                .free_frame(address)
            {
                return Err(FrameOwnerError {
                    source,
                    remaining_frames: self.len,
                });
            }
            self.len -= 1;
        }
        Ok(())
    }
}

impl<B: FrameBackend, const CAPACITY: usize> Drop for IncompleteFrameOwner<B, CAPACITY> {
    fn drop(&mut self) {
        while self.len != 0 {
            let address = self.frames[self.len - 1];
            let _ = self
                .backend
                .as_mut()
                .expect("incomplete owner retains its backend")
                .free_frame(address);
            self.len -= 1;
        }
    }
}

pub struct FrameOwnerBuildError<B: FrameBackend, const CAPACITY: usize> {
    cause: FrameOwnerCause<B::Error>,
    owner: IncompleteFrameOwner<B, CAPACITY>,
}

impl<B: FrameBackend, const CAPACITY: usize> FrameOwnerBuildError<B, CAPACITY> {
    pub const fn cause(&self) -> &FrameOwnerCause<B::Error> {
        &self.cause
    }

    pub const fn remaining_frames(&self) -> usize {
        self.owner.len
    }

    pub fn try_release(&mut self) -> Result<(), FrameOwnerError<B::Error>> {
        self.owner.try_release()
    }
}

#[must_use]
pub struct PageTableFrameOwner<B: FrameBackend, const CAPACITY: usize> {
    backend: B,
    frames: [PhysicalFrame; CAPACITY],
    len: usize,
    root: PhysicalFrame,
    owned: bool,
}

impl<B: FrameBackend, const CAPACITY: usize> PageTableFrameOwner<B, CAPACITY> {
    pub fn allocate<const TABLES: usize, const ENTRIES: usize, const REMOVALS: usize>(
        plan: &ConstructionPlan<TABLES, ENTRIES, REMOVALS>,
        backend: B,
    ) -> Result<Self, FrameOwnerBuildError<B, CAPACITY>> {
        let mut partial = IncompleteFrameOwner::new(backend);
        if plan.table_count() > CAPACITY {
            return Err(FrameOwnerBuildError {
                cause: FrameOwnerCause::PlannedFrameCountExceedsCapacity,
                owner: partial,
            });
        }

        for index in 0..plan.table_count() {
            let slot = FrameSlot::from_index(index).expect("plan table count already uses slots");
            let address = match partial
                .backend
                .as_mut()
                .expect("incomplete owner retains its backend")
                .allocate_frame()
            {
                Ok(address) => address,
                Err(source) => {
                    let cause = FrameOwnerCause::Allocation { slot, source };
                    let _ = partial.try_release();
                    return Err(FrameOwnerBuildError {
                        cause,
                        owner: partial,
                    });
                }
            };
            if let Some(earlier) = partial.frames[..partial.len]
                .iter()
                .position(|candidate| *candidate == address)
            {
                let cause = FrameOwnerCause::DuplicateFrame {
                    slot,
                    earlier_slot: FrameSlot::from_index(earlier)
                        .expect("owned frame position fits a frame slot"),
                };
                let _ = partial.try_release();
                return Err(FrameOwnerBuildError {
                    cause,
                    owner: partial,
                });
            }
            let frame = match PhysicalFrame::new(address) {
                Ok(frame) => frame,
                Err(source) => {
                    let cause = FrameOwnerCause::InvalidFrame { slot, source };
                    partial.push(address);
                    let _ = partial.try_release();
                    return Err(FrameOwnerBuildError {
                        cause,
                        owner: partial,
                    });
                }
            };
            partial.push(address);
            if let Err(source) = partial
                .backend
                .as_mut()
                .expect("incomplete owner retains its backend")
                .zero_frame(frame)
            {
                let cause = FrameOwnerCause::Zeroing { slot, source };
                let _ = partial.try_release();
                return Err(FrameOwnerBuildError {
                    cause,
                    owner: partial,
                });
            }
        }

        let mut frames = [PhysicalFrame::EMPTY; CAPACITY];
        for (output, address) in frames.iter_mut().zip(&partial.frames[..partial.len]) {
            *output = PhysicalFrame::new(*address)
                .expect("all successfully acquired frames were validated");
        }
        let root = frames[plan.root_frame_slot().as_index()];
        let owner = Self {
            backend: partial
                .backend
                .take()
                .expect("successful acquisition retains its backend"),
            frames,
            len: partial.len,
            root,
            owned: true,
        };
        partial.len = 0;
        Ok(owner)
    }

    pub const fn frame_count(&self) -> usize {
        self.len
    }

    pub const fn root_frame(&self) -> PhysicalFrame {
        self.root
    }

    pub fn frames(&self) -> &[PhysicalFrame] {
        &self.frames[..self.len]
    }

    pub const fn is_transferred(&self) -> bool {
        !self.owned
    }

    pub const fn assignments(&self) -> FrameAssignments<CAPACITY> {
        FrameAssignments::from_parts(self.frames, self.len)
    }

    pub fn try_release(&mut self) -> Result<(), FrameOwnerError<B::Error>> {
        while self.owned && self.len != 0 {
            let frame = self.frames[self.len - 1];
            if let Err(source) = self.backend.free_frame(frame.address()) {
                return Err(FrameOwnerError {
                    source,
                    remaining_frames: self.len,
                });
            }
            self.len -= 1;
        }
        if self.len == 0 {
            self.owned = false;
        }
        Ok(())
    }

    pub fn transfer(mut self) -> TransferredPageTableFrames<CAPACITY> {
        self.owned = false;
        TransferredPageTableFrames {
            frames: self.frames,
            len: self.len,
            root: self.root,
        }
    }
}

impl<B: FrameBackend, const CAPACITY: usize> Drop for PageTableFrameOwner<B, CAPACITY> {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        while self.len != 0 {
            let frame = self.frames[self.len - 1];
            let _ = self.backend.free_frame(frame.address());
            self.len -= 1;
        }
        self.owned = false;
    }
}

pub struct TransferredPageTableFrames<const CAPACITY: usize> {
    frames: [PhysicalFrame; CAPACITY],
    len: usize,
    root: PhysicalFrame,
}

impl<const CAPACITY: usize> TransferredPageTableFrames<CAPACITY> {
    pub const fn frame_count(&self) -> usize {
        self.len
    }

    pub const fn root_frame(&self) -> PhysicalFrame {
        self.root
    }

    pub fn frames(&self) -> &[PhysicalFrame] {
        &self.frames[..self.len]
    }

    pub const fn assignments(&self) -> FrameAssignments<CAPACITY> {
        FrameAssignments::from_parts(self.frames, self.len)
    }
}
