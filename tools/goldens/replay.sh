#!/usr/bin/env bash
# tools/goldens/replay.sh: capture IPC wire goldens from the real Go daemon.
#
# Builds the Go daemon from the working tree into a scratch dir, starts it in
# the foreground on a scratch socket with a scratch HOME and config, sends
# every IPC command over the socket, and saves the raw request and response
# bytes to crates/protocol/tests/fixtures/wire/NN_<case>.{request,response}.jsonl.
#
# Safety:
#   - PATH starts with a shim dir: `osascript`, `defaults` and `open` are
#     no-ops that log their argv. The real wallpaper never changes and the
#     theme always reads as "light" (shim `defaults` exits 1, as macOS does in
#     Light Mode).
#   - HOME and XDG_CONFIG_HOME point into the scratch dir, so state.json, the
#     PID file and the socket never touch ~/.config/wallpaperd.
#   - The config enables ONLY [local], pointed at scratch images. Every network
#     provider is disabled, rotation is disabled, and HTTP(S)_PROXY points at a
#     closed port as a second guard.
#   - An EXIT trap stops the daemon and verifies no scratch daemon survives.
#
# The scratch dir has a fixed name (/tmp/wpd-goldens-replay) so the paths
# inside the fixtures are identical on every run. See wire/README.md for the
# fields that still vary between runs (set_at).
#
# Usage (from anywhere): bash tools/goldens/replay.sh
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$REPO/crates/protocol/tests/fixtures/wire"
ROOT="/tmp/wpd-goldens-replay"

BIN="$ROOT/bin"
SHIM="$ROOT/shim"
FAKE_HOME="$ROOT/home"
PICS="$ROOT/pics"        # the only [local] dir
SETDIR="$ROOT/set"       # images used with the `set` command
CFG="$ROOT/config.toml"
SOCK="$ROOT/run/daemon.sock"
REQ="$ROOT/req"
SHIM_LOG="$ROOT/shim.log"
DAEMON_LOG="$ROOT/daemon.log"
DAEMON_PID=""

log() { printf 'replay: %s\n' "$*" >&2; }
die() { log "FAIL: $*"; exit 1; }

