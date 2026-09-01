# Contributing

Thank you for considering a contribution to UnnamedOS Core.

Before opening a pull request, discuss substantial architectural changes in an issue and keep each change narrowly scoped. New wire formats, unsafe boundaries, firmware assumptions, ownership transfers, or dependency exceptions require an ADR or an update to an existing ADR.

Run the supported quality gate:

```text
cargo xtask check
cargo xtask build-kernel
cargo xtask inspect-kernel
```

When QEMU and OVMF are available, also run all three QEMU scenarios documented in the README. Add deterministic tests for behavior changes and explain any unsafe code with a local safety argument. Do not commit firmware, disk images, build outputs, logs, secrets, or machine-specific configuration.

By submitting a contribution, you agree that it may be distributed under Apache-2.0 and that you have the right to submit it. Follow the [Code of Conduct](CODE_OF_CONDUCT.md). Security reports must use the private process in [SECURITY.md](SECURITY.md).
