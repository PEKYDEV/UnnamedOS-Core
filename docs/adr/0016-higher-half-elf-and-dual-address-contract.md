# ADR-0016: Higher-half ELF64 és dual-address load/mapping szerződés

- Állapot: Accepted
- Dátum: 2026-09-03
- Döntéshozók: UnnamedOS projekt
- Mérföldkő: Phase 1J-E

## Kontextus

Az ADR-0015 után a loader valós CPU-állapotból activation-readiness eredményt képez, de a futó kernel továbbra is `0x00200000` címre linkelt identity ELF, a BootInfo pedig major 1 és nem ad át lapozótábla-ownershipet. Ilyen állapotban egy ideiglenes CR3-váltás és visszaállítás nem bizonyítana tartós handoffot. Az aktiválás addig tilos, amíg a higher-half image, annak teljes mappingje, a major-2 ownership handoff és az örökölt címtértől független post-switch entry együtt nincs kész.

## Döntés

### Két egyértelmű kernelartifact

A változatlan runtime artifact neve `unnamedos-kernel.elf`; fizikai és virtuális entryje `0x0000000000200000`, minden `PT_LOAD` elemére `p_paddr == p_vaddr`, és továbbra is kizárólag ez kerül `EFI/UNNAMEDOS/KERNEL.ELF` néven az ESP-be.

A külön, nem futtatott artifact neve `unnamedos-kernel-higher-half.elf`. Ugyanazt a `no_std`, `no_main`, allocatormentes Rust forrást használja, de a `higher-half` Cargo feature a `linker-higher-half.ld` scriptet választja. A build explicit `-C no-redzone=yes -C code-model=kernel` flaget és az `x86_64-unknown-none` targetet használ. A `kernel` code model az `0xffffffff80000000..0xffffffffffffffff` felső 2 GiB régióhoz illeszkedik; PIE és dynamic linker nincs.

### Pontos címkapcsolat

- fizikai load base: `0x0000000000200000`;
- higher-half offset: `0xffffffff80000000`;
- virtuális load/entry base: `0xffffffff80200000`;
- elfogadott fizikai ablak: `[0x0000000000200000, 0x0000000004200000)`;
- elfogadott virtuális kernel-image régió: `[0xffffffff80000000, 0xffffffffc0000000)`;
- minden load szegmensre: `p_vaddr.checked_sub(p_paddr) == 0xffffffff80000000`.

A Phase 1J-E referenciaartifact ellenőrzött layoutja:

| Szegmens | Fizikai tartomány | Virtuális tartomány | Jog |
|---|---|---|---|
| text | `0x200000..0x203a11` | `0xffffffff80200000..0xffffffff80203a11` | RX |
| rodata | `0x204000..0x2047ec` | `0xffffffff80204000..0xffffffff802047ec` | R+NX |
| data/BSS | `0x205000..0x207000` | `0xffffffff80205000..0xffffffff80207000` | RW+NX |

A linkerscript `AT(VMA - offset)` kifejezésekkel ad külön LMA-t minden output sectionnek, és linker-assert rögzíti a base-eket és az egységes offsetet. Minden cím- és tartományművelet checked; wrapping fordítás nincs.

### ELF-policy

Az identity és higher-half policy külön belépési ponttal rendelkezik. A közös strukturális parser ELF64, little-endian, normalizált `ELFOSABI_NONE`, `ET_EXEC`, `EM_X86_64` képet követel, és elutasítja a hibás/truncated táblákat, W+X-et, `PT_INTERP`, `PT_DYNAMIC`, TLS-t, runtime relocationt, undefined runtime symbolt, fájl- és memóriatúlcsordulást, valamint a fizikai vagy virtuális átfedést.

Az identity validátor változatlanul megköveteli a `p_paddr == p_vaddr`, `0x00200000` entry/base és RX/R/RW+BSS összetételt. A higher-half validátor külön megköveteli az egységes offsetet, a két policyablak containmentjét, a canonical higher-half entryt, az executable fizikai backinget, pontos RX/R/RW jogosultságokat, 4 KiB-os alignmentet és BSS-t. Egyik policy nem következtethető pusztán cím-egyezésből, és a két artifact nem cserélhető fel.

### Loader copy és mapping felelősség

A `HigherHalfLoadPlan` a validált ELF `p_paddr` mezőiből képezi a fizikai allokációs, copy- és BSS-zero műveleteket. A `p_vaddr` mezőkből külön `MappingPlan` készül: text `KernelText/KERNEL_RX`, rodata `KernelRodata/KERNEL_R`, data/BSS `KernelData/KERNEL_RW`, mindenhol write-back cache policyval. A virtuális entryt a mapping terv visszafordítja az önállóan validált fizikai entryre.

A tiszta hostbizonyíték ezt a mapping tervet változtatás nélkül átadja a meglévő `ConstructionPlan` részére. A referencia-layout négy table-frame-et és hét bejegyzést igényel, removal nélkül; minden leaf a kívánt fizikai frame-re mutat, W+X nincs, és a final tervben nincs low vagy transition mapping. A normál UEFI út nem olvassa, nem allokálja és nem materializálja ezt a második artifactot vagy hierarchy-t.

### Aktiválás és handoff

A higher-half entry csak jövőbeli virtuális belépési pont; ebben a fázisban nincs rá ugrás. A BootInfo v1.0 pontosan 128 byte marad, reserved mező nem kap új jelentést, page-table metadata nem kerül átadásra. CR0, CR3, CR4, EFER, PAT, GDT és IDT nem módosul; WRMSR és INVLPG nincs.

Az identity artifact akkor távolítható el, amikor a major-2 ownership envelope, a magas BootInfo/map/stack elérhetőség, a teljes transition terv és a nem visszatérő post-switch handoff együtt bizonyított. A CR3-aktiválás csak ezt követő külön, állapotmódosító mérföldkő lehet.

## Ellenőrzés

Hosttesztek rögzítik a canonical határokat, minden PT_LOAD permissionkombinációt, offsetet, külön fizikai/virtuális overlapet, entryfordítást, policyablakokat, BSS-t, alignmentet, overflowt, symbol/relocation hibákat, minden bounded truncated prefix panicmentességét, a két policy együttélését, valamint a mapping- és page-table terv pontos eredményét. Az `xtask` külön buildeli, validálja, hash-eli, inspectálja és két izolált buildből byte-azonosan összehasonlítja a higher-half artifactot. A meglévő QEMU marker- és exitkód-szerződés változatlan.
