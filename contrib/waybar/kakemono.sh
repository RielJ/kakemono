#!/usr/bin/env bash
# kakemono.sh: waybar custom module that shows the current wallpaper.
# Prints one line of waybar JSON ({"text","tooltip","class"}).
#
# Requires: kakemono, jq. Set KAKEMONO to the binary's full path if waybar's PATH
# doesn't include it.

KAKEMONO="${KAKEMONO:-kakemono}"

if ! STATUS=$("$KAKEMONO" status --json 2>/dev/null) || [ -z "$STATUS" ]; then
    jq -cn '{text: "🖼 off", tooltip: "kakemono daemon is not running", class: "stopped"}'
    exit 0
fi

echo "$STATUS" | jq -c '
  (.current // {}) as $c
  | {
      text: ("🖼 " + ((($c.title // "") | if . == "" then ($c.source // "none") else .[0:30] end))
             + (if .paused then " ⏸" else "" end)),
      tooltip: ([
          ($c.title // empty),
          (if ($c.author // "") != "" then "by " + $c.author else empty end),
          ("source: " + ($c.source // "none")),
          ($c.path // empty),
          ("theme: " + .theme + (if .paused then ", paused" else "" end))
        ] | join("\n")),
      class: ([.theme, (if .paused then "paused" else "running" end)])
    }'
