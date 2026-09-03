# Third-party components

Copyright 2026 Pechlof Erik

The project-owned code in UnnamedOS Core is licensed under Apache-2.0. Cargo dependencies are fetched from crates.io and are not vendored in this repository. Versions below are locked by `Cargo.lock`.

## Runtime dependency tree

| Component | Version | License | Role |
|---|---:|---|---|
| `uefi` | 0.39.0 | MIT OR Apache-2.0 | UEFI entry point and firmware types |
| `uefi-raw` | 0.15.1 | MIT OR Apache-2.0 | Raw UEFI ABI |
| `uguid` | 2.2.1 | MIT OR Apache-2.0 | GUID representation |
| `bitflags` | 2.13.1 | MIT OR Apache-2.0 | UEFI flag types |
| `cfg-if` | 1.0.4 | MIT OR Apache-2.0 | Target configuration |
| `log` | 0.4.34 | MIT OR Apache-2.0 | Internal logging API; logger features are disabled |
| `ptr_meta` | 0.3.2 | MIT | Pointer metadata support |
| `ucs2` | 0.3.3 | MPL-2.0 | UEFI UCS-2 text types |
| `bit_field` | 0.10.3 | MIT OR Apache-2.0 | Transitive bit operations for `ucs2` |

Build-only procedural macro dependencies are `uefi-macros 0.19.0`, `ptr_meta_derive 0.3.2`, `proc-macro2 1.0.107`, `quote 1.0.47`, `syn 2.0.119`, `syn 3.0.4`, and `unicode-ident 1.0.24`. They are licensed under MIT, Apache-2.0, or both; `unicode-ident` additionally carries Unicode-3.0 terms.

The workspace-owned `boot-protocol`, `kernel-image`, `memory-layout`, and `xtask` crates have no external runtime dependencies. The page-table planner, owner, materializer, verifier, read-only CPUID/control-state probe, higher-half linker, and dual-address planning code are allocator-free project code and add no dependency or license exception. The `kernel` depends only on `boot-protocol`; the bootloader's `memory-layout` dependency is workspace-owned. No QEMU or firmware binary is distributed by this repository.

## Scoped MPL-2.0 exception: `ucs2 0.3.3`

`ucs2 0.3.3` is accepted only as the unmodified transitive dependency `bootloader -> uefi 0.39.0 -> ucs2 0.3.3`. Its crates.io checksum is `df79298e11f316400c57ec268f3c2c29ac3c4d4777687955cd3d4f3a35ce7eba`. The package is statically linked into the loader but is neither vendored nor modified here. Its corresponding source is available from [crates.io](https://crates.io/crates/ucs2/0.3.3) and the upstream [`rust-osdev/ucs2`](https://github.com/rust-osdev/ucs2) repository. Direct use, modification, replacement, or a version change requires a new license review.

The MPL-2.0 text is preserved at [`LICENSES/MPL-2.0.txt`](LICENSES/MPL-2.0.txt). Common MIT and Unicode-3.0 texts are preserved at [`LICENSES/MIT.txt`](LICENSES/MIT.txt) and [`LICENSES/Unicode-3.0.txt`](LICENSES/Unicode-3.0.txt). The repository root [`LICENSE`](LICENSE) contains Apache-2.0.

## Development and CI tools

Rust 1.98.0 is pinned for builds and is licensed under MIT OR Apache-2.0. QEMU and OVMF are optional, externally installed test tools and are not distributed. The CI workflow pins `actions/checkout` 7.0.1 by commit; it is CI-only and MIT-licensed.
