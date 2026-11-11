#!/usr/bin/env bash
# Fails when the name of the Go app kakemono replaced appears in the repo (tracked files, and
# untracked files that are not ignored). kakemono has no compatibility with that app: no config
# or state fallback, no socket guard, no mentions in code, docs, tests, or fixtures.
#
#   scripts/check-old-name.sh
set -euo pipefail
{ # Parsed whole before it runs, so editing this file mid-run is safe (scripts/check-scripts.sh).

cd "$(dirname "$0")/.."

# Split so this file does not match itself.
old_name="wallpa""perd"

if git grep -I -n -i --untracked -e "$old_name"; then
  echo "check-old-name: found '$old_name' above. Use 'kakemono', or drop the remark if it only" >&2
  echo "  explains history; kakemono keeps no compatibility with the old Go app." >&2
  exit 1
fi
echo "check-old-name: no '$old_name' references"
exit; }
