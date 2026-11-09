/**
 * Thin wrapper around the Wails Go bindings.
 *
 * In the real Wails build `window.go.main.SettingsApp` is injected by the
 * runtime (the key is the Go struct name, not "App").
 * During plain `npm run dev` we fall back to mock data so the UI can be
 * worked on without a running daemon.
 */

import type { Config, StatusData } from '../types/config'

declare global {
  interface Window {
    go?: { main?: { SettingsApp?: Record<string, (...args: unknown[]) => Promise<unknown>> } }
  }
}

// ─── Mock data ────────────────────────────────────────────────────────────────

const MOCK_CONFIG: Config = {
  daemon: {
    rotation_enabled: true,
    interval: '30m', download_dir: '~/Pictures/Wallpapers/cache',
    cache_size: 50,  trash_dir: '~/Pictures/Wallpapers/trash',
    socket_path: '~/.config/wallpaperd/daemon.sock',
  },
  wallhaven: { enabled: false, api_key: '', purity: '100', sorting: 'random', categories: '111' },
  unsplash:  { enabled: false, access_key: '' },
  reddit:    { enabled: false, subreddits: ['EarthPorn', 'wallpapers'] },
  bing:      { enabled: true,  market: 'en-US' },
  apod:      { enabled: false, api_key: 'DEMO_KEY' },
  local:     { enabled: false, dirs: [], recursive: true },
  light:     { sources: ['bing'], queries: ['landscape', 'nature'] },
  dark:      { sources: ['bing'], queries: ['space', 'night', 'dark'] },
  favorites: { dir: '~/Pictures/Wallpapers/favorites' },
  picker: { position: 'bottom' },
}

const MOCK_STATUS: StatusData = {
  running: true,
  current: {
    path: '~/Pictures/Wallpapers/cache/bing-2024-01-01.jpg',
    source: 'bing', source_url: 'https://www.bing.com/', image_url: '',
    title: 'Snowy Mountains at Sunrise', author: 'Bing Daily',
    width: 3840, height: 2160, set_at: new Date().toISOString(),
  },
  paused: false, interval: '30m', theme: 'light',
  history_count: 5, history_index: 4,
}

// ─── Runtime detection ────────────────────────────────────────────────────────

function isWailsAvailable(): boolean {
  return typeof window !== 'undefined' && window.go?.main?.SettingsApp !== undefined
}

async function callWails<T>(method: string, ...args: unknown[]): Promise<T> {
  if (isWailsAvailable()) {
    const fn = window.go!.main!.SettingsApp![method]
    if (typeof fn !== 'function') throw new Error(`Wails method SettingsApp.${method} not found`)
    return fn(...args) as Promise<T>
  }

  console.debug(`[wails mock] ${method}`, args)
  await new Promise(r => setTimeout(r, 80))

  switch (method) {
    case 'GetConfig':          return structuredClone(MOCK_CONFIG) as T
    case 'SaveConfig':         Object.assign(MOCK_CONFIG, args[0]); return undefined as T
    case 'GetStatus':          return structuredClone(MOCK_STATUS) as T
    case 'NextWallpaper':
    case 'PrevWallpaper':
    case 'FavoriteWallpaper':
    case 'TrashWallpaper':
    case 'PauseWallpaper':
    case 'ResumeWallpaper':
    case 'OpenFolder':
    case 'OpenFavoritesFolder':
    case 'RestartDaemon':      return undefined as T
    case 'SelectDirectory':    return '/Users/mock/Pictures' as T
    case 'SelectFile':         return '/Users/mock/Pictures/photo.jpg' as T
    case 'RevealInFinder':     return undefined as T
    case 'IsDaemonRunning':    return true as T
    case 'GetThumbnail':       return '' as T
    default: throw new Error(`Unknown mock method: ${method}`)
  }
}

// ─── Public API ───────────────────────────────────────────────────────────────

export const WailsAPI = {
  getConfig:           (): Promise<Config>    => callWails('GetConfig'),
  saveConfig:  (cfg: Config): Promise<void>   => callWails('SaveConfig', cfg),
  getStatus:           (): Promise<StatusData> => callWails('GetStatus'),
  nextWallpaper:       (): Promise<void>      => callWails('NextWallpaper'),
  prevWallpaper:       (): Promise<void>      => callWails('PrevWallpaper'),
  favoriteWallpaper:   (): Promise<void>      => callWails('FavoriteWallpaper'),
  trashWallpaper:      (): Promise<void>      => callWails('TrashWallpaper'),
  pauseWallpaper:      (): Promise<void>      => callWails('PauseWallpaper'),
  resumeWallpaper:     (): Promise<void>      => callWails('ResumeWallpaper'),
  openFolder:          (path: string): Promise<void> => callWails('OpenFolder', path),
  openFavoritesFolder: (): Promise<void>      => callWails('OpenFavoritesFolder'),
  selectDirectory:     (): Promise<string>    => callWails('SelectDirectory'),
  selectFile:          (): Promise<string>    => callWails('SelectFile'),
  revealInFinder:      (path: string): Promise<void> => callWails('RevealInFinder', path),
  isDaemonRunning:     (): Promise<boolean>   => callWails('IsDaemonRunning'),
  getThumbnail:        (): Promise<string>    => callWails('GetThumbnail'),
  restartDaemon:       (): Promise<void>      => callWails('RestartDaemon'),
}
