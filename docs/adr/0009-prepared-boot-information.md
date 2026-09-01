# ADR-0009: Provisional boot information és UEFI-konverzió

- Állapot: Accepted
- Dátum: 2026-08-30
- Döntéshozók: UnnamedOS projekt

## Phase 1D-G kiegészítés

A 64 KiB-os bootstrap stack teljes laptartománya külön `BOOTSTRAP_STACK` reserved overlayként kerül a provisional és final mapba. Az overlap a handoff előtt hibát ad.

## Kontextus

A későbbi handoffhoz a firmware-től független, validált memóriatérkép és framebuffer-leírás szükséges. A pre-`ExitBootServices` map key bármely firmware-allokáció vagy free hatására elavulhat, ezért ebben a fázisban nem kezelhető végleges handoff-mapként.

## Döntés

A loader globális allocator nélkül négy `LOADER_DATA` allokációt készít: legfeljebb 256 KiB raw UEFI mapot, egy 64 KiB/2048 elemű normalizálási scratch buffert, egy azonos kapacitású végleges descriptor buffert és egy boot-info lapot. A raw map lekérése legfeljebb három bounded retryt enged, descriptoronként a firmware által visszaadott stride-ot használja, és 40 byte-nál kisebb stride-ot vagy kapacitástúllépést elutasít.

### UEFI mapping

| UEFI típus | UnnamedOS kind |
|---|---|
| Conventional | Usable |
| Loader code/data | Loader |
| Boot-services code/data | Reserved |
| Runtime code/data | Runtime |
| ACPI reclaim | ACPI reclaim |
| ACPI NVS, MMIO, MMIO port, PAL | Reserved |
| Unusable, unaccepted | Unusable |
| Persistent | Persistent |
| Unknown/vendor | Reserved |

A normalizált descriptorok fizikai cím szerint rendezettek, nem nullák, nem fedik egymást és overflow-biztosak. Csak azonos kind/attribútumú, folytonos, nem explicit rezervációs alapdescriptorok vonhatók össze.

### Reservation overlay

A fix, 64 elemű rendezett lista külön rezerválja a `LoadedKernel` szegmenseket, a boot-info lapot, a raw/scratch/final map buffereket és a framebuffer map által ténylegesen lefedett részét. Az overlay darabolhat descriptorokat, megőrzi a teljes alapmap-lefedést, nem minősít reserved tartományt használhatóvá, és kapacitás- vagy fedési hibánál nem fogad el részleges eredményt.

### GOP

Kötelező a nem nulla, közvetlenül címezhető GOP framebuffer. RGB/BGR 8:8:8:8 és a velük pontosan egyező bitmask fogadható el; BLT-only, más bitmask, nulla dimenzió, rövid buffer, hibás stride vagy overflow elutasítandó. A loader nem ír a framebufferbe.

### Ownership

A `PreparedBootInfo` `#[must_use]`, nem másolható, privát mezőjű, és birtokolja mind a négy allokációt, a validált wire értéket, a framebuffer másolatát és a `Provisional` státuszú raw map metadatát. `try_release(&mut self)` fordított sorrendű, csak sikeres free után töröl ownershipet és retry-képes; `Drop` kizárólag best-effort fallback. Publikus disarm/raw handoff nincs.

## Következmények

- A `BootInfo` továbbra is 128 byte, a minimum descriptor 32 byte; ABI-verzió nem változik.
- A snapshot után a konverzió és validáció végéig nincs allokáció, free, filesystem-művelet vagy új protokollnyitás.
- A fázis végén a boot info felszabadul, ezért nem adható át kernelnek.
- Final mapot egy későbbi, közvetlenül `ExitBootServices` előtti fázisnak kell újra lekérnie.

## Ellenőrzés

Hosttesztek fedik a mappinget, stride/capacity/retry szabályokat, rendezést, overlapet, merge-et, overlay-darabolást, GOP-policyt, ABI-validációt és release/retryt. QEMU q35 + OVMF alatt a P1G markerfolyam igazolja a működő GOP-ot, nem üres 32 byte stride-os mapot, kernel/boot/map rezervációkat, valid `BootInfo` objektumot és teljes release-t. Külön framebuffer-policy QEMU fixture nincs: az OVMF aktuális GOP módjának biztonságos, determinisztikus BLT-only/bitmask átállítása nem biztosított; a negatív policy hosttesztekben marad.
