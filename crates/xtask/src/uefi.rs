use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::{doctor, kernel};

const UEFI_TARGET: &str = "x86_64-unknown-uefi";
const DEBUG_EXIT_PORT: &str = "0xf4";
const QEMU_SUCCESS_EXIT_CODE: i32 = 33;
const QEMU_FAILURE_EXIT_CODE: i32 = 35;
const HEADLESS_TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(test)]
const P1C_MARKERS: [&str; 3] = ["UNOS:P1C:ENTRY", "UNOS:P1C:UEFI_OK", "UNOS:P1C:PASS"];
const VALID_MARKERS: [&str; 30] = [
    "UNOS:P1C:ENTRY",
    "UNOS:P1C:UEFI_OK",
    "UNOS:P1C:PASS",
    "UNOS:P1D:KERNEL_OPEN",
    "UNOS:P1D:KERNEL_READ",
    "UNOS:P1D:KERNEL_VALID",
    "UNOS:P1D:PASS",
    "UNOS:P1E:PLAN_VALID",
    "UNOS:P1E:SEGMENTS_ALLOCATED",
    "UNOS:P1E:SEGMENTS_ZEROED",
    "UNOS:P1E:SEGMENTS_COPIED",
    "UNOS:P1E:LOAD_VERIFIED",
    "UNOS:P1F:OWNERSHIP_READY",
    "UNOS:P1F:METADATA_VALID",
    "UNOS:P1F:SOURCE_RELEASED",
    "UNOS:P1F:OWNERSHIP_PROVEN",
    "UNOS:P1G:GOP_READY",
    "UNOS:P1G:BUFFERS_READY",
    "UNOS:P1G:MAP_CAPTURED",
    "UNOS:P1G:MAP_CONVERTED",
    "UNOS:P1G:RESERVATIONS_VALID",
    "UNOS:P1G:BOOTINFO_VALID",
    "UNOS:P1G:OWNERSHIP_READY",
    "UNOS:P1G:MEMORY_RELEASED",
    "UNOS:P1G:PASS",
    "UNOS:P1E:MEMORY_RELEASED",
    "UNOS:P1E:PASS",
    "UNOS:P1F:MEMORY_RELEASED",
    "UNOS:P1F:RELEASE_PROVEN",
    "UNOS:P1F:PASS",
];
const EXIT_BOOT_SERVICES_MARKERS: [&str; 29] = [
    "UNOS:P1C:ENTRY",
    "UNOS:P1C:UEFI_OK",
    "UNOS:P1C:PASS",
    "UNOS:P1D:KERNEL_OPEN",
    "UNOS:P1D:KERNEL_READ",
    "UNOS:P1D:KERNEL_VALID",
    "UNOS:P1D:PASS",
    "UNOS:P1E:PLAN_VALID",
    "UNOS:P1E:SEGMENTS_ALLOCATED",
    "UNOS:P1E:SEGMENTS_ZEROED",
    "UNOS:P1E:SEGMENTS_COPIED",
    "UNOS:P1E:LOAD_VERIFIED",
    "UNOS:P1F:OWNERSHIP_READY",
    "UNOS:P1F:METADATA_VALID",
    "UNOS:P1F:SOURCE_RELEASED",
    "UNOS:P1F:OWNERSHIP_PROVEN",
    "UNOS:P1G:GOP_READY",
    "UNOS:P1G:BUFFERS_READY",
    "UNOS:P1G:MAP_CAPTURED",
    "UNOS:P1G:MAP_CONVERTED",
    "UNOS:P1G:RESERVATIONS_VALID",
    "UNOS:P1G:BOOTINFO_VALID",
    "UNOS:P1G:OWNERSHIP_READY",
    "UNOS:P1H:EXIT_READY",
    "UNOS:P1H:BOOT_SERVICES_EXITED",
    "UNOS:P1H:FINAL_MAP_CONVERTED",
    "UNOS:P1H:BOOTINFO_FINAL",
    "UNOS:P1H:OWNERSHIP_TRANSFERRED",
    "UNOS:P1H:PASS",
];
const KERNEL_HANDOFF_MARKERS: [&str; 41] = [
    "UNOS:P1C:ENTRY",
    "UNOS:P1C:UEFI_OK",
    "UNOS:P1C:PASS",
    "UNOS:P1D:KERNEL_OPEN",
    "UNOS:P1D:KERNEL_READ",
    "UNOS:P1D:KERNEL_VALID",
    "UNOS:P1D:PASS",
    "UNOS:P1E:PLAN_VALID",
    "UNOS:P1E:SEGMENTS_ALLOCATED",
    "UNOS:P1E:SEGMENTS_ZEROED",
    "UNOS:P1E:SEGMENTS_COPIED",
    "UNOS:P1E:LOAD_VERIFIED",
    "UNOS:P1F:OWNERSHIP_READY",
    "UNOS:P1F:METADATA_VALID",
    "UNOS:P1F:SOURCE_RELEASED",
    "UNOS:P1F:OWNERSHIP_PROVEN",
    "UNOS:P1J:PLAN_ACCEPTED",
    "UNOS:P1J:FRAMES_ALLOCATED",
    "UNOS:P1J:HIERARCHY_MATERIALIZED",
    "UNOS:P1J:HIERARCHY_VERIFIED",
    "UNOS:P1G:GOP_READY",
    "UNOS:P1G:BUFFERS_READY",
    "UNOS:P1G:MAP_CAPTURED",
    "UNOS:P1G:MAP_CONVERTED",
    "UNOS:P1G:RESERVATIONS_VALID",
    "UNOS:P1G:BOOTINFO_VALID",
    "UNOS:P1G:OWNERSHIP_READY",
    "UNOS:P1H:EXIT_READY",
    "UNOS:P1H:BOOT_SERVICES_EXITED",
    "UNOS:P1H:FINAL_MAP_CONVERTED",
    "UNOS:P1J:FINAL_MAP_RESERVED",
    "UNOS:P1H:BOOTINFO_FINAL",
    "UNOS:P1H:OWNERSHIP_TRANSFERRED",
    "UNOS:P1J:OWNERSHIP_TRANSFERRED",
    "UNOS:P1H:PASS",
    "UNOS:P1I:HANDOFF_READY",
    "UNOS:P1I:KERNEL_ENTRY",
    "UNOS:P1I:STACK_OK",
    "UNOS:P1I:BOOTINFO_OK",
    "UNOS:P1I:MEMORY_MAP_OK",
    "UNOS:P1I:PASS",
];
const PAGE_TABLE_ALLOCATION_FAILURE_MARKERS: [&str; 19] = [
    "UNOS:P1C:ENTRY",
    "UNOS:P1C:UEFI_OK",
    "UNOS:P1C:PASS",
    "UNOS:P1D:KERNEL_OPEN",
    "UNOS:P1D:KERNEL_READ",
    "UNOS:P1D:KERNEL_VALID",
    "UNOS:P1D:PASS",
    "UNOS:P1E:PLAN_VALID",
    "UNOS:P1E:SEGMENTS_ALLOCATED",
    "UNOS:P1E:SEGMENTS_ZEROED",
    "UNOS:P1E:SEGMENTS_COPIED",
    "UNOS:P1E:LOAD_VERIFIED",
    "UNOS:P1F:OWNERSHIP_READY",
    "UNOS:P1F:METADATA_VALID",
    "UNOS:P1F:SOURCE_RELEASED",
    "UNOS:P1F:OWNERSHIP_PROVEN",
    "UNOS:P1J:PLAN_ACCEPTED",
    "UNOS:P1J:ROLLBACK_COMPLETE",
    "UNOS:P1J:FAIL:ALLOC",
];
const MISSING_MARKERS: [&str; 4] = [
    "UNOS:P1C:ENTRY",
    "UNOS:P1C:UEFI_OK",
    "UNOS:P1C:PASS",
    "UNOS:P1D:FAIL:OPEN",
];
const CORRUPT_MARKERS: [&str; 6] = [
    "UNOS:P1C:ENTRY",
    "UNOS:P1C:UEFI_OK",
    "UNOS:P1C:PASS",
    "UNOS:P1D:KERNEL_OPEN",
    "UNOS:P1D:KERNEL_READ",
    "UNOS:P1D:FAIL:ELF",
];
const POLICY_MARKERS: [&str; 8] = [
    "UNOS:P1C:ENTRY",
    "UNOS:P1C:UEFI_OK",
    "UNOS:P1C:PASS",
    "UNOS:P1D:KERNEL_OPEN",
    "UNOS:P1D:KERNEL_READ",
    "UNOS:P1D:KERNEL_VALID",
    "UNOS:P1D:PASS",
    "UNOS:P1E:FAIL:PLAN",
];

