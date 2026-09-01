# ADR-0006: ELF64 bootstrap kernel image szerződés

- Állapot: Accepted
- Dátum: 2026-08-30
- Döntéshozók: UnnamedOS projekt

## Phase 1D-G kiegészítés

Az ELF `_start` most assembly stub és tényleges handoff célpont. Az artifact-audit továbbra is tiltja a relocationt, interpretert és dynamic szegmenst, továbbá megköveteli a kernel P1I markereit és kizárja a kernel-only markereket a bootloaderből.

## Kontextus

A külön UEFI loader és kernel közötti következő határhoz determinisztikus, rosszindulatú bemeneten is biztonságosan validálható kernelimage szükséges. A Phase 1D-A csak az artifactot és a betöltési szerződést hozza létre; betöltés, handoff és firmware-életciklus-váltás még nincs.

## Döntés

A bootstrap kernel dependency-mentes, `no_std`/`no_main`, `x86_64-unknown-none` Rust bináris, explicit `_start` szimbólummal. Az image ELF64, little-endian, ELF version 1, `ELFOSABI_NONE`, `ET_EXEC`, `EM_X86_64`; a program header mérete 56 byte. Ideiglenes identity-mapped link- és entrycíme `0x0020_0000`. Ez nem a végleges magas címes kernelmodell.

A linker script crate-lokális `-T` és `-no-pie` argumentumokkal készít 4 KiB-aligned RX `.text`, R `.rodata`, valamint RW `.data`/`.bss` `PT_LOAD` szegmenseket. A Rust/LLVM objektumok GNU OSABI jelölését a friss link után az xtask kizárólag az ELF identification OSABI byte-ján determinisztikusan `ELFOSABI_NONE` értékre normalizálja, majd a teljes image-et újravalidálja. Régi artifact normalizálása vagy stagingje nem engedélyezett.

Nem megengedett `PT_INTERP`, `PT_DYNAMIC`, `PT_TLS`, runtime relocation, W+X szegmens, `p_filesz > p_memsz`, nem canonical vagy átfedő címtartomány, hibás alignment/kongruencia, illetve eltérő `p_paddr` és `p_vaddr`. Az entrynek végrehajtható `PT_LOAD` szegmensbe kell esnie. Minden fájl- és címtartomány-művelet overflow-biztos.

A dependency-mentes, unsafe nélküli `kernel-image` crate byte slice-ból, explicit little-endian dekódolással ad validált image view-t és biztonságos `PT_LOAD` iterátort. Fizikai címet nem dereferál és szegmensadatot nem másol.

## Staging

- Elsődleges artifact: `target/unnamedos/kernel/unnamedos-kernel.elf`.
- ESP artifact: `target/unnamedos/esp/EFI/UNNAMEDOS/KERNEL.ELF`.
- Loader: `target/unnamedos/esp/EFI/BOOT/BOOTX64.EFI`.

A két kernelpéldány SHA-256 hashének egyeznie kell. A `build-boot` tiszta ESP-t készít; az önálló `build-uefi` nem törölheti a staged kernelt.

## Következmények

- A loader kizárólag a validált szerződésnek megfelelő image-et fogadja el; Phase 1D-C-ben a formátumvalidációtól külön load-policy alapján ideiglenesen célmemóriába másolja a `PT_LOAD` szegmenseket.
- A bootstrap identity mapping egyszerűsíti az első handoffot, de később külön ADR-ben magas címes modellre cserélendő.
- A Phase 1D-D-ben a teljesen ellenőrzött image metadata és céloldalai előbb `LoadedKernel` ownershipbe kerülnek, a forrás ELF felszabadul, majd a teszt végén a céloldalak is explicit felszabadulnak. A kernel továbbra sem fut; nincs `ExitBootServices`, memóriaátadás vagy lapozásmódosítás.
- A Phase 1D-E a `LoadedKernel` metadata lapjaiból kernel-image rezervációkat képez a provisional boot mapban; ehhez nem tart meg ELF-referenciát. A boot info validációja után a bootadatlapok, majd a kernelszegmensek felszabadulnak.

## Ellenőrzés

A host fixture-tesztek minden szerződéssértést strukturált hibával ellenőriznek. A `build-kernel` és a loader a tényleges artifactot ugyanazzal a `kernel-image` validátorral fogadja el; az `inspect-kernel` rögzíti az entryt, program headereket, szegmenstartományokat, W^X tulajdonságot, BSS-t, teljes load range-et és SHA-256 hash-t. A QEMU-szcenáriók az érvényes, hiányzó, ELF-magic szinten sérült és ELF-valid, de load-policyt sértő staged image-et külön ESP-n ellenőrzik.
