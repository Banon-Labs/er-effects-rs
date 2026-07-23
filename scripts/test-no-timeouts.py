#!/usr/bin/env python3
"""Regression tests for scripts/check-no-timeouts.py."""
from __future__ import annotations

import importlib.util
import shutil
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CHECK_PATH = REPO_ROOT / "scripts" / "check-no-timeouts.py"
FIXTURE_ROOT = REPO_ROOT / "target" / "no-timeouts-fixtures"


def load_checker():
    spec = importlib.util.spec_from_file_location("check_no_timeouts", CHECK_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {CHECK_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_fixture(relative: str, body: str) -> Path:
    path = FIXTURE_ROOT / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")
    return path


def rule_codes(checker, relative: str, body: str) -> set[str]:
    path = write_fixture(relative, body)
    return {finding.rule.code for finding in checker.scan_file(path)}


def assert_codes(checker, relative: str, body: str, expected: set[str]) -> None:
    actual = rule_codes(checker, relative, body)
    if actual != expected:
        raise AssertionError(f"{relative}: expected {sorted(expected)}, got {sorted(actual)}")


def main() -> int:
    if FIXTURE_ROOT.exists():
        shutil.rmtree(FIXTURE_ROOT)
    checker = load_checker()

    cases = [
        ("bad-sleep.sh", "#!/usr/bin/env bash\nsleep 1\n", {"shell-sleep-command"}),
        ("bounded-timeout.sh", "#!/usr/bin/env bash\ntimeout 5 command\n", set()),
        ("bad-timeout-too-large.sh", "#!/usr/bin/env bash\ntimeout 31 command\n", {"shell-timeout-unbounded-or-over-30"}),
        ("bad-timeout-variable.sh", "#!/usr/bin/env bash\ntimeout \"$limit\" command\n", {"shell-timeout-unbounded-or-over-30"}),
        ("bounded-read.sh", "#!/usr/bin/env bash\nread -r -t 1 value\n", set()),
        ("bad-read-too-large.sh", "#!/usr/bin/env bash\nread -r -t 31 value\n", {"shell-timeout-unbounded-or-over-30"}),
        ("bad-thread.rs", "fn main() { std::thread::sleep(duration); }\n", {"rust-thread-sleep"}),
        ("bad-tokio-sleep.rs", "async fn f() { tokio::time::sleep(limit).await; }\n", {"rust-async-sleep"}),
        ("bad-elapsed.rs", "fn f(start: Instant) { if start.elapsed() > limit { panic!(); } }\n", {"rust-elapsed-deadline"}),
        ("bad-duration-max.rs", "fn f() { wait_for_instance(Duration::MAX); }\n", {"rust-duration-max", "rust-timeout-wait-api"}),
        ("bad-wait-api.rs", "fn f() { wait_for_system_init(module, duration); }\n", {"rust-timeout-wait-api"}),
        ("bad-python-sleep.py", "import time\ntime.sleep(1)\n", {"python-sleep-or-wait-for"}),
        ("bounded-python-timeout.py", "subprocess.run(args, timeout=30)\n", set()),
        ("bad-python-missing-timeout.py", "subprocess.run(args)\n", {"python-subprocess-missing-timeout"}),
        ("bad-python-timeout-variable.py", "subprocess.run(args, timeout=limit)\n", {"python-subprocess-unbounded-or-over-30-timeout"}),
        ("bad-python-timeout-too-large.py", "subprocess.run(args, timeout=31)\n", {"python-subprocess-unbounded-or-over-30-timeout"}),
        ("bad-js-timer.ts", "setTimeout(resolve, 1);\n", {"js-timer-api"}),
        ("bad-ci.yml", "jobs:\n  check:\n    timeout-minutes: 1\n", {"yaml-timeout-minutes-over-30-seconds"}),
        ("good-read.sh", "#!/usr/bin/env bash\nwhile IFS= read -r line; do printf '%s\\n' \"$line\"; done\n", set()),
        # A bare `read -r` next to a `-type` operand must not trip the read-timeout rule (the `-t`
        # substring of `-type` is not a `read -t` flag).
        ("good-find-read.sh", "#!/usr/bin/env bash\nfind . -type f | while IFS= read -r f; do echo \"$f\"; done\n", set()),
        # subprocess.Popen takes no timeout= kwarg (timeout belongs on .communicate()/.wait()).
        ("good-popen.py", "import subprocess\np = subprocess.Popen(['x'], stdout=subprocess.PIPE)\n", set()),
        ("good-rust.rs", "fn f() { std::thread::yield_now(); let _interval = Duration::from_millis(250); }\n", set()),
        ("good-python.py", "event.wait(); process.wait()\n", set()),
        ("good-js.ts", "await readiness; emitter.once('ready', resolve);\n", set()),
    ]
    for relative, body, expected in cases:
        assert_codes(checker, relative, body, expected)

    print("no-timeouts scanner regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
