#!/usr/bin/env bash
# Rust gate for kakemono. Two modes:
#
#   scripts/gate.sh quick <crate>...   while working: check + tests of the named crates only
#   scripts/gate.sh full [<crate>...]  once, before commit: fmt, clippy, all tests, doctests;
#                                      the named crates are also clippy-checked for the Linux
#                                      and Windows targets (only crates with cfg'd OS code)
#
# Crate names are short (`config`, `engine`, `ui`) or full (`kakemono-config`).
#
# Environment:
#   KK_LANES=N   number of lanes building at once; caps this run at ncpu/N jobs (min 2) and
#                runs at lower priority, so parallel lanes don't starve each other or the desktop.
#
# Tests run under cargo-nextest (.config/nextest.toml kills a test after 2 min, so a deadlock
# fails instead of hanging). Doctests run with `cargo test --doc` (nextest does not run them).
set -euo pipefail

cd "$(dirname "$0")/.."

mode="${1:-}"
shift || true

pkg_flags=()
for c in "$@"; do
  case "$c" in
    kakemono-* | kakemono) pkg_flags+=(-p "$c") ;;
    cli) pkg_flags+=(-p kakemono) ;;
    *) pkg_flags+=(-p "kakemono-$c") ;;
  esac
done

run=()
if [[ -n "${KK_LANES:-}" ]]; then
  ncpu="$(getconf _NPROCESSORS_ONLN)"
  jobs=$((ncpu / KK_LANES))
  ((jobs < 2)) && jobs=2
  export CARGO_BUILD_JOBS="$jobs"
  export NEXTEST_TEST_THREADS="$jobs"
  run=(nice -n 10)
fi

step() {
  printf '\n==> %s\n' "$*"
  "${run[@]}" "$@"
}

if ! command -v cargo-nextest >/dev/null; then
  echo "cargo-nextest not found: brew install cargo-nextest (or cargo install cargo-nextest --locked)" >&2
  exit 2
fi

case "$mode" in
  quick)
    if ((${#pkg_flags[@]} == 0)); then
      echo "usage: scripts/gate.sh quick <crate>..." >&2
      exit 2
    fi
    step cargo check "${pkg_flags[@]}" --all-targets
    step cargo nextest run "${pkg_flags[@]}"
    ;;
  full)
    step cargo fmt --all -- --check
    step cargo clippy --workspace --all-targets --all-features -- -D warnings
    step cargo nextest run --workspace --all-features
    step cargo test --workspace --all-features --doc
    if ((${#pkg_flags[@]} > 0)); then
      for target in x86_64-unknown-linux-gnu x86_64-pc-windows-msvc; do
        step cargo clippy "${pkg_flags[@]}" --all-targets --all-features --target "$target" -- -D warnings
      done
    fi
    printf '\n==> gate passed\n'
    ;;
  *)
    sed -n '2,19p' "$0" >&2
    exit 2
    ;;
esac
