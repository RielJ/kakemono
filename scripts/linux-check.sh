#!/usr/bin/env bash
# Runs CI's Linux checks in a local Ubuntu 24.04 container (Docker), so Linux-only failures show
# up before a push. The container gets a copy of the working tree (tracked and untracked files,
# minus ignored ones), so nothing it builds touches this checkout's target/ or node_modules/.
#
# Usage: scripts/linux-check.sh [STEP...]
#   rust      cargo fmt --check, clippy -D warnings, cargo test (ci.yml `rust`)
#   frontend  npm ci, typecheck, build, test (ci.yml `frontend`)
#   deb       build the sidecar and the .deb, then check it (release.yml `ui`, deb only)
#   Default: rust frontend deb.
#
# Builds the image for the host's architecture (arm64 on Apple silicon), so the deb is an arm64
# package; CI's release builds x86_64.
#
# Cleanup: the container is always removed. The image and the cache volumes (cargo registry,
# target dir, node_modules) are removed when the run ends, unless KK_LINUX_KEEP=1, which keeps
# them to make the next run fast. `scripts/linux-check.sh --clean` removes kept ones. Docker's
# build cache (BuildKit layers, about 2.5 GB after a build) is shared with every other image
# build, so this script leaves it; `docker builder prune` removes it.
#
# Environment:
#   KK_LINUX_KEEP=1     keep the image and cache volumes after the run
#   CARGO_BUILD_JOBS=N  passed into the container (default: half the CPUs Docker has)
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

image=kakemono-linux-check
volumes=(kakemono-linux-registry kakemono-linux-target kakemono-linux-node-modules)
container="kakemono-linux-check-$$"

clean() {
  docker image rm -f "$image" >/dev/null 2>&1 || true
  docker volume rm -f "${volumes[@]}" >/dev/null 2>&1 || true
}

if [[ "${1:-}" == "--clean" ]]; then
  clean
  echo "linux-check: removed the image and cache volumes"
  echo "linux-check: Docker's shared build cache is left; 'docker builder prune' removes it"
  exit 0
fi
if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
fi

steps=("$@")
[[ ${#steps[@]} -gt 0 ]] || steps=(rust frontend deb)

command -v docker >/dev/null || { echo "linux-check: docker is not installed" >&2; exit 1; }
docker info >/dev/null 2>&1 || { echo "linux-check: the Docker daemon is not running" >&2; exit 1; }

finish() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  if [[ "${KK_LINUX_KEEP:-0}" != 1 ]]; then
    clean
  fi
}
trap finish EXIT

echo "==> docker build $image"
node_major="$(tr -d '[:space:]' < .node-version)"
docker build -q -t "$image" --build-arg "NODE_MAJOR=$node_major" \
  -f scripts/linux/Dockerfile - < <(tar -cf - -C "$root" scripts/linux/Dockerfile rust-toolchain.toml) >/dev/null

jobs="${CARGO_BUILD_JOBS:-$(( $(docker info --format '{{.NCPU}}') / 2 ))}"
[[ $jobs -ge 1 ]] || jobs=1

# The working tree as CI would check it out, plus uncommitted changes. macOS tar would add an
# AppleDouble `._name` file for each file with extended attributes; COPYFILE_DISABLE stops that.
export COPYFILE_DISABLE=1
git ls-files -z --cached --others --exclude-standard \
  | while IFS= read -r -d '' f; do [[ -e "$f" ]] && printf '%s\0' "$f"; done \
  | tar --null -T - -cf - \
  | docker run --rm -i --name "$container" \
      -e CARGO_BUILD_JOBS="$jobs" -e CI=true \
      -v kakemono-linux-registry:/home/runner/.cargo/registry \
      -v kakemono-linux-target:/target \
      -v kakemono-linux-node-modules:/src/crates/ui/frontend/node_modules \
      "$image" bash -c 'tar -xf - -C /src && exec bash /src/scripts/linux/inside.sh "$@"' _ "${steps[@]}"
