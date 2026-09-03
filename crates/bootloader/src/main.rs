#![no_main]
#![no_std]

mod boot_info_loader;
#[cfg(feature = "kernel-handoff-test")]
mod handoff;
mod kernel_loader;
#[cfg(feature = "kernel-handoff-test")]
mod page_table_loader;
mod segment_loader;
mod serial;

#[cfg(any(feature = "qemu-test", feature = "exit-boot-services-test"))]
mod test_exit;

use core::panic::PanicInfo;
use uefi::{Status, entry};

const ENTRY_MARKER: &[u8] = b"UNOS:P1C:ENTRY";
const UEFI_OK_MARKER: &[u8] = b"UNOS:P1C:UEFI_OK";
const PASS_MARKER: &[u8] = b"UNOS:P1C:PASS";
const PANIC_MARKER: &[u8] = b"UNOS:P1C:PANIC";
const PHASE_1D_PASS_MARKER: &[u8] = b"UNOS:P1D:PASS";

#[entry]
fn main() -> Status {
    let mut serial = serial::SerialPort::com1();
    serial.initialize();
    serial.write_line(ENTRY_MARKER);
    serial.write_line(UEFI_OK_MARKER);
    serial.write_line(PASS_MARKER);

    let kernel = match kernel_loader::load_and_validate(&mut serial) {
        Ok(kernel) => kernel,
        Err(error) => {
            serial.write_line(error.marker());

            #[cfg(feature = "qemu-test")]
            {
                test_exit::failure()
            }

            #[cfg(not(feature = "qemu-test"))]
            {
                return Status::LOAD_ERROR;
            }
        }
    };
    let _validated_contract = (kernel.entry, kernel.load_segment_count);
    serial.write_line(PHASE_1D_PASS_MARKER);

    if let Err(error) = segment_loader::load_verify_and_release(kernel, &mut serial) {
        serial.write_line(error.marker());

        #[cfg(feature = "qemu-test")]
        {
            test_exit::failure()
        }

        #[cfg(not(feature = "qemu-test"))]
        {
            return Status::LOAD_ERROR;
        }
    }
    #[cfg(feature = "qemu-test")]
    {
        test_exit::success()
    }

    #[cfg(not(feature = "qemu-test"))]
    {
        Status::SUCCESS
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    let mut serial = serial::SerialPort::com1();
    serial.initialize();
    serial.write_line(PANIC_MARKER);

    #[cfg(feature = "qemu-test")]
    {
        test_exit::failure()
    }

    #[cfg(not(feature = "qemu-test"))]
    {
        loop {
            core::hint::spin_loop();
        }
    }
}
