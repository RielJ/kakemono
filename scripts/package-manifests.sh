#!/usr/bin/env bash
# Renders the package manager manifests in packaging/ for one release.
#
#   scripts/package-manifests.sh <version> <SHA256SUMS> <out dir> [template...]
#
# Each template is a path under packaging/ (default: all of them). In a template, @VERSION@
# becomes the version, @SHA:<asset>@ the asset's checksum from SHA256SUMS, and @SHA:LICENSE@
# the checksum of ./LICENSE (the AUR package downloads it from the tag). The rendered files
# keep their paths under <out dir>, e.g. <out dir>/homebrew/Casks/kakemono.rb, less a `.in`
# suffix: a template that is not valid until rendered, like YAML, ends in `.in`.
#
# Fails, writing nothing for that template, when an asset is missing from SHA256SUMS or a
# placeholder is left over.
set -euo pipefail
{ # Parsed whole before it runs, so editing this file mid-run is safe (scripts/check-scripts.sh).

usage() {
  sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
}
(($# >= 3)) || usage
version=$1 sums=$2 out=$3
shift 3

root=$(cd "$(dirname "$0")/.." && pwd)
packaging="$root/packaging"
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] || {
  echo "not a version: $version" >&2
  exit 2
}
[[ -r $sums ]] || {
  echo "no SHA256SUMS at $sums" >&2
  exit 2
}

if (($# == 0)); then
  # No mapfile: macOS ships bash 3.2.
  templates=()
  while IFS= read -r t; do templates+=("$t"); done < <(
    cd "$packaging" && find . -type f ! -name '.*' | sed 's|^\./||' | sort
  )
else
  templates=("$@")
fi

license_sha=$(shasum -a 256 "$root/LICENSE" | cut -d' ' -f1)

# The checksum of asset $1 in SHA256SUMS (`sha256sum` format; a leading `*` marks binary mode).
sha_of() {
  awk -v want="$1" '{ name = $2; sub(/^\*/, "", name); if (name == want) { print $1; exit } }' "$sums"
}

failed=0
for t in "${templates[@]}"; do
  src="$packaging/$t"
  [[ -f $src ]] || {
    echo "$t: no such template" >&2
    failed=1
    continue
  }
  text=$(<"$src")
  text=${text//@VERSION@/$version}
  text=${text//@SHA:LICENSE@/$license_sha}
  missing=()
  while read -r token; do
    [[ -n $token ]] || continue
    asset=${token#@SHA:}
    asset=${asset%@}
    sha=$(sha_of "$asset")
    if [[ -z $sha ]]; then
      missing+=("$asset")
      continue
    fi
    text=${text//"$token"/$sha}
  done < <(grep -o '@SHA:[^@]*@' <<<"$text" | sort -u)
  if ((${#missing[@]})); then
    echo "$t: not in $sums: ${missing[*]}" >&2
    failed=1
    continue
  fi
  if grep -q '@[A-Z]*[:@]' <<<"$text"; then
    echo "$t: placeholders left: $(grep -o '@[A-Z]*[:@][^@ ]*@\{0,1\}' <<<"$text" | sort -u | tr '\n' ' ')" >&2
    failed=1
    continue
  fi
  dest="$out/${t%.in}"
  mkdir -p "$(dirname "$dest")"
  printf '%s\n' "$text" >"$dest"
  echo "$dest"
done
exit "$failed"
exit; }
