// TypeScript types mirroring the Go config structs in internal/config/config.go
// and IPC protocol types in internal/ipc/protocol.go.
//
// These are canonical — import from this module throughout the frontend.

export interface DaemonConfig {
  rotation_enabled: boolean  // whether auto-rotation is active
  interval: string           // e.g. "30m"
  download_dir: string
  cache_size: number
  trash_dir: string
  socket_path: string
}

export interface WallhavenConfig {
  enabled: boolean
  api_key: string
  purity: string      // "100" = SFW
  sorting: string     // "random"
  categories: string  // "111" = all
}

export interface UnsplashConfig {
  enabled: boolean
  access_key: string
}

export interface RedditConfig {
  enabled: boolean
  subreddits: string[]
}

export interface BingConfig {
  enabled: boolean
  market: string
}

export interface APODConfig {
  enabled: boolean
  api_key: string
}

export interface LocalConfig {
  enabled: boolean
  dirs: string[]
  recursive: boolean
}

export interface ThemeConfig {
  sources: string[]
  queries: string[]
}

export interface FavoritesConfig {
  dir: string
}

export interface PickerConfig {
  position: string  // 'bottom' | 'top' | 'left' | 'right'
}

export interface Config {
  daemon: DaemonConfig
  wallhaven: WallhavenConfig
  unsplash: UnsplashConfig
  reddit: RedditConfig
  bing: BingConfig
  apod: APODConfig
  local: LocalConfig
  light: ThemeConfig
  dark: ThemeConfig
  favorites: FavoritesConfig
  picker: PickerConfig
}

// ── IPC / Status types ────────────────────────────────────────────────────────

export interface WallpaperInfo {
  path: string
  source: string
  source_url: string
  image_url: string
  title: string
  author: string
  width: number
  height: number
  set_at: string
}

export interface StatusData {
  running: boolean      // whether the daemon is alive
  current: WallpaperInfo | null
  paused: boolean
  interval: string
  theme: string         // "light" | "dark"
  history_count: number
  history_index: number
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/** All provider names, matching the key names in Config */
export type ProviderName = 'wallhaven' | 'unsplash' | 'reddit' | 'bing' | 'apod' | 'local'

export const ALL_PROVIDERS: ProviderName[] = [
  'bing', 'wallhaven', 'unsplash', 'reddit', 'apod', 'local',
]

export const PROVIDER_LABELS: Record<ProviderName, string> = {
  bing:      'Bing Daily',
  wallhaven: 'Wallhaven',
  unsplash:  'Unsplash',
  reddit:    'Reddit',
  apod:      'NASA APOD',
  local:     'Local Files',
}

// ── Tab identifiers ───────────────────────────────────────────────────────────

export type TabId = 'general' | 'providers' | 'themes' | 'favorites'
