#!/usr/bin/env bash
# Runs CI's Windows checks (ci.yml's rust job on windows-latest) in the local Windows VM, so
# Windows failures show up before a push. Needs the VM from scripts/windows-vm/vm.sh (install,
# then setup).
#
#   scripts/windows-check.sh                   clippy, then the tests
#   scripts/windows-check.sh test [args...]    only `cargo test`; extra args go to cargo,
#                                              e.g. `test -p kakemono-fs`
#   scripts/windows-check.sh clippy            only clippy
#
# The VM is started if needed and left running (stop it with `vm.sh stop`), unless
# KK_WIN_STOP=1. KK_SRC=<path> checks another checkout, such as a git worktree. The checkout lives at C:\kk\src in the VM and the build cache at
# C:\kk\target, which is kept between runs.
#
# The VM runs aarch64-pc-windows-msvc while CI runs x86_64-pc-windows-msvc. The Windows APIs,
# NTFS and path rules are the same for both, which is what these tests exercise.
set -euo pipefail
{ # Parsed whole before it runs, so editing this file mid-run is safe (scripts/check-scripts.sh).

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
vm="$root/scripts/windows-vm/vm.sh"
cd "${KK_SRC:-$root}"

mode="${1:-all}"
[[ $# -gt 0 ]] && shift
case "$mode" in
  all | test | clippy) ;;
  -h | --help) awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "$0"; exit 0 ;;
  *) echo "windows-check: unknown mode '$mode'" >&2; exit 2 ;;
esac

"$vm" status | grep -q '^running' || "$vm" start

# The working tree as CI would check it out, plus uncommitted changes. No AppleDouble files.
echo "==> copying the working tree to C:\\kk\\src" >&2
"$vm" ssh '(if exist C:\kk\src rmdir /s /q C:\kk\src) & mkdir C:\kk\src'
export COPYFILE_DISABLE=1
git ls-files -z --cached --others --exclude-standard \
  | while IFS= read -r -d '' f; do [[ -e "$f" ]] && printf '%s\0' "$f"; done \
  | tar --null -T - -czf - \
  | "$vm" ssh 'tar -xzf - -C C:\kk\src'

run() {
  echo "==> $*" >&2
  "$vm" ssh "cd /d C:\\kk\\src && set CARGO_TARGET_DIR=C:\\kk\\target&& set CARGO_TERM_COLOR=always&& set CARGO_INCREMENTAL=0&& set RUST_BACKTRACE=1&& $*"
}

status=0
if [[ "$mode" == all || "$mode" == clippy ]]; then
  run cargo clippy --workspace --all-targets --all-features -- -D warnings || status=$?
fi
if [[ "$mode" == all || "$mode" == test ]]; then
  if [[ $# -gt 0 ]]; then
    run cargo test --all-features --no-fail-fast "$@" || status=$?
  else
    run cargo test --workspace --all-features --no-fail-fast || status=$?
  fi
fi

[[ "${KK_WIN_STOP:-0}" == 1 ]] && "$vm" stop
if ((status == 0)); then echo "windows-check: passed" >&2; else echo "windows-check: FAILED" >&2; fi
exit "$status"
exit; }
