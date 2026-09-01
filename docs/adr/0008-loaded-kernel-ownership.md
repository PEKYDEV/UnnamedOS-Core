# ADR-0008: `LoadedKernel` ownership és felszabadítás

- Állapot: Accepted
- Dátum: 2026-08-30
- Döntéshozók: UnnamedOS projekt

## Phase 1D-G kiegészítés

A `HandoffReady` csak élő `LoadedKernel`, finalizálható `PreparedBootInfo` és élő `BootstrapStack` ownerből képezhető. Az egyetlen belső transfer után egyik firmware-backend sem érhető el; a vezérlésátadás nem tér vissza és nem futtat firmware-free műveletet.

## Kontextus

A validált és célmemóriába másolt kernelnek a forrás ELF scratch bufferétől független, pontosan nyilvántartott tulajdonosa szükséges a későbbi handoff előkészítéséhez. Egy fogyasztó `release(self)` API részleges firmware free-hibánál elveszíthetné a még birtokolt tartományokat.

## Döntés

A `LoadedKernel` csak a teljes validáció, load-plan, allokáció, teljes nullázás, másolás és byte-szintű ellenőrzés után hozható létre egy egyszer használható `VerifiedTargets` állapotból. A transfer kiüríti a buildert. A típus `#[must_use]`, nem `Copy`, nem `Clone`, nincs default konstruktora, mezői privátak, fix kapacitásúak, és nem tárolnak forrásreferenciát.

A publikus metadata API értékként adja az entrypointot, a load range-et, a szegmensszámot, az összes pageszámot és az executable entry szegmens indexét; rövid életű iterátora másolt, read-only szegmensmetadatát szolgáltat. Nem ad raw pointert, mutable vagy `'static` slice-ot, allocator handle-t, közvetlenül hívható entrypointot vagy publikus ownership-disarm lehetőséget.

Az explicit `try_release(&mut self)` fordított sorrendben dolgozik. Egy tétel csak sikeres backend free után kerül ki az ownershipből. Az első hiba visszaadja a szegmensindexet, a megmaradt szegmensek számát és a backendhibát; a sikertelen és még nem próbált tételek birtokoltként maradnak, így retry során nincs double free. Az üres ownership release idempotens. A `Drop` az összes megmaradt tételen best-effort kísérletet tesz, de hibát nem tud jelenteni, ezért nem helyettesíti az explicit utat.

## Következmények

- A forrás scratch buffer felszabadítása nem érvényteleníti a `LoadedKernel` metadatáját vagy célownershipét.
- Egy időben egy komponens birtokolja és szabadíthatja a célallokációkat.
- A normál rollback/release út nem ad publikus ownership-disarmot. A Phase 1D-F kizárólag a teljes `ExitReady` typestate fogyasztásakor, közvetlenül az atomikus firmware-határ előtt végez egy kicsi, belső `ManuallyDrop` transfert.
- A Phase 1D-E a read-only szegmensmetadatából épít rezervációkat, miközben a `LoadedKernel` ownership változatlan marad. A `PreparedBootInfo` release-e megelőzi a kernelszegmensek release-ét.
- Sikeres Phase 1D-F transfer után a `PostExitState` csak nyers címeket és másolt fix metadatát tartalmaz; nincs firmware-free-t hívó destruktora. Kernel-entry hívás továbbra sincs.

## Ellenőrzés

Hosttesztek fedik a typestate transfert, metadata gettereket, forrásfüggetlenséget, explicit és üres release-t, fordított sorrendet, második free hibáját, megmaradt ownershipet, retryt, double-free kizárását, Drop fallbacket, valamint mindkét QEMU probe siker- és hibaágait. A pozitív qemu-test futás a forrás felszabadítása után ellenőrzi a referencia-entryt, range-et, 3 szegmenst és 7 lapot; élő ownership alatt az első céloldal nem foglalható újra, explicit release után foglalható és azonnal felszabadul.
