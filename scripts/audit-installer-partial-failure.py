#!/usr/bin/env python3
"""Sweep 1 detector: which hook/patch installers lose sibling hooks to one refusal?

On 1.17 an address with no verified mapping is refused by design. An installer that
turns that refusal into a function-level `return`/`?` kills every other hook it was
going to install -- so one unmapped address silently disables a dozen working ones
(bd `one-refused-hook-must-not-abort-the-installer-2026-08-30`).

This flags a function when both hold:
  * it performs 2+ install/resolve actions (siblings exist to lose), and
  * it has an abort construct (`?`, `.unwrap()`, `return`, `let ... else { .. return }`)
    that is not inside a `continue`-shaped or labelled-block-shaped recovery.

It deliberately over-reports: the output is a review list, not a verdict, and it is a
manual tool -- deliberately not wired into `scripts/check.sh`. What follows is why, and
what became of the nineteen rows it printed on 2026-08-31.

Why this does not gate
======================
Its stated subject is now covered precisely by two gates that do run:

  * `scripts/check-hook-batch-abort.py` -- the batched path. `MhHook::new` + `queue_enable`
    + `MH_ApplyQueued` is the only shape that can leave an already-created detour stranded,
    because the `register_*_hook` registrars call `MH_EnableHook` immediately and never
    touch MinHook's pending set. That gate enumerates the batched universe exactly.
  * `scripts/check-detour-rva-coverage.py` -- the addresses that go missing in the first place.

This one keys on `return`-appears-anywhere with two or more actions, which cannot tell an
abort apart from a re-entry guard, a module-base early-out, or the "nothing queued, so
nothing to apply" check that the correct fix for the P0 introduces. Wiring it would mean
bolting a fifteen-row acknowledgement list onto a matcher whose own rule cannot distinguish
the rows -- a green that means nothing.

The nineteen rows of 2026-08-31, ADJUDICATED
============================================
  4  matcher fault, now FIXED: the install pattern matched the function's own `fn NAME(`
     declaration, putting all four of er-hook's single-hook dispatch wrappers over the
     `actions >= 2` bar. See `body_of`. The count is 15 after the fix.
  1  real defect, now fixed: `er-refill-all/src/runtime.rs::install` registered
     `DepositoryDialog::ctor` before `::dtor` through an immediate-enable registrar, so a
     refused dtor address left the ctor detour armed with nothing to clear the latch it
     sets -- and the caller registers the FrameBegin task regardless of the early return,
     so `tick()` read through a freed `DepositoryDialog*` every frame for the rest of the
     session. Fixed by registering the dtor first, which makes both partial states inert.
  3  already remediated, flagged only by the coarse `return` rule: the two 2026-08-30
     per-hook-latch rewrites (`title_resources_stats_text.rs`, `stats_loading_text.rs`) and
     `install_scaleform_handler_lifecycle_guard`, which is declared atomic with a reason.
 11  correct by construction: re-entry guards, `MH_Initialize` early-outs, module-base
     guards, read-only probes that merely call `write_global_u8`, and per-hook recovery
     that the rule cannot see.

Zero were a live queued-but-never-applied batch failure. Run `--selftest` to see the
positive controls fire.
"""
import re
import sys
import os

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from rustfn import functions  # noqa: E402

INSTALL = re.compile(
    r'MhHook::new|MH_CreateHook|register_shared_hook(?:_with_budget)?|register_union_hook'
    r'|patch_3byte_stub|apply_xor_ret_stub|write_code_byte|write_global_u8'
)
RESOLVE = re.compile(r'game_rva(?:_named)?\s*\(|resolve_detour_address\s*\(|resolve_game_address(?:_fmt)?\s*\(')
ABORT_Q = re.compile(r'(game_rva(?:_named)?\s*\([^;]*?\)|MhHook::new\s*\([^;]*?\)|register_shared_hook[^;]*?\))\s*\?')
ABORT_UNWRAP = re.compile(r'(game_rva(?:_named)?|MhHook::new|resolve_detour_address)[^;\n]*?\.(unwrap|expect)\s*\(')
LET_ELSE_RET = re.compile(r'let\s+(?:Ok|Some)\s*\([^)]*\)\s*=\s*[^;]*?else\s*\{[^}]*?\breturn\b', re.S)
RECOVERY = re.compile(r'\bcontinue\b|\bbreak\s+\'')


