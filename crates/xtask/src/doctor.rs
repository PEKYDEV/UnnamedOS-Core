use std::collections::BTreeSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

const REQUIRED_TOOLCHAIN: &str = "1.98.0";
const REQUIRED_TARGETS: [&str; 2] = ["x86_64-unknown-none", "x86_64-unknown-uefi"];
const MIN_FIRMWARE_BYTES: u64 = 64 * 1024;
const MAX_FIRMWARE_BYTES: u64 = 128 * 1024 * 1024;

pub fn run() -> Result<bool, String> {
    println!("UnnamedOS Phase 1 environment doctor");

    let installed = installed_rust_targets()?;
    let missing_targets = missing_targets(&installed);
    if missing_targets.is_empty() {
        println!(
            "[ok] Rust {REQUIRED_TOOLCHAIN} targets: {}",
            REQUIRED_TARGETS.join(", ")
        );
    } else {
        println!("[missing] Rust targets: {}", missing_targets.join(", "));
        println!(
            "          Fix: rustup target add --toolchain {REQUIRED_TOOLCHAIN} {}",
            missing_targets.join(" ")
        );
    }

    let qemu = match resolve_qemu() {
        Ok(resolved) => {
            println!(
                "[ok] QEMU ({source}): {version}",
                source = resolved.source,
                version = resolved.version
            );
            println!("     {}", resolved.path.display());
            Some(resolved)
        }
        Err(diagnostic) => {
            diagnostic.print("QEMU");
            None
        }
    };

    let firmware = match resolve_firmware(qemu.as_ref().map(|qemu| qemu.path.as_path())) {
        Ok(resolved) => {
            println!("[ok] OVMF/EDK2 ({})", resolved.source);
            println!(
                "     CODE: {} ({} bytes)",
                resolved.code.display(),
                resolved.code_bytes
            );
            println!(
                "     VARS template: {} ({} bytes)",
                resolved.vars_template.display(),
                resolved.vars_bytes
            );
            Some(resolved)
        }
        Err(diagnostic) => {
            diagnostic.print("OVMF/EDK2");
            None
        }
    };

    let ready = missing_targets.is_empty() && qemu.is_some() && firmware.is_some();
    if ready {
        println!("Phase 1 prerequisites are available.");
    } else {
        println!("Phase 1 prerequisites are incomplete; host checks remain usable.");
    }

    Ok(ready)
}

pub(crate) struct Phase1Paths {
    pub qemu: PathBuf,
    pub ovmf_code: PathBuf,
    pub ovmf_vars_template: PathBuf,
}

pub(crate) fn resolve_phase1_paths() -> Result<Phase1Paths, String> {
    let qemu = resolve_qemu().map_err(|diagnostic| diagnostic.render("QEMU"))?;
    let firmware =
        resolve_firmware(Some(&qemu.path)).map_err(|diagnostic| diagnostic.render("OVMF/EDK2"))?;

    Ok(Phase1Paths {
        qemu: qemu.path,
        ovmf_code: firmware.code,
        ovmf_vars_template: firmware.vars_template,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResolutionSource {
    EnvironmentOverride,
    Path,
    QemuDistribution,
    KnownOfficialInstall,
}

impl fmt::Display for ResolutionSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::EnvironmentOverride => "environment override",
            Self::Path => "PATH",
            Self::QemuDistribution => "QEMU distribution",
            Self::KnownOfficialInstall => "known official install location",
        };
        formatter.write_str(label)
    }
}

#[derive(Debug)]
struct Diagnostic {
    problem: String,
    fix: String,
}

