# Repository scope

UnnamedOS Core publishes the reviewed foundation needed to build and test the current boot path: the versioned boot protocol, UEFI loader, ELF validator and loader, bootstrap kernel, host tooling, public architecture decisions, and deterministic tests.

The private canonical development repository also contains internal project operations and may contain unpublished or proprietary product layers. Those materials are not dependencies of this repository and are not required to build its current workspace. Public releases are generated from an explicit allowlist and receive independent Git history.

This repository does not currently provide the product desktop, general-purpose drivers, application platform, installer, updater, networking stack, browser, release signing infrastructure, firmware, or production security assurances.
