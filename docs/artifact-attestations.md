# Artifact Attestations

Every DLL published on this repo's GitHub Releases carries a [GitHub artifact
attestation](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations):
signed SLSA build provenance proving the exact bytes were built by GitHub's
hosted runners, from this repository, at a specific commit, by the release
workflow -- not hand-built and uploaded by anyone's laptop.

## Rolling main build

Every successful push to `main` builds every shipped ME3-loadable DLL and
publishes an immutable prerelease tagged `main-<full-commit-SHA>`. These builds
share the same development version in purpose, but GitHub's immutable-release
setting requires a unique tag and asset set for each commit. Download the newest
`main-*` prerelease from the repository Releases page; use a numbered release
when you need an immutable stable version.

The build derives its package and artifact list from `scripts/me3-dll-list.py`,
so a newly shippable DLL cannot be silently omitted from either release type.

## Verifying a downloaded DLL

With the [GitHub CLI](https://cli.github.com/) (logged in via `gh auth login`):

```bash
gh attestation verify er_effects_rs.dll --repo Banon-Labs/er-effects-rs
```

Substitute the path of whichever DLL you downloaded. A successful verification
prints the workflow, commit, and repo the file was built from. The attestation
is bound to the file's digest, so renaming the file or fetching it from a
mirror does not break verification -- any byte change does.

Offline verification is possible with `gh attestation verify --bundle <path>`
plus `--custom-trusted-root`; see `gh attestation verify --help`.

To additionally pin the exact workflow that signed it:

```bash
gh attestation verify er_effects_rs.dll --repo Banon-Labs/er-effects-rs \
  --signer-workflow Banon-Labs/er-effects-rs/.github/workflows/release.yml
```

## What is (and is not) attested

Attested: every `*.dll` asset attached to a GitHub Release by the release
workflow, plus the `SHA256SUMS` manifest alongside them.

Not attested / not shipped:

- `amd_ags_x64.dll` (the `er-ags-stub` crate) is deliberately excluded from
  releases: it impersonates AMD's vendor DLL filename and exists only as a
  local swap-in for RenderDoc capture runs.
- Locally built packages (for example the Mushroom Man asset zip, which embeds
  game-derived assets that cannot be built in CI). Only the bare DLLs built in
  CI are attested; a locally assembled package should embed the CI-built,
  attested DLLs when it is intended for distribution.

## What this does not do

An attestation is provenance, not Authenticode. It does not chain to the
Windows trust store, does not accumulate SmartScreen/Defender reputation, and
does not stop an AV heuristic from flagging an unknown injected DLL. It
answers exactly one question -- "did these bytes really come from this source
tree?" -- which is the question a mod user (or their AV vendor's false-positive
form) can act on.
