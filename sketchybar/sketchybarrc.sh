#!/usr/bin/env bash
# sketchybarrc.sh — drop-in snippet to add the wallpaper item to sketchybar.
# Source this file from your main sketchybarrc, or copy the relevant lines.
#
# Requirements:
#   - wallpaperd daemon must be running
#   - jq must be installed
#   - Place wallpaper.sh and wallpaper_popup.sh in $PLUGIN_DIR

PLUGIN_DIR="$HOME/.config/sketchybar/plugins"

# Register the custom event fired by wallpaperd when the wallpaper changes.
sketchybar --add event wallpaper_changed

# Main wallpaper item — right side of the bar.
sketchybar --add item wallpaper right \
    --set wallpaper \
        update_freq=0 \
        icon="🖼" \
        label="..." \
        script="$PLUGIN_DIR/wallpaper.sh" \
        click_script="sketchybar --set \$NAME popup.drawing=toggle" \
    --subscribe wallpaper wallpaper_changed mouse.entered mouse.exited mouse.exited.global

# Popup buttons — shown when the wallpaper item is clicked.
sketchybar --add item wallpaper.next popup.wallpaper \
    --set wallpaper.next \
        icon="⏭" \
        label="Next" \
        click_script="$PLUGIN_DIR/wallpaper_popup.sh"

sketchybar --add item wallpaper.prev popup.wallpaper \
    --set wallpaper.prev \
        icon="⏮" \
        label="Previous" \
        click_script="$PLUGIN_DIR/wallpaper_popup.sh"

sketchybar --add item wallpaper.favorite popup.wallpaper \
    --set wallpaper.favorite \
        icon="⭐" \
        label="Favorite" \
        click_script="$PLUGIN_DIR/wallpaper_popup.sh"

sketchybar --add item wallpaper.trash popup.wallpaper \
    --set wallpaper.trash \
        icon="🗑" \
        label="Trash" \
        click_script="$PLUGIN_DIR/wallpaper_popup.sh"

sketchybar --add item wallpaper.pause popup.wallpaper \
    --set wallpaper.pause \
        icon="⏸" \
        label="Pause/Resume" \
        click_script="$PLUGIN_DIR/wallpaper_popup.sh"

sketchybar --add item wallpaper.settings popup.wallpaper \
    --set wallpaper.settings \
        icon="⚙️" \
        label="Settings" \
        click_script="$PLUGIN_DIR/wallpaper_popup.sh"

# Run once immediately to populate the label.
sketchybar --trigger wallpaper_changed
