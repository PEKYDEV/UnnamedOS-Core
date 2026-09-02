# ADR-0012: Első saját x86-64 virtuálismemória-architektúra

- Állapot: Accepted
- Dátum: 2026-09-02
- Döntéshozók: UnnamedOS projekt
- Mérföldkő: Phase 1J-A

## Kontextus

A Phase 1I handoff a QEMU/OVMF által örökölt identity mappinggel fut. A jelenlegi bootstrap kernel `p_paddr == p_vaddr`, fizikai és virtuális entryje `0x0020_0000`, a betöltési ablak `[0x0020_0000, 0x0420_0000)`, a 64 KiB-os stack pedig 4 GiB alatt található. A 128 byte-os BootInfo fizikai címeket tartalmaz, lapozótábla-ownershipet nem.

A saját címteret az első lapozótábla létrehozása előtt úgy kell rögzíteni, hogy a loader–kernel életciklus, a későbbi userspace izoláció, a W^X védelem és a fizikai referenciahardver egyaránt ellenőrizhető maradjon. Ez az ADR és a `memory-layout` crate csak tiszta tervezési szerződés; nem hoz létre lapozótáblát és nem módosít CPU-állapotot.

## Döntés

### Címzési mód és lapméret

Az első támogatott mód négy szintű x86-64 lapozás, 48 bites canonical virtuális címekkel és kötelező 4 KiB-os baseline lapokkal. A lower canonical half `[0x0000000000000000, 0x0000800000000000)`, a higher canonical half `[0xffff800000000000, 2^64)`; a közöttük levő címek hibásak. LA57 nem támogatott, ezért `CR4.LA57` értékének nullának kell lennie.

A contract legfeljebb 46 bites, `[0, 0x0000400000000000)` fizikai címeket fogad el. Ez a 64 TiB-os direct-map kapacitás szándékos első implementációs korlát, nem hardverállítás. Minden tényleges fizikai címnek ezen felül a CPUID `0x80000008:EAX[7:0]` által jelentett `MAXPHYADDR` alatt kell lennie. A QEMU q35 és a Dell i5-9500/UHD 630 cél ettől kisebb címtartományban működik; nagyobb fizikai cím determinisztikusan `UnsupportedPhysicalAddress` hibát ad.

### Virtuális címtér

Minden tartomány fél-nyílt, `[start, end)` alakú. A határok 4 KiB-ra igazítottak.

| Rendeltetés | Kezdet | Kizáró felső határ | Méret/politika |
|---|---:|---:|---|
| null/low guard | `0x0000000000000000` | `0x0000000000010000` | mindig unmapped |
| jövőbeli userspace | `0x0000000000010000` | `0x00007ffffffff000` | Phase 1-ben unmapped |
| canonical felső guard | `0x00007ffffffff000` | `0x0000800000000000` | mindig unmapped |
| higher-half direct map | `0xffff800000000000` | `0xffffc00000000000` | 64 TiB; `VA = base + PA` |
| jövőbeli kernelszolgáltatások | `0xffffc00000000000` | `0xffffe00000000000` | heap, per-CPU, IPC és kernelobjektumok számára rezervált |
| MMIO | `0xffffe00000000000` | `0xffffe80000000000` | explicit eszközleképezések |
| framebuffer | `0xffffe80000000000` | `0xffffe90000000000` | külön grafikus aperture |
| magas rezervált tér | `0xffffe90000000000` | `0xffffffff80000000` | unmapped, későbbi ADR nélkül nem használható |
| kernel image | `0xffffffff80000000` | `0xffffffffc0000000` | 1 GiB, jövőbeli ELF `PT_LOAD` szegmensek |
| kernel-local | `0xffffffffc0000000` | `0xfffffffffffff000` | stackek és korai kernel-lokális mappingek |
| felső guard | `0xfffffffffffff000` | `2^64` | mindig unmapped |

