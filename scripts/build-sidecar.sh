#!/usr/bin/env bash
# Builds the `kakemono` CLI and stages it as the Tauri sidecar of kakemono-ui (plan Task 47).
#
# Tauri's `bundle.externalBin` (crates/ui/src-tauri/tauri.bundle.conf.json) wants the binary at
# crates/ui/src-tauri/binaries/kakemono-<target-triple>[.exe]. The bundler then installs it as
# `kakemono` beside `kakemono-ui` (kakemono.app/Contents/MacOS, /usr/bin, the Windows install
# dir), which is where the UI's `find_cli` and the CLI's `kakemono pick` look for each other.
#
# Usage: scripts/build-sidecar.sh [--target TRIPLE] [--debug] [--cross]
#   --target TRIPLE  build for TRIPLE (default: the host triple, built without --target)
#   --debug          dev profile instead of release (pairs with `tauri build --debug`)
#   --cross          build with `cross` instead of `cargo` (CI only; Linux arm64)
#
# Prints the staged path on stdout. Honors CARGO_BUILD_JOBS and other cargo env vars.
set -euo pipefail
{ # Parsed whole before it runs, so editing this file mid-run is safe (scripts/check-scripts.sh).

target=""
profile="release"
builder="cargo"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --target)
      [[ $# -ge 2 ]] || { echo "build-sidecar: --target needs a triple" >&2; exit 2; }
      target="$2"
      shift 2
      ;;
    --debug)
      profile="debug"
      shift
      ;;
    --cross)
      builder="cross"
      shift
      ;;
    -h | --help)
      sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "build-sidecar: unknown argument: $1 (see --help)" >&2
      exit 2
      ;;
  esac
done

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

host="$(rustc -vV | sed -n 's/^host: //p')"
[[ -n "$host" ]] || { echo "build-sidecar: cannot read the host triple from rustc -vV" >&2; exit 1; }
triple="${target:-$host}"

args=(build --locked -p kakemono --bin kakemono)
[[ "$profile" == "release" ]] && args+=(--release)
[[ -n "$target" ]] && args+=(--target "$target")

echo "build-sidecar: $builder ${args[*]}" >&2
"$builder" "${args[@]}"

target_dir="${CARGO_TARGET_DIR:-$root/target}"
if [[ -n "$target" ]]; then
  out_dir="$target_dir/$target/$profile"
else
  out_dir="$target_dir/$profile"
fi

ext=""
[[ "$triple" == *windows* ]] && ext=".exe"

src="$out_dir/kakemono$ext"
[[ -f "$src" ]] || { echo "build-sidecar: $src was not built" >&2; exit 1; }

dest_dir="$root/crates/ui/src-tauri/binaries"
dest="$dest_dir/kakemono-$triple$ext"
mkdir -p "$dest_dir"
cp -f "$src" "$dest"
chmod +x "$dest"
echo "$dest"
exit; }
