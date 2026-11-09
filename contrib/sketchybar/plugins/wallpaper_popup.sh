#!/usr/bin/env bash
# wallpaper_popup.sh: handles clicks on the wallpaper popup buttons.
# sketchybar sets NAME to the name of the clicked item.
#
# Requires: kakemono, jq. Set KAKEMONO to the binary's full path if sketchybar's PATH
# doesn't include it.

KAKEMONO="${KAKEMONO:-kakemono}"

case "$NAME" in
    *next)
        "$KAKEMONO" next
        ;;
    *prev)
        "$KAKEMONO" prev
        ;;
    *favorite)
        "$KAKEMONO" favorite
        ;;
    *trash)
        "$KAKEMONO" trash
        ;;
    *pause)
        IS_PAUSED=$("$KAKEMONO" status --json 2>/dev/null | jq -r '.paused')
        if [ "$IS_PAUSED" = "true" ]; then
            "$KAKEMONO" resume
        else
            "$KAKEMONO" pause
        fi
        # Pause and resume don't change the wallpaper, so the hook doesn't fire. Refresh the label.
        sketchybar --trigger wallpaper_changed
        ;;
    *settings)
        # Starts the kakemono app (or focuses it) and returns.
        "$KAKEMONO" settings
        ;;
esac

# Close the popup after any action.
sketchybar --set wallpaper popup.drawing=off