pub fn build_uefi() -> Result<(), String> {
    let paths = build_and_stage(BuildMode::Normal)?;
    println!("UEFI loader staged: {}", paths.esp_boot.display());
    Ok(())
}

pub(crate) fn build_uefi_for_boot() -> Result<PathBuf, String> {
    build_and_stage(BuildMode::Normal).map(|paths| paths.esp_boot)
}

pub fn run_uefi() -> Result<(), String> {
    kernel::build_kernel_for_uefi_test()?;
    let build = build_and_stage(BuildMode::Normal)?;
    let environment = doctor::resolve_phase1_paths()?;
    let run = prepare_run(&build.output_root, &environment.ovmf_vars_template)?;
    let config = QemuConfig {
        ovmf_code: &environment.ovmf_code,
        ovmf_vars: &run.vars_copy,
        esp: &build.esp_root,
        serial_log: None,
        qemu_test: false,
    };
    let args = qemu_arguments(&config);

    println!("Starting interactive QEMU; close QEMU to end the command.");
    let status = Command::new(&environment.qemu)
        .args(&args)
        .current_dir(repository_root())
        .status()
        .map_err(|error| format!("QEMU startup failure: {error}"))?;
    verify_vars_source_unchanged(&run)?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "QEMU exited with {status}; no boot success was inferred"
        ))
    }
}

pub fn test_uefi() -> Result<(), String> {
    let kernel = kernel::build_kernel_for_uefi_test()?;
    let build = build_and_stage(BuildMode::QemuTest)?;
    let environment = doctor::resolve_phase1_paths()?;
    let test_root = build.output_root.join("test-uefi");
    remove_directory_if_present(&test_root)?;

    for scenario in Scenario::ALL {
        let fixture = prepare_fixture(&test_root, scenario, &build.esp_boot, &kernel)?;
        let run = prepare_run(&fixture.root, &environment.ovmf_vars_template)?;
        execute_scenario(scenario, &fixture, &run, &environment)?;
    }

    println!("UEFI staged-kernel scenarios passed: valid, missing, corrupt, policy.");
    println!("QEMU exit codes: valid={QEMU_SUCCESS_EXIT_CODE}, failures={QEMU_FAILURE_EXIT_CODE}");
    println!(
        "Timeout per scenario: {} seconds",
        HEADLESS_TIMEOUT.as_secs()
    );
    Ok(())
}

