use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use kernel_image::{BOOTSTRAP_PAGE_SIZE, LoadSegment, ValidatedImage, validate_bootstrap_image};

use crate::{sha256, uefi};

const KERNEL_TARGET: &str = "x86_64-unknown-none";
const KERNEL_BINARY: &str = "unnamedos-kernel";

pub fn build_kernel() -> Result<(), String> {
    let staged = build_and_stage_kernel()?;
    println!("kernel.contract=valid");
    println!("kernel.stage={}", staged.primary.display());
    println!("kernel.esp_stage={}", staged.esp.display());
    println!("kernel.sha256={}", sha256::hex(staged.digest));
    Ok(())
}

pub(crate) fn build_kernel_for_uefi_test() -> Result<PathBuf, String> {
    build_and_stage_kernel().map(|kernel| kernel.primary)
}

pub fn inspect_kernel() -> Result<(), String> {
    let path = kernel_paths().primary_stage;
    let bytes = fs::read(&path).map_err(|error| {
        format!(
            "kernel artifact is unavailable at {}: {error}",
            path.display()
        )
    })?;
    let image = validate_kernel_contract(&bytes)?;
    audit_kernel_markers(&bytes)?;
    print!("{}", render_summary(&image, sha256::digest(&bytes)));
    Ok(())
}

const KERNEL_P1I_MARKERS: [&[u8]; 5] = [
    b"UNOS:P1I:KERNEL_ENTRY",
    b"UNOS:P1I:STACK_OK",
    b"UNOS:P1I:BOOTINFO_OK",
    b"UNOS:P1I:MEMORY_MAP_OK",
    b"UNOS:P1I:PASS",
];

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
fn audit_kernel_markers(bytes: &[u8]) -> Result<(), String> {
    for marker in KERNEL_P1I_MARKERS {
        if !contains_bytes(bytes, marker) {
            return Err(format!(
                "kernel is missing P1I marker {}",
                String::from_utf8_lossy(marker)
            ));
        }
    }
    Ok(())
}
pub(crate) fn audit_handoff_artifacts(kernel: &Path, bootloader: &Path) -> Result<(), String> {
    let kernel_bytes =
        fs::read(kernel).map_err(|error| format!("kernel artifact audit failed: {error}"))?;
    let loader_bytes = fs::read(bootloader)
        .map_err(|error| format!("bootloader artifact audit failed: {error}"))?;
    validate_kernel_contract(&kernel_bytes)?;
    audit_kernel_markers(&kernel_bytes)?;
    for marker in KERNEL_P1I_MARKERS {
        if contains_bytes(&loader_bytes, marker) {
            return Err(format!(
                "bootloader contains kernel-only marker {}",
                String::from_utf8_lossy(marker)
            ));
        }
    }
    Ok(())
}

pub fn build_boot() -> Result<(), String> {
    let paths = kernel_paths();
    remove_directory_if_present(&paths.esp_root)?;
    let kernel = build_and_stage_kernel()?;
    let bootloader = uefi::build_uefi_for_boot()?;
    verify_boot_esp(&paths.esp_root)?;
    println!("boot.contract=staged");
    println!("boot.bootloader={}", bootloader.display());
    println!("boot.kernel={}", kernel.esp.display());
    println!("boot.kernel_sha256={}", sha256::hex(kernel.digest));
    Ok(())
}

struct KernelPaths {
    source_artifact: PathBuf,
    esp_root: PathBuf,
    primary_stage: PathBuf,
    esp_stage: PathBuf,
}

struct StagedKernel {
    primary: PathBuf,
    esp: PathBuf,
    digest: [u8; 32],
}

fn kernel_paths() -> KernelPaths {
    let root = repository_root();
    let output_root = root.join("target").join("unnamedos");
    let esp_root = output_root.join("esp");
    KernelPaths {
        source_artifact: root
            .join("target")
            .join(KERNEL_TARGET)
            .join("debug")
            .join(KERNEL_BINARY),
        primary_stage: output_root.join("kernel").join("unnamedos-kernel.elf"),
        esp_stage: esp_root.join("EFI").join("UNNAMEDOS").join("KERNEL.ELF"),
        esp_root,
    }
}

