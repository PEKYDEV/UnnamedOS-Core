# ADR-0007: Fixed-address bootstrap load policy és memória-ownership

- Állapot: Accepted
- Dátum: 2026-08-30
- Döntéshozók: UnnamedOS projekt

## Phase 1D-G kiegészítés

A validált entry csak executable `PT_LOAD` szegmensből kerülhet a handoff állapotba. Sikeres P1I átadáskor a fixed-address kerneloldalak már nem szabadulnak fel: a firmware nélküli kernel ownership részévé válnak.

## Kontextus

A validált ET_EXEC kernel szegmenseit a későbbi handoff előtt bizonyíthatóan a deklarált fizikai címekre kell helyezni. A formátumvalidáció önmagában nem határozza meg, mely fizikai címtartomány használható biztonságosan, és nem kezeli az UEFI allokációk rollbackjét.

## Döntés

A bootstrap loader-policy a `0x0020_0000..0x0420_0000` felső végén kizáró fizikai ablakot használja. Minden `PT_LOAD` szegmens kezdete 4 KiB-aligned; teljes, lapra kerekített tartományának az ablakban kell maradnia. A lapra kerekített tartományok nem fedhetik egymást, összesített méretük legfeljebb 64 MiB. A policy külön réteg a `kernel-image` ELF-formátumvalidátor felett.

Az allocatormentes load plan legfeljebb `MAX_PROGRAM_HEADERS` számú fix kapacitású elemet tartalmaz. Rögzíti a forrásoffsetet, fájl- és memóriaméretet, eredeti és lapra kerekített céltartományt, pageszámot, flageket, BSS-t, nullázási, másolási és padding-tartományt. A terv csak teljes siker esetén válik elérhetővé, fizikai címet nem dereferál, és elutasítja a forrás scratch allokáció bármely lapjával átfedő célt.

Minden célitem pontos című `AllocateAddress`/`LOADER_DATA` allokációt kap, fallback és load bias nélkül. A fix kapacitású ownership-állapot minden sikeres allokációt nyilvántart. Részleges hiba és minden későbbi műveleti hiba fordított sorrendű rollbacket indít. Sikeres `FreePages` után az adott tétel kikerül a nyilvántartásból; sikertelen free és a még nem próbált tételek birtokoltként maradnak, ezért az explicit release double free nélkül újrapróbálható. A free hiba és a megmaradt ownership darabszáma megfigyelhető.

A loader teljes célallokációkat nulláz, majd pontos `p_filesz` byteokat másol. Ezután byte-onként ellenőrzi a forrás/cél egyezést, a teljes BSS-t és a `p_memsz` utáni page paddinget. A forrás scratch buffer változatlanságát külön ellenőrzi. Siker esetén a célallokációk `LoadedKernel` ownershipbe kerülnek, a forrás felszabadul, majd a fázis végén az explicit, fordított sorrendű release felszabadít minden céloldalt.

## Következmények

- A Phase 1D-D a fixed-address elhelyezés mellett a forrásfüggetlen metadata- és célownershipet is bizonyítja, de nem készít entrypoint-függvénypointert és nem hajt végre kernelt.
- Nincs `ExitBootServices`, boot-info handoff vagy lapozásmódosítás.
- A későbbi handoff-fázisnak külön döntésben kell meghatároznia, mikor maradhat célmemória tartósan a kernel tulajdonában.
- A Phase 1D-E provisional map overlaye a `LoadedKernel` lapra kerekített allocation range-eit `KERNEL_IMAGE` kinddal rezerválja, miközben az objektum továbbra is birtokolja azokat.

## Ellenőrzés

Hosttesztek fedik a window-, rounding-, overlap-, source-overlap-, ownership-, rollback-, copy-, BSS-, padding-, entry- és markerállapotokat fake backenddel. Négy külön QEMU/OVMF futás ellenőrzi a pozitív másolást, a hiányzó kernelt, a sérült ELF-et, valamint a valid ELF-ként megmaradó, de ablakon kívülre mutató policy-fixture elutasítását.
