#!/usr/bin/env python3
"""Is this one line a Conventional Commits subject? The single place that answers it.

Why this file exists at all. `.github/workflows/release.yml` has run Release Please since it was
written, and Release Please reads `feat:`/`fix:` prefixes off main to decide the next version. It
has never had any to read: measured 2026-09-03, 15 of the last 400 subjects on main match the
convention and none of the recent ones do, which is why `.release-please-manifest.json` still says
0.1.6. The repo was carrying a release mechanism whose input nothing produced.

Why the enforcement is on a PR title and not on a commit message. The obvious server-side answer --
a repository ruleset with a `commit_message_pattern` rule -- does not work, and it fails silently
rather than loudly. Measured on this repo, 2026-09-03: `POST /repos/{r}/rulesets` accepts the rule
and returns 201 listing it, and with that rule active on a probe branch requiring
`^(feat|fix|chore|...): `, both of these still succeeded --

    Post /repos/{r}/merges   commit_message="Merge pull request #999 from ..."   -> 201, sha 0f61506
    put  /repos/{r}/contents message="junk message that violates the rule"       -> 201

-- so GitHub does not apply metadata rules to commits it creates server-side, which is every commit
the merge button makes. (GitHub's current "Available rules for rulesets" page no longer documents
metadata restrictions at all; treat the rule type as retired-but-accepted.) The lever that does
hold is the pull request TITLE: with the repo's `merge_commit_title` set to `PR_TITLE` (flipped
2026-09-03, from `MERGE_MESSAGE`, which is what produced five months of `Merge pull request #N
from ...` subjects), the PR title becomes the merge commit's subject, and a required status check
on that title gates the merge button before the commit exists.

So one regex has three consumers and they must not drift apart:

    scripts/hooks/commit-msg                     every local commit, before it exists
    .github/workflows/conventional-commits.yml   the PR title, as a required check
    .github/workflows/conventional-commits.yml   head's subject after a push to main

The third is the one that catches a human editing the merge dialog's textarea, which no server-side
rule can prevent -- it is detective, not preventive, and it says so where it runs.

    python3 scripts/conventional-commit-subject.py --subject "feat: add a thing"
    python3 scripts/conventional-commit-subject.py --file .git/COMMIT_EDITMSG
    python3 scripts/conventional-commit-subject.py --selftest
"""

from __future__ import annotations

import argparse
import re
import sys

# The types Release Please recognises, plus the ones this repo already writes. Adding a type here
# is the only supported way to widen the grammar -- a second regex somewhere else is the drift this
# file exists to prevent.
TYPES = (
    "feat",
    "fix",
    "chore",
    "docs",
    "style",
    "refactor",
    "perf",
    "test",
    "build",
    "ci",
    "revert",
)

# type(optional scope)optional-! : space description
#
# The trailing description is `.+` deliberately: GitHub appends " (#123)" to a merge subject built
# from a PR title, so the description must stay free-form for the same string to pass both as a PR
# title and as the merge commit it becomes.
SUBJECT_RE = re.compile(
    r"^(?:" + "|".join(TYPES) + r")(?:\([^()\n]+\))?!?: .+",
)

# A subject long enough to be truncated in `git log --oneline` and in the GitHub UI. Conventional
# Commits itself sets no limit; this is the repo's, and it is a refusal rather than a warning
# because a warning printed by a git hook is a warning nobody reads.
MAX_SUBJECT_LEN = 100

# Subjects git itself authors, or that carry no semantic payload of their own. Each is exempt for a
# concrete reason, not for tidiness:
#
#   Merge ...    `git merge` runs commit-msg too. Refusing these would make it impossible to merge
#                main into a branch locally, and the merge commit that reaches main is not this one
#                -- that one is built from the PR title, which the workflow gates.
#   fixup!/      consumed and discarded by `git rebase --autosquash`; they never reach main.
#   squash!
#   Revert "..." git's own `git revert` template. `revert: ` is in types for a hand-written one.
EXEMPT_PREFIXES = (
    "Merge ",
    "fixup!",
    "squash!",
    "amend!",
    'Revert "',
)


def classify(subject: str) -> tuple[bool, str]:
    """(is_acceptable, reason). Reason is empty when acceptable and unexempt."""
    subject = subject.rstrip("\n")
    if not subject.strip():
        return False, "the subject is empty"
    for prefix in EXEMPT_PREFIXES:
        if subject.startswith(prefix):
            return True, f"exempt: git-authored or autosquash subject ({prefix.strip()})"
    if not SUBJECT_RE.match(subject):
        return False, (
            "does not start with a Conventional Commits type. Expected "
            "'<type>[(scope)][!]: <description>' where <type> is one of: " + ", ".join(TYPES)
        )
    if len(subject) > MAX_SUBJECT_LEN:
        return False, f"subject is {len(subject)} characters; the limit is {MAX_SUBJECT_LEN}"
    return True, ""


