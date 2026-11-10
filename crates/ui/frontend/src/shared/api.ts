/**
 * Frontend API surface. Every call goes to the Tauri backend (`crates/ui/src-tauri`) with
 * `invoke`, or to a Tauri plugin (dialog, opener). Command names and argument shapes match
 * `src-tauri/src/commands.rs`, `commands_desktop.rs`, `commands_sources.rs`, `picker.rs` and
 * `lifecycle.rs`; see
 * `.pi/contracts/ui.md`.
 *
 * Config and status types are generated from Rust by ts-rs (`./bindings`); do not redeclare
 * them here.
 */

import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { openPath } from '@tauri-apps/plugin-opener'

import type { CheckSourceData } from './bindings/CheckSourceData'
import type { Config } from './bindings/Config'
import type { DownloadNowData } from './bindings/DownloadNowData'
import type { SourceType } from './bindings/SourceType'
import type { StatusData } from './bindings/StatusData'
import type { WallpaperInfo } from './bindings/WallpaperInfo'

export type { CheckSourceData, Config, DownloadNowData, SourceType, StatusData, WallpaperInfo }

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
 * kinds from `lifecycle.rs` (`cli_*`, `autostart`), plus `daemon_unreachable` from
 * `commands_sources.rs`. For `cli_failed` the message carries the CLI's stderr.
 *
 * The source commands (`checkSource`, `downloadNow`, `fetchDroppedUrl`, `importDroppedFile`)
 * reject with `{ kind: 'daemon_unreachable', message }` when the daemon isn't running and
 * can't be started, or the connection fails or times out (test with `isDaemonUnreachable`).
 * When the daemon answers but refuses (for example a source with no images), the kind is
 * `daemon` and the message is the daemon's text. Bad input is `invalid`.
 */
export interface CommandError {
  kind:
    | 'daemon_not_running'
    | 'daemon_unreachable'
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

/**
 * True when a source command couldn't reach the daemon at all (kind `daemon_unreachable`), as
 * opposed to the daemon answering with an error. "Add anyway" is offered only for this.
 */
export function isDaemonUnreachable(err: unknown): boolean {
  return isCommandError(err) && err.kind === 'daemon_unreachable'
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

/**
 * Picker strip entries: history (newest first), then images from the folders of enabled
 * sources (online downloads, then `manual`, then local sources), then favorites.
 */
export function listThumbnails(): Promise<Thumbnail[]> {
  return invoke<Thumbnail[]>('thumbnails')
}

/** The daemon's diagnostics report (IPC `doctor` data). */
export function doctor(): Promise<unknown> {
  return invoke<unknown>('doctor')
}

// ── Sources and dropped images (`commands_sources.rs`) ───────────────────────

/**
 * Asks the daemon how many images a source row would give (IPC `check_source`), starting the
 * daemon first when it doesn't answer. Rejects `daemon` with the daemon's text (for example
 * `no images found`) when the source gives none, and `daemon_unreachable` when the daemon
 * can't be reached.
 */
export function checkSource(kind: SourceType, location: string): Promise<CheckSourceData> {
  return invoke<CheckSourceData>('check_source', { kind, location })
}

/**
 * Downloads one image now for source `id` (IPC `download_now`). Without `id`, one image for
 * every enabled online source that has none yet; `path` is then `null` when none needed one.
 */
export function downloadNow(id?: string): Promise<DownloadNowData> {
  return invoke<DownloadNowData>('download_now', { id: id ?? null })
}

/**
 * Deletes the downloads of source `id` (`download_dir/<id>`). Never touches `manual`,
 * favorites, trash or a local source's folder; rejects `invalid` for such an id.
 */
export function deleteSourceFiles(id: string): Promise<void> {
  return invoke<void>('delete_source_files', { id })
}

/** Opens the folder of source `id` in the file manager (an online source's download folder is created first). */
export function openSourceFolder(id: string): Promise<void> {
  return invoke<void>('open_source_folder', { id })
}

/**
 * Sets a dropped image URL as the wallpaper (IPC `fetch`): an http(s) URL, a `file://` URL or
 * an absolute path. The daemon saves a copy in `manual_dir`. Resolves to the new wallpaper.
 */
export function fetchDroppedUrl(url: string): Promise<WallpaperInfo> {
  return invoke<WallpaperInfo>('fetch_dropped_url', { url })
}

/**
 * Saves a dropped file's bytes as a new file in `manual_dir` and sets it as the wallpaper.
 * The bytes go as the raw request body; the name goes percent-encoded in the `x-file-name`
 * header. Only JPEG, PNG and WebP up to 64 MB; anything else rejects `invalid`.
 */
export function importDroppedFile(name: string, bytes: ArrayBuffer | Uint8Array): Promise<WallpaperInfo> {
  return invoke<WallpaperInfo>('import_dropped_file', bytes, {
    headers: { 'x-file-name': encodeURIComponent(name) },
  })
}

// ── Desktop backend (`commands_desktop.rs`) ─────────────────────────────────

/** The backend the daemon is using (`doctor` data `backend`). */
export interface BackendReport {
  /** Short id: `macos`, `windows`, `gnome`, `awww`, `command`, `unconfigured`, … */
  name: string
  /** For command backends: the argv joined for display, plus ` (kept running)` when persistent. */
  detail: string
}

/** How `doctor` with `apply` went. `error` is the daemon's own text. */
export interface ApplyReport {
  ok: boolean
  path: string | null
  error: string | null
}

/** The parts of the daemon's `doctor` data the Desktop panel shows. */
export interface DoctorReport {
  version: string
  pid: number
  backend: BackendReport
  theme: string
  /** Set only by `testDesktop`. */
  apply: ApplyReport | null
}

/** IPC `doctor`: the backend the running daemon uses. Rejects `daemon_not_running` when down. */
export function desktopBackend(): Promise<DoctorReport> {
  return invoke<DoctorReport>('desktop_backend')
}

/**
 * IPC `reload`, then `doctor` with `apply`: the daemon re-sets the current wallpaper with its
 * setter. A failed re-set resolves with `apply.ok = false` and the daemon's error text; a
 * failed reload rejects.
 */
export function testDesktop(): Promise<DoctorReport> {
  return invoke<DoctorReport>('test_desktop')
}

/**
 * Runs `kakemono detect` (`write`: `detect --write`) and resolves to its report text. A
 * refused `--write` (pinned or legacy config) rejects `cli_failed` with the CLI's stderr.
 */
export function detectDesktop(write: boolean): Promise<string> {
  return invoke<string>('detect_desktop', { write })
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

/**
 * Multi-select image file picker for an Images source. Resolves to `[]` when the user cancels.
 * The plugin types the result as `string[] | null`; a lone string is accepted too.
 */
export async function selectFiles(): Promise<string[]> {
  const picked: unknown = await openDialog({
    directory: false,
    multiple: true,
    filters: [{ name: 'Images', extensions: ['jpg', 'jpeg', 'png', 'webp', 'heic'] }],
  })
  if (typeof picked === 'string') return [picked]
  if (Array.isArray(picked)) return picked.filter((p): p is string => typeof p === 'string')
  return []
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
