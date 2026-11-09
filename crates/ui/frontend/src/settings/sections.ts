/** Settings sidebar sections. */
export type TabId = 'general' | 'sources' | 'themes' | 'favorites' | 'desktop' | 'hooks'

export const TAB_TITLES: Record<TabId, string> = {
  general: 'General',
  sources: 'Sources',
  themes: 'Themes',
  favorites: 'Favorites',
  desktop: 'Desktop',
  hooks: 'Hooks',
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
