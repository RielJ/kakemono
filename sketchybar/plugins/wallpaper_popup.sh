#!/usr/bin/env bash
# wallpaper_popup.sh — handles popup button clicks for the wallpaper item.
# The NAME environment variable is set by sketchybar to the clicked item's name.

case "$NAME" in
    *next)
        wallpaperd next
        ;;
    *prev)
        wallpaperd prev
        ;;
    *favorite)
        wallpaperd favorite
        ;;
    *trash)
        wallpaperd trash
        ;;
    *pause)
        STATUS=$(wallpaperd status --json 2>/dev/null)
        IS_PAUSED=$(echo "$STATUS" | jq -r '.paused')
        if [ "$IS_PAUSED" = "true" ]; then
            wallpaperd resume
        else
            wallpaperd pause
        fi
        ;;
    *settings)
        # Open the settings GUI — runs detached so the popup can close cleanly.
        wallpaperd settings &
        ;;
esac

# Close the popup after any action.
sketchybar --set wallpaper popup.drawing=off
