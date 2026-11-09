#!/usr/bin/env bash
# wallpaper.sh: sketchybar plugin that shows the current wallpaper.
# Subscribes to: wallpaper_changed (fired by the kakemono [hooks] on_change hook).
#
# Requires: kakemono, jq. Set KAKEMONO to the binary's full path if sketchybar's PATH
# doesn't include it.

KAKEMONO="${KAKEMONO:-kakemono}"

if ! STATUS=$("$KAKEMONO" status --json 2>/dev/null) || [ -z "$STATUS" ]; then
    sketchybar --set "$NAME" label="No daemon" icon="🖼"
    exit 0
fi

SOURCE=$(echo "$STATUS" | jq -r '.current.source // "?"')
TITLE=$(echo "$STATUS" | jq -r '.current.title // ""')
PAUSED=$(echo "$STATUS" | jq -r 'if .paused then " ⏸" else "" end')
THEME=$(echo "$STATUS" | jq -r 'if .theme == "dark" then "🌙" else "☀️" end')

# Show the title when there is one, else the source.
if [ -n "$TITLE" ]; then
    LABEL="${THEME} ${TITLE:0:25}${PAUSED}"
else
    LABEL="${THEME} ${SOURCE}${PAUSED}"
fi

sketchybar --set "$NAME" label="$LABEL" icon="🖼"
