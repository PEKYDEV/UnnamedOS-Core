# ADR-0013: Determinisztikus lapozótábla-terv, ownership és jövőbeli handoff metadata

- Állapot: Accepted
- Dátum: 2026-09-02
- Döntéshozók: UnnamedOS projekt
- Mérföldkő: Phase 1J-B

## Kontextus

Az ADR-0012 rögzítette az első saját négyszintű x86-64 címteret, de a futó Phase 1I út továbbra is az UEFI-től örökölt lapozást használja. A tényleges frame-allokáció és hierarchia-materializálás előtt külön, hoston bizonyítható szerződés kell a táblák számítására, ownershipjére, a későbbi kernelhandoff metadatájára és a CPU-preconditionökre.

A jelenlegi BootInfo ABI major 1, minor 0, pontosan 128 byte. A v1 olvasó a nagyobb minor változat változatlan 128 byte-os prefixét elfogadja és a végét figyelmen kívül hagyja. Emiatt egy új, kötelező ownership rekord minor bővítésként nem tehető biztonságossá: egy régi kernel nem tudná, hogy el kell utasítania.

## Döntés

### Absztrakt construction plan

A `memory-layout` crate a validált `MappingPlan` elemeiből inaktív, négyszintű, kizárólag 4 KiB-os leafeket tartalmazó `ConstructionPlan` értéket képez. A root PML4 frame-slot mindig 0. A bemeneti mappingek virtuális cím szerint rendezettek; új PDPT, PD és PT tábla az első szükséges leaf feldolgozásakor kapja a következő monoton frame-slotot. A táblák és bejegyzések sorrendje ezért bemenetazonos futásokban determinisztikus.

Minden `PlannedTable` rögzíti a szintet, frame-slotot, parent slotot és parent indexet. Minden `PlannedEntry` rögzíti a tartalmazó táblát, a 0–511 indexet, a cél table-slotot vagy fizikai framet és az entry flagjeit. A terv pontos table-, entry- és removal-darabszámot ad, valamint paddingfüggetlen, little-endian canonical byte-kódolást biztosít regressziós összehasonlításhoz.

A planner felbont minden mappinget 4 KiB-os leafekre. Duplicate leaf, inkompatibilis remap, hiányzó parent, nem canonical cím, nem igazított vagy 64 TiB cap feletti fizikai frame, hibás flag, overflow és bármely fix kapacitás kimerülése strukturált hibát ad. Guard range nem hoz létre present entryt. Eltérő permission vagy cache policy soha nem olvad össze.

### Entry-kódolás

Az intermediate entry `PRESENT|WRITABLE`, supervisor-only, nem huge és NX nélküli. A permisszív writable és executable traversal szükséges ahhoz, hogy ugyanazon parent alatt RX és RW+NX leaf is működhessen; a végső jogosultságot mindig a leaf szűkíti.

- kernel text: `PRESENT|GLOBAL`, vagyis RX;
- kernel rodata, BootInfo és final map: `PRESENT|GLOBAL|NX`, vagyis RO+NX;
- data/BSS, stack, page table: `PRESENT|WRITABLE|GLOBAL|NX`;
- framebuffer és MMIO: ugyanez `PWT|PCD` bitekkel, az első UC policy szerint;
- `USER` és `PS` minden bejegyzésben nulla;
- writable és executable leaf együtt tilos.

Az intermediate címmező a table-slot tényleges frame-hozzárendelése után, a leaf címmező azonnal kódolható. Minden frame 4 KiB-ra igazított és `[0, 0x0000400000000000)` tartományú.

### Transition és final terv

A transitional tervben kizárólag az ADR-0012 szerinti egy 4 KiB-os RX trampoline és egy összefüggő 16 lapos RW+NX bootstrap stack lehet low identity alias. A guard továbbra is absent. A removal terv lower-half PML4 indexenként pontosan egy root-entry törlést rögzít az elvárt child slot és flags értékével. A root entry törlése után az orphan lower hierarchy nem fordíthat címet; felszabadítása csak későbbi, dokumentált TLB-invalidation és ownership-lépés után történhet.

A final terv nem tartalmazhat transition kindot vagy lower canonical-half present mappinget. A jelen csomag nem hajt végre entry-törlést és nem tölt CR3-at.

### Frame ownership

