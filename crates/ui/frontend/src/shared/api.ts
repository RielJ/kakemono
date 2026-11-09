/**
 * Frontend API surface. Every call goes to the Tauri backend (`crates/ui/src-tauri`) with
 * `invoke`, or to a Tauri plugin (dialog, opener). Command names and argument shapes match
 * `src-tauri/src/commands.rs`, `picker.rs` and `lifecycle.rs`; see `.pi/contracts/ui.md`.
 *
 * Config and status types are generated from Rust by ts-rs (`./bindings`); do not redeclare
 * them here.
 */

import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { openPath } from '@tauri-apps/plugin-opener'

import type { Config } from './bindings/Config'
import type { StatusData } from './bindings/StatusData'

export type { Config, StatusData }

/** One picker entry, in the Go picker's shape (the `thumbnails` command returns these). */
export interface Thumbnail {
  path: string
  source: string
  title: string
  isCurrent: boolean
  /** `thumb://` URL for an admitted path; `''` when the path is outside the download, favorites and local dirs (show a placeholder). */
  imageUrl: string
}

/**
 * A partial config for `save_config`: only the keys that changed. Objects merge into their
 * TOML tables, `null` removes a key, anything else replaces the value (comments survive).
 */
export type ConfigPatch = Patch<Config>

type Patch<T> = T extends readonly unknown[]
  ? T
  : T extends object
    ? { [K in keyof T]?: Patch<T[K]> | null }
    : T

/** What `save_config` did after writing the file. */
export interface SaveOutcome {
  /** The daemon answered IPC `reload`. */
  reloaded: boolean
  /** Why the reload failed; `null` when it worked or the daemon is not running. */
  reload_error: string | null
}

/** Static facts from `app_info`. */
export interface AppInfo {
  /** Rust `std::env::consts::OS`: `macos`, `windows`, `linux`, … */
  os: string
  /** The config file being read and edited. */
  config_path: string
  /** True when that file is the Go `wallpaperd` config. */
  legacy_config: boolean
  version: string
}

/**
 * How a backend command fails: `CommandError` in `commands.rs`, plus the `LifecycleError`
 * kinds from `lifecycle.rs` (`cli_*`, `autostart`). For `cli_failed` the message carries the
 * CLI's stderr.
 */
export interface CommandError {
  kind:
    | 'daemon_not_running'
    | 'daemon'
    | 'config'
    | 'invalid'
    | 'io'
    | 'opener'
    | 'internal'
    | 'cli_not_found'
    | 'cli_failed'
    | 'cli_output'
    | 'autostart'
  message: string
}

function isCommandError(err: unknown): err is CommandError {
  return (
    typeof err === 'object' &&
    err !== null &&
    'kind' in err &&
    typeof err.kind === 'string' &&
    'message' in err &&
    typeof err.message === 'string'
  )
}

/** True when a command failed because nothing listens on the daemon socket. */
export function isDaemonNotRunning(err: unknown): boolean {
  return isCommandError(err) && err.kind === 'daemon_not_running'
}

/** Human-readable text for any rejection from this module. */
export function errorMessage(err: unknown): string {
  if (isCommandError(err)) return err.message
  if (err instanceof Error) return err.message
  return String(err)
}

// ── Config ──────────────────────────────────────────────────────────────────

export function getConfig(): Promise<Config> {
  return invoke<Config>('get_config')
}

/** Writes the changed keys to `config.toml` and asks the daemon to reload it. */
export function saveConfig(patch: ConfigPatch): Promise<SaveOutcome> {
  return invoke<SaveOutcome>('save_config', { patch })
}

export function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>('app_info')
}

// ── Daemon ──────────────────────────────────────────────────────────────────

/** Rejects with kind `daemon_not_running` when the daemon is down. */
export function getStatus(): Promise<StatusData> {
  return invoke<StatusData>('status')
}

export function nextWallpaper(): Promise<void> {
  return invoke<void>('next')
}

export function prevWallpaper(): Promise<void> {
  return invoke<void>('prev')
}

/** Copies `path` to the favourites dir, or the current wallpaper when `path` is omitted. */
export function favoriteWallpaper(path?: string): Promise<void> {
  return invoke<void>('favorite', { path: path ?? null })
}

