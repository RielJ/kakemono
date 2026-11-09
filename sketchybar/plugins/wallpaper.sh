#!/usr/bin/env bash
# wallpaper.sh — sketchybar plugin that shows current wallpaper status.
# Subscribes to: wallpaper_changed
#
# Requires: wallpaperd, jq

STATUS=$(wallpaperd status --json 2>/dev/null)
if [ $? -ne 0 ] || [ -z "$STATUS" ]; then
    sketchybar --set "$NAME" label="No daemon" icon="🖼"
    exit 0
fi

SOURCE=$(echo "$STATUS" | jq -r '.current.source // "?"')
TITLE=$(echo "$STATUS" | jq -r '.current.title // ""')
PAUSED=$(echo "$STATUS" | jq -r 'if .paused then "⏸" else "" end')
THEME=$(echo "$STATUS" | jq -r 'if .theme == "dark" then "🌙" else "☀️" end')

# Build label: show title if available, else source
if [ -n "$TITLE" ]; then
    LABEL="${THEME} ${TITLE:0:25}${PAUSED}"
else
    LABEL="${THEME} ${SOURCE}${PAUSED}"
fi

sketchybar --set "$NAME" label="$LABEL" icon="🖼"
