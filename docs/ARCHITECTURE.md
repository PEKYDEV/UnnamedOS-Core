# Core architecture

The reference platform is QEMU q35 with OVMF on x86-64. A thin UEFI application validates a separately linked ELF64 kernel, plans and owns fixed-address page allocations, copies loadable segments, builds allocator-free boot metadata, exits boot services exactly once, and transfers control using the System V AMD64 calling convention.

The loader and kernel communicate only through `boot-protocol`, a `no_std`, C-compatible, little-endian wire ABI. Raw physical addresses remain integers and are never dereferenced during protocol validation. Version, reserved fields, strides, counts, ranges, and framebuffer geometry are checked with overflow-safe arithmetic before a validated view is produced.

The current bootstrap kernel validates the handoff, emits deterministic serial evidence, and terminates the reference QEMU scenario. It intentionally provides no scheduler, allocator-backed runtime, driver framework, userspace, or graphical shell.

Normative decisions and exact layouts are recorded in [`docs/adr`](adr).