def body_of(mask):
    """The masked text from the opening brace onwards -- the signature removed.

    `mask` begins at the `fn NAME(` declaration, and several of the install primitives this
    scans for are themselves functions in `er-hook`, so a declaration scored a hit on itself.
    Counting the signature put all four of er-hook's single-hook dispatch wrappers over the
    `actions >= 2` threshold and onto the review list as batch installers, which they are not:
    `register_union_hook`, `register_union_hook_resolved`, `register_union_hook_runtime_derived`
    and `register_shared_hook_with_budget` each bind exactly one target and delegate. Four of
    the nineteen 2026-08-31 findings were this and nothing else.

    Name-comparison does not fix it, because `INSTALL` matches `register_union_hook` as a
    prefix of `fn register_union_hook_resolved` -- the matched text and the function name are
    different strings, so the self-hit survives the comparison. Dropping the signature does fix
    it, and it also drops the parameter list, where a `stub: [u8; STUB_LEN]` argument name can
    collide with nothing but noise.
    """
    brace = mask.find('{')
    return mask if brace < 0 else mask[brace:]


def scan(paths):
    rows = []
    for p in paths:
        for name, line, body, full_mask in functions(p):
            mask = body_of(full_mask)
            installs = len(INSTALL.findall(mask))
            resolves = len(RESOLVE.findall(mask))
            actions = max(installs, resolves)
            if installs == 0:
                continue
            aborts = []
            if ABORT_Q.search(mask):
                aborts.append('?')
            if ABORT_UNWRAP.search(mask):
                aborts.append('unwrap')
            if LET_ELSE_RET.search(mask):
                aborts.append('let-else-return')
            if re.search(r'\breturn\b', mask) and actions >= 2:
                aborts.append('return')
            recovery = bool(RECOVERY.search(mask))
            if actions >= 2 and aborts:
                rows.append((p, name, line, installs, resolves, ','.join(sorted(set(aborts))), recovery))
    return rows


def selftest():
    import tempfile
    bad = '''
fn install_two() -> Result<(), String> {
    let a = game_rva(0x1000)?;
    unsafe { MhHook::new(a as *mut _, x as *mut _) }?.queue_enable().ok();
    let b = game_rva(0x2000)?;
    unsafe { MhHook::new(b as *mut _, y as *mut _) }?.queue_enable().ok();
    Ok(())
}
fn install_two_ok() {
    for (rva, imp) in TARGETS {
        let Ok(a) = game_rva(rva) else { log("REFUSED"); continue; };
        let Ok(h) = (unsafe { MhHook::new(a as *mut _, imp) }) else { log("FAILED"); continue; };
        unsafe { h.queue_enable().ok() };
    }
}
'''
    fd, p = tempfile.mkstemp(suffix='.rs')
    os.write(fd, bad.encode())
    os.close(fd)
    rows = scan([p])
    os.unlink(p)
    names = {r[1]: r for r in rows}
    assert 'install_two' in names, f'POSITIVE CONTROL FAILED: abort-shaped installer not flagged ({rows})'
    assert '?' in names['install_two'][5], names['install_two']
    assert 'install_two_ok' not in names or names['install_two_ok'][6], \
        'continue-shaped installer flagged without recovery credit'
    print('audit-installer-partial-failure selftest OK: positive control fired on `?`-abort installer,')
    print('  and the continue-shaped installer was either clean or credited with recovery.')


def main():
    if '--selftest' in sys.argv:
        selftest()
        return
    import glob
    paths = [a for a in sys.argv[1:] if not a.startswith('-')] or \
        sorted(glob.glob('crates/**/*.rs', recursive=True))
    rows = scan(paths)
    print(f'{"file":78} {"fn":42} {"ln":>5} {"inst":>4} {"resv":>4} {"abort":22} recovery?')
    for r in sorted(rows):
        print(f'{r[0]:78} {r[1]:42} {r[2]:>5} {r[3]:>4} {r[4]:>4} {r[5]:22} {"yes" if r[6] else "NO"}')
    print(f'\n{len(rows)} candidate installers')


if __name__ == '__main__':
    main()