fn build_and_stage_kernel() -> Result<StagedKernel, String> {
    let paths = kernel_paths();
    clear_kernel_staging(&paths)?;
    remove_file_if_present(&paths.source_artifact)?;

    let status = Command::new("cargo")
        .current_dir(repository_root())
        .args(["build", "-p", "kernel", "--target", KERNEL_TARGET])
        .status()
        .map_err(|error| format!("could not start kernel build: {error}"))?;
    if !status.success() {
        return Err(format!("kernel build failed with {status}"));
    }

    normalize_rust_elf_os_abi(&paths.source_artifact)?;
    stage_kernel(&paths, &paths.source_artifact)
}

fn normalize_rust_elf_os_abi(artifact: &Path) -> Result<(), String> {
    let mut bytes = fs::read(artifact).map_err(|error| {
        format!(
            "fresh kernel artifact is missing or unreadable at {}: {error}",
            artifact.display()
        )
    })?;
    if bytes.len() < 8 || bytes[..4] != [0x7f, b'E', b'L', b'F'] {
        return Err("fresh kernel artifact is not an ELF image".to_owned());
    }
    match bytes[7] {
        0 => Ok(()),
        3 => {
            bytes[7] = 0;
            fs::write(artifact, bytes).map_err(|error| {
                format!(
                    "could not normalize kernel ELFOSABI_NONE at {}: {error}",
                    artifact.display()
                )
            })
        }
        value => Err(format!(
            "kernel toolchain emitted unsupported ELF OS ABI {value}"
        )),
    }
}

fn stage_kernel(paths: &KernelPaths, artifact: &Path) -> Result<StagedKernel, String> {
    clear_kernel_staging(paths)?;
    let bytes = fs::read(artifact).map_err(|error| {
        format!(
            "fresh kernel artifact is missing or unreadable at {}: {error}",
            artifact.display()
        )
    })?;
    validate_kernel_contract(&bytes)?;

    create_parent(&paths.primary_stage)?;
    create_parent(&paths.esp_stage)?;
    fs::copy(artifact, &paths.primary_stage)
        .map_err(|error| format!("could not stage primary kernel artifact: {error}"))?;
    fs::copy(artifact, &paths.esp_stage)
        .map_err(|error| format!("could not stage ESP kernel artifact: {error}"))?;

    let source_digest = sha256::digest(&bytes);
    let primary_digest = sha256::file_digest(&paths.primary_stage)?;
    let esp_digest = sha256::file_digest(&paths.esp_stage)?;
    if source_digest != primary_digest || source_digest != esp_digest {
        clear_kernel_staging(paths)?;
        return Err("kernel staging SHA-256 mismatch".to_owned());
    }

    Ok(StagedKernel {
        primary: paths.primary_stage.clone(),
        esp: paths.esp_stage.clone(),
        digest: source_digest,
    })
}

fn validate_kernel_contract(bytes: &[u8]) -> Result<ValidatedImage<'_>, String> {
    let image = validate_bootstrap_image(bytes)
        .map_err(|error| format!("kernel ELF contract violation: {error:?}"))?;
    Ok(image)
}

fn render_summary(image: &ValidatedImage<'_>, digest: [u8; 32]) -> String {
    let mut output = String::new();
    output.push_str("contract=valid\n");
    output.push_str("elf.class=ELF64\n");
    output.push_str("elf.endianness=little\n");
    output.push_str("elf.object_type=ET_EXEC\n");
    output.push_str("elf.machine=EM_X86_64\n");
    output.push_str(&format!("elf.entry={:#018x}\n", image.entry()));
    output.push_str(&format!(
        "elf.program_headers={}\n",
        image.program_header_count()
    ));
    output.push_str(&format!(
        "elf.load_segments={}\n",
        image.load_segment_count()
    ));
    for (index, segment) in image.load_segments().enumerate() {
        render_segment(&mut output, index, segment);
    }
    let (start, end) = image.load_address_range();
    output.push_str(&format!("elf.load_range={start:#018x}..{end:#018x}\n"));
    output.push_str(&format!("elf.sha256={}\n", sha256::hex(digest)));
    output
}

