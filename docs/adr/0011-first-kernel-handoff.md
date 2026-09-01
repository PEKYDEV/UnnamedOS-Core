# ADR-0011: Első x86-64 kernelhandoff

- Állapot: Accepted
- Dátum: 2026-08-30
- Döntéshozók: UnnamedOS projekt

## Kontextus

A final `BootInfo` és a firmware nélküli ownership már rendelkezésre áll. A kernel első tényleges végrehajtásához explicit stack-, regiszter- és paging-szerződés kell, amely nem támaszkodik Rust ABI-ra vagy UEFI-típusokra.

## Döntés

A referencia-handoff x86-64 long mode-ban történik, az UEFI-től örökölt lapozás változtatása nélkül. Az identity mapping kizárólag az aktuális QEMU q35 + OVMF bootstrap-platform szerződése. A loader `cli`, `cld`, `RBP=0` állapotot hoz létre, `RDI`-ben a final `BootInfo` címét, `RSI`-ben a stack alját, `RDX`-ben a stack kizáró felső címét adja át. `RSP` a 16 byte-aligned felső cím; nincs visszatérési cím, az átadás `jmp`, a kernel nem térhet vissza.

A `BootstrapStack` 16 darab 4 KiB-os `LOADER_DATA` lap, 4 GiB alatt. A teljes 64 KiB inicializált, az alsó igazított szó sentinel/canary, nem guard page. A stack külön owner: pre-exit hiba esetén firmware-free történik, ownership-transfer után backend és destruktor nélkül él tovább. A final map `BOOTSTRAP_STACK` overlayként, nem usable tartományként jelöli.

Az izolált loader assembly csak validált scalar argumentumokat fogad, kiírja a `HANDOFF_READY` markert, beállítja a gépállapotot és ugrik. A kernel stabil `global_asm!` `_start` stubja megőrzi az argumentumokat és a belépési `RSP`-t, majd System V `extern "C"` Rust bootstrapot hív; red zone tiltott. A kernel saját COM1/debug-exit I/O-t használ.

A kernel a `KERNEL_ENTRY` után egy dokumentált unsafe határon olvassa a `BootInfo`-t, canaryt és descriptor tömböt. Ellenőrzi a pointert, a pontos stackhatárokat, a teljes boot-protocolt és azt, hogy a kernel, stack, `BootInfo` és mapbuffer teljes tartománya nem usable descriptorokkal fedett.

## Következmények

- A kernel ténylegesen fut és ő adja a P1I sikeres debug-exitet.
- Saját CR3/paging, GDT, IDT, exception- vagy interruptkezelés továbbra sincs.
- Az örökölt identity mappinget a következő memória-architektúra munkacsomagnak kell kiváltania.

## Ellenőrzés

A `cargo xtask test-kernel-handoff` artifact-auditot és q35+OVMF futást végez. A P1H után pontosan egyszer várja a hat P1I markert és 33-as processexitet. Hosttesztek fedik a stack policyt, rollbacket/transfert, reserved overlayt, argumentumokat, canaryt, BootInfo-t és maprezervációkat.
