# ADR-0004: Verziózott boot information ABI

- Állapot: Accepted
- Dátum: 2026-08-29
- Döntéshozók: UnnamedOS projekt

## Phase 1D-G kiegészítés

A 128 byte-os wire layout változatlan. A `MemoryDescriptor.kind = 12` a dedikált bootstrap stack reserved overlaye. A kernel a final map descriptorait csak a teljes scalar `BootInfo` validáció után dereferálja; a kernel, stack, `BootInfo` és mapbuffer nem lehet `USABLE` kinddal fedett.

## Kontextus

A loader és a külön fordított kernel között olyan adatátadás kell, amely nem függ a Rust instabil ABI-jától, és hibás vagy inkompatibilis bemenetet felismerhetővé tesz.

## Megfontolt lehetőségek

1. Rust típusok közvetlen átadása: kényelmes, de nem stabil ABI.
2. Verziózott, C-kompatibilis fix szélességű struktúrák: explicit és több nyelvből ellenőrizhető.
3. Öndeskriptív soros formátum: rugalmasabb, de indokolatlan parser- és allokációs teher bootkor.

## Döntés

A handoff egy `repr(C)` elrendezésű, explicit méretű egész típusokat használó boot information blokk. A fejléc legalább magic értéket, major/minor ABI-verziót, teljes méretet és feature flag mezőt tartalmaz. A címek és hosszok `u64` értékek; nyelvi pointer vagy Rust-specifikus enum nem kerül a wire formátumba.

A normalizált memóriatérkép és framebuffer-leírás külön, elemszám/elemméret/cím descriptorral kapcsolódik. Ismeretlen major verzió elutasítandó; kompatibilis minor verziónál a méretmezők alapján az ismeretlen végződés figyelmen kívül hagyható.

## Megvalósított ABI v1.0 layout

A wire formátum x86-64 little-endian. Minden alábbi struktúra `repr(C)`, 8 byte alignmentű, és kizárólag explicit méretű egész mezőket tartalmaz.

### `BootInfoHeader` — 32 byte

| Offset | Mező | Típus |
|---:|---|---|
| 0 | `magic` (`UNOSBOOT`) | `u64` |
| 8 | `abi_major` | `u16` |
| 10 | `abi_minor` | `u16` |
| 12 | `header_size` | `u16` |
| 14 | `reserved0` | `u16` |
| 16 | `total_size` | `u32` |
| 20 | `reserved1` | `u32` |
| 24 | `flags` | `u64` |

### `MemoryMapInfo` — 40 byte

| Offset | Mező | Típus |
|---:|---|---|
| 0 | `physical_address` | `u64` |
| 8 | `descriptor_count` | `u64` |
| 16 | `descriptor_stride` | `u32` |
| 20 | `descriptor_version` | `u16` |
| 22 | `reserved0` | `u16` |
| 24 | `byte_length` | `u64` |
| 32 | `reserved1` | `u64` |

A minimális `MemoryDescriptor` 32 byte: `kind: u32`, `reserved0: u32`, `physical_start: u64`, `page_count: u64`, `attributes: u64`. A stride legalább 32, és 8 többszöröse.

### `FramebufferInfo` — 40 byte

| Offset | Mező | Típus |
|---:|---|---|
| 0 | `physical_address` | `u64` |
| 8 | `byte_length` | `u64` |
| 16 | `width` | `u32` |
| 20 | `height` | `u32` |
| 24 | `pixels_per_scanline` | `u32` |
| 28 | `pixel_format` | `u32` |
| 32 | `reserved0` | `u64` |

Pixelformátum `1`: RGBX8888; `2`: BGRX8888. Mindkettő 4 byte/pixel.

### `BootInfo` — 128 byte

| Offset | Mező | Méret |
|---:|---|---:|
| 0 | `header` | 32 |
| 32 | `memory_map` | 40 |
| 72 | `framebuffer` | 40 |
| 112 | `reserved0` | 8 |
| 120 | `reserved1` | 8 |

## Kompatibilitási és validációs szabályok

- ABI v1.0-nál `header_size = 32` és `total_size = 128`.
- Ismeretlen major verzió hibás. Nagyobb minor verzió elfogadható, ha az első 128 byte változatlan, `header_size = 32`, és `total_size >= 128`; az ismeretlen végződést a v1.0 olvasó nem dereferálja.
- Minden reserved mező nulla; jelenleg a flags is nulla.
- A memóriatérkép bytehossza pontosan `descriptor_count × descriptor_stride`.
- A framebuffer minimális hossza `pixels_per_scanline × height × 4`.
- Minden szorzás és címtartomány-képzés checked művelet; overflow hibát ad.
- A `BootInfo::validate` csak skalár metadataértékeket ellenőriz, fizikai címet nem dereferál. A descriptorok külön, hozzáférhetővé tételük után validálandók.
- A nyers `repr(C)` típusokat a `ValidatedBootInfo` és `ValidatedMemoryDescriptor` biztonságos, nem-wire nézetei választják el a validált értelmezéstől.

### Memory kind értékek

| Érték | Jelentés |
|---:|---|
| 1 | azonnal használható conventional memória |
| 2 | általános reserved memória |
| 3 | ACPI reclaim |
| 4 | firmware runtime memória |
| 5 | kernel image |
| 6 | boot-information lap |
| 7 | raw, scratch vagy konvertált boot memory-map buffer |
| 8 | loader code/data |
| 9 | framebuffer rezerváció |
| 10 | hibás vagy még el nem fogadott memória |
| 11 | persistent memória |

A Phase 1D-E nem változtatja meg a 128 byte-os `BootInfo` vagy a 32 byte-os minimum descriptor layoutot. A fenti, korábban név nélkül hagyott `kind` értékeket rögzíti. Ismeretlen érték nem értelmezhető használható memóriaként.

A létrehozott map provisional: a benne tárolt diagnosztikai UEFI key a következő firmware-allokáció vagy free után elavulhat. Final handoff előtt új map szükséges.

A Phase 1D-F a sikeres firmware-exithez tartozó mapból ugyanebbe a változatlan ABI-ba ír. Csak ezen a post-exit útvonalon lesz a Boot Services code/data `USABLE`; a `LOADER_DATA` konzervatívan `LOADER` marad, majd a kernel-, boot-info-, raw/scratch/final-map és mapba eső framebuffer-tartományok explicit kind overlayt kapnak. Runtime, MMIO, ACPI NVS és ismeretlen típus nem lesz usable. A végleges struktúrán és minden descriptoron újra lefut a teljes validáció.

## Következmények

- A loader birtokolja és a kernelhandoff idejére életben tartja az összes hivatkozott memóriát.
- A pontos struktúrák a dependency-mentes, `no_std`, unsafe kódot tiltó `boot-protocol` crate-ben vannak.
- Layout-, méret-, alignment-, verzió- és hibás bemeneti hosttesztek kötelezők.

## Ellenőrzés

Host oldali layouttesztek és egy loader/kernel round-trip boot teszt igazolja a szerződést.
