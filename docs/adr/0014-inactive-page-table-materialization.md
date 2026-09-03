# ADR-0014: Inaktív UEFI lapozótábla-materializálás és rezerváció

- Állapot: Accepted
- Dátum: 2026-09-03
- Döntéshozók: UnnamedOS projekt
- Mérföldkő: Phase 1J-C

## Kontextus

Az ADR-0012 rögzíti a négy szintű x86-64 címtér policyját, az ADR-0013 pedig a determinisztikus konstrukciós tervet és a rollback-safe frame ownert. A következő elkülönítendő határ a terv fizikai UEFI lapokba írása, független visszaolvasása és a frame-ek final memory mapban való megtartása. A jelenlegi kernel továbbra is a firmware-től örökölt címtérben, változatlan alacsony entryvel és BootInfo v1.0 ABI-val fut.

## Döntés

### Runtime transition terv

A Phase 1J-C runtime terv pontosan az elfogadott ideiglenes identity mappingeket materializálja: az aktuális kernel entryt tartalmazó egy 4 KiB-os RX lapot és a dedikált 16 lapos RW+NX bootstrap stacket. A két tartományon kívül lower-half present mapping nincs, a removal terv a hozzájuk tartozó egyetlen PML4 root entry pontos törlését írja le. A referencia QEMU-konfigurációban ehhez öt table-frame szükséges; az allokátor mindig a tényleges `ConstructionPlan::table_count()` értékét használja.

A teljes higher-half steady-state mapping, benne a kernel-, BootInfo-, final-map-, direct-map-, framebuffer- és saját table-frame aliasokkal, az ELF higher-half migráció és aktiválási szerződés része marad. A most létrehozott hierarchy ezért fizikailag teljes a jóváhagyott bounded transition tervhez, de nem aktiválható címteret jelent.

### UEFI allokáció és ownership

Minden frame külön `AllocateType::AnyPages`, `MemoryType::LOADER_DATA`, egyoldalas UEFI allokáció. A sorrend frame-slot szerint monoton; folytonosság nem feltétel. A közös owner elutasítja a nulla, nem igazított, duplikált vagy 64 TiB capen kívüli címet, és csak az összes frame teljes nullázása után jön létre. Részleges hiba fordított sorrendben rollbackel; sikertelen free megtartja a még birtokolt elemeket és retryzható. Allokáció a final map capture után nem történhet.

### Unsafe materializációs határ

A tervértelmezés, indexszámítás, entry-kódolás, ownership-ellenőrzés és verifikáció safe kód. Az unsafe UEFI adapter kizárólag a saját, egyoldalas LOADER_DATA frame nullázására, valamint egy ellenőrzött 0–511 indexű aligned `u64` írására vagy olvasására készít ideiglenes pointert. A safety invariant az allokáció provenance-ét, a pontos 4096 byte-os határt, igazítást, kizárólagos ownert, élettartamot, frame-ek nem átfedését és a hierarchy inaktív állapotát együttesen követeli meg.

A materializáló mind az 512 entryt explicit kiírja minden birtokolt lapra. A tervben nem szereplő entry nulla, a table-slot tényleges frame-címre oldódik, a leaf érték és cache/permission bit változatlanul a plannerből származik. Mapping target fizikai címét a kód nem dereferálja, firmware-owned lapozótáblát nem módosít.

### Független read-back verifier

A verifier közvetlenül mind az 512 entryt visszaolvassa, és a planner rekordjaiból, nem a writer sorosításából számít elvárt értéket. Ellenőrzi a rootot, parent–child fizikai címet és szintet, elérhetőséget, tulajdonolt child-framet, ciklust, minden nem várt nem nulla entryt, address capet, reserved biteket, `PRESENT/RW/NX/PWT/PCD/GLOBAL` policyt, valamint a `USER`, `PS` és leaf W+X tilalmát. A hiányzó leaf, guard-sértés, pontatlan transition alias és removal root-entry strukturált slot/index hibát ad. Csak teljesen verifikált hierarchy léphet rezervációs állapotba.

### Final-map rezerváció és typestate

A typestate sorrend: `PlannedPageTables` → `AllocatedPageTables` → `MaterializedPageTables` → `VerifiedInactivePageTables` → `FinalMapReservedPageTables` → `TransferredInactivePageTables`. A page-table rezervációt kizárólag a verified állapot adhatja a közös reservation listához. Fizikailag szomszédos frame-ek csak azonos `MEMORY_KIND_PAGE_TABLE` besorolással koaleszkálnak; minden más frame külön tartomány.

Az ideiglenes és a final UEFI map overlay után minden frame teljes lefedése újra ellenőrzött, usable vagy más kind elutasítandó. A BootInfo v1.0 változatlan 128 byte, a current kernel a `MEMORY_KIND_PAGE_TABLE` descriptorokat konzervatív, nem használható memóriaként kapja. A frame-ek a boot teljes hátralévő életére rezerváltak; csak a későbbi aktiválás és v2 ownership handoff teheti őket kernel által kezelt erőforrássá.

A pre-exit owner armed marad. A nem visszatérő ExitBootServices-határon a stackkel, kernellel és BootInfo-bufferekkel együtt pontosan egyszer disarmolódik. A post-exit rekord csak fix kapacitású frame-címeket, rootot és darabszámot tartalmaz, nincs firmware-hívó `Drop`; ezek a loader post-exit állapotából olvashatók.

### Negatív runtime bizonyíték

A teszt-feature a harmadik frame-kísérlet előtt, firmware-hívás nélkül injektál allokációs hibát. A két sikeres allokáció fordított rollbackje után csak a `P1J:ROLLBACK_COMPLETE` és `P1J:FAIL:ALLOC` marker jelenhet meg, exit code 35-tel; materializáció, final-map előkészítés és kernelhandoff nem történhet. A production feature ezt az ágat nem tartalmazza.

## Halasztott munka

CPUID probing, CR0/CR3/CR4/EFER/PAT módosítás, NXE/WP engedélyezés, higher-half ELF, teljes steady-state hierarchy, BootInfo v2 emission és bármilyen CR3-váltás nincs ebben a csomagban. A futó CR3-at a kód nem olvassa és nem írja; az örökölt UEFI address space változatlan marker- és kernelhandoff-regressziója adja az inaktivitási bizonyítékot.

## Ellenőrzés

Hosttesztek fedik a teljes 512-entry materializálást, minden bounded korrupciót, parent/child feloldást, flag- és reserved-bit hibákat, idegen childot, ciklust, rossz szintet, unreachable táblát, write hibát, rollbacket, transfert, overlay splitet, koaleszkálást, capacity/coverage/kind hibát és a változatlan BootInfo v1.0 validációt. QEMU külön bizonyítja a sikeres inaktív construction/final-map/transfert és a részleges allokáció rollbackjét.
