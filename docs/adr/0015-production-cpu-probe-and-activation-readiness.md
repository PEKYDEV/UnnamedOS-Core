# ADR-0015: Production CPU-probe és activation-readiness

- Állapot: Accepted
- Dátum: 2026-09-03
- Döntéshozók: UnnamedOS projekt
- Mérföldkő: Phase 1J-D

## Kontextus

Az ADR-0014 fizikailag materializálja és rezerválja a bounded transition hierarchy-t, de a CPU tényleges képességeit és örökölt pagingállapotát eddig csak absztrakt host-contract modellezte. Aktiválás előtt valós, read-only mérés és a birtokolt frame-ekre vonatkozó külön readiness-döntés szükséges. A hierarchy ebben a fázisban továbbra is inaktív, a BootInfo v1.0 és az alacsony kernel-entry változatlan.

## Döntés

### Probe-határ

Az x86-64 UEFI adapter lekéri a maximális basic és extended CPUID leafet, majd kizárólag bizonyítottan támogatott leafeket olvas: `1`, `7:0`, `0x80000001` és `0x80000008`. Ezekből az MSR-, PAE-, LA57-, long-mode- és NX-képesség, valamint a fizikai és lineáris címszélesség kerül a `RawCpuSnapshot` értékbe. CR0, CR3 és CR4 inline assemblyvel csak olvasott. `IA32_EFER` RDMSR csak az MSR- és long-mode-képesség megállapítása után fut. Író utasítás nincs.

A hardver-hozzáférés a bináris kis `cpu_probe` moduljára korlátozott. A nyers snapshot validálása és a readiness-osztályozás safe, allocatormentes, `no_std` kód a `memory-layout` crate-ben; szintetikus snapshotokkal hoston teljesen tesztelhető.

### Érvényes örökölt állapot

Kötelező a long-mode-, NX-, MSR- és PAE-támogatás, továbbá az aktív CR0.PG, CR4.PAE, EFER.LME és EFER.LMA. A jelentett fizikai szélesség 36–52 bit, a lineáris szélesség 48 vagy LA57-képes CPU esetén 57 bit lehet. CR4.LA57 mindig nulla; az effektív modell ezért pontosan négy szint és 48 canonical bit. Az LA57 támogatása önmagában nem elutasítás.

CR3 címrésze a jelentett fizikai szélességből képzett maszkkal különül el. PCIDE=0 esetén csak PWT/PCD lehet alsó flag; PCIDE=1 esetén az alsó 12 bit PCID, amelyet a kezdeti saját root nem használhat. Tiltott alsó vagy magas bit, nulla root, szélességen kívüli cím és capability/control-state ellentmondás fail-closed hiba.

### Readiness és fizikai kompatibilitás

Csak `ValidatedCpuSnapshot` állíthat elő `ActivationReadiness` értéket. Az osztályozás külön rögzíti az NXE és CR0.WP `Enabled` vagy `MustEnableBeforeActivation` állapotát, PGE-t, PCID-t, az örökölt CR3 értelmezett rootját, a javasolt saját rootot, a 48 bites effektív módot és a transition elvi engedélyezhetőségét.

Minden birtokolt table-frame, a javasolt root és a bounded transition minden mapped fizikai végcíme a CPU címszélessége és a projekt 64 TiB capje alatt kell legyen. A `VerifiedInactivePageTables` csak e bizonyíték után válhat `ActivationPreparedPageTables` értékké; a readiness az ExitBootServices után is a post-exit állapot része marad.

### CR3-stabilitás

A probe a table-előkészítés előtt rögzíti a teljes örökölt CR3 értéket. A kód a materializáció/verifikáció után, a final-map előkészítés után és ExitBootServices után pontos egyezést követel. Ez megőrzi a PWT/PCD vagy PCID szemantikát is; változás fatális invariánssértés. Stabil marker nem tartalmaz rootcímet vagy nyers regisztert.

### Aktiválástól való elválasztás

A readiness kizárólag a későbbi műveletsorrendet írja le. Ha szükséges, a jövőbeli aktiváló kód előbb EFER.NXE-t, majd CR0.WP-t engedélyezhet, ezután töltheti be a review-zott saját CR3-at. E csomag nem ír CR0/CR3/CR4/EFER értéket, nem futtat WRMSR-t vagy INVLPG-t, nem aktivál lapozótáblát és nem ugrik higher-half címre.

### Negatív runtime bizonyíték

A külön teszt-feature a valódi snapshot elkészítése és a hierarchy materializálása után kizárólag a policy bemenetén injektál hiányzó NX-képességet. Az öt table-frame, a bootstrap stack és a loaded kernel explicit felszabadul, majd `CPU_ROLLBACK_COMPLETE` és `FAIL:CPU_POLICY` markerrel, 35-ös kóddal áll le. ExitBootServices és kernelhandoff nem történik; production buildben nincs injekció.

## Halasztott munka

Higher-half ELF-layout, teljes steady-state mapping, BootInfo v2 ownership-emission, NXE/WP írás, CR3-aktiválás, transition-removal, framebuffer-renderelés és screenshot nincs ebben a csomagban.

## Ellenőrzés

Hosttesztek fedik a leaf- és feature-hiányokat, kötelező aktív állapotokat, ellentmondásokat, LA57 48/57 bites eseteit, fizikai szélességhatárokat, legacy és PCID CR3-értelmezést, minden NXE/WP kombinációt, frame/root/range kompatibilitást és CR3-stabilitást. QEMU bizonyítja a valós pozitív probe-ot, a változatlan CR3-at, az inaktív handoffot és az injektált policy-hiba teljes pre-exit rollbackjét.
