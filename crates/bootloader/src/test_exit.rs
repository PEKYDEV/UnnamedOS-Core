use core::arch::asm;

const DEBUG_EXIT_PORT: u16 = 0x00f4;
const SUCCESS_CODE: u32 = 0x10;
const FAILURE_CODE: u32 = 0x11;

pub fn success() -> ! {
    exit(SUCCESS_CODE)
}

pub fn failure() -> ! {
    exit(FAILURE_CODE)
}

fn exit(code: u32) -> ! {
    // SAFETY: This module is compiled only with the `qemu-test` feature. The
    // xtask runner attaches `isa-debug-exit` at the same fixed 0xF4 port. The
    // `out` instruction changes only that test device and no Rust memory.
    unsafe {
        asm!(
            "out dx, eax",
            in("dx") DEBUG_EXIT_PORT,
            in("eax") code,
            options(nomem, nostack, preserves_flags)
        );
    }

    loop {
        core::hint::spin_loop();
    }
}
