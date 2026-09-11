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

# A linked worktree cannot be the source, and the failure is not obvious when it happens. act does
# not run `actions/checkout` against a remote; it copies the local tree into the container. In a
# `git worktree` checkout `.git` is a one-line pointer file naming a directory under the main
# checkout's `.git/worktrees/`, and that directory is not copied -- so every `git` command inside
# the container exits 128. Measured on `--stage lint`: check-comment-caps.py died with
# `CalledProcessError ... 'git' 'ls-files' '-z' ... exit status 128`, and so would every other gate
# that reads the index. Stages whose gates never shell out to git (docs, for one) are unaffected,
# which is what makes this confusing rather than obvious.
#
# So clone `HEAD` into a scratch directory and point act at that. A clone has a real `.git`, and
# it is also closer to what CI does -- `actions/checkout` produces exactly this shape:
# tracked files only, no gitignored game images, no vendor/. The cost is the thing to say out loud:
# this runs the committed `HEAD`, not the working tree. Commit first, or set `ACT_SOURCE` to a
# path of your own.
source_dir=${ACT_SOURCE:-$repo_root}
if [[ -z ${ACT_SOURCE:-} && -f "$repo_root/.git" ]]; then
	source_dir=${ACT_CLONE_DIR:-${TMPDIR:-/tmp}/er-act-source}
	rm -rf "$source_dir"
	printf 'act-check: %s is a linked worktree, whose .git is a pointer act cannot copy.\n' "$repo_root" >&2
	printf 'act-check: cloning HEAD into %s and running act there instead.\n' "$source_dir" >&2
	printf 'act-check: that is the committed HEAD, not your working tree. Commit first.\n' >&2
	git clone --quiet --no-hardlinks "$repo_root" "$source_dir" || exit 2
	git -C "$source_dir" checkout --quiet "$(git -C "$repo_root" rev-parse HEAD)" || exit 2
fi

printf 'act-check: DOCKER_HOST=%s\n' "$DOCKER_HOST" >&2
printf 'act-check: source=%s\n' "$source_dir" >&2
printf 'act-check: %s\n' "${argv[*]}" >&2
cd "$source_dir" || exit 2
exec "${argv[@]}"
