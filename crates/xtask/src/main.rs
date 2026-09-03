mod check;
mod doctor;
mod kernel;
mod sha256;
mod uefi;

use std::env;
use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <check|doctor|build-uefi|run-uefi|test-uefi|test-exit-boot-services|test-kernel-handoff|test-page-tables|build-kernel|inspect-kernel|build-boot>";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let command = args.next();

    if args.next().is_some() {
        eprintln!("{USAGE}");
        return ExitCode::from(64);
    }

    match command.as_deref() {
        Some("check") => match check::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("check failed: {error}");
                ExitCode::FAILURE
            }
        },
        Some("doctor") => match doctor::run() {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(2),
            Err(error) => {
                eprintln!("doctor failed: {error}");
                ExitCode::FAILURE
            }
        },
        Some("build-uefi") => command_result(uefi::build_uefi()),
        Some("run-uefi") => command_result(uefi::run_uefi()),
        Some("test-uefi") => command_result(uefi::test_uefi()),
        Some("test-exit-boot-services") => command_result(uefi::test_exit_boot_services()),
        Some("test-kernel-handoff") => command_result(uefi::test_kernel_handoff()),
        Some("test-page-tables") => command_result(uefi::test_page_tables()),
        Some("build-kernel") => command_result(kernel::build_kernel()),
        Some("inspect-kernel") => command_result(kernel::inspect_kernel()),
        Some("build-boot") => command_result(kernel::build_boot()),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(64)
        }
    }
}

fn command_result(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
