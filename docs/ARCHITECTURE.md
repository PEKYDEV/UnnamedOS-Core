# Core architecture

The reference platform is QEMU q35 with OVMF on x86-64. A thin UEFI application validates a separately linked ELF64 kernel, plans and owns fixed-address page allocations, copies loadable segments, builds allocator-free boot metadata, exits boot services exactly once, and transfers control using the System V AMD64 calling convention.

The loader and kernel communicate only through `boot-protocol`, a `no_std`, C-compatible, little-endian wire ABI. Raw physical addresses remain integers and are never dereferenced during protocol validation. Version, reserved fields, strides, counts, ranges, and framebuffer geometry are checked with overflow-safe arithmetic before a validated view is produced.

The current bootstrap kernel validates the handoff, emits deterministic serial evidence, and terminates the reference QEMU scenario. It intentionally provides no scheduler, allocator-backed runtime, driver framework, userspace, or graphical shell.

Phase 1J-A accepts a four-level, 4 KiB, 48-bit-canonical address-space contract. The lower half is reserved for future userspace. The higher half contains a 64 TiB RAM-only direct map at `0xffff800000000000..0xffffc00000000000`, dedicated kernel-service, MMIO and framebuffer regions, a kernel-image window at `0xffffffff80000000..0xffffffffc0000000`, and kernel-local space. Kernel mappings are supervisor-only and W^X; MMIO/framebuffer mappings are initially uncached. The transition permits only one RX trampoline page plus the 16-page RW+NX bootstrap stack below 4 GiB, and the final plan permits no low mapping.

The loader will allocate and own the initial hierarchy until the non-returning CR3 handoff; the kernel will then remove transition mappings before enabling interrupts or allocators. The dependency-free `memory-layout` crate validates this plan without allocation, address dereferences, privileged instructions, or unsafe code. Runtime page-table construction and CR3 modification are not implemented yet.

Phase 1J-B deterministically expands validated mappings into an inactive PML4/PDPT/PD/PT plan with fixed capacities, monotonic frame slots, exact parent indices, leaf flags, canonical serialization, and one lower-PML4 removal operation per temporary alias region. A backend-neutral owner proves exact allocation, zeroing, reverse rollback, retryable release, and explicit transfer. Required ownership metadata is reserved for a future major-2 linear extension envelope so the current 128-byte v1.0 BootInfo remains unchanged and older kernels reject the new contract. Production materialization and activation remain unimplemented.

Normative decisions and exact layouts are recorded in [`docs/adr`](adr).
