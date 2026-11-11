#!/usr/bin/env bash
# Checks every shell script under scripts/ (or the files given) for the run-time edit guard.
#
# Bash reads a script a little at a time while it runs. Editing a long-running script (a VM
# install, a Docker build) shifts the bytes bash has not read yet, so it runs a fragment of
# some other line. That once printed vm.sh's "removed <vm folder>" at the end of an install.
# Wrapping the body in `{ ...; exit; }` makes bash parse the whole body before running any of it.
#
#   scripts/check-scripts.sh [file...]
set -euo pipefail
{ # Parsed whole before it runs, so editing this file mid-run is safe (scripts/check-scripts.sh).

cd "$(dirname "$0")/.."

files=("$@")
if ((${#files[@]} == 0)); then
  while IFS= read -r -d '' f; do files+=("$f"); done < <(git ls-files -z --cached --others --exclude-standard 'scripts/*.sh' 'scripts/**/*.sh')
fi

bad=0
for f in "${files[@]}"; do
  if ! bash -n "$f"; then
    bad=1
    continue
  fi
  # The guard: a line opening `{` after `set -e...`, and `exit; }` as the last line.
  if ! grep -qE '^\{ ' "$f" || [[ "$(tail -n 1 "$f")" != 'exit; }' ]]; then
    echo "check-scripts: $f is not wrapped in '{ ...; exit; }'." >&2
    echo "  Add this line after 'set -euo pipefail':" >&2
    echo "    { # Parsed whole before it runs, so editing this file mid-run is safe (scripts/check-scripts.sh)." >&2
    echo "  and make 'exit; }' the last line." >&2
    bad=1
  fi
done

((bad == 0)) && echo "check-scripts: ${#files[@]} scripts ok"
exit "$bad"
exit; }