fn render_segment(output: &mut String, index: usize, segment: LoadSegment) {
    let file_end = segment.file_offset() + segment.file_size();
    let memory_end = segment.address() + segment.memory_size();
    let flags = [
        if segment.is_readable() { 'R' } else { '-' },
        if segment.is_writable() { 'W' } else { '-' },
        if segment.is_executable() { 'X' } else { '-' },
    ];
    output.push_str(&format!(
        "elf.load.{index}=file:{:#018x}..{file_end:#018x},memory:{:#018x}..{memory_end:#018x},filesz:{:#x},memsz:{:#x},flags:{}{}{},align:{:#x},pages:{}\n",
        segment.file_offset(),
        segment.address(),
        segment.file_size(),
        segment.memory_size(),
        flags[0],
        flags[1],
        flags[2],
        segment.alignment(),
        segment
            .page_count(BOOTSTRAP_PAGE_SIZE)
            .expect("validated segment and fixed page size")
    ));
}

fn verify_boot_esp(esp_root: &Path) -> Result<(), String> {
    for relative in ["EFI/BOOT/BOOTX64.EFI", "EFI/UNNAMEDOS/KERNEL.ELF"] {
        let path = esp_root.join(relative);
        let length = fs::metadata(&path)
            .map_err(|error| {
                format!(
                    "boot ESP artifact is missing at {}: {error}",
                    path.display()
                )
            })?
            .len();
        if length == 0 {
            return Err(format!("boot ESP artifact is empty: {}", path.display()));
        }
    }
    Ok(())
}

fn clear_kernel_staging(paths: &KernelPaths) -> Result<(), String> {
    remove_file_if_present(&paths.primary_stage)?;
    remove_file_if_present(&paths.esp_stage)
}

