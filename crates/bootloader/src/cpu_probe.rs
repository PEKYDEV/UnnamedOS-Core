//! Read-only x86-64 CPU probe. Policy remains in `memory-layout`.

#[cfg(not(target_arch = "x86_64"))]
compile_error!("the UnnamedOS production CPU probe requires x86-64");

use core::arch::{
    asm,
    x86_64::{__cpuid, __cpuid_count},
};

use memory_layout::RawCpuSnapshot;

const IA32_EFER: u32 = 0xc000_0080;
const CPUID_MSR: u32 = 1 << 5;
const CPUID_LONG_MODE: u32 = 1 << 29;

pub(crate) const STATE_CAPTURED_MARKER: &[u8] = b"UNOS:P1J:CPU_STATE_CAPTURED";
pub(crate) const CAPABILITIES_VALIDATED_MARKER: &[u8] = b"UNOS:P1J:CPU_CAPABILITIES_VALIDATED";
pub(crate) const REQUIREMENTS_CLASSIFIED_MARKER: &[u8] =
    b"UNOS:P1J:ACTIVATION_REQUIREMENTS_CLASSIFIED";
pub(crate) const HIERARCHY_COMPATIBLE_MARKER: &[u8] = b"UNOS:P1J:HIERARCHY_CPU_COMPATIBLE";
pub(crate) const CR3_UNCHANGED_MARKER: &[u8] = b"UNOS:P1J:CR3_UNCHANGED";
pub(crate) const ACTIVATION_PREPARED_MARKER: &[u8] = b"UNOS:P1J:ACTIVATION_PREPARED_INACTIVE";
#[cfg(feature = "cpu-readiness-failure-test")]
pub(crate) const ROLLBACK_MARKER: &[u8] = b"UNOS:P1J:CPU_ROLLBACK_COMPLETE";

pub(crate) fn capture() -> RawCpuSnapshot {
    // Leaf zero is architecturally available; x86-64 guarantees CPUID support.
    let basic_maximum = __cpuid(0).eax;
    // Extended leaf zero is queried before any optional extended leaf.
    let extended_maximum = __cpuid(0x8000_0000).eax;
    let basic = if basic_maximum >= 1 {
        // The maximum basic leaf proved leaf 1 is supported.
        __cpuid(1)
    } else {
        empty_cpuid()
    };
    let structured = if basic_maximum >= 7 {
        // The maximum basic leaf proved leaf 7 exists; subleaf zero is
        // the discovery subleaf and is always valid for this leaf.
        __cpuid_count(7, 0)
    } else {
        empty_cpuid()
    };
    let extended = if extended_maximum >= 0x8000_0001 {
        // The maximum extended leaf proved this feature leaf exists.
        __cpuid(0x8000_0001)
    } else {
        empty_cpuid()
    };
    let widths = if extended_maximum >= 0x8000_0008 {
        // The maximum extended leaf proved the width leaf exists.
        __cpuid(0x8000_0008)
    } else {
        empty_cpuid()
    };
    let cr0 = read_cr0();
    let cr3 = read_cr3();
    let cr4 = read_cr4();
    let efer = if basic.edx & CPUID_MSR != 0 && extended.edx & CPUID_LONG_MODE != 0 {
        read_efer()
    } else {
        0
    };
    RawCpuSnapshot {
        maximum_basic_leaf: basic_maximum,
        maximum_extended_leaf: extended_maximum,
        basic_feature_edx: basic.edx,
        structured_feature_ecx: structured.ecx,
        extended_feature_edx: extended.edx,
        address_width_eax: widths.eax,
        cr0,
        cr3,
        cr4,
        efer,
    }
}

pub(crate) fn read_cr3() -> u64 {
    let value: u64;
    // SAFETY: reading CR3 is a non-mutating privileged observation in the
    // current UEFI x86-64 execution context; no memory is accessed.
    unsafe { asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

fn read_cr0() -> u64 {
    let value: u64;
    // SAFETY: reading CR0 is non-mutating and the loader runs at firmware
    // privilege; the instruction has no memory or stack effect.
    unsafe { asm!("mov {}, cr0", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

fn read_cr4() -> u64 {
    let value: u64;
    // SAFETY: reading CR4 is non-mutating and the loader runs at firmware
    // privilege; the instruction has no memory or stack effect.
    unsafe { asm!("mov {}, cr4", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

fn read_efer() -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: CPUID has established MSR and long-mode support before this
    // call. IA32_EFER is therefore architecturally present. RDMSR only reads
    // state and the loader executes at firmware privilege.
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") IA32_EFER,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags)
        )
    };
    (u64::from(high) << 32) | u64::from(low)
}

const fn empty_cpuid() -> core::arch::x86_64::CpuidResult {
    core::arch::x86_64::CpuidResult {
        eax: 0,
        ebx: 0,
        ecx: 0,
        edx: 0,
    }
}