pub fn test_exit_boot_services() -> Result<(), String> {
    let kernel = kernel::build_kernel_for_uefi_test()?;
    let build = build_and_stage(BuildMode::ExitBootServices)?;
    let environment = doctor::resolve_phase1_paths()?;
    let test_root = build.output_root.join("test-exit-boot-services");
    remove_directory_if_present(&test_root)?;
    let fixture = prepare_fixture(&test_root, Scenario::Valid, &build.esp_boot, &kernel)?;
    let run = prepare_run(&fixture.root, &environment.ovmf_vars_template)?;
    let config = QemuConfig {
        ovmf_code: &environment.ovmf_code,
        ovmf_vars: &run.vars_copy,
        esp: &fixture.esp,
        serial_log: Some(&run.serial_log),
        qemu_test: true,
    };
    let child = Command::new(&environment.qemu)
        .args(qemu_arguments(&config))
        .current_dir(repository_root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("QEMU startup failure: {error}"))?;
    let mut child = ProcessChild(child);
    let outcome = wait_for_child(&mut child, HEADLESS_TIMEOUT)?;
    verify_vars_source_unchanged(&run)?;
    let serial = fs::read_to_string(&run.serial_log)
        .map_err(|error| format!("post-exit serial log is unavailable: {error}"))?;
    if let Some(error) = startup_failure(outcome, &serial) {
        return Err(error);
    }
    classify_test_result(
        outcome,
        QEMU_SUCCESS_EXIT_CODE,
        validate_markers(&serial, &EXIT_BOOT_SERVICES_MARKERS),
    )?;
    println!(
        "scenario.exit-boot-services=passed; markers={}; exit={QEMU_SUCCESS_EXIT_CODE}",
        EXIT_BOOT_SERVICES_MARKERS.join(" -> ")
    );
    println!("Timeout: {} seconds", HEADLESS_TIMEOUT.as_secs());
    Ok(())
}

pub fn test_kernel_handoff() -> Result<(), String> {
    let kernel_path = kernel::build_kernel_for_uefi_test()?;
    let build = build_and_stage(BuildMode::KernelHandoff)?;
    kernel::audit_handoff_artifacts(&kernel_path, &build.esp_boot)?;
    let environment = doctor::resolve_phase1_paths()?;
    let test_root = build.output_root.join("test-kernel-handoff");
    remove_directory_if_present(&test_root)?;
    let fixture = prepare_fixture(&test_root, Scenario::Valid, &build.esp_boot, &kernel_path)?;
    let run = prepare_run(&fixture.root, &environment.ovmf_vars_template)?;
    let config = QemuConfig {
        ovmf_code: &environment.ovmf_code,
        ovmf_vars: &run.vars_copy,
        esp: &fixture.esp,
        serial_log: Some(&run.serial_log),
        qemu_test: true,
    };
    let child = Command::new(&environment.qemu)
        .args(qemu_arguments(&config))
        .current_dir(repository_root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("QEMU startup failure: {error}"))?;
    let mut child = ProcessChild(child);
    let outcome = wait_for_child(&mut child, HEADLESS_TIMEOUT)?;
    verify_vars_source_unchanged(&run)?;
    let serial = fs::read_to_string(&run.serial_log)
        .map_err(|error| format!("handoff serial log is unavailable: {error}"))?;
    if let Some(error) = startup_failure(outcome, &serial) {
        return Err(error);
    }
    classify_test_result(
        outcome,
        QEMU_SUCCESS_EXIT_CODE,
        validate_markers(&serial, &KERNEL_HANDOFF_MARKERS),
    )?;
    println!(
        "scenario.kernel-handoff=passed; markers={}; exit={QEMU_SUCCESS_EXIT_CODE}",
        KERNEL_HANDOFF_MARKERS.join(" -> ")
    );
    println!("Timeout: {} seconds", HEADLESS_TIMEOUT.as_secs());
    Ok(())
}

pub fn test_page_tables() -> Result<(), String> {
    let kernel_path = kernel::build_kernel_for_uefi_test()?;
    let build = build_and_stage(BuildMode::PageTableAllocationFailure)?;
    let environment = doctor::resolve_phase1_paths()?;
    let test_root = build.output_root.join("test-page-tables");
    remove_directory_if_present(&test_root)?;
    let fixture = prepare_fixture(&test_root, Scenario::Valid, &build.esp_boot, &kernel_path)?;
    let run = prepare_run(&fixture.root, &environment.ovmf_vars_template)?;
    let config = QemuConfig {
        ovmf_code: &environment.ovmf_code,
        ovmf_vars: &run.vars_copy,
        esp: &fixture.esp,
        serial_log: Some(&run.serial_log),
        qemu_test: true,
    };
    let child = Command::new(&environment.qemu)
        .args(qemu_arguments(&config))
        .current_dir(repository_root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("QEMU startup failure: {error}"))?;
    let mut child = ProcessChild(child);
    let outcome = wait_for_child(&mut child, HEADLESS_TIMEOUT)?;
    verify_vars_source_unchanged(&run)?;
    let serial = fs::read_to_string(&run.serial_log)
        .map_err(|error| format!("page-table serial log is unavailable: {error}"))?;
    if let Some(error) = startup_failure(outcome, &serial) {
        return Err(error);
    }
    classify_test_result(
        outcome,
        QEMU_FAILURE_EXIT_CODE,
        validate_markers(&serial, &PAGE_TABLE_ALLOCATION_FAILURE_MARKERS),
    )?;
    println!(
        "scenario.page-table-allocation-failure=passed; markers={}; exit={QEMU_FAILURE_EXIT_CODE}",
        PAGE_TABLE_ALLOCATION_FAILURE_MARKERS.join(" -> ")
    );
    println!("Timeout: {} seconds", HEADLESS_TIMEOUT.as_secs());
    Ok(())
}

