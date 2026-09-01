use core::arch::asm;

use bootloader::validate_handoff_arguments;

use crate::serial::SerialPort;

pub fn jump_to_kernel(
    entry: u64,
    boot_info: u64,
    stack_bottom: u64,
    stack_top: u64,
    serial: &mut SerialPort,
) -> ! {
    if validate_handoff_arguments(entry, boot_info, stack_bottom, stack_top).is_err() {
        serial.write_line(b"UNOS:P1I:FAIL:HANDOFF");
        crate::test_exit::failure();
    }
    serial.write_line(b"UNOS:P1I:HANDOFF_READY");
    // SAFETY: scalar arguments were checked against the documented bootstrap
    // ABI. Boot services are gone, the target lies in the validated executable
    // image, and the transferred stack owner keeps the complete range live.
    unsafe {
        asm!(
            "cli",
            "cld",
            "mov rsp, rcx",
            "xor rbp, rbp",
            "jmp rax",
            in("rax") entry,
            in("rdi") boot_info,
            in("rsi") stack_bottom,
            in("rdx") stack_top,
            in("rcx") stack_top,
            options(noreturn)
        )
    }
}
