#!/usr/bin/env bash
# Checks that the .deb in a Tauri bundle dir installs both binaries in /usr/bin: the app
# (kakemono-ui) and its sidecar CLI (kakemono). Used by the release workflow and
# scripts/linux-check.sh.
#
# Usage: scripts/check-deb.sh BUNDLE_DIR   (the dir holding deb/, e.g. target/release/bundle)
set -euo pipefail
{ # Parsed whole before it runs, so editing this file mid-run is safe (scripts/check-scripts.sh).

bundle="${1:?usage: check-deb.sh BUNDLE_DIR}"
shopt -s nullglob
debs=("$bundle"/deb/*.deb)
[[ ${#debs[@]} -eq 1 ]] || { echo "check-deb: want one .deb in $bundle/deb, found ${#debs[@]}" >&2; exit 1; }
deb="${debs[0]}"

# Entries may or may not start with ./, depending on the tool that built the package.
files="$(dpkg-deb -c "$deb" | awk '{print $NF}' | sed 's|^\./||')"
for bin in usr/bin/kakemono usr/bin/kakemono-ui; do
  grep -qx "$bin" <<<"$files" || {
    echo "check-deb: $bin is not in $deb. It holds:" >&2
    echo "$files" >&2
    exit 1
  }
done
echo "check-deb: $deb holds usr/bin/kakemono and usr/bin/kakemono-ui"
exit; }
