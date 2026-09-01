use std::env;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let linker_script = manifest.join("linker.ld");

    println!("cargo:rerun-if-changed={}", linker_script.display());
    println!(
        "cargo:rustc-link-arg-bin=unnamedos-kernel=-T{}",
        linker_script.display()
    );
    println!("cargo:rustc-link-arg-bin=unnamedos-kernel=-no-pie");
    println!("cargo:rustc-link-arg-bin=unnamedos-kernel=--no-dynamic-linker");
}