def subject_of(message: str) -> str:
    """The first non-comment, non-blank line of a commit message file.

    `git commit -v` writes the diff and the '# Please enter the commit message' block into the same
    file the hook is handed, and a message that opens with a blank line is a real thing a user can
    produce. Taking `splitlines()[0]` blindly would read a comment or an empty string as the
    subject and pass a message that has none.
    """
    for line in message.splitlines():
        if line.startswith("#"):
            continue
        if not line.strip():
            continue
        return line
    return ""


def report(subject: str, source: str) -> int:
    ok, reason = classify(subject)
    if ok:
        detail = f" ({reason})" if reason else ""
        print(f"[conventional-commit-subject] OK: {subject!r}{detail}")
        return 0
    print(f"[conventional-commit-subject] REFUSED -- {source}", file=sys.stderr)
    print(f"  subject : {subject!r}", file=sys.stderr)
    print(f"  problem : {reason}", file=sys.stderr)
    print("", file=sys.stderr)
    print("  examples:", file=sys.stderr)
    print("    feat: make the loading portrait describe the character that loads", file=sys.stderr)
    print("    fix(er-invasion-warp): re-pin the ersc.dll entry points", file=sys.stderr)
    print("    chore!: drop the 1.16.2 address table", file=sys.stderr)
    print("", file=sys.stderr)
    print(
        "  the type is what Release Please reads to pick the next version: "
        "feat -> minor, fix -> patch, ! -> major.",
        file=sys.stderr,
    )
    return 1


def selftest() -> int:
    accept = [
        "feat: add a thing",
        "fix: repair a thing",
        "fix(er-quickload): repair a scoped thing",
        "feat!: a breaking thing",
        "feat(scope)!: a breaking scoped thing",
        "chore(main): release 0.1.7",
        # What a merge commit built from a PR title actually looks like once GitHub is done
        # with it. If this case ever fails, the main-subject job goes red on every merge.
        "fix: repair a thing (#399)",
        "revert: undo the thing",
        # Exempt shapes -- accepted, but for a stated reason rather than by matching.
        "Merge branch 'main' into feature",
        "Merge pull request #398 from Banon-Labs/whatever",
        "Merge remote-tracking branch 'origin/main'",
        "fixup! feat: add a thing",
        'Revert "feat: add a thing"',
    ]
    reject = [
        "",
        "   ",
        "add a thing",
        "Make the loading-screen portrait describe the character that loads",
        "feat add a thing",  # no colon
        "feat:add a thing",  # no space after the colon
        "feat: ",  # no description
        "feats: add a thing",  # not a known type
        "Feat: add a thing",  # types are lower-case
        "feat(): add a thing",  # empty scope
        "wip",
        "feat: " + "x" * MAX_SUBJECT_LEN,  # over the length limit
    ]

    failures = []
    for subject in accept:
        ok, reason = classify(subject)
        if not ok:
            failures.append(f"should ACCEPT {subject!r} but refused it: {reason}")
    for subject in reject:
        ok, _ = classify(subject)
        if ok:
            failures.append(f"should REJECT {subject!r} but accepted it")

    # subject_of, which is the half the hook depends on and the workflow never exercises.
    cases = [
        ("feat: a thing\n\nbody\n", "feat: a thing"),
        ("# a comment\nfeat: a thing\n", "feat: a thing"),
        ("\n\nfeat: a thing\n", "feat: a thing"),
        ("# only comments\n", ""),
        ("", ""),
    ]
    for message, expected in cases:
        got = subject_of(message)
        if got != expected:
            failures.append(f"subject_of({message!r}) = {got!r}, expected {expected!r}")

    # The grammar is not a private opinion. Every type this file accepts has to be a type Release
    # Please can act on, or the gate would enforce a convention the release path ignores.
    for release_type in ("feat", "fix"):
        if release_type not in TYPES:
            failures.append(f"{release_type!r} must be accepted; Release Please keys versions off it")

    if failures:
        print("[conventional-commit-subject] SELFTEST FAILED", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1
    print(
        f"[conventional-commit-subject] selftest OK "
        f"({len(accept)} accepted, {len(reject)} refused, {len(cases)} subject_of cases)"
    )
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--subject", help="the one line to judge (a PR title, or `git log -1 --pretty=%%s`)")
    group.add_argument("--file", help="a commit message file; its first real line is judged")
    group.add_argument("--selftest", action="store_true", help="run the built-in cases and exit")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()
    if args.file:
        try:
            with open(args.file, encoding="utf-8", errors="replace") as handle:
                message = handle.read()
        except OSError as exc:
            print(f"[conventional-commit-subject] cannot read {args.file}: {exc}", file=sys.stderr)
            return 2
        return report(subject_of(message), f"the commit message in {args.file}")
    return report(args.subject, "this subject")


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
