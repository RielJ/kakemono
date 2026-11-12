/**
 * Frontend API surface. Every call goes to the Tauri backend (`crates/ui/src-tauri`) with
 * `invoke`, or to the Tauri dialog plugin. Command names and argument shapes match
 * `src-tauri/src/commands/`, `commands_desktop.rs`, `commands_sources/`, `picker.rs` and
 * `lifecycle.rs`; the one event the picker listens for here is `picker-shown`. See
 * `.pi/contracts/ui.md`.
 *
 * Config, status, command reply and error types are generated from Rust by ts-rs
 * (`./bindings`); do not redeclare them here.
 */

import { Channel, invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open as openDialog } from '@tauri-apps/plugin-dialog'

import type { AppInfo } from './bindings/AppInfo'
import type { ApplyReport } from './bindings/ApplyReport'
import type { BackendReport } from './bindings/BackendReport'
import type { BrowseAction } from './bindings/BrowseAction'
import type { BrowseListing } from './bindings/BrowseListing'
import type { BrowseSourceState } from './bindings/BrowseSourceState'
import type { BrowseSourceStatus } from './bindings/BrowseSourceStatus'
import type { BrowseUseData } from './bindings/BrowseUseData'
import type { CheckSourceData } from './bindings/CheckSourceData'
import type { Config } from './bindings/Config'
import type { DoctorReport } from './bindings/DoctorReport'
import type { DownloadNowData } from './bindings/DownloadNowData'
import type { ErrorKind } from './bindings/ErrorKind'
import type { LoginItems } from './bindings/LoginItems'
import type { PickerTab } from './bindings/PickerTab'
import type { RemoteThumb } from './bindings/RemoteThumb'
import type { SaveOutcome } from './bindings/SaveOutcome'
import type { ServiceStatus } from './bindings/ServiceStatus'
import type { SourceType } from './bindings/SourceType'
import type { StatusData } from './bindings/StatusData'
import type { TabListing } from './bindings/TabListing'
import type { Thumbnail } from './bindings/Thumbnail'
import type { WallpaperInfo } from './bindings/WallpaperInfo'
import type { UpdateInfo } from './bindings/UpdateInfo'
import type { UpdateProgress } from './bindings/UpdateProgress'
import type { WireError } from './bindings/WireError'

export type {
  AppInfo,
  ApplyReport,
  BackendReport,
  BrowseAction,
  BrowseListing,
  BrowseSourceState,
  BrowseSourceStatus,
  BrowseUseData,
  CheckSourceData,
  Config,
  DoctorReport,
  DownloadNowData,
  ErrorKind,
  LoginItems,
  RemoteThumb,
  SaveOutcome,
  ServiceStatus,
  SourceType,
  StatusData,
  TabListing,
  Thumbnail,
  UnlistenFn,
  UpdateInfo,
  UpdateProgress,
  WallpaperInfo,
}

/**
 * An on-disk picker tab (`downloaded`, `local`, `favorites` or `cache`): the generated
 * `PickerTab` binding, renamed so it can't be mixed up with the picker's own tab list, which
 * also has `sources`.
 */
export type DiskTab = PickerTab

/**
 * A partial config for `save_config`: only the keys that changed. Objects merge into their
 * TOML tables, `null` removes a key, anything else replaces the value (comments survive).
 */
export type ConfigPatch = Patch<Config>

type Patch<T> = T extends readonly unknown[] ? T : T extends object ? { [K in keyof T]?: Patch<T[K]> | null } : T

/**
 * How a backend command fails: `WireError` from `src-tauri/src/error.rs`, the one shape every
 * command error serialises to. For `cli_failed` the message carries the CLI's stderr.
 *
 * The source commands (`checkSource`, `downloadNow`, `fetchDroppedUrl`, `importDroppedFile`)
 * reject with `{ kind: 'daemon_unreachable', message }` when the daemon isn't running and
 * can't be started, or the connection fails or times out (test with `isDaemonUnreachable`).
 * When the daemon answers but refuses (for example a source with no images), the kind is
 * `daemon` and the message is the daemon's text. Bad input is `invalid`.
 */
export type CommandError = WireError

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

/**
 * True when a command outlived its deadline (kind `daemon_timeout`). For `browseUse` this is
 * not a failure: the download goes on in the background and the message says so.
 */