A `FrameBackend` egy frame allokálását, teljes nullázását és felszabadítását modellezi. A `PageTableFrameOwner` csak akkor jön létre, ha a terv pontos table-darabszámához minden frame sikeresen allokált, 4 KiB-ra igazított, egyedi, capen belüli és nullázott. A root fizikai frame a slot 0 hozzárendelése.

Allokációs, validációs vagy nullázási hibánál a részleges owner fordított sorrendben rollbackel. Sikertelen free megőrzi a még birtokolt frame-eket, jelenti a darabszámot és retryzható. Sikeres free egyszer távolítja el az ownershipből az elemet. A normál owner explicit release-e ugyanilyen; `Drop` csak pre-transfer best-effort védőháló. Az explicit `transfer` disarmolja a firmware-free utat, és source tervtől független, fix kapacitású frame-listát ad át. Részlegesen nullázott hierarchy nem reprezentálható kész ownerként.

### Jövőbeli BootInfo v2 envelope

A BootInfo v1.0 wire layout, reserved mezők, emitter és futó kernel változatlan marad. Page-table ownership nem használhat reserved mezőt, és nem lesz v1 minor bővítés.

A jövőbeli aktiválási fázis ABI major 2 envelope-ot használhat. A változatlan 128 byte-os core prefix után 8 byte-ra igazított, lineáris TLV rekordok következnek; nincs next pointer, ezért ciklus nem képezhető. A 16 byte-os extension header mezői: `kind: u32`, `version: u16`, `header_size: u16`, `total_size: u32`, `flags: u32`. Ismeretlen optional kind vagy verzió méret alapján átugorható; ismeretlen required rekord elutasítandó. A v1 kernel a major 2 fejlécet már a dereferálás előtt elutasítja.

A page-table ownership rekord teljes mérete 80 byte: 16 byte header és 64 byte payload. A payload mezői sorrendben: hierarchy version, level count, transition/final state, page size, root physical frame, owned-frame-list fizikai cím, frame count, descriptor stride, 64 TiB fizikai cap és három nulla reserved `u64`. Az owned-frame descriptor 16 byte: fizikai frame és nulla reserved mező.

Validációkor a root pontosan egyszer szerepel, minden frame igazított, egyedi, capen belüli és `MEMORY_KIND_PAGE_TABLE = 13` final-map overlay által fedett. A parser csak átadott byte-slice-ot és már hozzáférhető descriptor slice-ot olvas; wire fizikai címet nem dereferál. Minden offset, méret, stride és tartomány checked, a truncation minden bytehatáron hibát ad.

### CPU capability contract

A tiszta `CpuCapabilities` adatmodell külön kezeli a tervezési és aktiválási gate-et. Tervezéshez long mode aktív, pontosan négy lapozási szint, NX- és CR0.WP-támogatás, 48 bites linear width, 36–52 közötti fizikai width, LA57=0 és a tényleges legmagasabb fizikai cím lefedése szükséges. Aktiváláshoz ezen felül `EFER.NXE=1` és `CR0.WP=1` állapot kell. A modell nem futtat CPUID-t és nem ír vezérlőregisztert.

## Elutasított alternatívák

- V1 reserved mező felhasználása: megsértené a nullakövetelményt és a régi olvasók szerződését.
- Kötelező v1 minor tail: a régi v1 olvasó csendben kihagyná, ezért nem fail-closed.
- Pointerláncos extension lista: ciklus- és dereferálási kockázatot hozna a boot ABI-ba.
- Huge page vagy permission-alapú automatikus merge: eltérne a 4 KiB baseline-tól és elrejtené a policykülönbségeket.

## Halasztott munka

UEFI-backed production frame-allokáció, final-map overlay létrehozása, table-frame-ek fizikai írása, CPUID/CR0/CR4/EFER probing, NXE/WP engedélyezés, CR3-váltás, higher-half ELF és transition-removal végrehajtás későbbi fázis. A Phase 1J-B sem az ELF-et, sem a 128 byte-os BootInfo emittert, sem a kernel entryt vagy QEMU markereket nem módosítja.

## Ellenőrzés

Hosttesztek rögzítik az indexhatárokat, table-dedupot, exact flags és canonical encoding értékeket, guard/transition/final szabályokat, minden kapacitáshibát, allokáció/nullázás/free hibapozíciókat, rollback retryt, transfert, extension truncationt, optional/required viselkedést, frame-list/map ownershipöt és minden CPU gate-et. A meglévő cross-build és QEMU regresszió változatlan handoffot bizonyít.
