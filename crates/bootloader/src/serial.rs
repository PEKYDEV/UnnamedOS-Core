use core::arch::asm;

const COM1_BASE: u16 = 0x03f8;
const TRANSMITTER_EMPTY: u8 = 0x20;
const TRANSMIT_POLL_LIMIT: u32 = 100_000;

pub struct SerialPort {
    base: u16,
}

impl SerialPort {
    pub const fn com1() -> Self {
        Self { base: COM1_BASE }
    }

    pub fn initialize(&mut self) {
        write_port_u8(self.base + 1, 0x00);
        write_port_u8(self.base + 3, 0x80);
        write_port_u8(self.base, 0x03);
        write_port_u8(self.base + 1, 0x00);
        write_port_u8(self.base + 3, 0x03);
        write_port_u8(self.base + 2, 0xc7);
        write_port_u8(self.base + 4, 0x03);
    }

    pub fn write_line(&mut self, line: &[u8]) {
        for &byte in line {
            self.write_byte(byte);
        }
        self.write_byte(b'\r');
        self.write_byte(b'\n');
    }

    fn write_byte(&mut self, byte: u8) {
        for _ in 0..TRANSMIT_POLL_LIMIT {
            if read_port_u8(self.base + 5) & TRANSMITTER_EMPTY != 0 {
                write_port_u8(self.base, byte);
                return;
            }
            core::hint::spin_loop();
        }
    }
}

fn write_port_u8(port: u16, value: u8) {
    // SAFETY: The loader runs on x86-64 at UEFI application privilege. The
    // caller supplies only the fixed legacy COM1 register range, and `out`
    // touches no Rust-managed memory or stack state.
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

fn read_port_u8(port: u16) -> u8 {
    let value: u8;
    // SAFETY: The loader runs on x86-64 at UEFI application privilege. The
    // caller supplies only the fixed COM1 line-status register, and `in`
    // writes solely to the declared byte output without touching memory.
    unsafe {
        asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
    value
}