export function isDaemonTimeout(err: unknown): boolean {
  return isCommandError(err) && err.kind === 'daemon_timeout'
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
 * The current wallpaper as a picker entry, its `thumb://` URL admitted for the Settings
 * window's "Now showing" card; `null` when nothing is showing or the file is gone.
 */
export function currentThumbnail(): Promise<Thumbnail | null> {
  return invoke<Thumbnail | null>('current_thumbnail')
}

/**
 * One page (40 entries) of an on-disk picker tab, from `offset`. On `offset` 0 the current
 * wallpaper is pinned first when the tab holds it (`currentIndex` 0).
 */
export function listTab(tab: DiskTab, offset: number): Promise<TabListing> {
  return invoke<TabListing>('tab_thumbnails', { tab, offset })
}

// ── Sources tab (`commands_sources/browse.rs`) ───────────────────────────────

export interface BrowsePageArgs {
  /** The session from the previous page; omit to start a new one. */
  session?: string | null
  page: number
  /** Browse only this `[[sources]]` id; omit for every enabled online source. */
  source?: string | null
}

/**
 * One page of remote previews (IPC `browse`). An unknown or expired `session` comes back
 * `restarted` at page 0. Rejects with the daemon's text when browsing is off or unavailable.
 */
export function browsePage({ session, page, source }: BrowsePageArgs): Promise<BrowseListing> {
  return invoke<BrowseListing>('browse_page', { session: session ?? null, page, source: source ?? null })
}

export interface BrowseUseArgs {
  sourceId: string
  remoteId: string
  action: BrowseAction
}

/**
 * Downloads a browsed image into `download_dir/<sourceId>/`, then sets it, copies it to
 * favorites or just keeps it (IPC `browse_use`). Waits up to 120 s; past that it rejects
 * `daemon_timeout` ("Still downloading in the background…", see `isDaemonTimeout`).
 */
export function browseUse({ sourceId, remoteId, action }: BrowseUseArgs): Promise<BrowseUseData> {
  return invoke<BrowseUseData>('browse_use', { sourceId, remoteId, action })
}

// ── Sources and dropped images (`commands_sources/`) ─────────────────────────

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
 * refused `--write` (a pinned config) rejects `cli_failed` with the CLI's stderr.
 */
export function detectDesktop(write: boolean): Promise<string> {
  return invoke<string>('detect_desktop', { write })
}

/**
 * Starts the daemon unless it answers (`kakemono daemon`, which refuses a second one).
 * Resolves to `true` once it answers; rejects with the CLI's stderr when it cannot start.
 */
export function startDaemon(): Promise<boolean> {
  return invoke<boolean>('ensure_daemon')
}

// ── App updates (src-tauri/src/updater.rs) ──────────────────────────────────

/** The newer release, or `null` when this is the newest version. */
export function checkUpdate(): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>('check_update')
}

/**
 * Downloads and installs the update found by the last `checkUpdate`, then restarts the app, so
 * on success the promise never settles. Rejects when any step fails.
 */
export function installUpdate(onProgress: (progress: UpdateProgress) => void): Promise<void> {
  const progress = new Channel<UpdateProgress>()
  progress.onmessage = onProgress
  return invoke<void>('install_update', { progress })
}

/** Opens the release page of the update found by the last check, in the browser. */
export function openUpdatePage(): Promise<void> {
  return invoke<void>('open_update_page')
}

// ── Start at login ──────────────────────────────────────────────────────────

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

/**
 * Opens a folder in the file manager (`open_folder` command). A leading `~` is expanded;
 * anything that is not an existing folder rejects `invalid`, so no file is ever launched.
 */
export function openFolder(path: string): Promise<void> {
  return invoke<void>('open_folder', { path })
}

/** Shows the file in Finder, Explorer or the file manager (`reveal` command). */
export function revealInFinder(path: string): Promise<void> {
  return invoke<void>('reveal', { path })
}

// ── Windows ─────────────────────────────────────────────────────────────────

/** Hides the calling window. The tray app keeps running. */
export function closeWindow(): Promise<void> {
  return getCurrentWindow().hide()
}

/**
 * Closes (destroys) the calling window. Used by the Welcome window: the backend opens Settings
 * when it closes, however it closed.
 */
export function destroyWindow(): Promise<void> {
  return getCurrentWindow().close()
}

/** Opens or focuses the Welcome window (`open_welcome`). Writes nothing. */
export function openWelcome(): Promise<void> {
  return invoke<void>('open_welcome')
}

/** Whether this session shows the tray icon (`tray_available`); false on a Linux desktop without one. */
export function trayAvailable(): Promise<boolean> {
  return invoke<boolean>('tray_available')
}

/** Shows the picker at its saved or edge position. */
export function showPicker(): Promise<void> {
  return invoke<void>('show_picker')
}

/** Hides the picker, saving a dragged position first. */
export function hidePicker(): Promise<void> {
  return invoke<void>('hide_picker')
}

/** Emitted to the picker each time it is shown (`picker.rs`, `SHOWN_EVENT`). */
export const PICKER_SHOWN = 'picker-shown'

/** Calls `handler` each time the picker is shown. Resolves to the function that stops it. */
export function onPickerShown(handler: () => void): Promise<UnlistenFn> {
  return listen<null>(PICKER_SHOWN, () => handler())
}

/** One end of the picker strip: left/right when horizontal, top/bottom when vertical. */
export type StripEnd = 'start' | 'end'

/** A resize handle was grabbed: remembers the picker's size and the screen it is on. */
export function pickerResizeBegin(): Promise<void> {
  return invoke<void>('picker_resize_begin')
}

/**
 * Resizes the picker along its strip: the grabbed `end` is `delta` logical px from where it
 * was grabbed. Rust keeps it on the screen and at least the minimum length.
 */
export function pickerResize(end: StripEnd, delta: number): Promise<void> {
  return invoke<void>('picker_resize', { end, delta })
}

/** Forgets a dragged picker position (`[picker] placement = "edge"`). */
export function resetPickerPosition(): Promise<void> {
  return invoke<void>('reset_picker_position')
}
