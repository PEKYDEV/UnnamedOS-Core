use std::process::Command;

const REQUIRED_RUSTC_VERSION: &str = "1.98.0";

pub fn run() -> Result<(), String> {
    check_rustc_version()?;
    run_step("format", "cargo", &["fmt", "--all", "--", "--check"])?;
    run_step(
        "host clippy",
        "cargo",
        &[
            "clippy",
            "-p",
            "boot-protocol",
            "-p",
            "kernel-image",
            "-p",
            "memory-layout",
            "-p",
            "xtask",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run_step(
        "host tests",
        "cargo",
        &[
            "test",
            "-p",
            "boot-protocol",
            "-p",
            "kernel-image",
            "-p",
            "memory-layout",
            "-p",
            "xtask",
        ],
    )?;
    run_step(
        "bootloader host policy clippy",
        "cargo",
        &[
            "clippy",
            "-p",
            "bootloader",
            "--lib",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run_step(
        "bootloader host policy tests",
        "cargo",
        &["test", "-p", "bootloader", "--lib"],
    )?;
    run_step(
        "kernel host validation clippy",
        "cargo",
        &["clippy", "-p", "kernel", "--lib", "--", "-D", "warnings"],
    )?;
    run_step(
        "kernel host validation tests",
        "cargo",
        &["test", "-p", "kernel", "--lib"],
    )?;
    run_step(
        "kernel clippy",
        "cargo",
        &[
            "clippy",
            "-p",
            "kernel",
            "--target",
            "x86_64-unknown-none",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run_step(
        "UEFI loader clippy",
        "cargo",
        &[
            "clippy",
            "-p",
            "bootloader",
            "--target",
            "x86_64-unknown-uefi",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run_step(
        "UEFI qemu-test loader clippy",
        "cargo",
        &[
            "clippy",
            "-p",
            "bootloader",
            "--target",
            "x86_64-unknown-uefi",
            "--features",
            "qemu-test",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run_step(
        "UEFI ExitBootServices loader clippy",
        "cargo",
        &[
            "clippy",
            "-p",
            "bootloader",
            "--target",
            "x86_64-unknown-uefi",
            "--features",
            "qemu-test,exit-boot-services-test",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run_step(
        "UEFI kernel-handoff loader clippy",
        "cargo",
        &[
            "clippy",
            "-p",
            "bootloader",
            "--target",
            "x86_64-unknown-uefi",
            "--features",
            "qemu-test,kernel-handoff-test",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    println!("UnnamedOS host checks passed.");
    Ok(())
}

fn check_rustc_version() -> Result<(), String> {
    println!("==> Rust toolchain");
    let output = Command::new("rustc")
        .arg("--version")
        .output()
        .map_err(|error| format!("could not start `rustc`: {error}"))?;

    if !output.status.success() {
        return Err(format!("`rustc --version` exited with {}", output.status));
    }

    let output = String::from_utf8_lossy(&output.stdout);
    let actual = parse_rustc_version(&output)
        .ok_or_else(|| format!("could not parse `rustc --version` output: {output:?}"))?;
    if actual != REQUIRED_RUSTC_VERSION {
        return Err(format!(
            "Rust {REQUIRED_RUSTC_VERSION} is required, but rustc reports {actual}"
        ));
    }

    println!("Rust {actual}");
    Ok(())
}

fn parse_rustc_version(output: &str) -> Option<&str> {
    let mut fields = output.split_whitespace();
    match (fields.next(), fields.next()) {
        (Some("rustc"), Some(version)) => Some(version),
        _ => None,
    }
}

fn run_step(label: &str, program: &str, args: &[&str]) -> Result<(), String> {
    println!("==> {label}");
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|error| format!("could not start `{program}`: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "`{program} {}` exited with {status}",
            args.join(" ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rustc_release_version() {
        assert_eq!(
            parse_rustc_version("rustc 1.98.0 (88d9e12ae 2026-08-18)"),
            Some("1.98.0")
        );
    }

    #[test]
    fn rejects_unexpected_version_output() {
        assert_eq!(parse_rustc_version("not-rustc 1.98.0"), None);
    }
}
