#!/usr/bin/env bash
# Rust gate for kakemono. Two modes:
#
#   scripts/gate.sh quick <crate>...   while working: check + tests of the named crates only
#   scripts/gate.sh full [<crate>...]  once, before commit: fmt, clippy, all tests, doctests;
#                                      the named crates are also clippy-checked for the Linux
#                                      and Windows targets (only crates with cfg'd OS code)
#
# Cross clippy and C dependencies: crates that pull in reqwest build aws-lc-sys, which needs a C
# compiler for the target. For Linux the gate uses `x86_64-linux-gnu-gcc` if installed, else zig
# (scripts/cross/zig-cc-linux). For Windows it uses `cargo xwin` if installed, else zig
# (scripts/cross/zig-cc-windows). Without either, the step is skipped and the gate says so
# (CI's native Linux and Windows clippy jobs still cover it).
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
cross_flags=() # pkg_flags minus the ui crate: GTK's -sys crates need a target pkg-config
skipped=()
for c in "$@"; do
  case "$c" in
    kakemono-* | kakemono) p="$c" ;;
    cli) p=kakemono ;;
    *) p="kakemono-$c" ;;
  esac
  pkg_flags+=(-p "$p")
  if [[ "$p" == kakemono-ui ]]; then
    skipped+=("kakemono-ui: GTK needs a target pkg-config/sysroot; covered by CI's native jobs")
  else
    cross_flags+=(-p "$p")
  fi
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

# True when the named crates build C code (aws-lc-sys, ring, or anything with a `cc` build dep)
# for this target.
needs_c() {
  cargo tree "${cross_flags[@]}" --target "$1" -e normal,build,dev --prefix none 2>/dev/null |
    grep -qE '^(aws-lc-sys|ring|cc) '
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
    if ((${#cross_flags[@]} > 0)); then
      for target in x86_64-unknown-linux-gnu x86_64-pc-windows-msvc; do
        clippy=(cargo clippy)
        if needs_c "$target"; then
          case "$target" in
            *linux*)
              if ! command -v x86_64-linux-gnu-gcc >/dev/null; then
                if ! command -v zig >/dev/null; then
                  skipped+=("$target: needs x86_64-linux-gnu-gcc or zig (brew install zig)")
                  continue
                fi
                export CC_x86_64_unknown_linux_gnu="$PWD/scripts/cross/zig-cc-linux"
                export AR_x86_64_unknown_linux_gnu="$PWD/scripts/cross/zig-ar"
              fi
              ;;
            *windows*)
              if command -v cargo-xwin >/dev/null; then
                clippy=(cargo xwin clippy)
              elif command -v zig >/dev/null; then
                export CC_x86_64_pc_windows_msvc="$PWD/scripts/cross/zig-cc-windows"
                export AR_x86_64_pc_windows_msvc="$PWD/scripts/cross/zig-lib"
              else
                skipped+=("$target: needs cargo-xwin or zig (brew install zig); covered by CI")
                continue
              fi
              ;;
          esac
        fi
        step "${clippy[@]}" "${cross_flags[@]}" --all-targets --all-features --target "$target" -- -D warnings
      done
    fi
    for s in "${skipped[@]+"${skipped[@]}"}"; do
      printf '\n==> skipped cross clippy for %s\n' "$s"
    done
    printf '\n==> gate passed\n'
    ;;
  *)
    sed -n '2,19p' "$0" >&2
    exit 2
    ;;
esac
