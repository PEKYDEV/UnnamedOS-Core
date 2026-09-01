# ADR-0010: Final memory map és `ExitBootServices` határ

- Állapot: Accepted
- Dátum: 2026-08-30
- Döntéshozók: UnnamedOS projekt

## Phase 1D-G kiegészítés

A P1H határ változatlan marad. A P1I út egy szigorúbb `HandoffReady` állapotból ugyanazt a kétpróbás exit mechanizmust használja, majd finalizálás után a dedikált stackre vált és a kernelre ugrik. Post-exit UEFI-hívás, allokáció vagy free nincs.

## Kontextus

A provisional map key bármely firmware-allokáció vagy free után elavul. A kernel- és bootadat-ownership destruktorai boot-services free-t hívhatnak, ezért sikeres exit után nem maradhatnak élő Rust ownerként. Az atomikus határnak rollbackképes pre-exit és firmware-hívástól mentes post-exit állapotot kell szétválasztania.

## Vizsgált `uefi 0.39.0` API

A lockolt wrapper pontos szignatúrája `pub unsafe fn exit_boot_services(custom_memory_type: Option<MemoryType>) -> MemoryMapOwned`. Meghívja a crate helper-exitjét, `MemoryMapBackingMemory::new` segítségével UEFI poolból `map_size + 8 × desc_size` buffert allokál, majd legfeljebb kétszer futtatja a GetMemoryMap+ExitBootServices párt. Sikertelen második kísérletnél cold resetet végez; hibát nem ad vissza. A visszaadott `MemoryMapOwned` birtokolja a pool backinget, amelynek `Drop` implementációja post-exit már nem hív `free_pool`-t. Az exit után minden protokoll, boot-services allocator, pool-owner és boot-services objektum érvénytelen; csak konfigurációs táblák és runtime services maradnak firmware-oldalon használhatók.

A wrapper belső poolallokációja miatt nem teljesíti az előre lefoglalt, pontosan rezerválható backing és az `EXIT_READY` előtti utolsó allokáció követelményét. Ezért a loader a lockolt crate által elérhető raw system-table függvénypointereket használja. Az alkalmazott exit szignatúra `unsafe extern "efiapi" fn(image_handle: Handle, map_key: usize) -> Status`; előtte ugyanabból a táblából a firmware-stride-ot visszaadó `GetMemoryMap` fut a már birtokolt 256 KiB-os lapbufferre.

## Döntés

A lifecycle `BootServicesState → KernelOwned → ExitReady → TransferredBootState → PostExitState`. Az első három állapot valódi `LoadedKernel`/`PreparedBootInfo` ownert tart, ezért bármely exit előtti hiba normál Drop rollbacket végez. Minden protokollguard és source ELF addigra megszűnik; a watchdog letiltása után készül a final map.

Az `EXIT_READY` nyers COM1 marker után a kicsi belső transfer `ManuallyDrop` alatt elfogyasztja a két ownert, és csak másolt címeket, range-et, szegmens- és framebuffer-metadatát tart meg. Ezután az első `ExitBootServices` hívás fut. Csak `INVALID_PARAMETER`/elavult key esetén engedélyezett egy új GetMemoryMap és egy második exit; további kísérlet nincs. Post-exit kizárólag nyers COM1 és QEMU debug-exit I/O használható.

A sikeres exit mapja a meglévő scratch/final bufferekben allocatormentesen rendeződik és validálódik. Conventional és a már kilépett Boot Services code/data usable; minden `LOADER_DATA` konzervatívan loader-owned marad. Runtime, MMIO, ACPI NVS, PAL és unknown reserved/runtime marad. A kernel, boot-info, raw/scratch/final map és a mapba eső framebuffer explicit overlayt kap. A változatlan 128 byte-os `BootInfo` a final descriptor címet, darabszámot, 32 byte-os stride-ot, firmware descriptor-verziót és GOP-adatokat tartalmazza, minden reserved mező nullával.

## Következmények

- A `PostExitState` nem `Copy`, nem tartalmaz firmware-backendet vagy firmware-free-t hívó típust, és nincs saját `Drop` implementációja.
- A raw út nem állítja át a `uefi` crate belső boot-services flagjét; ezért az exit után a kód szerkezetileg sem hívhat crate boot-services API-t.
- Az exit-map backing pontos raw lapallokációja rezervált; a többi `LOADER_DATA` is konzervatívan nem usable.
- Kernel-entry, stackváltás, lapozás és handoff továbbra sincs.

## Ellenőrzés

Hosttesztek fedik a typestate-sorrendet, rollbacket, double-free kizárását, kétpróbás retry-policyt, post-exit mappinget, final ABI-validációt és a destruktormentes post-exit állapotot. A `cargo xtask test-exit-boot-services` külön feature-rel valós q35+OVMF futásban pontos P1H markersorrendet és 33-as processexitet követel. A korábbi missing/corrupt/policy utak nem érhetnek P1H markerhez. Valós firmware exit-hiba nem injektálható biztonságosan; a retry-policy ezért hoston determinisztikusan tesztelt.
