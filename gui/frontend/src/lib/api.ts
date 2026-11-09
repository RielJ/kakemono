/**
 * api.ts — Public API surface used throughout the React frontend.
 *
 * Delegates to WailsAPI (which calls the Wails-bound Go methods or falls
 * back to mocks in plain-browser dev mode).
 */

import type { Config, StatusData } from '../types/config'
import { WailsAPI } from '../services/wails'

export async function getConfig(): Promise<Config>            { return WailsAPI.getConfig() }
export async function saveConfig(cfg: Config): Promise<void>  { return WailsAPI.saveConfig(cfg) }
export async function getStatus(): Promise<StatusData>        { return WailsAPI.getStatus() }
export async function nextWallpaper(): Promise<void>          { return WailsAPI.nextWallpaper() }
export async function prevWallpaper(): Promise<void>          { return WailsAPI.prevWallpaper() }
export async function favoriteWallpaper(): Promise<void>      { return WailsAPI.favoriteWallpaper() }
export async function trashWallpaper(): Promise<void>         { return WailsAPI.trashWallpaper() }
export async function pauseWallpaper(): Promise<void>         { return WailsAPI.pauseWallpaper() }
export async function resumeWallpaper(): Promise<void>        { return WailsAPI.resumeWallpaper() }
export async function selectDirectory(): Promise<string>        { return WailsAPI.selectDirectory() }
export async function selectFile(): Promise<string>             { return WailsAPI.selectFile() }
export async function openFolder(path: string): Promise<void>   { return WailsAPI.openFolder(path) }
export async function openFavoritesFolder(): Promise<void>      { return WailsAPI.openFavoritesFolder() }
export async function revealInFinder(path: string): Promise<void> { return WailsAPI.revealInFinder(path) }
export async function getThumbnail(): Promise<string>           { return WailsAPI.getThumbnail() }
