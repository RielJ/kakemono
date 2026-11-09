/** Settings sidebar sections. Task 42 adds `desktop` and `hooks`. */
export type TabId = 'general' | 'sources' | 'themes' | 'favorites'

export const TAB_TITLES: Record<TabId, string> = {
  general: 'General',
  sources: 'Sources',
  themes: 'Themes',
  favorites: 'Favorites',
}

/** Wallpaper sources, in the order the Sources section lists them. Keys match `Config`. */
export const SOURCES = ['bing', 'wallhaven', 'unsplash', 'reddit', 'apod', 'local'] as const
export type SourceName = (typeof SOURCES)[number]

export const SOURCE_LABELS: Record<SourceName, string> = {
  bing: 'Bing Daily',
  wallhaven: 'Wallhaven',
  unsplash: 'Unsplash',
  reddit: 'Reddit',
  apod: 'NASA APOD',
  local: 'Local folders',
}

/** What the platform calls its file manager, for "Show in …" labels. */
export function fileManagerName(os: string | undefined): string {
  if (os === 'macos') return 'Finder'
  if (os === 'windows') return 'Explorer'
  return 'the file manager'
}
