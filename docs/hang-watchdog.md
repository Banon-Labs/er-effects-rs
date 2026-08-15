# Hang watchdog

## Why it exists

A crash logger only sees a process that *faults*. A process that **freezes** raises no exception at
all, so a lockup produces an empty crash log.

That is not hypothetical. The 2026-08-14 crash log from a Seamless Co-op invasion session contained
no exception whatsoever across the 4m47s of actual play. The only fatal record was a shutdown-path
`DLPanic` that fired *after* the window was closed -- evidence about quitting, not about the bug the
session was run to capture. See bd `er-shutdown-drain-dlpanic-soloparamrepo-race-2026-08-14`.

The watchdog closes that blind spot: it detects the freeze itself and captures every thread's state
while the game is still frozen and readable.

## How it detects a hang

`MainUpdate` (`0x140dea370` on ER 1.16.2) increments a single dword once per main-loop frame:

```
140dea394  INC dword ptr [0x143d8567c]
```

That dword has exactly **one** writer in the entire binary -- this instruction. All five other
references to it are reads. So it advances if and only if the main loop ticks, which makes "counter
frozen for N seconds" a sound main-thread liveness oracle with no competing write sources.

The address is version-pinned (RVA `0x3d8567c`, added to the loaded `eldenring.exe` base -- the exe is
ASLR-relocated, so the static VA is never used directly).

## Why a wrong address cannot produce a fake hang

The watchdog **refuses to arm** until it has actually watched the counter advance several times. On a
game build where the field has moved, the address either reads as unmapped or sits still, and the
watchdog disarms and says so rather than reporting a permanent freeze that is not happening.

Before trusting a quiet run, confirm it armed. In `er-crash-log.txt`:

```
hang watchdog armed addr=0x... rva=0x3d8567c frame_counter=12345 stall_seconds=30
```

A `hang watchdog disarmed reason=frame-counter-never-advanced` line instead means the RVA needs
re-deriving against the current game build. `reason=game-module-not-found` means the DLL was loaded
into something that is not `eldenring.exe`, and the watchdog correctly stayed inert.

## What it captures

On a stall it writes, in this order:

| File | Contents |
| --- | --- |
| `er-crash-hang-latest.txt` | Every thread: tid, `main_thread=true/false`, rip, rsp, and a module-resolved stack scan |
| `er-crash-hang-minidump.dmp` | Real unwind for every thread, same moment, plus referenced memory |
| `er-crash-log.txt` | The same text record appended after the arm line |

The standalone file is written first and is not shared with any other writer, so it stays intact even
if a concurrent fault record interleaves into the appended copy.

The main thread is identified by creation time -- the process's first thread is the earliest-created
one -- so the stalled stack is labelled rather than left for the reader to guess.

## Safety properties

* **It never allocates while a thread is suspended.** Suspending a thread that holds the heap lock and
  then allocating would deadlock the very process being diagnosed. Each thread is suspended, read into
  fixed-size storage, resumed, and only then formatted.
* **It never kills, faults, or changes game state.** Diagnostic only.
* **Reports are bounded** (3 per process), and one stall produces one report -- the counter must
  advance again before another can fire.

## Known limit

This detects a **main-thread** stall. A render-thread or worker deadlock while the main loop keeps
ticking will not trip it. If a freeze is observed and the log shows the watchdog armed but never
reported, that is real evidence: the main loop was still running, and the problem is elsewhere.

## Using it

Load `er_crash_logging_dll.dll` as a `[[natives]]` entry **after** `ersc.dll`.

Order matters: `SetUnhandledExceptionFilter` is last-writer-wins, so the crash logger has to install
after Seamless's crashpad to end up on top -- and it chains whatever it displaced rather than
swallowing it. Loading it first puts our filter underneath ersc's, where it never runs.

`~/Elden/seamless-invasion-hangwatch.me3` is a ready profile in that order.

Tuning: `CrashLogConfig::hang_stall_seconds` (default 30). Set it to 0 to disable the watchdog.
