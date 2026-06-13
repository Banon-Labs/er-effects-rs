#!/usr/bin/env python3
"""Regression tests for scripts/check-runtime-probe-contract.py."""
from __future__ import annotations

import importlib.util
import shutil
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CHECK_PATH = REPO_ROOT / "scripts" / "check-runtime-probe-contract.py"
FIXTURE_ROOT = REPO_ROOT / "target" / "runtime-probe-contract-fixtures"


def load_checker():
    spec = importlib.util.spec_from_file_location("check_runtime_probe_contract", CHECK_PATH)
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


def configure_module_paths(checker) -> None:
    checker.REPO_ROOT = FIXTURE_ROOT
    checker.AUTO_DIR = FIXTURE_ROOT / ".auto"
    checker.RUNTIME_TRIGGER_PATH = checker.AUTO_DIR / "run-runtime-once"
    checker.MEASURE_PATH = checker.AUTO_DIR / "measure.sh"
    checker.RUNTIME_WRAPPER_PATH = checker.AUTO_DIR / "run_runtime_experiment.sh"
    checker.RUNTIME_PROBE_PATH = checker.AUTO_DIR / "runtime_probe.sh"
    checker.RUNTIME_POLICY_PATH = checker.AUTO_DIR / "runtime_experiment_policy.rego"
    checker.SMOKE_DRIVER_PATH = FIXTURE_ROOT / "scripts" / "er-smoke-driver.sh"
    checker.AUTO_LOG_PATH = checker.AUTO_DIR / "log.jsonl"


def base_fixture() -> None:
    write_fixture(
        ".auto/measure.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\necho static-only measurement\n",
    )
    write_fixture(
        ".auto/run_runtime_experiment.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\necho 'Runtime probes are disabled fail-closed' >&2\nexit 2\n",
    )
    write_fixture(
        ".auto/runtime_probe.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\ntrap cleanup_runtime EXIT\nvalidate_runtime_policy\nsetup_runtime_payload\n",
    )
    write_fixture(
        ".auto/runtime_experiment_policy.rego",
        "package auto.runtime_experiment\nimport rego.v1\ndefault allow := false\ndeny contains message if { message := \"runtime probes are disabled fail-closed\" }\n",
    )
    write_fixture(
        "scripts/er-smoke-driver.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\nrequire_runtime_driver_opt_in() { [[ \"${ER_EFFECTS_ALLOW_RUNTIME_DRIVER:-0}\" == \"1\" ]] || exit 2; }\npreflight() { :; }\ndrive() {\n  require_runtime_driver_opt_in\n  preflight\n}\n",
    )


def assert_rules(checker, expected: set[str]) -> None:
    actual = {finding.rule for finding in checker.scan_contract()}
    if actual != expected:
        raise AssertionError(f"expected {sorted(expected)}, got {sorted(actual)}")


def main() -> int:
    if FIXTURE_ROOT.exists():
        shutil.rmtree(FIXTURE_ROOT)
    checker = load_checker()
    configure_module_paths(checker)

    base_fixture()
    assert_rules(checker, set())

    write_fixture(".auto/run-runtime-once", "probe\n")
    assert_rules(checker, {"active-runtime-trigger"})
    (FIXTURE_ROOT / ".auto" / "run-runtime-once").unlink()

    write_fixture(
        ".auto/measure.sh",
        "#!/usr/bin/env bash\n./.auto/runtime_probe.sh\n",
    )
    assert_rules(checker, {"measure-launches-runtime"})
    base_fixture()

    write_fixture(
        ".auto/run_runtime_experiment.sh",
        "#!/usr/bin/env bash\nprintf probe > .auto/run-runtime-once\nexport AUTO_ALLOW_RUNTIME_PROBE=1\nexec ./.auto/measure.sh\n",
    )
    assert_rules(checker, {"runtime-wrapper-arms-launch"})
    base_fixture()

    write_fixture(
        ".auto/runtime_experiment_policy.rego",
        "package auto.runtime_experiment\nimport rego.v1\ndefault allow := false\nallow if { input.explicit_opt_in == true }\n",
    )
    assert_rules(
        checker,
        {"runtime-policy-allows-probes", "runtime-policy-missing-disabled-deny"},
    )
    base_fixture()

    write_fixture(
        "scripts/helper.py",
        "payload = {'timeout_seconds': 900}\n",
    )
    assert_rules(checker, {"agent-tool-timeout-field"})
    (FIXTURE_ROOT / "scripts" / "helper.py").unlink()
    base_fixture()

    write_fixture(
        "scripts/er-smoke-driver.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\npreflight() { :; }\ndrive() {\n  preflight\n}\n",
    )
    assert_rules(
        checker,
        {"runtime-driver-missing-explicit-opt-in", "runtime-driver-guard-not-first"},
    )

    print("runtime probe contract regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
