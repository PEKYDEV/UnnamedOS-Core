use core::mem::ManuallyDrop;

use crate::{PageAllocation, PageBackend, UEFI_PAGE_SIZE};

pub const BOOTSTRAP_STACK_PAGES: u64 = 16;
pub const BOOTSTRAP_STACK_SIZE: u64 = BOOTSTRAP_STACK_PAGES * UEFI_PAGE_SIZE;
pub const BOOTSTRAP_IDENTITY_LIMIT: u64 = 0x1_0000_0000;
pub const BOOTSTRAP_STACK_CANARY: u64 = 0x554e_4f53_5354_414b;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapStackError {
    WrongSize,
    Misaligned,
    Overflow,
    OutsideIdentityRange,
    Overlap,
    NotInitialized,
    Released,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransferredBootstrapStack {
    pub bottom: u64,
    pub top: u64,
    pub canary_address: u64,
    pub canary_value: u64,
}

#[must_use]
pub struct BootstrapStack<B: PageBackend> {
    backend: B,
    allocation: PageAllocation,
    owned: bool,
}

impl<B: PageBackend> BootstrapStack<B> {
    pub fn from_initialized(
        mut backend: B,
        allocation: PageAllocation,
        forbidden: &[PhysicalRange],
    ) -> Result<Self, BootstrapStackError> {
        let range = match validate_bootstrap_stack(allocation.page_start, allocation.page_count) {
            Ok(range) => range,
            Err(error) => {
                let _ = backend.free(allocation.page_start, allocation.page_count);
                return Err(error);
            }
        };
        if forbidden.iter().any(|item| ranges_overlap(range, *item)) {
            let _ = backend.free(allocation.page_start, allocation.page_count);
            return Err(BootstrapStackError::Overlap);
        }
        Ok(Self {
            backend,
            allocation,
            owned: true,
        })
    }

    pub const fn allocation(&self) -> PageAllocation {
        self.allocation
    }
    pub const fn bottom(&self) -> u64 {
        self.allocation.page_start
    }
    pub const fn top(&self) -> u64 {
        self.allocation.page_start + BOOTSTRAP_STACK_SIZE
    }
    pub const fn is_released(&self) -> bool {
        !self.owned
    }

    pub fn try_release(&mut self) -> Result<(), B::Error> {
        if self.owned {
            self.backend
                .free(self.allocation.page_start, self.allocation.page_count)?;
            self.owned = false;
        }
        Ok(())
    }

    pub(crate) fn transfer(self) -> TransferredBootstrapStack {
        let owner = ManuallyDrop::new(self);
        TransferredBootstrapStack {
            bottom: owner.bottom(),
            top: owner.top(),
            canary_address: owner.bottom(),
            canary_value: BOOTSTRAP_STACK_CANARY,
        }
    }
}

impl<B: PageBackend> Drop for BootstrapStack<B> {
    fn drop(&mut self) {
        if self.owned
            && self
                .backend
                .free(self.allocation.page_start, self.allocation.page_count)
                .is_ok()
        {
            self.owned = false;
        }
    }
}

pub fn validate_bootstrap_stack(
    start: u64,
    pages: u64,
) -> Result<PhysicalRange, BootstrapStackError> {
    if pages != BOOTSTRAP_STACK_PAGES {
        return Err(BootstrapStackError::WrongSize);
    }
    if !start.is_multiple_of(UEFI_PAGE_SIZE) {
        return Err(BootstrapStackError::Misaligned);
    }
    let end = start
        .checked_add(BOOTSTRAP_STACK_SIZE)
        .ok_or(BootstrapStackError::Overflow)?;
    if end > BOOTSTRAP_IDENTITY_LIMIT {
        return Err(BootstrapStackError::OutsideIdentityRange);
    }
    Ok(PhysicalRange { start, end })
}

pub const fn ranges_overlap(left: PhysicalRange, right: PhysicalRange) -> bool {
    left.start < right.end && right.start < left.end
}

pub fn validate_handoff_arguments(
    entry: u64,
    boot_info: u64,
    bottom: u64,
    top: u64,
) -> Result<(), BootstrapStackError> {
    if entry == 0 || boot_info == 0 || !boot_info.is_multiple_of(8) {
        return Err(BootstrapStackError::Misaligned);
    }
    let range = validate_bootstrap_stack(bottom, BOOTSTRAP_STACK_PAGES)?;
    if range.end != top || !top.is_multiple_of(16) {
        return Err(BootstrapStackError::WrongSize);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::{Reservation, ReservationList, ReservationSource, apply_reservations};
    use boot_protocol::{MEMORY_KIND_BOOTSTRAP_STACK, MEMORY_KIND_USABLE, MemoryDescriptor};
    use std::{cell::RefCell, rc::Rc, vec::Vec};

    #[derive(Clone)]
    struct Backend(Rc<RefCell<Vec<u64>>>);
    impl PageBackend for Backend {
        type Error = ();
        fn allocate_at(&mut self, _: u64, _: u64) -> Result<(), ()> {
            Ok(())
        }
        fn free(&mut self, start: u64, _: u64) -> Result<(), ()> {
            self.0.borrow_mut().push(start);
            Ok(())
        }
    }
    #[test]
    fn size_alignment_overflow_and_identity_limit_are_checked() {
        assert!(validate_bootstrap_stack(0x10000, 16).is_ok());
        assert_eq!(
            validate_bootstrap_stack(1, 16),
            Err(BootstrapStackError::Misaligned)
        );
        assert_eq!(
            validate_bootstrap_stack(0x10000, 15),
            Err(BootstrapStackError::WrongSize)
        );
        assert_eq!(
            validate_bootstrap_stack(u64::MAX & !0xfff, 16),
            Err(BootstrapStackError::Overflow)
        );
        assert_eq!(
            validate_bootstrap_stack(BOOTSTRAP_IDENTITY_LIMIT, 16),
            Err(BootstrapStackError::OutsideIdentityRange)
        );
    }
    #[test]
    fn forbidden_overlap_is_rejected() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let result = BootstrapStack::from_initialized(
            Backend(calls.clone()),
            PageAllocation {
                page_start: 0x10000,
                page_count: 16,
            },
            &[PhysicalRange {
                start: 0x18000,
                end: 0x19000,
            }],
        );
        assert!(matches!(result, Err(BootstrapStackError::Overlap)));
        assert_eq!(&*calls.borrow(), &[0x10000]);
    }
    #[test]
    fn rollback_frees_once_and_transfer_disarms_free() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let stack = BootstrapStack::from_initialized(
            Backend(calls.clone()),
            PageAllocation {
                page_start: 0x10000,
                page_count: 16,
            },
            &[],
        )
        .unwrap();
        drop(stack);
        assert_eq!(&*calls.borrow(), &[0x10000]);
        calls.borrow_mut().clear();
        let stack = BootstrapStack::from_initialized(
            Backend(calls.clone()),
            PageAllocation {
                page_start: 0x10000,
                page_count: 16,
            },
            &[],
        )
        .unwrap();
        let raw = stack.transfer();
        assert_eq!(raw.top - raw.bottom, BOOTSTRAP_STACK_SIZE);
        assert!(calls.borrow().is_empty());
    }
    #[test]
    fn handoff_arguments_reject_bad_pointer_and_bounds() {
        assert!(validate_handoff_arguments(0x200000, 0x3000, 0x10000, 0x20000).is_ok());
        assert!(validate_handoff_arguments(0, 0x3000, 0x10000, 0x20000).is_err());
        assert!(validate_handoff_arguments(0x200000, 3, 0x10000, 0x20000).is_err());
        assert!(validate_handoff_arguments(0x200000, 0x3000, 0x10000, 0x1ffff).is_err());
    }
    #[test]
    fn stack_reservation_is_a_non_usable_overlay() {
        let base = [MemoryDescriptor {
            kind: MEMORY_KIND_USABLE,
            reserved0: 0,
            physical_start: 0x10000,
            page_count: 32,
            attributes: 0,
        }];
        let mut reservations = ReservationList::new();
        reservations
            .push(Reservation {
                physical_start: 0x18000,
                page_count: BOOTSTRAP_STACK_PAGES,
                kind: MEMORY_KIND_BOOTSTRAP_STACK,
                source: ReservationSource::BootstrapStack,
            })
            .unwrap();
        reservations.finish().unwrap();
        let mut output = [base[0]; 4];
        let count = apply_reservations(&base, &reservations, &mut output).unwrap();
        assert_eq!(count, 3);
        assert_eq!(output[1].kind, MEMORY_KIND_BOOTSTRAP_STACK);
        assert_eq!(output[1].page_count, BOOTSTRAP_STACK_PAGES);
    }
}
