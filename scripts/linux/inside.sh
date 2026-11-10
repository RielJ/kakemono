#!/usr/bin/env bash
# Runs inside the container scripts/linux-check.sh starts, from a copy of the source in /src.
# Steps mirror .github/workflows/ci.yml (rust, frontend) and release.yml (deb).
set -euo pipefail
cd /src

step() { printf '\n==> %s\n' "$*"; }

npm_ready=0
npm_ci() {
  [[ $npm_ready -eq 1 ]] && return
  step "npm ci"
  (cd crates/ui/frontend && npm ci --no-audit --no-fund)
  npm_ready=1
}

for mode in "$@"; do
  case "$mode" in
    rust)
      step "cargo fmt --all --check"
      cargo fmt --all --check
      step "cargo clippy --workspace --all-targets --all-features -- -D warnings"
      cargo clippy --workspace --all-targets --all-features -- -D warnings
      step "cargo test --workspace --all-features --no-fail-fast"
      cargo test --workspace --all-features --no-fail-fast
      ;;
    frontend)
      npm_ci
      cd crates/ui/frontend
      step "npm run typecheck"; npm run typecheck
      step "npm run build"; npm run build
      step "npm test"; npm test
      cd /src
      ;;
    deb)
      npm_ci
      step "build the sidecar"
      bash scripts/build-sidecar.sh
      step "tauri build --bundles deb"
      (cd crates/ui/frontend && TAURI_APP_PATH=../src-tauri npm run tauri -- build --ci \
        --bundles deb --config ../src-tauri/tauri.bundle.conf.json)
      step "check the .deb"
      bash scripts/check-deb.sh "$CARGO_TARGET_DIR/release/bundle"
      ;;
    *)
      echo "inside.sh: unknown step: $mode" >&2
      exit 2
      ;;
  esac
done
step "linux check passed: $*"