A direct map kizárólag elfogadott RAM-descriptorokat képez le. MMIO, framebuffer, firmware runtime és hibás/unknown tartomány nem kerülhet bele. Kernelkód, read-only kerneladat, BootInfo és boot map nem kaphat a direct mapon keresztül szélesebb írható aliast; a direct mapot ezek körül fel kell darabolni vagy azonosan szűk jogosultsággal kell leképezni. Egy fizikai lap írható és végrehajtható aliasának kombinációja is W+X sértés.

### ELF és fizikai betöltés

A fizikai bootstrap load window változatlanul `[0x00200000, 0x04200000)`. A későbbi migrációban az ELF `p_paddr` ebbe az ablakba mutat, míg `p_vaddr` a `[0xffffffff80000000, 0xffffffffc0000000)` kernel-image régióba. Az entry canonical higher-half virtuális cím lesz. A linker migráció előtt a Rust `x86_64-unknown-none` target, a választott code model, minden relocation és a tényleges ELF artifact külön ellenőrzendő. A jelen csomag nem változtatja meg az ELF-et, linkerscriptet vagy a `p_paddr == p_vaddr` bootstrap-validátort.

### Jogosultság és cache policy

Minden mapping supervisor-only. A kód R+X, a rodata R+NX, a data/BSS R+W+NX. A BootInfo és final memory map R+NX. A stack, framebuffer és lapozótáblák R+W+NX. W+X mapping és írható/végrehajtható fizikai alias tilos.

A 64 KiB-os bootstrap stack magas aliasa előtt pontosan egy unmapped 4 KiB-os guard page áll. Az első implementáció csak write-back RAM-ot, valamint uncached MMIO/framebuffer mappinget készít. Framebuffer write-combining és a pontos PAT-indexelés külön ADR és hardverteszt tárgya; addig WC nem állítható be feltételezésből.

NX lapbejegyzés csak akkor használható, ha a maximális extended CPUID leaf legalább `0x80000001`, és `CPUID.80000001H:EDX.NX[20]` egy. `EFER.NXE` értékét a nem végrehajtható bejegyzéseket tartalmazó új CR3 betöltése előtt kell beállítani. `CR0.WP` értékét szintén az új címteret használó kernelkód előtt kell egyre állítani, hogy supervisor módban is érvényesüljön a read-only védelem. Hiányzó precondition esetén a loader a CR3-váltás előtt strukturált, fail-closed hibával leáll.

### Ownership és átmenet

A bootloader a firmware boot services aktív állapotában allokálja és nullázza a lapozótábla-frame-eket, majd egy fix kapacitású owner alatt építi fel és validálja a teljes tervet. Pre-exit hiba fordított sorrendű firmware rollbacket végez. A final map minden lapozótábla-frame-et nem használható kernelrezervációként tart meg.

A CR3-váltás egy dedikált, 4 KiB-os RX trampoline-ról történik. Az új címteret átmenetileg pontosan ez az egy trampoline-lap és a 16 lapos, RW+NX bootstrap stack identity aliasa egészítheti ki; összesen legfeljebb 17 lap/69632 byte, minden cím 4 GiB alatt. Általános low identity map, a teljes kernel fizikai ablaka vagy a teljes első 4 GiB leképezése tilos.

A trampoline ellenőrzi a CPU-preconditionöket, beállítja NXE/WP-t, betölti az új CR3-at, stack nélküli utasítássorral magas stack aliasra vált, majd a higher-half kernel entryre ugrik. A bootloader addig birtokolja a frame-eket, amíg az új CR3 és a nem visszatérő handoff létre nem jön; ezután az ownership a kernelé. Post-exit vagy post-CR3 hiba fail-stop, firmware free nem hívható.

