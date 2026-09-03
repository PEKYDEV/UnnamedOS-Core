# UnnamedOS Core

UnnamedOS Core is the open-source boot and kernel foundation of an experimental x86-64 operating system. The current implementation boots through UEFI, validates and loads a separate ELF64 kernel, exits firmware boot services, and transfers a versioned boot-information contract to the kernel.

This project is early-stage systems software. It is not suitable for production use or for processing sensitive data. The graphical desktop, general hardware support, application platform, installer, updater, networking stack, and browser are not complete.

## Current capabilities

- `no_std` versioned boot-information ABI with strict host-side validation
- thin UEFI loader and separately linked ELF64 bootstrap kernel
- bounded ELF parsing, fixed-address segment loading, and explicit ownership transfer
- allocator-free memory-map and framebuffer metadata preparation
- dependency-free, host-tested contract for the first owned higher-half address space
- deterministic QEMU q35/OVMF tests through the first kernel handoff
- host tests, formatting, Clippy, cross-target builds, and artifact inspection through `xtask`

The architecture favors narrow contracts, checked arithmetic, explicit state transitions, and deterministic failure over implicit platform behavior. See [Architecture](docs/ARCHITECTURE.md), the [ADRs](docs/adr), and the [repository scope](docs/REPOSITORY_SCOPE.md).

## Build and test

Install Rust through rustup; the pinned toolchain and targets are selected by `rust-toolchain.toml`. QEMU x86-64 and a matching OVMF CODE/VARS pair are required only for firmware tests.

```text
cargo xtask doctor
cargo xtask check
cargo xtask build-boot
cargo xtask test-uefi
cargo xtask test-exit-boot-services
cargo xtask test-kernel-handoff
cargo xtask test-page-tables
```

`doctor` reports optional environment prerequisites and may return a diagnostic failure when QEMU or OVMF is missing. `check` remains the host-side quality gate. The QEMU commands use deterministic serial markers and debug-exit status codes; the final handoff scenario exits with status 33 when successful.

## Development and security

UnnamedOS follows a human-centered development process in which AI serves as an integrated tool supporting human work. Every change undergoes deterministic testing, license checks, and architectural review. Security-critical public releases will be preceded by independent expert audits.

Treat all current releases as experimental. Please review [CONTRIBUTING.md](CONTRIBUTING.md) before proposing changes. Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md), never through a public issue.

Phase 1J-C allocates the exact UEFI `LOADER_DATA` frames for the bounded transition plan, materializes and independently verifies all table entries, reserves every owned frame in the final boot map, and transfers inactive ownership across `ExitBootServices`. The reference plan uses five table frames. CPU probing, the complete higher-half hierarchy, BootInfo v2, and CR3 modification remain deferred. See [docs/ROADMAP.md](docs/ROADMAP.md).

## License and names

Project-owned source in this repository is licensed under the [Apache License 2.0](LICENSE). Third-party components retain their own licenses; see [THIRD_PARTY.md](THIRD_PARTY.md). The software license grants no rights to the UnnamedOS or Aevra names, logos, visual identity, or branding; see [TRADEMARKS.md](TRADEMARKS.md).