fn execute_scenario(
    scenario: Scenario,
    fixture: &Fixture,
    run: &RunState,
    environment: &doctor::Phase1Paths,
) -> Result<(), String> {
    let config = QemuConfig {
        ovmf_code: &environment.ovmf_code,
        ovmf_vars: &run.vars_copy,
        esp: &fixture.esp,
        serial_log: Some(&run.serial_log),
        qemu_test: true,
    };
    let args = qemu_arguments(&config);

    let child = Command::new(&environment.qemu)
        .args(&args)
        .current_dir(repository_root())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("QEMU startup failure: {error}"))?;
    let mut child = ProcessChild(child);
    let outcome = wait_for_child(&mut child, HEADLESS_TIMEOUT)?;
    verify_vars_source_unchanged(run)?;

    let serial = fs::read_to_string(&run.serial_log).map_err(|error| {
        format!(
            "serial log is unavailable at {}: {error}",
            run.serial_log.display()
        )
    })?;
    if let Some(error) = startup_failure(outcome, &serial) {
        return Err(error);
    }
    let marker_result = validate_markers(&serial, scenario.expected_markers());
    classify_test_result(outcome, scenario.expected_exit_code(), marker_result)?;

    println!(
        "scenario.{}=passed; markers={}; exit={}",
        scenario.name(),
        scenario.expected_markers().join(" -> "),
        scenario.expected_exit_code()
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scenario {
    Valid,
    Missing,
    Corrupt,
    Policy,
}

impl Scenario {
    const ALL: [Self; 4] = [Self::Valid, Self::Missing, Self::Corrupt, Self::Policy];

    const fn name(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Missing => "missing",
            Self::Corrupt => "corrupt",
            Self::Policy => "policy",
        }
    }

    const fn expected_markers(self) -> &'static [&'static str] {
        match self {
            Self::Valid => &VALID_MARKERS,
            Self::Missing => &MISSING_MARKERS,
            Self::Corrupt => &CORRUPT_MARKERS,
            Self::Policy => &POLICY_MARKERS,
        }
    }

    const fn expected_exit_code(self) -> i32 {
        match self {
            Self::Valid => QEMU_SUCCESS_EXIT_CODE,
            Self::Missing | Self::Corrupt | Self::Policy => QEMU_FAILURE_EXIT_CODE,
        }
    }
}

struct Fixture {
    root: PathBuf,
    esp: PathBuf,
}

fn prepare_fixture(
    test_root: &Path,
    scenario: Scenario,
    bootloader: &Path,
    kernel: &Path,
) -> Result<Fixture, String> {
    let root = test_root.join(scenario.name());
    let esp = root.join("esp");
    let esp_boot = esp.join("EFI/BOOT/BOOTX64.EFI");
    let esp_kernel = esp.join("EFI/UNNAMEDOS/KERNEL.ELF");
    fs::create_dir_all(esp_boot.parent().expect("fixed boot path parent"))
        .map_err(|error| format!("could not create {} fixture: {error}", scenario.name()))?;
    fs::copy(bootloader, &esp_boot)
        .map_err(|error| format!("could not stage fixture bootloader: {error}"))?;

    if scenario != Scenario::Missing {
        fs::create_dir_all(esp_kernel.parent().expect("fixed kernel path parent"))
            .map_err(|error| format!("could not create kernel fixture path: {error}"))?;
        fs::copy(kernel, &esp_kernel)
            .map_err(|error| format!("could not stage fixture kernel: {error}"))?;
    }
    if scenario == Scenario::Corrupt {
        let mut bytes = fs::read(&esp_kernel)
            .map_err(|error| format!("could not read corrupt fixture: {error}"))?;
        if bytes.len() < 4 {
            return Err("kernel fixture is too small to corrupt its ELF magic".to_owned());
        }
        bytes[0] ^= 0xff;
        fs::write(&esp_kernel, bytes)
            .map_err(|error| format!("could not corrupt fixture ELF magic: {error}"))?;
    }
    if scenario == Scenario::Policy {
        move_second_load_segment_outside_window(&esp_kernel)?;
    }

    Ok(Fixture { root, esp })
}

fn move_second_load_segment_outside_window(path: &Path) -> Result<(), String> {
    const OUTSIDE_WINDOW: u64 = 0x0420_0000;
    let mut bytes =
        fs::read(path).map_err(|error| format!("could not read policy fixture: {error}"))?;
    let program_offset = read_u64(&bytes, 32)?;
    let entry_size = u64::from(read_u16(&bytes, 54)?);
    let entry_count = u64::from(read_u16(&bytes, 56)?);
    let mut load_index = 0_u8;
    let mut changed = false;
    for index in 0..entry_count {
        let header = program_offset
            .checked_add(
                index
                    .checked_mul(entry_size)
                    .ok_or("program index overflow")?,
            )
            .ok_or("program header overflow")?;
        let header = usize::try_from(header).map_err(|_| "program header does not fit usize")?;
        if read_u32(&bytes, header)? == 1 {
            if load_index == 1 {
                write_u64(&mut bytes, header + 16, OUTSIDE_WINDOW)?;
                write_u64(&mut bytes, header + 24, OUTSIDE_WINDOW)?;
                changed = true;
                break;
            }
            load_index += 1;
        }
    }
    if !changed {
        return Err("policy fixture has fewer than two PT_LOAD segments".to_owned());
    }
    kernel_image::validate_bootstrap_image(&bytes)
        .map_err(|error| format!("policy fixture must remain ELF-valid: {error:?}"))?;
    fs::write(path, bytes).map_err(|error| format!("could not write policy fixture: {error}"))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "fixture u16 range is outside ELF".to_owned())?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "fixture u32 range is outside ELF".to_owned())?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| "fixture u64 range is outside ELF".to_owned())?;
    Ok(u64::from_le_bytes(
        value.try_into().expect("eight-byte range"),
    ))
}

