# ADR-0001: Vékony UEFI loader és külön ELF64 kernel

- Állapot: Accepted
- Dátum: 2026-08-29
- Döntéshozók: UnnamedOS projekt

## Phase 1D-G kiegészítés

A külön `BootstrapStack` owner teszi teljessé a handoff typestate-et. A final P1H állapotból dokumentált x86-64 assembly `jmp` adja át a vezérlést a kernel `_start` entryjének. A kernel saját COM1 kódja és debug-exitje bizonyítja, hogy ténylegesen fut; a paging még változatlanul UEFI-örökölt, saját GDT/IDT és interruptkezelés nincs.

## Kontextus

A bootfolyamatnak UEFI-ről kell indulnia, firmware-adatokat kell átadnia, majd a firmware-életciklustól elválasztott `no_std` kernelbe kell lépnie.

## Megfontolt lehetőségek

1. Egyetlen UEFI alkalmazásban maradó kernel: egyszerű kezdet, de összemossa a firmware- és kernelhatárt.
2. Vékony saját UEFI loader és külön ELF64 kernel: több tooling, de tiszta és tesztelhető handoff.
3. Külső bootkomponens: gyorsabb indulás, de külső architekturális és licenckötés.

## Döntés

Az első bootút egy vékony, Rustban készülő UEFI loaderből és egy külön ELF64 `no_std` kernelből áll. A loader kizárólag a kernel betöltését, a szükséges UEFI-adatok begyűjtését, a verziózott boot information összeállítását, az `ExitBootServices` végrehajtását és a dokumentált kernelhandoffot végzi.

A Phase 1C-ben megvalósított első lépcső önálló `x86_64-unknown-uefi`, `no_std`/`no_main` alkalmazás. A `uefi` crate `0.39.0` minimális, alapértelmezett feature-ök nélküli konfigurációját használja; heapet és allocatort nem aktivál. A firmware-entry után safe Rust vezérléssel inicializálja a COM1 portot és kiírja a stabil Phase 1C markereket. Az unsafe határ kizárólag a dokumentált x86-64 port I/O.

A normál build sikeres validáció után `Status::SUCCESS`, loaderhiba esetén `Status::LOAD_ERROR` értékkel tér vissza a firmware-be. A `qemu-test` compile-time feature külön `0xF4` debug-exit portot használ az automatizált teszt lezárásához; normál buildben ez a kódút nincs jelen.

A Phase 1D-A létrehozza a külön, dependency-mentes `x86_64-unknown-none` kernelartifactot és a saját `no_std` ELF-validátort. A Phase 1D-B loader ugyanazon EFI filesystem fix `\EFI\UNNAMEDOS\KERNEL.ELF` útvonaláról, read-only regular file-ként olvassa az image-et. A 16 MiB-os korlátot seek/get-position/rewind után ellenőrzi, `LOADER_DATA` lapokat foglal, teljes és EOF-ellenőrzött olvasást végez, ugyanazzal a `kernel-image` szerződéssel validál, majd a lapokat felszabadítja. Globális allocator vagy `alloc` feature nincs.

Ebben a lépcsőben a validált image csak scratch memóriában létezik: nincs `PT_LOAD` szegmensmásolás, `ExitBootServices`, handoff vagy kernelvégrehajtás.

A Phase 1D-C a validált image-ből unsafe-mentes load plant képez, a `PT_LOAD` céloldalakat fixed-address UEFI allokációkkal ideiglenesen lefoglalja, teljesen nullázza, feltölti és byte-onként ellenőrzi. A forrás scratch buffer a validáció és ellenőrzés végéig él; a P1D `PASS` ezért validációs mérföldkő, nem ownership-lezárás. P1E siker esetén előbb a forrás, majd fordított sorrendben minden célallokáció felszabadul. Kernel-entry, handoff és `ExitBootServices` továbbra sincs.

A Phase 1D-D a teljes load-ellenőrzés után a célallokációkat `LoadedKernel` objektumba adja át. A forrás scratch buffer ezután felszabadítható, miközben a másolt metadata és a célownership változatlan marad. A fázis végén a céloldalak explicit, retry-képes release útvonalon felszabadulnak; publikus ownership-disarm, raw handoff vagy entrypoint-hívás nincs.

A Phase 1D-E a még élő `LoadedKernel` mellett, előre lefoglalt lapokból allocatormentes `PreparedBootInfo` objektumot épít. GOP-adatokat és provisional UEFI mapot konvertál a `boot-protocol` formátumára, explicit rezervációkkal validálja, majd a tesztben felszabadítja. Ez nem final map freeze: nincs `ExitBootServices`, ownership-disarm, boot-info átadás vagy kernel-entry.

A Phase 1D-F külön tesztútja a `LoadedKernel` és `PreparedBootInfo` ownershipet exit-ready typestate-be fogyasztja, friss map keyjel atomikusan végrehajtja az `ExitBootServices` határt, majd allocatormentesen finalizálja a mapot és a `BootInfo` példányt. A kernel entrypointját továbbra sem hívja meg.

## Következmények

- A loader és a kernel külön targettel és artefakttal épül.
- Az ELF64 csak konténerformátum; nem hoz Linux ABI-függést.
- A handoffhoz explicit stack-, belépési pont-, memóriatulajdon- és hibakezelési szerződés szükséges.
- Külső loader runtime-komponens nem kerül be automatikusan.

## Ellenőrzés

A Phase 1C a loader-entryt valós headless QEMU/OVMF boottal igazolja. A Phase 1D-A a külön ELF64 kernel build- és staging-szerződését a tényleges artifacton igazolja. A Phase 1D-B az érvényes image elfogadását és a fájl/ELF hibákat bizonyítja. A Phase 1D-C pozitív fixed-address másolási futást és külön load-policy negatív futást ad. A Phase 1D-D az ownershipet, a Phase 1D-E a GOP/memory-map konverziót és release-t, a Phase 1D-F pedig a final mapot, firmware-exitet és post-exit ownershipet bizonyítja. A teljes döntést a későbbi handoff-lépcső akkor igazolja, ha a kernel saját soros mérföldkőüzenetet ad.
