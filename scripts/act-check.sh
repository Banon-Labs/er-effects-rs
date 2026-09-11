#!/usr/bin/env bash
# Run .github/workflows/check.yml locally, in containers, with `act`.
#
# Why a wrapper rather than a line in a README: `act` needs five flags here and four of them are
# not guessable. Getting one wrong does not fail loudly -- it runs a different workflow than GitHub
# will, which is the one outcome a local CI runner must never produce quietly.
#
#   -W .github/workflows/check.yml   there are four workflows in this repo; the default runs them all
#   -P ubuntu-latest=...             act prompts for an image size on first use and there is no
#                                    prompt in an agent shell. Pinning the image also makes two
#                                    runs comparable.
#   --artifact-server-path DIR       the `report` job downloads what the stage jobs upload. Without
#                                    a server act skips the upload and the report job then reports
#                                    every step no report -- correctly, and uselessly.
#   --matrix stage:<name>            one stage instead of eleven, which is the point of running
#                                    locally at all.
#   -j <job>                         `plan`, `compiles`, `stage` or `report`.
#
# Usage:
#   bash scripts/act-check.sh --list                 what act thinks the jobs and stages are
#   bash scripts/act-check.sh --stage docs           one stage, in a container
#   bash scripts/act-check.sh --job plan             one job
#   bash scripts/act-check.sh --dry-run --stage lint plan it without starting a container
#
# A container runtime has to be reachable. On this machine that is Docker Desktop's user service
# (`systemctl --user start docker-desktop`), which exposes its socket at
# ~/.docker/desktop/docker.sock under the `desktop-linux` context -- `/var/run/docker.sock` does
# not exist here and act's default would fail on it, so DOCKER_HOST is resolved below.
set -uo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
act_bin=${ACT_BIN:-$HOME/.local/bin/act}
image=${ACT_UBUNTU_IMAGE:-catthehacker/ubuntu:act-latest}
artifacts=${ACT_ARTIFACT_DIR:-${TMPDIR:-/tmp}/er-act-artifacts}

if [[ ! -x $act_bin ]]; then
	cat >&2 <<EOF
act-check: no act at $act_bin

Install it into the user prefix (no root needed):
  curl -fsSL https://raw.githubusercontent.com/nektos/act/master/install.sh -o "\$HOME/.local/src/act-install.sh"
  bash "\$HOME/.local/src/act-install.sh" -b "\$HOME/.local/bin"

Or point ACT_BIN at one you already have.
EOF
	exit 2
fi

# act talks to the daemon over DOCKER_HOST. Resolve it from the docker context rather than
# assuming /var/run/docker.sock: a rootless or Desktop install has no socket there, and act's
# error for that case names the socket rather than the reason.
if [[ -z ${DOCKER_HOST:-} ]]; then
	for candidate in "$HOME/.docker/desktop/docker.sock" "$HOME/.docker/run/docker.sock" \
		"${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/docker.sock" /var/run/docker.sock; do
		if [[ -S $candidate ]]; then
			export DOCKER_HOST="unix://$candidate"
			break
		fi
	done
fi
if [[ -z ${DOCKER_HOST:-} ]]; then
	echo "act-check: found no docker socket. Start a runtime first; on this machine that is" >&2
	echo "  systemctl --user start docker-desktop" >&2
	exit 2
fi

mode=run
job=""
stage=""
extra=()
while [[ $# -gt 0 ]]; do
	case "$1" in
	--list) mode=list && shift ;;
	--dry-run) extra+=(--dryrun) && shift ;;
	--stage)
		stage=${2:?--stage needs a name}
		job=stage
		shift 2
		;;
	--job)
		job=${2:?--job needs a name}
		shift 2
		;;
	*) extra+=("$1") && shift ;;
	esac
done

argv=("$act_bin" -W .github/workflows/check.yml -P "ubuntu-latest=$image"
	--artifact-server-path "$artifacts")
if [[ $mode == list ]]; then
	argv+=(--list)
else
	[[ -n $job ]] && argv+=(-j "$job")
	[[ -n $stage ]] && argv+=(--matrix "stage:$stage")
fi
argv+=("${extra[@]+"${extra[@]}"}")

mkdir -p "$artifacts"
printf 'act-check: DOCKER_HOST=%s\n' "$DOCKER_HOST" >&2
printf 'act-check: %s\n' "${argv[*]}" >&2
cd "$repo_root" || exit 2
exec "${argv[@]}"
