#!/usr/bin/env bash
# Writes latest.json, the manifest the app's updater reads (crates/ui/src-tauri/src/updater.rs),
# from the named release assets in DIST. Used by the release workflow.
#
# Each updater platform key maps to an asset and its minisign signature (<asset>.sig). The
# plugin looks up "<os>-<arch>-<installer>" first, then "<os>-<arch>", so every bundle has an
# installer key and each OS/arch's default bundle also has the plain key. The script fails if
# an asset or a signature is missing: a manifest without a platform breaks every update on it.
#
# Usage: scripts/updater-manifest.sh DIST VERSION BASE_URL
#   DIST      folder holding kakemono-<os>-<arch>.<ext> and the .sig files
#   VERSION   the release version without the v, e.g. 0.3.0
#   BASE_URL  where the assets download from, e.g.
#             https://github.com/RielJ/kakemono/releases/download/v0.3.0
set -euo pipefail
{ # Parsed whole before it runs, so editing this file mid-run is safe (scripts/check-scripts.sh).

dist="${1:?usage: updater-manifest.sh DIST VERSION BASE_URL}"
version="${2:?usage: updater-manifest.sh DIST VERSION BASE_URL}"
base="${3:?usage: updater-manifest.sh DIST VERSION BASE_URL}"

# "<keys> <asset>": the asset serves every comma-separated updater key.
entries=(
  "darwin-aarch64,darwin-aarch64-app kakemono-macos-arm64.app.tar.gz"
  "darwin-x86_64,darwin-x86_64-app kakemono-macos-x64.app.tar.gz"
  "linux-x86_64,linux-x86_64-appimage kakemono-linux-x64.AppImage"
  "linux-x86_64-deb kakemono-linux-x64.deb"
  "linux-x86_64-rpm kakemono-linux-x64.rpm"
  "windows-x86_64,windows-x86_64-nsis kakemono-windows-x64-setup.exe"
  "windows-x86_64-msi kakemono-windows-x64.msi"
)

platforms='{}'
for entry in "${entries[@]}"; do
  keys="${entry%% *}"
  asset="${entry#* }"
  for f in "$dist/$asset" "$dist/$asset.sig"; do
    [[ -s "$f" ]] || { echo "updater-manifest: missing $f" >&2; exit 1; }
  done
  signature="$(<"$dist/$asset.sig")"
  IFS=, read -ra key_list <<<"$keys"
  for key in "${key_list[@]}"; do
    platforms="$(jq --arg key "$key" --arg url "$base/$asset" --arg sig "$signature" \
      '. + {($key): {url: $url, signature: $sig}}' <<<"$platforms")"
  done
done

jq -n --arg version "$version" --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg notes "kakemono $version. See ${base%/download/*}/tag/v$version for the changes." \
  --argjson platforms "$platforms" \
  '{version: $version, notes: $notes, pub_date: $date, platforms: $platforms}' >"$dist/latest.json"
echo "updater-manifest: wrote $dist/latest.json for $version ($(jq '.platforms | length' "$dist/latest.json") platform keys)"
exit; }
