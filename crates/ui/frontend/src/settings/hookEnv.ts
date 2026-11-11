/** Variables the daemon sets for `[hooks] on_change` (engine `hooks.rs`). */
export const HOOK_ENV: readonly { name: string; text: string }[] = [
  { name: 'KAKEMONO_PATH', text: 'Absolute path of the new wallpaper' },
  { name: 'KAKEMONO_SOURCE', text: 'Source that supplied it, e.g. bing or local' },
  { name: 'KAKEMONO_TITLE', text: 'Title, when the source has one' },
  { name: 'KAKEMONO_AUTHOR', text: 'Author or photographer, when known' },
  { name: 'KAKEMONO_SOURCE_URL', text: 'Page the image came from' },
  { name: 'KAKEMONO_THEME', text: 'light or dark' },
]

/** Copied from the daemon's environment; everything else is cleared. */
export const PASSED_THROUGH = [
  'PATH',
  'HOME',
  'DISPLAY',
  'WAYLAND_DISPLAY',
  'XDG_RUNTIME_DIR',
  'DBUS_SESSION_BUS_ADDRESS',
]
