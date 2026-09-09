# Decisive log lines

The full run logs are `*.log` and therefore gitignored; they stayed on the machine that
produced them. These are the lines the diagnosis actually turned on, quoted verbatim.

## The stall (old DLL `2362869a`) -- the redirect rewriting its own destination

```text
[+457ms] 2026-09-09 08:08:27:75 dll:ea04595d save-override: REDIRECT #0 access=0x80000000 disp=3 ok=false ret=0xffffffffffffffff 'C:\users\steamuser\AppData\Roaming\EldenRing\GraphicsConfig.xml'
    -> 'Z:\home\banon\.local\share\Steam\steamapps\compatdata\1245620\pfx\drive_c\users\steamuser\AppData\Roaming\EldenRing\76561197986456766\er-quickload-save-redirect-stage\eldenring\graphicsconfig.xml'
[+693ms] 2026-09-09 08:08:27:99 dll:ea04595d save-override: REDIRECT #1 access=0x0 disp=3 ok=false ret=0xffffffffffffffff 'Z:\home\banon\.local\share\Steam\steamapps\compatdata\1245620\pfx\drive_c\users\steamuser\AppData\Roaming\EldenRing\76561197986456766\er-quickload-save-redirect-stage\EldenRing\'
    -> 'Z:\home\banon\.local\share\Steam\steamapps\compatdata\1245620\pfx\drive_c\users\steamuser\AppData\Roaming\EldenRing\76561197986456766\er-quickload-save-redirect-stage\eldenring\76561197986456766\er-quickload-save-redirect-stage\eldenring\'
[+693ms] 2026-09-09 08:08:27:99 dll:ea04595d save-override: REDIRECT #2 access=0x0 disp=3 ok=false ret=0xffffffffffffffff 'Z:\home\banon\.local\share\Steam\steamapps\compatdata\1245620\pfx\drive_c\users\steamuser\AppData\Roaming\EldenRing\76561197986456766\er-quickload-save-redirect-stage\'
    -> 'Z:\home\banon\.local\share\Steam\steamapps\compatdata\1245620\pfx\drive_c\users\steamuser\AppData\Roaming\EldenRing\76561197986456766\er-quickload-save-redirect-stage\eldenring\76561197986456766\er-quickload-save-redirect-stage\'
```

## Where it parked, and what never ran

```text
[+14853ms] 2026-09-09 08:08:42:14 dll:ea04595d product-core-autoload: waiting for the semantic Load-Game MenuMemberFuncJob node (owner=0xbe338000 dialog=0x12fc3880 slot=0) -- TitleTopDialog/registry/n
[+14632ms] 2026-09-09 08:08:41:92 dll:ea04595d loadgame-scan: done hits=0 rows_walked=2 found_member_node=0x0 found_item=0x0
```

## The same configuration on the fixed DLL `dfee9d87`

```text
[+617ms] 2026-09-09 08:59:51:51 dll:9a5aadae save-override: REDIRECT #0 access=0x80000000 disp=3 ok=false ret=0xffffffffffffffff 'C:\users\steamuser\AppData\Roaming\EldenRing\GraphicsConfig.xml'
    -> 'Z:\home\banon\.local\share\Steam\steamapps\compatdata\1245620\pfx\drive_c\users\steamuser\AppData\Roaming\EldenRing\76561197986456766\er-quickload-save-redirect-stage\eldenring\graphicsconfig.xml'
[+20351ms] 2026-09-09 09:00:11:25 dll:9a5aadae save-override: REDIRECT #1 access=0x80000000 disp=3 ok=true ret=0x28bc 'C:\users\steamuser\AppData\Roaming\EldenRing\76561197986456766\ER0000.co2'
    -> 'Z:\home\banon\.local\share\Steam\steamapps\compatdata\1245620\pfx\drive_c\users\steamuser\AppData\Roaming\EldenRing\76561197986456766\er-quickload-save-redirect-stage\eldenring\76561197986456766\er0000.co2'
[+21770ms] 2026-09-09 09:00:12:67 dll:9a5aadae loadgame-builder: 0x140826510 built for slot=0 (expected=-1) effective=0 owner=0x10e7f0 callers=[#0=0x6ffff9d6dfbc{self+0x9
[+22658ms] 2026-09-09 09:00:13:55 dll:9a5aadae title-setstate-trace: SetState(owner=0x5386800, state=5(PlayGame)) committed_was=10(MenuJobWait) req_co
ENTERING WORLD 11/11 (MAP STEP DONE 4/8 - IDLE/DONE)
```