fn create_parent(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {}: {error}", parent.display()))
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

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask must live at crates/xtask")
        .to_path_buf()
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let unique = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "unnamedos-kernel-{label}-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn test_paths(root: &Path) -> KernelPaths {
        let output_root = root.join("target/unnamedos");
        let esp_root = output_root.join("esp");
        KernelPaths {
            source_artifact: root.join("artifact/unnamedos-kernel"),
            primary_stage: output_root.join("kernel/unnamedos-kernel.elf"),
            esp_stage: esp_root.join("EFI/UNNAMEDOS/KERNEL.ELF"),
            esp_root,
        }
    }

    pub(crate) fn valid_kernel() -> Vec<u8> {
        let mut bytes = vec![0_u8; 0x3010];
        bytes[..16].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        put_u16(&mut bytes, 16, 2);
        put_u16(&mut bytes, 18, 62);
        put_u32(&mut bytes, 20, 1);
        put_u64(&mut bytes, 24, kernel_image::BOOTSTRAP_LINK_ADDRESS);
        put_u64(&mut bytes, 32, 64);
        put_u16(&mut bytes, 52, 64);
        put_u16(&mut bytes, 54, 56);
        put_u16(&mut bytes, 56, 3);
        for (index, offset, address, filesz, memsz, flags) in [
            (0, 0x1000, 0x200000, 16, 16, 5),
            (1, 0x2000, 0x201000, 16, 16, 4),
            (2, 0x3000, 0x202000, 16, 0x1010, 6),
        ] {
            let header = 64 + index * 56;
            put_u32(&mut bytes, header, 1);
            put_u32(&mut bytes, header + 4, flags);
            put_u64(&mut bytes, header + 8, offset);
            put_u64(&mut bytes, header + 16, address);
            put_u64(&mut bytes, header + 24, address);
            put_u64(&mut bytes, header + 32, filesz);
            put_u64(&mut bytes, header + 40, memsz);
            put_u64(&mut bytes, header + 48, 4096);
        }
        bytes
    }

    fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    #[test]
    fn kernel_staging_paths_are_fixed() {
        let paths = test_paths(Path::new("repo"));
        assert!(
            paths
                .primary_stage
                .ends_with("target/unnamedos/kernel/unnamedos-kernel.elf")
        );
        assert!(
            paths
                .esp_stage
                .ends_with("target/unnamedos/esp/EFI/UNNAMEDOS/KERNEL.ELF")
        );
    }

    #[test]
    fn missing_or_invalid_artifact_removes_stale_staging() {
        let directory = TestDirectory::new("stale");
        let paths = test_paths(&directory.0);
        create_parent(&paths.primary_stage).expect("primary parent");
        create_parent(&paths.esp_stage).expect("esp parent");
        fs::write(&paths.primary_stage, b"stale").expect("stale primary");
        fs::write(&paths.esp_stage, b"stale").expect("stale esp");
        assert!(stage_kernel(&paths, &paths.source_artifact).is_err());
        assert!(!paths.primary_stage.exists());
        assert!(!paths.esp_stage.exists());

        create_parent(&paths.source_artifact).expect("artifact parent");
        fs::write(&paths.source_artifact, b"not an ELF").expect("invalid artifact");
        assert!(stage_kernel(&paths, &paths.source_artifact).is_err());
        assert!(!paths.primary_stage.exists());
        assert!(!paths.esp_stage.exists());
    }

    #[test]
    fn valid_artifact_stages_identical_hashes_in_paths_with_spaces() {
        let directory = TestDirectory::new("path with spaces");
        let paths = test_paths(&directory.0);
        create_parent(&paths.source_artifact).expect("artifact parent");
        fs::write(&paths.source_artifact, valid_kernel()).expect("artifact");
        let staged = stage_kernel(&paths, &paths.source_artifact).expect("valid staging");
        assert_eq!(sha256::file_digest(&staged.primary), Ok(staged.digest));
        assert_eq!(sha256::file_digest(&staged.esp), Ok(staged.digest));
    }

    #[test]
    fn rust_gnu_osabi_marker_is_normalized_before_validation() {
        let directory = TestDirectory::new("osabi");
        let artifact = directory.0.join("kernel.elf");
        let mut bytes = valid_kernel();
        bytes[7] = 3;
        fs::write(&artifact, bytes).expect("artifact");
        normalize_rust_elf_os_abi(&artifact).expect("normalization");
        let normalized = fs::read(artifact).expect("normalized artifact");
        assert_eq!(normalized[7], 0);
        validate_kernel_contract(&normalized).expect("normalized contract");
    }

    #[test]
    fn clean_esp_can_contain_bootloader_and_kernel_together() {
        let directory = TestDirectory::new("combined");
        let paths = test_paths(&directory.0);
        fs::create_dir_all(&paths.esp_root).expect("esp");
        fs::write(paths.esp_root.join("stale.bin"), b"stale").expect("stale");
        remove_directory_if_present(&paths.esp_root).expect("clean esp");
        create_parent(&paths.source_artifact).expect("artifact parent");
        fs::write(&paths.source_artifact, valid_kernel()).expect("artifact");
        stage_kernel(&paths, &paths.source_artifact).expect("kernel staging");
        let bootloader = paths.esp_root.join("EFI/BOOT/BOOTX64.EFI");
        create_parent(&bootloader).expect("boot parent");
        fs::write(&bootloader, b"efi").expect("bootloader");
        verify_boot_esp(&paths.esp_root).expect("complete esp");
        assert!(!paths.esp_root.join("stale.bin").exists());
    }

    #[test]
    fn inspect_summary_is_deterministic() {
        let bytes = valid_kernel();
        let image = validate_kernel_contract(&bytes).expect("valid contract");
        let first = render_summary(&image, sha256::digest(&bytes));
        let second = render_summary(&image, sha256::digest(&bytes));
        assert_eq!(first, second);
        assert!(first.starts_with("contract=valid\nelf.class=ELF64\n"));
        assert!(first.contains("flags:R-X"));
        assert!(first.contains("flags:RW-"));
        assert!(first.contains("elf.sha256="));
    }
}