/**
 * Trashes the current wallpaper. With `expected`, the daemon trashes only if that path is
 * still current and rejects otherwise (atomic; Task 45b).
 */
export function trashWallpaper(expected?: string): Promise<void> {
  return invoke<void>('trash', { expected: expected ?? null })
}

export function pauseWallpaper(): Promise<void> {
  return invoke<void>('pause')
}

export function resumeWallpaper(): Promise<void> {
  return invoke<void>('resume')
}

/** Sets a specific image as the wallpaper (IPC `set`). */
export function setFile(path: string): Promise<void> {
  return invoke<void>('set_file', { path })
}

/** History and cache entries for the picker strip. */
export function listThumbnails(): Promise<Thumbnail[]> {
  return invoke<Thumbnail[]>('thumbnails')
}

/** The daemon's diagnostics report (IPC `doctor` data). */
export function doctor(): Promise<unknown> {
  return invoke<unknown>('doctor')
}

export function daemonRunning(): Promise<boolean> {
  return invoke<boolean>('daemon_running')
}

/**
 * Starts the daemon unless it answers (`kakemono daemon`, which refuses a second one).
 * Resolves to `true` once it answers; rejects with the CLI's stderr when it cannot start.
 */
export function startDaemon(): Promise<boolean> {
  return invoke<boolean>('ensure_daemon')
}

// ── Start at login ──────────────────────────────────────────────────────────

/** `kakemono service status --json`. */
export interface ServiceStatus {
  installed: boolean
  running: boolean
  /** Where the unit or plist lives (set even when not installed); `null` where there is no file (Windows). */
  path: string | null
}

/** Both halves of "Start at login" (`start_at_login` in `lifecycle.rs`). */
export interface LoginItems {
  /** On only when the daemon service is installed and the UI starts at login. */
  enabled: boolean
  service: ServiceStatus
  ui_autostart: boolean
}

export function getStartAtLogin(): Promise<LoginItems> {
  return invoke<LoginItems>('start_at_login')
}

/**
 * Installs or removes the daemon's login service and the UI's autostart entry together. A
 * change that fails halfway is undone; the rejection carries the reason. Resolves to the state
 * read back afterwards.
 */
export function setStartAtLogin(enabled: boolean): Promise<LoginItems> {
  return invoke<LoginItems>('set_start_at_login', { enabled })
}

// ── Native dialogs, the file manager and the browser ────────────────────────

/** Directory picker. Resolves to `''` when the user cancels, as the Wails version did. */
export async function selectDirectory(): Promise<string> {
  const picked = await openDialog({ directory: true, multiple: false })
  return picked ?? ''
}

/** Image file picker. Resolves to `''` when the user cancels. */
export async function selectFile(): Promise<string> {
  const picked = await openDialog({
    directory: false,
    multiple: false,
    filters: [{ name: 'Images', extensions: ['jpg', 'jpeg', 'png', 'webp', 'heic'] }],
  })
  return picked ?? ''
}

/** Opens a folder in the file manager (opener plugin, scoped to `$HOME/**`). */
export function openFolder(path: string): Promise<void> {
  return openPath(path)
}

export async function openFavoritesFolder(): Promise<void> {
  const cfg = await getConfig()
  return openPath(cfg.favorites.dir)
}

/** Shows the file in Finder, Explorer or the file manager (`reveal` command). */
export function revealInFinder(path: string): Promise<void> {
  return invoke<void>('reveal', { path })
}

/** Opens an http(s) URL in the default browser; other schemes are refused. */
export function openUrl(url: string): Promise<void> {
  return invoke<void>('open_url', { url })
}

// ── Windows ─────────────────────────────────────────────────────────────────

/** Hides the calling window. The tray app keeps running. */
export function closeWindow(): Promise<void> {
  return getCurrentWindow().hide()
}

/** Shows the picker at its saved or edge position. */
export function showPicker(): Promise<void> {
  return invoke<void>('show_picker')
}

/** Hides the picker, saving a dragged position first. */
export function hidePicker(): Promise<void> {
  return invoke<void>('hide_picker')
}

/** Forgets a dragged picker position (`[picker] placement = "edge"`). */
export function resetPickerPosition(): Promise<void> {
  return invoke<void>('reset_picker_position')
}