impl Diagnostic {
    fn new(problem: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            problem: problem.into(),
            fix: fix.into(),
        }
    }

    fn print(&self, component: &str) {
        println!("[missing] {component}: {}", self.problem);
        println!("          Fix: {}", self.fix);
    }

    fn render(&self, component: &str) -> String {
        format!("{component}: {}; fix: {}", self.problem, self.fix)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PathCandidate {
    path: PathBuf,
    source: ResolutionSource,
}

#[derive(Debug)]
struct ResolvedQemu {
    path: PathBuf,
    source: ResolutionSource,
    version: String,
}

#[derive(Debug)]
struct FirmwareCandidate {
    code: PathBuf,
    vars_template: PathBuf,
    source: ResolutionSource,
    root: Option<PathBuf>,
}

#[derive(Debug)]
struct ResolvedFirmware {
    code: PathBuf,
    vars_template: PathBuf,
    source: ResolutionSource,
    code_bytes: u64,
    vars_bytes: u64,
}

fn installed_rust_targets() -> Result<BTreeSet<String>, String> {
    let output = Command::new("rustup")
        .args([
            "target",
            "list",
            "--installed",
            "--toolchain",
            REQUIRED_TOOLCHAIN,
        ])
        .output()
        .map_err(|error| format!("could not start `rustup`: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "`rustup target list --installed --toolchain {REQUIRED_TOOLCHAIN}` exited with {}",
            output.status
        ));
    }

    Ok(parse_installed_targets(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn parse_installed_targets(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn missing_targets(installed: &BTreeSet<String>) -> Vec<&'static str> {
    REQUIRED_TARGETS
        .iter()
        .copied()
        .filter(|target| !installed.contains(*target))
        .collect()
}

fn resolve_qemu() -> Result<ResolvedQemu, Diagnostic> {
    let override_path = env::var_os("UNNAMEDOS_QEMU");
    let candidates = qemu_candidates(
        override_path.clone(),
        env::var_os("PATH"),
        &known_qemu_directories(),
    );

    if override_path.is_some() {
        return probe_qemu_candidate(&candidates[0]);
    }

    for candidate in &candidates {
        if candidate.path.is_file() {
            return probe_qemu_candidate(candidate);
        }
    }

    Err(Diagnostic::new(
        "`qemu-system-x86_64` was not found through an override, PATH, or a known install location",
        "set UNNAMEDOS_QEMU to the QEMU executable or install the official Windows x86-64 build",
    ))
}

fn probe_qemu_candidate(candidate: &PathCandidate) -> Result<ResolvedQemu, Diagnostic> {
    if !candidate.path.is_file() {
        return Err(Diagnostic::new(
            format!("{} is not a file", candidate.path.display()),
            "correct UNNAMEDOS_QEMU or install QEMU in a supported location",
        ));
    }
    File::open(&candidate.path).map_err(|error| {
        Diagnostic::new(
            format!("{} is not readable: {error}", candidate.path.display()),
            "grant the current user read access or select another QEMU executable",
        )
    })?;

    let output = Command::new(&candidate.path)
        .arg("--version")
        .output()
        .map_err(|error| {
            Diagnostic::new(
                format!("could not run {}: {error}", candidate.path.display()),
                "verify that the selected file is a working Windows x86-64 QEMU executable",
            )
        })?;
    if !output.status.success() {
        return Err(Diagnostic::new(
            format!("QEMU version probe exited with {}", output.status),
            "repair the QEMU installation or point UNNAMEDOS_QEMU to a working executable",
        ));
    }

    let first_line = first_nonempty_line(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| {
            Diagnostic::new(
                "QEMU returned no version text",
                "repair the QEMU installation or select another stable QEMU build",
            )
        })?
        .to_owned();
    let version = parse_qemu_version(&first_line).ok_or_else(|| {
        Diagnostic::new(
            format!("unrecognized QEMU version output: {first_line}"),
            "select a stable qemu-system-x86_64 build",
        )
    })?;
    if version.to_ascii_lowercase().contains("rc") {
        return Err(Diagnostic::new(
            format!("release-candidate QEMU version {version} is not accepted"),
            "install a stable QEMU release",
        ));
    }

    Ok(ResolvedQemu {
        path: candidate.path.clone(),
        source: candidate.source,
        version,
    })
}

fn qemu_candidates(
    override_path: Option<OsString>,
    path_value: Option<OsString>,
    known_directories: &[PathBuf],
) -> Vec<PathCandidate> {
    if let Some(path) = override_path {
        return vec![PathCandidate {
            path: PathBuf::from(path),
            source: ResolutionSource::EnvironmentOverride,
        }];
    }

    let mut candidates = Vec::new();
    if let Some(path_value) = path_value {
        for directory in env::split_paths(&path_value) {
            for name in executable_names("qemu-system-x86_64") {
                candidates.push(PathCandidate {
                    path: directory.join(name),
                    source: ResolutionSource::Path,
                });
            }
        }
    }
    for directory in known_directories {
        for name in executable_names("qemu-system-x86_64") {
            candidates.push(PathCandidate {
                path: directory.join(name),
                source: ResolutionSource::KnownOfficialInstall,
            });
        }
    }
    candidates
}

fn known_qemu_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        directories.push(PathBuf::from(local_app_data).join("Programs").join("QEMU"));
    }
    if let Some(program_files) = env::var_os("ProgramFiles") {
        directories.push(PathBuf::from(program_files).join("qemu"));
    }
    if let Some(program_files_x86) = env::var_os("ProgramFiles(x86)") {
        directories.push(PathBuf::from(program_files_x86).join("qemu"));
    }
    directories
}

fn first_nonempty_line(output: &str) -> Option<&str> {
    output.lines().map(str::trim).find(|line| !line.is_empty())
}

fn parse_qemu_version(line: &str) -> Option<String> {
    line.strip_prefix("QEMU emulator version ")
        .and_then(|remainder| remainder.split_whitespace().next())
        .map(str::to_owned)
}

fn resolve_firmware(qemu_path: Option<&Path>) -> Result<ResolvedFirmware, Diagnostic> {
    let code_override = env::var_os("UNNAMEDOS_OVMF_CODE");
    let vars_override = env::var_os("UNNAMEDOS_OVMF_VARS_TEMPLATE");
    let candidates = firmware_candidates(
        code_override.clone(),
        vars_override.clone(),
        qemu_path,
        &known_qemu_directories(),
    )?;

    if code_override.is_some() && vars_override.is_some() {
        return probe_firmware_candidate(&candidates[0]);
    }

    for candidate in &candidates {
        if candidate.code.is_file() && candidate.vars_template.is_file() {
            return probe_firmware_candidate(candidate);
        }
    }

    Err(Diagnostic::new(
        "no compatible x86-64 CODE and VARS-template pair was found",
        "install the official QEMU Windows build with EDK2, or set both UNNAMEDOS_OVMF_CODE and UNNAMEDOS_OVMF_VARS_TEMPLATE",
    ))
}

fn firmware_candidates(
    code_override: Option<OsString>,
    vars_override: Option<OsString>,
    qemu_path: Option<&Path>,
    known_qemu_directories: &[PathBuf],
) -> Result<Vec<FirmwareCandidate>, Diagnostic> {
    match (code_override, vars_override) {
        (Some(code), Some(vars_template)) => {
            return Ok(vec![FirmwareCandidate {
                code: PathBuf::from(code),
                vars_template: PathBuf::from(vars_template),
                source: ResolutionSource::EnvironmentOverride,
                root: None,
            }]);
        }
        (Some(_), None) => {
            return Err(Diagnostic::new(
                "UNNAMEDOS_OVMF_CODE is set without UNNAMEDOS_OVMF_VARS_TEMPLATE",
                "set both firmware overrides or remove both",
            ));
        }
        (None, Some(_)) => {
            return Err(Diagnostic::new(
                "UNNAMEDOS_OVMF_VARS_TEMPLATE is set without UNNAMEDOS_OVMF_CODE",
                "set both firmware overrides or remove both",
            ));
        }
        (None, None) => {}
    }

    let mut roots = Vec::new();
    if let Some(qemu_directory) = qemu_path.and_then(Path::parent) {
        roots.push((
            qemu_directory.join("share"),
            ResolutionSource::QemuDistribution,
        ));
    }
    for directory in known_qemu_directories {
        roots.push((
            directory.join("share"),
            ResolutionSource::KnownOfficialInstall,
        ));
    }
    roots.extend([
        (
            PathBuf::from("/usr/share/OVMF"),
            ResolutionSource::KnownOfficialInstall,
        ),
        (
            PathBuf::from("/usr/share/edk2/x64"),
            ResolutionSource::KnownOfficialInstall,
        ),
        (
            PathBuf::from("/usr/share/qemu"),
            ResolutionSource::KnownOfficialInstall,
        ),
    ]);

    let names = [
        ("edk2-x86_64-code.fd", "edk2-i386-vars.fd"),
        ("OVMF_CODE_4M.fd", "OVMF_VARS_4M.fd"),
        ("OVMF_CODE.fd", "OVMF_VARS.fd"),
    ];
    Ok(roots
        .into_iter()
        .flat_map(|(root, source)| {
            names
                .iter()
                .map(move |(code, vars_template)| FirmwareCandidate {
                    code: root.join(code),
                    vars_template: root.join(vars_template),
                    source,
                    root: Some(root.clone()),
                })
        })
        .collect())
}

fn probe_firmware_candidate(candidate: &FirmwareCandidate) -> Result<ResolvedFirmware, Diagnostic> {
    let code_identity = canonical_or_original(&candidate.code);
    let vars_identity = canonical_or_original(&candidate.vars_template);
    if code_identity == vars_identity {
        return Err(Diagnostic::new(
            "CODE and VARS template resolve to the same file",
            "select distinct x86-64 CODE and VARS-template firmware files",
        ));
    }
    if !candidate.code.is_file() {
        return Err(Diagnostic::new(
            format!("CODE firmware is not a file: {}", candidate.code.display()),
            "correct UNNAMEDOS_OVMF_CODE or use the firmware shipped with QEMU",
        ));
    }
    if !candidate.vars_template.is_file() {
        return Err(Diagnostic::new(
            format!(
                "VARS template is not a file: {}",
                candidate.vars_template.display()
            ),
            "correct UNNAMEDOS_OVMF_VARS_TEMPLATE or use the firmware shipped with QEMU",
        ));
    }

    let code_bytes = readable_firmware_size(&candidate.code, "CODE")?;
    let vars_bytes = readable_firmware_size(&candidate.vars_template, "VARS template")?;
    if !looks_like_x86_64_code(&candidate.code) {
        return Err(Diagnostic::new(
            "CODE filename does not identify x86-64 OVMF/EDK2 firmware",
            "select edk2-x86_64-code.fd or an OVMF_CODE firmware for x86-64",
        ));
    }
    if !looks_like_vars_template(&candidate.vars_template) {
        return Err(Diagnostic::new(
            "VARS filename is not a recognized OVMF/EDK2 template",
            "select edk2-i386-vars.fd paired by QEMU or an OVMF_VARS template",
        ));
    }
    if let Some(root) = &candidate.root {
        verify_qemu_firmware_descriptor(root, &candidate.code, &candidate.vars_template)?;
    }

    Ok(ResolvedFirmware {
        code: candidate.code.clone(),
        vars_template: candidate.vars_template.clone(),
        source: candidate.source,
        code_bytes,
        vars_bytes,
    })
}

fn readable_firmware_size(path: &Path, label: &str) -> Result<u64, Diagnostic> {
    File::open(path).map_err(|error| {
        Diagnostic::new(
            format!("{label} firmware is not readable: {error}"),
            "grant the current user read access or select another firmware file",
        )
    })?;
    let length = fs::metadata(path)
        .map_err(|error| {
            Diagnostic::new(
                format!("could not read {label} metadata: {error}"),
                "repair the firmware installation",
            )
        })?
        .len();
    if !(MIN_FIRMWARE_BYTES..=MAX_FIRMWARE_BYTES).contains(&length) {
        return Err(Diagnostic::new(
            format!("{label} size {length} bytes is outside the accepted range"),
            "select an unmodified firmware file from the official QEMU distribution",
        ));
    }
    Ok(length)
}

fn canonical_or_original(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn looks_like_x86_64_code(path: &Path) -> bool {
    let name = lowercase_filename(path);
    (name.contains("x86_64") && name.contains("code") && name.ends_with(".fd"))
        || (name.starts_with("ovmf_code") && name.ends_with(".fd"))
}

fn looks_like_vars_template(path: &Path) -> bool {
    let name = lowercase_filename(path);
    ((name.contains("i386") || name.contains("x86_64"))
        && name.contains("vars")
        && name.ends_with(".fd"))
        || (name.starts_with("ovmf_vars") && name.ends_with(".fd"))
}

fn lowercase_filename(path: &Path) -> String {
    path.file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn verify_qemu_firmware_descriptor(
    root: &Path,
    code: &Path,
    vars_template: &Path,
) -> Result<(), Diagnostic> {
    if !lowercase_filename(code).starts_with("edk2-x86_64") {
        return Ok(());
    }
    let descriptor = root.join("firmware").join("60-edk2-x86_64.json");
    let contents = fs::read_to_string(&descriptor).map_err(|error| {
        Diagnostic::new(
            format!("QEMU x86-64 firmware descriptor is unavailable: {error}"),
            "repair the official QEMU EDK2 installation",
        )
    })?;
    let code_name = code.file_name().and_then(OsStr::to_str).unwrap_or_default();
    let vars_name = vars_template
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    if !contents.contains(code_name)
        || !contents.contains(vars_name)
        || !contents.contains("\"architecture\": \"x86_64\"")
        || !contents.contains("pc-q35-*")
    {
        return Err(Diagnostic::new(
            "QEMU does not describe the selected firmware pair as x86-64 q35 compatible",
            "select the CODE/VARS pair declared by QEMU's 60-edk2-x86_64.json",
        ));
    }
    Ok(())
}

fn executable_names(program: &str) -> Vec<OsString> {
    let mut names = vec![OsString::from(program)];
    if cfg!(windows) && Path::new(program).extension() != Some(OsStr::new("exe")) {
        names.push(OsString::from(format!("{program}.exe")));
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_installed_targets_without_blank_lines() {
        let targets = parse_installed_targets("x86_64-pc-windows-msvc\n\nx86_64-unknown-none  \n");

        assert!(targets.contains("x86_64-pc-windows-msvc"));
        assert!(targets.contains("x86_64-unknown-none"));
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn reports_only_missing_required_targets() {
        let installed = BTreeSet::from(["x86_64-unknown-none".to_owned()]);

        assert_eq!(missing_targets(&installed), vec!["x86_64-unknown-uefi"]);
    }

    #[test]
    fn parses_stable_qemu_version() {
        assert_eq!(
            parse_qemu_version("QEMU emulator version 11.1.0 (v11.1.0-12130-ge470268ff4)"),
            Some("11.1.0".to_owned())
        );
    }

    #[test]
    fn qemu_override_has_priority_and_preserves_spaces() {
        let override_path = PathBuf::from(r"C:\Tools With Spaces\QEMU\qemu-system-x86_64.exe");
        let candidates = qemu_candidates(
            Some(override_path.clone().into_os_string()),
            Some(OsString::from(r"C:\OnPath")),
            &[PathBuf::from(r"C:\Known")],
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].path, override_path);
        assert_eq!(candidates[0].source, ResolutionSource::EnvironmentOverride);
    }

    #[test]
    fn qemu_path_candidates_precede_known_installs() {
        let path_value = env::join_paths(["Path QEMU", "Other"])
            .expect("platform-neutral test PATH must be valid");
        let candidates = qemu_candidates(None, Some(path_value), &[PathBuf::from("Known QEMU")]);

        assert_eq!(candidates[0].source, ResolutionSource::Path);
        assert_eq!(
            candidates.last().expect("known candidate").source,
            ResolutionSource::KnownOfficialInstall
        );
    }

    #[test]
    fn firmware_overrides_have_priority_and_preserve_spaces() {
        let code = PathBuf::from(r"C:\Firmware Files\OVMF_CODE.fd");
        let vars = PathBuf::from(r"C:\Firmware Files\OVMF_VARS.fd");
        let candidates = firmware_candidates(
            Some(code.clone().into_os_string()),
            Some(vars.clone().into_os_string()),
            Some(Path::new(r"C:\QEMU\qemu-system-x86_64.exe")),
            &[PathBuf::from(r"C:\Known QEMU")],
        )
        .expect("complete override must be accepted");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].code, code);
        assert_eq!(candidates[0].vars_template, vars);
        assert_eq!(candidates[0].source, ResolutionSource::EnvironmentOverride);
    }

    #[test]
    fn incomplete_firmware_override_has_actionable_diagnostic() {
        let error = firmware_candidates(Some(OsString::from("OVMF_CODE.fd")), None, None, &[])
            .expect_err("incomplete override must fail");

        assert!(error.problem.contains("UNNAMEDOS_OVMF_CODE"));
        assert!(error.fix.contains("both"));
    }

    #[test]
    fn same_firmware_file_is_rejected_before_use() {
        let path = PathBuf::from("same.fd");
        let candidate = FirmwareCandidate {
            code: path.clone(),
            vars_template: path,
            source: ResolutionSource::EnvironmentOverride,
            root: None,
        };

        let error = probe_firmware_candidate(&candidate).expect_err("same file must fail");
        assert!(error.problem.contains("same file"));
    }

    #[test]
    fn recognizes_supported_x86_firmware_names() {
        assert!(looks_like_x86_64_code(Path::new("edk2-x86_64-code.fd")));
        assert!(looks_like_x86_64_code(Path::new("OVMF_CODE_4M.fd")));
        assert!(!looks_like_x86_64_code(Path::new("edk2-aarch64-code.fd")));
        assert!(looks_like_vars_template(Path::new("edk2-i386-vars.fd")));
        assert!(looks_like_vars_template(Path::new("OVMF_VARS.fd")));
    }
}