cleanup() {
  local status=$?
  if [[ -n "$DAEMON_PID" ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
    kill -TERM "$DAEMON_PID" 2>/dev/null || true
    for _ in $(seq 1 50); do
      kill -0 "$DAEMON_PID" 2>/dev/null || break
      sleep 0.1
    done
    kill -KILL "$DAEMON_PID" 2>/dev/null || true
    wait "$DAEMON_PID" 2>/dev/null || true
  fi
  if pgrep -f "$BIN/wallpaperd" >/dev/null 2>&1; then
    log "FAIL: scratch daemon still running:"
    pgrep -fl "$BIN/wallpaperd" >&2 || true
    status=1
  fi
  if [[ $status -ne 0 ]]; then
    if [[ -f "$DAEMON_LOG" ]]; then
      log "daemon log:"
      sed 's/^/  /' "$DAEMON_LOG" >&2 || true
    fi
    log "scratch dir kept for inspection: $ROOT"
  else
    rm -rf "$ROOT"
  fi
  rm -rf "$OUT.tmp"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# ── Scratch tree ──────────────────────────────────────────────────────────────
if pgrep -f "$BIN/wallpaperd" >/dev/null 2>&1; then
  die "a scratch daemon from an earlier run is still running ($BIN/wallpaperd)"
fi
rm -rf "$ROOT"
mkdir -p "$BIN" "$SHIM" "$FAKE_HOME" "$PICS" "$SETDIR" "$ROOT/run" "$REQ" \
  "$ROOT/cache" "$ROOT/trash" "$ROOT/favorites"
: >"$SHIM_LOG"

# PATH shims. Each logs "<name> <argv...>" to $SHIM_LOG.
for tool in osascript open; do
  cat >"$SHIM/$tool" <<EOF
#!/bin/sh
printf '%s' "$tool" >>"$SHIM_LOG"
for a in "\$@"; do printf ' [%s]' "\$a" >>"$SHIM_LOG"; done
printf '\n' >>"$SHIM_LOG"
exit 0
EOF
done
# `defaults read -g AppleInterfaceStyle` exits non-zero in Light Mode.
cat >"$SHIM/defaults" <<EOF
#!/bin/sh
printf 'defaults' >>"$SHIM_LOG"
for a in "\$@"; do printf ' [%s]' "\$a" >>"$SHIM_LOG"; done
printf '\n' >>"$SHIM_LOG"
exit 1
EOF
chmod +x "$SHIM"/*

# Tiny images. The local provider only checks the extension; content is a
# 1x1 PNG so anything that does inspect it still sees a valid image.
png1x1() {
  printf '\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\rIDATx\x9cc\xf8\x0f\x00\x00\x01\x01\x00\x05\x18\xd8N\x00\x00\x00\x00IEND\xaeB`\x82' >"$1"
}
# Exactly one image in the [local] dir, so every provider fetch is
# deterministic despite the provider's shuffle.
png1x1 "$PICS/alpha.png"
png1x1 "$SETDIR/beta.png"

cat >"$CFG" <<EOF
[daemon]
rotation_enabled = false
interval = "30m"
download_dir = "$ROOT/cache"
cache_size = 50
trash_dir = "$ROOT/trash"
socket_path = "$SOCK"

[wallhaven]
enabled = false

[unsplash]
enabled = false

[reddit]
enabled = false

[bing]
enabled = false

[apod]
enabled = false

[local]
enabled = true
dirs = ["$PICS"]
recursive = false

[light]
sources = ["local"]
queries = []

[dark]
sources = ["local"]
queries = []

[favorites]
dir = "$ROOT/favorites"
EOF

# ── Build (never the committed /wallpaperd binary) ───────────────────────────
log "building Go daemon and wire client into $BIN"
(cd "$REPO" && go build -o "$BIN/wallpaperd" . && go build -o "$BIN/wireclient" ./tools/goldens/wireclient)

# ── Start daemon ──────────────────────────────────────────────────────────────
export PATH="$SHIM:$PATH"
export HOME="$FAKE_HOME"
export XDG_CONFIG_HOME="$FAKE_HOME/.config"
export HTTP_PROXY="http://127.0.0.1:9" HTTPS_PROXY="http://127.0.0.1:9"
export http_proxy="$HTTP_PROXY" https_proxy="$HTTPS_PROXY"
unset NO_PROXY no_proxy

[[ "$(command -v osascript)" == "$SHIM/osascript" ]] || die "osascript shim not first in PATH"
[[ "$(command -v defaults)" == "$SHIM/defaults" ]] || die "defaults shim not first in PATH"

log "starting daemon on $SOCK"
"$BIN/wallpaperd" daemon --foreground --config "$CFG" --socket "$SOCK" >"$DAEMON_LOG" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 100); do
  [[ -S "$SOCK" ]] && break
  kill -0 "$DAEMON_PID" 2>/dev/null || die "daemon exited during startup"
  sleep 0.05
done
[[ -S "$SOCK" ]] || die "socket $SOCK did not appear"

# ── Cases ─────────────────────────────────────────────────────────────────────
rm -rf "$OUT.tmp"
mkdir -p "$OUT.tmp"
N=0

# send NAME EXPECT_OK REQUEST_BYTES (printf-style escapes are NOT applied;
# pass the exact bytes, including the trailing newline).
send() {
  local name="$1" expect="$2" bytes="$3"
  N=$((N + 1))
  local stem
  stem="$(printf '%02d_%s' "$N" "$name")"
  local req="$OUT.tmp/$stem.request.jsonl" resp="$OUT.tmp/$stem.response.jsonl"
  printf '%s' "$bytes" >"$req"
  "$BIN/wireclient" -socket "$SOCK" -req "$req" -resp "$resp" || die "$stem: wire client failed"
  [[ -s "$resp" ]] || die "$stem: empty response"
  [[ "$(tail -c1 "$resp" | od -An -c | tr -d ' ')" == '\n' ]] || die "$stem: response does not end in \\n"
  local want_lines got_lines
  want_lines="$(printf '%s' "$bytes" | wc -l | tr -d ' ')"
  got_lines="$(wc -l <"$resp" | tr -d ' ')"
  [[ "$want_lines" == "$got_lines" ]] || die "$stem: $want_lines request lines but $got_lines response lines"
  if [[ "$expect" != "-" ]]; then
    grep -q "^{\"ok\":$expect" "$resp" || die "$stem: expected ok=$expect, got: $(cat "$resp")"
  fi
  log "$stem: $(head -c 160 "$resp" | tr -d '\n')"
}

NL=$'\n'
# Initial state: no current wallpaper, empty history.
send status_initial          true  "{\"cmd\":\"status\"}$NL"
send prev_at_start           false "{\"cmd\":\"prev\"}$NL"
send favorite_no_current     false "{\"cmd\":\"favorite\"}$NL"
send trash_no_current        false "{\"cmd\":\"trash\"}$NL"
# Provider fetch (local provider, single image).
send next_fetch              true  "{\"cmd\":\"next\"}$NL"
# set (WallpaperInfo with only path/source/title/set_at populated).
send set                     true  "{\"cmd\":\"set\",\"args\":{\"path\":\"$SETDIR/beta.png\"}}$NL"
send prev                    true  "{\"cmd\":\"prev\"}$NL"
# Forward history: next replays history[1] without a fetch.
send next_from_history       true  "{\"cmd\":\"next\"}$NL"
send favorite                true  "{\"cmd\":\"favorite\"}$NL"
# Trash moves beta.png to trash_dir, then fetches a new one (alpha.png).
send trash                   true  "{\"cmd\":\"trash\"}$NL"
send set_missing_path        false "{\"cmd\":\"set\"}$NL"
send set_path_not_string     false "{\"cmd\":\"set\",\"args\":{\"path\":42}}$NL"
send set_nonexistent         false "{\"cmd\":\"set\",\"args\":{\"path\":\"/nonexistent/wallpaperd-golden.png\"}}$NL"
send pause                   true  "{\"cmd\":\"pause\"}$NL"
send status_paused           true  "{\"cmd\":\"status\"}$NL"
send resume                  true  "{\"cmd\":\"resume\"}$NL"
send status_resumed          true  "{\"cmd\":\"status\"}$NL"
send invalid_json            false "{not json$NL"
send unknown_command         false "{\"cmd\":\"bogus\"}$NL"
send missing_cmd_field       false "{\"args\":{}}$NL"
send extra_fields            true  "{\"cmd\":\"status\",\"args\":null,\"extra\":1}$NL"
# Framing: several commands on one connection, including a blank line and
# whitespace; one response line per request line, in order.
send multi_command_session   -     "{\"cmd\":\"pause\"}$NL{\"cmd\":\"status\"}$NL$NL  {\"cmd\" : \"resume\"}  $NL{\"cmd\":\"bogus\"}$NL{\"cmd\":\"status\"}$NL"

# ── Post-checks ───────────────────────────────────────────────────────────────
set_calls="$(grep -c '^osascript ' "$SHIM_LOG" || true)"
# next_fetch, set, prev, next_from_history, trash(->next) = 5 wallpaper sets.
[[ "$set_calls" == "5" ]] || die "expected 5 osascript calls, shim saw $set_calls"
grep -q '^defaults \[read\] \[-g\] \[AppleInterfaceStyle\]' "$SHIM_LOG" || die "defaults shim was not used for theme detection"
[[ -f "$FAKE_HOME/.config/wallpaperd/state.json" ]] || die "state.json not written under the scratch HOME"
[[ -f "$ROOT/favorites/beta.png" ]] || die "favorite did not copy beta.png"
[[ -f "$ROOT/trash/beta.png" ]] || die "trash did not move beta.png"

# Stop the daemon now (the trap re-checks) so the run ends clean. SIGTERM
# makes it save state.json one last time.
kill -TERM "$DAEMON_PID"
wait "$DAEMON_PID" 2>/dev/null || true
DAEMON_PID=""
[[ ! -e "$SOCK" ]] || die "daemon left its socket behind"

# The state file the real daemon wrote (State.Save bytes, no trailing \n).
cp "$FAKE_HOME/.config/wallpaperd/state.json" "$OUT.tmp/state_after_replay.json"

# Replace the fixture set atomically; keep the hand-written README.
[[ -f "$OUT/README.md" ]] && cp "$OUT/README.md" "$OUT.tmp/README.md"
rm -rf "$OUT"
mv "$OUT.tmp" "$OUT"

log "osascript shim calls: $set_calls (real wallpaper untouched)"
log "wrote $N cases to ${OUT#"$REPO"/}"
