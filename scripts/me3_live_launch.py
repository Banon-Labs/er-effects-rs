import subprocess, sys
ME3 = "/mnt/c/Users/choza/AppData/Local/garyttierney/me3/bin/me3.exe"
DLL = r"C:\Users\choza\build\y22i\guard\er-effects-rs\target\x86_64-pc-windows-msvc\release\er_effects_rs.dll"
CWD = "/mnt/c/Users/choza/build/y22i"
LOG = "/tmp/claude-1000/-home-choza-projects-er-effects-rs/b16ef7d2-c5c2-4cc2-885c-fd7f850a469e/me3-live.log"
log = open(LOG, "w", encoding="utf-8")
# stdin=PIPE, never closed -> me3's stdin never EOFs, so me3 stays alive as the monitor and the
# game is NOT torn down. No taskkill / no cap: p.wait() returns only when the USER closes the game.
p = subprocess.Popen([ME3, "launch", "-g", "eldenring", "-n", DLL],
                     cwd=CWD, stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT)
print(f"me3 launched pid={p.pid}; holding it alive (no teardown). waiting for game exit...", flush=True)
rc = p.wait()
print(f"me3 exited rc={rc} (game closed by user)", flush=True)