fn write_u64(bytes: &mut [u8], offset: usize, value: u64) -> Result<(), String> {
    bytes
        .get_mut(offset..offset + 8)
        .ok_or_else(|| "fixture u64 range is outside ELF".to_owned())?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BuildMode {
    Normal,
    QemuTest,
    ExitBootServices,
    KernelHandoff,
    PageTableAllocationFailure,
}

struct BuildPaths {
    output_root: PathBuf,
    esp_root: PathBuf,
    esp_boot: PathBuf,
}

fn build_and_stage(mode: BuildMode) -> Result<BuildPaths, String> {
    let root = repository_root();
    let output_root = root.join("target").join("unnamedos");
    let artifact = root
        .join("target")
        .join(UEFI_TARGET)
        .join("debug")
        .join("bootloader.efi");

    remove_file_if_present(&artifact)?;

    let mut command = Command::new("cargo");
    command
        .current_dir(&root)
        .args(["build", "-p", "bootloader", "--target", UEFI_TARGET]);
    match mode {
        BuildMode::Normal => {}
        BuildMode::QemuTest => {
            command.args(["--features", "qemu-test"]);
        }
        BuildMode::ExitBootServices => {
            command.args(["--features", "qemu-test,exit-boot-services-test"]);
        }
        BuildMode::KernelHandoff => {
            command.args(["--features", "qemu-test,kernel-handoff-test"]);
        }
        BuildMode::PageTableAllocationFailure => {
            command.args(["--features", "qemu-test,page-table-allocation-failure-test"]);
        }
    }
    let status = command
        .status()
        .map_err(|error| format!("could not start bootloader build: {error}"))?;
    if !status.success() {
        return Err(format!("bootloader build failed with {status}"));
    }

    let (esp_root, esp_boot) = stage_esp(&output_root, &artifact)?;
    Ok(BuildPaths {
        output_root,
        esp_root,
        esp_boot,
    })
}

fn stage_esp(output_root: &Path, artifact: &Path) -> Result<(PathBuf, PathBuf), String> {
    let esp_root = output_root.join("esp");
    let boot_directory = esp_root.join("EFI").join("BOOT");
    let esp_boot = boot_directory.join("BOOTX64.EFI");
    remove_file_if_present(&esp_boot)?;

    if !artifact.is_file() {
        return Err(format!(
            "fresh EFI artifact is missing: {}",
            artifact.display()
        ));
    }
    let artifact_length = fs::metadata(artifact)
        .map_err(|error| format!("could not inspect EFI artifact: {error}"))?
        .len();
    if artifact_length == 0 {
        return Err("fresh EFI artifact is empty".to_owned());
    }

    fs::create_dir_all(&boot_directory)
        .map_err(|error| format!("could not create ESP staging directory: {error}"))?;
    fs::copy(artifact, &esp_boot)
        .map_err(|error| format!("could not stage BOOTX64.EFI: {error}"))?;
    let staged_length = fs::metadata(&esp_boot)
        .map_err(|error| format!("could not inspect staged BOOTX64.EFI: {error}"))?
        .len();
    if staged_length != artifact_length {
        return Err("staged BOOTX64.EFI length differs from the fresh artifact".to_owned());
    }

    Ok((esp_root, esp_boot))
}

struct RunState {
    vars_source: PathBuf,
    vars_source_hash: u64,
    vars_copy: PathBuf,
    serial_log: PathBuf,
}

fn prepare_run(output_root: &Path, vars_source: &Path) -> Result<RunState, String> {
    let run_directory = output_root.join("run");
    remove_directory_if_present(&run_directory)?;
    fs::create_dir_all(&run_directory)
        .map_err(|error| format!("could not create clean QEMU run directory: {error}"))?;

    let vars_source_hash = file_hash(vars_source)?;
    let vars_copy = run_directory.join("OVMF_VARS.fd");
    fs::copy(vars_source, &vars_copy)
        .map_err(|error| format!("could not copy OVMF VARS template: {error}"))?;
    if file_hash(&vars_copy)? != vars_source_hash {
        return Err("VARS runtime copy differs from its source template".to_owned());
    }

    Ok(RunState {
        vars_source: vars_source.to_path_buf(),
        vars_source_hash,
        vars_copy,
        serial_log: run_directory.join("serial.log"),
    })
}

fn verify_vars_source_unchanged(run: &RunState) -> Result<(), String> {
    let after = file_hash(&run.vars_source)?;
    if after == run.vars_source_hash {
        Ok(())
    } else {
        Err("source OVMF VARS template changed during QEMU execution".to_owned())
    }
}

fn file_hash(path: &Path) -> Result<u64, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("could not open {} for hashing: {error}", path.display()))?;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("could not hash {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        for &byte in &buffer[..read] {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    Ok(hash)
}

struct QemuConfig<'a> {
    ovmf_code: &'a Path,
    ovmf_vars: &'a Path,
    esp: &'a Path,
    serial_log: Option<&'a Path>,
    qemu_test: bool,
}

fn qemu_arguments(config: &QemuConfig<'_>) -> Vec<OsString> {
    let mut args = vec![
        "-machine".into(),
        "q35".into(),
        "-accel".into(),
        "tcg".into(),
        "-m".into(),
        "128M".into(),
        "-drive".into(),
        format!(
            "if=pflash,format=raw,unit=0,readonly=on,file={}",
            config.ovmf_code.display()
        )
        .into(),
        "-drive".into(),
        format!(
            "if=pflash,format=raw,unit=1,file={}",
            config.ovmf_vars.display()
        )
        .into(),
        "-drive".into(),
        format!(
            "if=none,id=esp,format=raw,readonly=on,file=fat:ro:{}",
            fat_path(config.esp)
        )
        .into(),
        "-device".into(),
        "virtio-blk-pci,drive=esp".into(),
        "-net".into(),
        "none".into(),
        "-monitor".into(),
        "none".into(),
        "-no-reboot".into(),
    ];

    if let Some(serial_log) = config.serial_log {
        args.extend([
            OsString::from("-display"),
            OsString::from("none"),
            OsString::from("-serial"),
            OsString::from(format!("file:{}", serial_log.display())),
        ]);
    } else {
        args.extend([OsString::from("-serial"), OsString::from("stdio")]);
    }

    if config.qemu_test {
        args.extend([
            OsString::from("-device"),
            OsString::from(format!(
                "isa-debug-exit,iobase={DEBUG_EXIT_PORT},iosize=0x04"
            )),
        ]);
    }
    args
}

