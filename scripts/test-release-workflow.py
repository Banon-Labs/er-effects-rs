#!/usr/bin/env python3
"""Prevent the public DLL-download workflow from drifting back to a partial build.

GitHub only validates workflow YAML, not this repository's distribution contract:
every push to main must build every ME3-loadable DLL, publish an immutable main
prerelease, and publish numbered releases only after their assets are attached.
This scanner has no YAML dependency because it runs in the local and CI Python
baseline.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "release.yml"
RELEASE_CONFIG = REPO_ROOT / "release-please-config.json"
RELEASE_MANIFEST = REPO_ROOT / ".release-please-manifest.json"


def job_body(text: str, name: str) -> str:
    match = re.search(
        rf"^  {re.escape(name)}:\n(.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)", text, re.MULTILINE | re.DOTALL
    )
    if not match:
        raise AssertionError(f"missing workflow job: {name}")
    return match.group(1)


def require(text: str, needle: str, label: str) -> None:
    if needle not in text:
        raise AssertionError(f"missing {label}: {needle!r}")


def main() -> int:
    text = WORKFLOW.read_text(encoding="utf-8")
    try:
        config = json.loads(RELEASE_CONFIG.read_text(encoding="utf-8"))
        manifest = json.loads(RELEASE_MANIFEST.read_text(encoding="utf-8"))
        assert config["release-type"] == "simple"
        assert re.fullmatch(r"[0-9a-f]{40}", config["bootstrap-sha"])
        assert config["draft"] is True
        assert config["force-tag-creation"] is True
        assert config["include-v-in-tag"] is False
        assert config["packages"] == {".": {}}
        assert re.fullmatch(r"\d+\.\d+\.\d+", manifest["."])

        require(text, "branches: [main]", "main push trigger")

        release_please = job_body(text, "release-please")
        require(release_please, "config-file: release-please-config.json", "release config")
        require(release_please, "manifest-file: .release-please-manifest.json", "release manifest")

        build = job_body(text, "build-attest-publish-main")
        if re.search(r"^    needs:", build, re.MULTILINE):
            raise AssertionError("main build must not depend on Release Please")
        require(build, "bash scripts/er-build-dlls.sh --all", "all-DLL build command")
        require(build, "scripts/me3-dll-list.py --pairs", "dynamic artifact list")
        require(build, "actions/attest@v4", "DLL attestation")
        require(build, "actions/upload-artifact@v4", "versioned-release handoff")
        require(build, 'rolling_tag="main-${GITHUB_SHA}"', "unique rolling tag")
        require(build, 'gh release create "$rolling_tag" dist/*', "immutable prerelease creation")
        if "gh release edit \"$rolling_tag\"" in build or "--clobber" in build:
            raise AssertionError("immutable main prerelease must never be edited or clobbered")

        versioned = job_body(text, "upload-versioned-release")
        require(
            versioned,
            "needs: [release-please, build-attest-publish-main]",
            "versioned-release dependencies",
        )
        require(
            versioned,
            "needs.release-please.outputs.release_created == 'true'",
            "versioned-release condition",
        )
        require(versioned, "actions/download-artifact@v4", "staged-file retrieval")
        require(versioned, 'gh release upload "$tag_name" dist/*', "draft asset upload")
        require(versioned, 'gh release edit "$tag_name" --draft=false', "draft publication")
        if "--clobber" in versioned:
            raise AssertionError("immutable versioned release must not clobber assets")
    except (AssertionError, KeyError, json.JSONDecodeError) as err:
        print(f"[test-release-workflow] FAIL: {err}", file=sys.stderr)
        return 1

    print("[test-release-workflow] ok -- main and numbered releases publish all DLLs immutably")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
