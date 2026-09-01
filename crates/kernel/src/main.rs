#![no_main]
#![no_std]

use boot_protocol::{BootInfo, MemoryDescriptor};
use core::{
    arch::{asm, global_asm},
    mem::{align_of, size_of},
    panic::PanicInfo,
};
use kernel::{
    HandoffInputs, PhysicalRange, validate_boot_state, validate_canary, validate_handoff_inputs,
};

const COM1: u16 = 0x3f8;
const DEBUG_EXIT: u16 = 0xf4;
const CANARY: u64 = kernel::STACK_CANARY;

#[used]
#[unsafe(link_section = ".rodata.bootstrap")]
static BOOTSTRAP_CONTRACT: [u8; 15] = *b"UNOS-KERNEL-P1I";
#[used]
#[unsafe(link_section = ".data.bootstrap")]
static mut BOOTSTRAP_DATA: u64 = CANARY;
#[used]
#[unsafe(link_section = ".bss.bootstrap")]
static mut BOOTSTRAP_BSS: [u8; 4096] = [0; 4096];

global_asm!(
    ".section .text._start,\"ax\",@progbits", ".global _start", ".type _start,@function", "_start:",
    "mov rcx, rsp", "call kernel_bootstrap", "mov dx, {debug_port}", "mov al, 0x11", "out dx, al",
    "1: cli", "hlt", "jmp 1b", ".size _start, .-_start", debug_port = const DEBUG_EXIT,
);

unsafe extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}

#[unsafe(no_mangle)]
pub extern "C" fn kernel_bootstrap(
    boot_info_address: u64,
    stack_bottom: u64,
    stack_top: u64,
    entry_rsp: u64,
) -> ! {
    serial_init();
    serial_line(b"UNOS:P1I:KERNEL_ENTRY");
    let inputs = HandoffInputs {
        boot_info_address,
        stack_bottom,
        stack_top,
        entry_rsp,
    };
    if validate_handoff_inputs(inputs).is_err() {
        fail();
    }
    // SAFETY: scalar pointer alignment/range checks completed above. The loader
    // transferred the identity-mapped page and no mutable alias is created.
    let boot_info = unsafe { &*(boot_info_address as *const BootInfo) };
    // SAFETY: the validated stack range is identity mapped and the sentinel is
    // an initialized aligned u64 at its lower boundary.
    if validate_canary(unsafe { (stack_bottom as *const u64).read() }).is_err() {
        fail();
    }
    serial_line(b"UNOS:P1I:STACK_OK");
    if boot_info.validate().is_err() {
        fail();
    }
    serial_line(b"UNOS:P1I:BOOTINFO_OK");
    let count = match usize::try_from(boot_info.memory_map.descriptor_count) {
        Ok(value) => value,
        Err(_) => fail(),
    };
    let map_pointer = boot_info.memory_map.physical_address as *const MemoryDescriptor;
    if map_pointer.is_null()
        || !(map_pointer as usize).is_multiple_of(align_of::<MemoryDescriptor>())
    {
        fail();
    }
    // SAFETY: BootInfo scalar validation proved count*stride and the full map
    // range. Current ABI stride equals MemoryDescriptor size; identity mapping
    // is the explicit reference-platform bootstrap contract.
    let descriptors = unsafe { core::slice::from_raw_parts(map_pointer, count) };
    let kernel_start = core::ptr::addr_of!(__kernel_start) as u64;
    let kernel_end = core::ptr::addr_of!(__kernel_end) as u64;
    let targets = [
        PhysicalRange {
            start: kernel_start,
            end: kernel_end,
        },
        PhysicalRange {
            start: stack_bottom,
            end: stack_top,
        },
        PhysicalRange {
            start: boot_info_address,
            end: match boot_info_address.checked_add(size_of::<BootInfo>() as u64) {
                Some(v) => v,
                None => fail(),
            },
        },
        PhysicalRange {
            start: boot_info.memory_map.physical_address,
            end: match boot_info
                .memory_map
                .physical_address
                .checked_add(boot_info.memory_map.byte_length)
            {
                Some(v) => v,
                None => fail(),
            },
        },
    ];
    if validate_boot_state(boot_info, descriptors, &targets).is_err() {
        fail();
    }
    serial_line(b"UNOS:P1I:MEMORY_MAP_OK");
    serial_line(b"UNOS:P1I:PASS");
    debug_exit(0x10)
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    fail()
}
fn fail() -> ! {
    serial_line(b"UNOS:P1I:FAIL");
    debug_exit(0x11)
}
fn serial_init() {
    for (offset, value) in [
        (1, 0x00),
        (3, 0x80),
        (0, 0x03),
        (1, 0x00),
        (3, 0x03),
        (2, 0xc7),
        (4, 0x03),
    ] {
        out8(COM1 + offset, value);
    }
}
fn serial_line(bytes: &[u8]) {
    for &byte in bytes {
        serial_byte(byte);
    }
    serial_byte(b'\r');
    serial_byte(b'\n');
}
fn serial_byte(byte: u8) {
    for _ in 0..100_000 {
        if in8(COM1 + 5) & 0x20 != 0 {
            out8(COM1, byte);
            return;
        }
        core::hint::spin_loop();
    }
}
fn debug_exit(code: u8) -> ! {
    out8(DEBUG_EXIT, code);
    loop {
        unsafe {
            asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}
fn out8(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}
fn in8(port: u16) -> u8 {
    let value;
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}