A kernel legelső memória-inicializációs lépése — még interruptok, allocator vagy új task előtt — ellenőrzi, hogy RIP, RSP, BootInfo, final map és lapozótáblák magas címről elérhetők, eltávolít minden alsó canonical-half present bejegyzést, majd CR3 újratöltéssel teljes TLB flush-t végez. Ettől a ponttól a steady-state terv semmilyen low mappinget nem tartalmazhat.

A lapozótábla-frame-eket a kernel a direct map pontos `DIRECT_MAP_START + physical` aliasán éri el R+W+NX jogosultsággal. Az aktív hierarchy frame-jei nem kerülhetnek a frame allocator szabad listájára. Későbbi rootváltáskor csak nem aktív, már nem hivatkozott hierarchy szabadítható fel dokumentált TLB-invalidation után.

### BootInfo és ownership metadata

A jelenlegi 128 byte-os BootInfo és reserved mezői változatlanok. A későbbi page-table buildernek explicit, verziózott módon kell átadnia a root fizikai címét és az összes birtokolt table-frame rezervációját. Ezt egy következő ABI/ownership ADR dönti el; reserved mező nem használható fel csendben. A jövőbeli RDI BootInfo-pointer magas direct-map alias lehet, miközben a wire struktúrában tárolt címek továbbra is fizikai címek.

### Tiszta acceptance contract

A dependency- és allocatormentes, `no_std`, unsafe kódot tiltó `memory-layout` crate explicit `PhysicalAddress`, `VirtualAddress`, `PhysicalRange` és `VirtualRange` típusokat ad. Ellenőrzi az igazítást, roundingot, canonical címeket, overflowt, containmentet, overlapet, direct-map fordítást, mappinghosszakat, jogosultságot, cache policyt, fizikai aliasokat, régióhatárokat, transition/final állapotot és fix kapacitást. A terv virtuális kezdőcím szerint determinisztikusan rendezett; guard entry szándékosan unmapped és nem fordítható fizikai címre. A hibák stabil `LayoutError` variánsok.

## Elutasítási szabályok

Empty, reversed, nem lapigazított, canonical lyukon áthaladó, túlcsorduló vagy eltérő hosszú range hibás. Ugyanígy hibás a virtuális overlap, a régión kívüli mapping, a 64 TiB cap feletti fizikai cím, a load windowon kívüli kernelszegmens, a hibás direct-map összefüggés, a W+X vagy user mapping, a nem UC MMIO/framebuffer, a 17 lapnál nagyobb identity terv és a kapacitáskimerülés. Részleges terv nem tekinthető elfogadottnak.

## Halasztott funkciók

LA57, PCID, SMEP, SMAP, 2 MiB/1 GiB huge page, KASLR, demand paging, copy-on-write és user address space nem része az első architektúrának. Bevezetésük külön CPUID-, invalidation-, ownership-, security- és tesztdöntést igényel.

## Következmények

- A QEMU referenciaút és a Dell cél ugyanazt a determinisztikus négy szintű szerződést használhatja; hardverfüggő cím vagy PAT-feltételezés nincs beégetve.
- A magas kernel és a userspace külön canonical félben van, így a későbbi mikrokernel address space-ek alsó fele izolálható.
- A direct map egyszerűsíti a frame- és lapozótábla-kezelést, de csak explicit RAM- és aliaspolicy mellett.
- A következő implementációs csomag még nem válthat CR3-at: előbb fix kapacitású lapozótábla-konstrukciós planner/owner és ABI-ownership döntés szükséges.

## Ellenőrzés

Hosttesztek rögzítik a típusméreteket, mindkét canonical fél határait, a teljes layoutot, range- és permission-hibákat, mappingrendezést és fordítást, guardot, stack/BootInfo/map/framebuffer/MMIO policyt, fizikai alias-W^X-et, transition capet, final low-map tilalmat és kapacitáskimerülést. A meglévő cross-target és QEMU teszteknek változatlan markerrel és exitkóddal kell átmenniük, bizonyítva, hogy ez a csomag még nem változtatja meg a futó handoffot.
