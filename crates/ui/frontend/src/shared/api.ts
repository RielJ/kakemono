/**
 * Frontend API surface. Every call goes to the Tauri backend (`crates/ui/src-tauri`) with
 * `invoke`, or to a Tauri plugin (dialog, opener). This replaces the Wails bindings.
 *
 * Command names follow plan §H. Commands that the backend does not register yet reject
 * with Tauri's "command not found" error; callers already handle rejections.
 * See `.pi/contracts/ui.md` for which commands are wired.
 */

import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { openPath, revealItemInDir } from '@tauri-apps/plugin-opener'

import type { Config, StatusData } from '../settings/types/config'

/** One picker entry, in the Go picker's shape (the `thumbnails` command returns these). */
export interface Thumbnail {
  path: string
  source: string
  title: string
  isCurrent: boolean
  imageUrl: string
}

// ── Daemon IPC (through Tauri commands) ─────────────────────────────────────

export function getConfig(): Promise<Config> {
  return invoke<Config>('get_config')
}

export function saveConfig(cfg: Config): Promise<void> {
  return invoke<void>('save_config', { patch: cfg })
}

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

export function trashWallpaper(): Promise<void> {
  return invoke<void>('trash')
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

/**
 * Image URL for the current wallpaper. Thumbnails move to the `thumb://` protocol in a
 * later task; until then there is no preview.
 */
export function getThumbnail(): Promise<string> {
  return Promise.resolve('')
}

// ── Native dialogs and the file manager (Tauri plugins) ─────────────────────

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

export function openFolder(path: string): Promise<void> {
  return openPath(path)
}

export async function openFavoritesFolder(): Promise<void> {
  const cfg = await getConfig()
  return openPath(cfg.favorites.dir)
}

export function revealInFinder(path: string): Promise<void> {
  return revealItemInDir(path)
}

// ── Windows ─────────────────────────────────────────────────────────────────

/** Hides the calling window. The tray app keeps running. */
export function closeWindow(): Promise<void> {
  return getCurrentWindow().hide()
}