fn fat_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WaitOutcome {
    Exited(Option<i32>),
    Timeout,
}

trait ManagedChild {
    fn try_exit_code(&mut self) -> io::Result<Option<Option<i32>>>;
    fn kill(&mut self) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<()>;
}

struct ProcessChild(Child);

impl ManagedChild for ProcessChild {
    fn try_exit_code(&mut self) -> io::Result<Option<Option<i32>>> {
        self.0
            .try_wait()
            .map(|status| status.map(|status| status.code()))
    }

    fn kill(&mut self) -> io::Result<()> {
        self.0.kill()
    }

    fn wait(&mut self) -> io::Result<()> {
        self.0.wait().map(|_status| ())
    }
}

fn wait_for_child(child: &mut impl ManagedChild, timeout: Duration) -> Result<WaitOutcome, String> {
    let started = Instant::now();
    loop {
        match child.try_exit_code() {
            Ok(Some(code)) => return Ok(WaitOutcome::Exited(code)),
            Ok(None) if started.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                child
                    .kill()
                    .map_err(|error| format!("timeout; could not terminate QEMU: {error}"))?;
                child
                    .wait()
                    .map_err(|error| format!("timeout; could not reap QEMU: {error}"))?;
                return Ok(WaitOutcome::Timeout);
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("QEMU runner polling failure: {error}"));
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum MarkerError {
    Panic,
    BadLineEnding,
    Missing,
    DuplicateOrUnexpected,
    WrongOrder,
}

fn validate_markers(serial: &str, expected: &[&str]) -> Result<(), MarkerError> {
    let mut observed = Vec::new();
    for segment in serial.split_inclusive('\n') {
        if segment.contains("UNOS:") {
            if !segment.ends_with("\r\n") {
                return Err(MarkerError::BadLineEnding);
            }
            observed.push(segment.trim_end_matches("\r\n"));
        }
    }
    if serial.contains("UNOS:P1C:PANIC") {
        return Err(MarkerError::Panic);
    }
    if observed == expected {
        return Ok(());
    }
    if observed.len() < expected.len() {
        return Err(MarkerError::Missing);
    }
    if observed.len() > expected.len() {
        return Err(MarkerError::DuplicateOrUnexpected);
    }
    Err(MarkerError::WrongOrder)
}

fn classify_test_result(
    outcome: WaitOutcome,
    expected_exit_code: i32,
    markers: Result<(), MarkerError>,
) -> Result<(), String> {
    if markers == Err(MarkerError::Panic) {
        return Err("UEFI panic marker observed".to_owned());
    }
    if outcome == WaitOutcome::Timeout {
        return Err(format!(
            "UEFI boot timed out after {} seconds; QEMU was terminated",
            HEADLESS_TIMEOUT.as_secs()
        ));
    }
    if let Err(error) = markers {
        return Err(match error {
            MarkerError::Panic => unreachable!("panic was handled before other marker errors"),
            MarkerError::BadLineEnding => "serial marker did not use CRLF".to_owned(),
            MarkerError::Missing => "one or more serial milestones are missing".to_owned(),
            MarkerError::DuplicateOrUnexpected => {
                "duplicate or unexpected serial milestone observed".to_owned()
            }
            MarkerError::WrongOrder => "serial milestones are in the wrong order".to_owned(),
        });
    }

    match outcome {
        WaitOutcome::Exited(Some(code)) if code == expected_exit_code => Ok(()),
        WaitOutcome::Exited(code) => Err(format!(
            "unexpected QEMU exit code {}; expected {expected_exit_code}",
            code.map_or_else(|| "unavailable".to_owned(), |code| code.to_string())
        )),
        WaitOutcome::Timeout => unreachable!("timeout was handled before exit classification"),
    }
}

fn startup_failure(outcome: WaitOutcome, serial: &str) -> Option<String> {
    match outcome {
        WaitOutcome::Exited(Some(code))
            if code != QEMU_SUCCESS_EXIT_CODE
                && code != QEMU_FAILURE_EXIT_CODE
                && serial.trim().is_empty() =>
        {
            Some(format!(
                "QEMU startup failure: exit code {code} with an empty serial log"
            ))
        }
        _ => None,
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask must live at crates/xtask")
        .to_path_buf()
}

fn remove_file_if_present(path: &Path) -> Result<(), String> {
    if path.exists() {
        fs::remove_file(path)
            .map_err(|error| format!("could not remove stale {}: {error}", path.display()))?;
    }
    Ok(())
}

fn remove_directory_if_present(path: &Path) -> Result<(), String> {
    if path.exists() {
        fs::remove_dir_all(path)
            .map_err(|error| format!("could not clean {}: {error}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let unique = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path =
                env::temp_dir().join(format!("unnamedos-{label}-{}-{unique}", std::process::id()));
            fs::create_dir_all(&path).expect("test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn esp_target_path_is_fixed_ascii_layout() {
        let output = Path::new("target").join("unnamedos");
        let expected = output.join("esp/EFI/BOOT/BOOTX64.EFI");
        let actual = output
            .join("esp")
            .join("EFI")
            .join("BOOT")
            .join("BOOTX64.EFI");
        assert_eq!(actual, expected);
        assert!(actual.to_string_lossy().is_ascii());
    }

    #[test]
    fn missing_artifact_cannot_leave_stale_esp_binary() {
        let directory = TestDirectory::new("missing-artifact");
        let output = directory.path().join("output");
        let stale = output.join("esp/EFI/BOOT/BOOTX64.EFI");
        fs::create_dir_all(stale.parent().expect("parent")).expect("stale parent");
        fs::write(&stale, b"stale").expect("stale artifact");

        let error = stage_esp(&output, &directory.path().join("missing.efi"))
            .expect_err("missing artifact must fail");
        assert!(error.contains("fresh EFI artifact is missing"));
        assert!(!stale.exists());
    }

    #[test]
    fn fresh_artifact_replaces_old_staging() {
        let directory = TestDirectory::new("fresh-artifact");
        let output = directory.path().join("output");
        let artifact = directory.path().join("bootloader.efi");
        fs::write(&artifact, b"fresh").expect("fresh artifact");
        let stale = output.join("esp/EFI/BOOT/BOOTX64.EFI");
        fs::create_dir_all(stale.parent().expect("parent")).expect("stale parent");
        fs::write(&stale, b"stale").expect("stale artifact");

        let (_, staged) = stage_esp(&output, &artifact).expect("staging");
        assert_eq!(fs::read(staged).expect("staged bytes"), b"fresh");
    }

    #[test]
    fn bootloader_staging_preserves_staged_kernel() {
        let directory = TestDirectory::new("preserve-kernel");
        let output = directory.path().join("output");
        let artifact = directory.path().join("bootloader.efi");
        let kernel = output.join("esp/EFI/UNNAMEDOS/KERNEL.ELF");
        fs::create_dir_all(kernel.parent().expect("kernel parent")).expect("kernel parent");
        fs::write(&kernel, b"kernel").expect("kernel");
        fs::write(&artifact, b"fresh EFI").expect("artifact");

        stage_esp(&output, &artifact).expect("bootloader staging");
        assert_eq!(fs::read(kernel).expect("preserved kernel"), b"kernel");
    }

    #[test]
    fn qemu_arguments_preserve_paths_with_spaces_and_flash_permissions() {
        let config = QemuConfig {
            ovmf_code: Path::new(r"C:\QEMU Files\CODE.fd"),
            ovmf_vars: Path::new(r"C:\Run Files\VARS.fd"),
            esp: Path::new(r"C:\Build Files\esp"),
            serial_log: Some(Path::new(r"C:\Log Files\serial.log")),
            qemu_test: true,
        };
        let args = qemu_arguments(&config);
        let args: Vec<String> = args
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();

        assert!(args.contains(&"-machine".to_owned()));
        assert!(args.contains(&"q35".to_owned()));
        assert!(args.iter().any(|argument| {
            argument.contains("if=pflash")
                && argument.contains("readonly=on")
                && argument.contains("QEMU Files")
        }));
        assert!(args.iter().any(|argument| {
            argument.contains("if=pflash")
                && !argument.contains("readonly=on")
                && argument.contains("Run Files")
        }));
        assert!(
            args.iter()
                .any(|argument| argument.contains("fat:ro:C:/Build Files/esp"))
        );
        assert!(args.contains(&"virtio-blk-pci,drive=esp".to_owned()));
        assert!(
            args.iter()
                .any(|argument| argument.contains("isa-debug-exit"))
        );
        assert!(!args.iter().any(|argument| argument.contains("-netdev")));
    }

    #[test]
    fn normal_qemu_arguments_exclude_debug_exit() {
        let config = QemuConfig {
            ovmf_code: Path::new("CODE.fd"),
            ovmf_vars: Path::new("VARS.fd"),
            esp: Path::new("esp"),
            serial_log: None,
            qemu_test: false,
        };
        let args = qemu_arguments(&config);
        assert!(
            !args
                .iter()
                .any(|argument| argument.to_string_lossy().contains("isa-debug-exit"))
        );
    }

    #[test]
    fn vars_template_hash_is_unchanged_when_runtime_copy_changes() {
        let directory = TestDirectory::new("vars-copy");
        let output = directory.path().join("output");
        let source = directory.path().join("source-vars.fd");
        fs::write(&source, b"template").expect("source template");

        let run = prepare_run(&output, &source).expect("run state");
        fs::write(&run.vars_copy, b"mutated runtime state").expect("runtime mutation");
        verify_vars_source_unchanged(&run).expect("source must remain unchanged");
        assert_eq!(fs::read(source).expect("source"), b"template");
    }

    #[test]
    fn accepts_exact_marker_sequence_with_crlf() {
        let serial = "firmware\r\nUNOS:P1C:ENTRY\r\nUNOS:P1C:UEFI_OK\r\nUNOS:P1C:PASS\r\n";
        assert_eq!(validate_markers(serial, &P1C_MARKERS), Ok(()));
    }

    #[test]
    fn rejects_missing_duplicate_wrong_order_and_panic_markers() {
        assert_eq!(
            validate_markers("UNOS:P1C:ENTRY\r\nUNOS:P1C:UEFI_OK\r\n", &P1C_MARKERS),
            Err(MarkerError::Missing)
        );
        assert_eq!(
            validate_markers(
                "UNOS:P1C:ENTRY\r\nUNOS:P1C:ENTRY\r\nUNOS:P1C:UEFI_OK\r\nUNOS:P1C:PASS\r\n",
                &P1C_MARKERS,
            ),
            Err(MarkerError::DuplicateOrUnexpected)
        );
        assert_eq!(
            validate_markers(
                "UNOS:P1C:UEFI_OK\r\nUNOS:P1C:ENTRY\r\nUNOS:P1C:PASS\r\n",
                &P1C_MARKERS,
            ),
            Err(MarkerError::WrongOrder)
        );
        assert_eq!(
            validate_markers("UNOS:P1C:PANIC\r\n", &P1C_MARKERS),
            Err(MarkerError::Panic)
        );
    }

    #[test]
    fn rejects_non_crlf_marker() {
        assert_eq!(
            validate_markers("UNOS:P1C:ENTRY\n", &P1C_MARKERS),
            Err(MarkerError::BadLineEnding)
        );
    }

    #[test]
    fn classifies_expected_unexpected_failure_and_timeout_outcomes() {
        assert_eq!(
            classify_test_result(WaitOutcome::Exited(Some(33)), 33, Ok(())),
            Ok(())
        );
        assert_eq!(
            classify_test_result(WaitOutcome::Exited(Some(35)), 35, Ok(())),
            Ok(())
        );
        assert!(
            classify_test_result(WaitOutcome::Exited(Some(7)), 33, Ok(()))
                .expect_err("unexpected exit")
                .contains("unexpected QEMU exit code")
        );
        assert!(
            classify_test_result(WaitOutcome::Timeout, 33, Err(MarkerError::Missing))
                .expect_err("timeout")
                .contains("timed out")
        );
        assert!(
            startup_failure(WaitOutcome::Exited(Some(1)), "")
                .expect("empty early exit is a startup failure")
                .contains("startup failure")
        );
        assert!(startup_failure(WaitOutcome::Exited(Some(33)), "").is_none());
    }

    #[test]
    fn scenario_marker_contracts_are_exact() {
        for scenario in Scenario::ALL {
            let serial = scenario
                .expected_markers()
                .iter()
                .map(|marker| format!("{marker}\r\n"))
                .collect::<String>();
            assert_eq!(
                validate_markers(&serial, scenario.expected_markers()),
                Ok(())
            );
        }
        assert_eq!(Scenario::Valid.expected_exit_code(), 33);
        assert_eq!(Scenario::Missing.expected_exit_code(), 35);
        assert_eq!(Scenario::Corrupt.expected_exit_code(), 35);
        assert_eq!(Scenario::Policy.expected_exit_code(), 35);
        let exit_serial = EXIT_BOOT_SERVICES_MARKERS
            .iter()
            .map(|marker| format!("{marker}\r\n"))
            .collect::<String>();
        assert_eq!(
            validate_markers(&exit_serial, &EXIT_BOOT_SERVICES_MARKERS),
            Ok(())
        );
        let handoff_serial = KERNEL_HANDOFF_MARKERS
            .iter()
            .map(|marker| format!("{marker}\r\n"))
            .collect::<String>();
        assert_eq!(
            validate_markers(&handoff_serial, &KERNEL_HANDOFF_MARKERS),
            Ok(())
        );
        let negative_serial = PAGE_TABLE_ALLOCATION_FAILURE_MARKERS
            .iter()
            .map(|marker| format!("{marker}\r\n"))
            .collect::<String>();
        assert_eq!(
            validate_markers(&negative_serial, &PAGE_TABLE_ALLOCATION_FAILURE_MARKERS),
            Ok(())
        );
        assert!(
            PAGE_TABLE_ALLOCATION_FAILURE_MARKERS
                .iter()
                .all(|marker| !matches!(
                    *marker,
                    "UNOS:P1J:FRAMES_ALLOCATED"
                        | "UNOS:P1J:HIERARCHY_MATERIALIZED"
                        | "UNOS:P1J:HIERARCHY_VERIFIED"
                        | "UNOS:P1J:FINAL_MAP_RESERVED"
                        | "UNOS:P1J:OWNERSHIP_TRANSFERRED"
                ))
        );
        for scenario in [Scenario::Missing, Scenario::Corrupt, Scenario::Policy] {
            assert!(
                scenario
                    .expected_markers()
                    .iter()
                    .all(|marker| !marker.starts_with("UNOS:P1H:"))
            );
        }
    }

    #[test]
    fn fixtures_are_isolated_and_corruption_does_not_touch_source() {
        let directory = TestDirectory::new("scenario isolation with spaces");
        let bootloader = directory.path().join("source loader.efi");
        let kernel = directory.path().join("source kernel.elf");
        fs::write(&bootloader, b"loader").expect("loader source");
        let original_kernel = crate::kernel::tests::valid_kernel();
        fs::write(&kernel, &original_kernel).expect("kernel source");

        let fixtures = Scenario::ALL.map(|scenario| {
            prepare_fixture(directory.path(), scenario, &bootloader, &kernel).expect("fixture")
        });
        assert_ne!(fixtures[0].esp, fixtures[1].esp);
        assert_ne!(fixtures[1].esp, fixtures[2].esp);
        assert_ne!(fixtures[2].esp, fixtures[3].esp);
        assert!(fixtures[0].esp.join("EFI/UNNAMEDOS/KERNEL.ELF").is_file());
        assert!(!fixtures[1].esp.join("EFI/UNNAMEDOS/KERNEL.ELF").exists());
        assert_ne!(
            fs::read(fixtures[2].esp.join("EFI/UNNAMEDOS/KERNEL.ELF")).expect("corrupt fixture")[0],
            0x7f
        );
        assert_eq!(fs::read(kernel).expect("source unchanged"), original_kernel);
    }

    struct FakeChild {
        killed: Cell<bool>,
        waited: Cell<bool>,
    }

    impl ManagedChild for FakeChild {
        fn try_exit_code(&mut self) -> io::Result<Option<Option<i32>>> {
            Ok(None)
        }

        fn kill(&mut self) -> io::Result<()> {
            self.killed.set(true);
            Ok(())
        }

        fn wait(&mut self) -> io::Result<()> {
            self.waited.set(true);
            Ok(())
        }
    }

    #[test]
    fn timeout_kills_and_reaps_child() {
        let mut child = FakeChild {
            killed: Cell::new(false),
            waited: Cell::new(false),
        };
        let outcome = wait_for_child(&mut child, Duration::ZERO).expect("timeout outcome");
        assert_eq!(outcome, WaitOutcome::Timeout);
        assert!(child.killed.get());
        assert!(child.waited.get());
    }
}
