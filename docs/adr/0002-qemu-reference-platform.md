# ADR-0002: QEMU q35 és OVMF referencia-platform

- Állapot: Accepted
- Dátum: 2026-08-29
- Döntéshozók: UnnamedOS projekt

## Kontextus

Az automatizált boothoz egyetlen dokumentált virtuális gépprofil és firmware-konfiguráció szükséges.

## Megfontolt lehetőségek

1. QEMU `q35` + OVMF: UEFI- és PCIe-központú, automatizálható referencia.
2. QEMU `pc`/i440fx: széles körben ismert, de nem az elsődleges modern gépmodell.
3. Fizikai gép elsődleges célként: lassú és nehezen reprodukálható korai visszacsatolás.

## Döntés

Az első referencia QEMU `q35` x86-64 gép EDK2/OVMF firmware-rel és szoftveres TCG emulációval. Nem szükséges virtualizációs kernel-driver. Az OVMF CODE image csak olvasható, a VARS image futásonként ignored build outputba készített elkülönített másolat lesz. A COM1 soros kimenet géppel feldolgozható naplóba kerül; a teszt-only debug-exit pontos konfigurációját a Phase 1 runner rögzíti.

## Aktivált referencia-eszközök

- QEMU 11.1.0, verziókimenet: `v11.1.0-12130-ge470268ff4`.
- Hivatalos Windows x86-64 installer: `qemu-w64-setup-20260811.exe`; SHA-256: `f98a8aeb5f7faea9765b6dee28316c266cd179d80354a2fed8e50176f9a2e59f`.
- EDK2 commit: `4dfdca63a93497203f197ec98ba20e2327e4afe4`.
- CODE: `edk2-x86_64-code.fd`, 3 653 632 byte.
- VARS-template: `edk2-i386-vars.fd`, 540 672 byte.

A QEMU `60-edk2-x86_64.json` firmware-descriptora ezt a párt `x86_64` architektúrához és `pc-q35-*` gépekhez rendeli. A `doctor` ezt a deklarációt, a fájlok elkülönülését, olvashatóságát és ésszerű méretét is ellenőrzi.

## Phase 1C effektív futtatási profil

- `-machine q35 -accel tcg -m 128M`;
- read-only raw pflash az eredeti CODE firmware-rel;
- írható raw pflash a futásonként frissen másolt VARS-template-tel;
- `if=none` read-only `fat:ro:` ESP és `virtio-blk-pci` blokk-eszköz;
- `-net none -monitor none -no-reboot`;
- headless tesztben `-display none`, fájlba irányított COM1 és `isa-debug-exit,iobase=0xf4,iosize=0x04`;
- normál futásban COM1 a stdio-ra kerül, `isa-debug-exit` nincs jelen.

A headless runner 20 másodperces timeoutot alkalmaz, timeout vagy pollinghiba után leállítja és begyűjti a child processt. A bootsikerhez a három Phase 1C marker egyszeri, helyes sorrendű megjelenése, panic hiánya és 33-as QEMU exitkód szükséges. A 35-ös exitkód loaderhibát, más exitkód runner- vagy indulási hibát jelent. A futás előtt és után ellenőrzött FNV-1a hash bizonyítja, hogy a forrás VARS-template változatlan maradt.

Feloldási prioritás: `UNNAMEDOS_QEMU`/firmware override-ok, `PATH`, a feloldott QEMU saját `share` könyvtára, majd ismert hivatalos telepítési helyek. Gépfüggő abszolút útvonal nem része a repositorynak.

## Következmények

- A `doctor` külön ellenőrzi a QEMU-t, annak verzióját és az OVMF CODE/VARS-template párt.
- A tényleges QEMU- és EDK2-verzió a `THIRD_PARTY.md` fájlban rögzített build/test-only eszköz.
- A virtuális eredmény nem jelent fizikai hardvertámogatást.

## Ellenőrzés

A Phase 1 headless teszt ugyanazzal a gépprofillel felismeri a sikert, panicot, QEMU-indítási hibát, firmware-hiányt és timeoutot.
