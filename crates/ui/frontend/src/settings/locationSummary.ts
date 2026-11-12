import type { SourceType } from '../shared/bindings/SourceType'

// A pasted location, read the way the daemon reads it (`crates/sources/src/instance.rs`), and
// summarized for the Sources table. The table shows the summary; the tooltip, the Edit dialog
// and the config keep the full text.
//
// Both sides are checked against `crates/sources/tests/locations.json`, so a parser change on
// one side that the other doesn't follow fails a test.

/** The Wallhaven search parameters a URL can set, under their URL names. */
export const WALLHAVEN_PARAMS = [
  'categories',
  'purity',
  'sorting',
  'topRange',
  'order',
  'atleast',
  'resolutions',
  'ratios',
  'colors',
] as const
export type WallhavenParam = (typeof WALLHAVEN_PARAMS)[number]

export interface WallhavenSearch {
  /** The `q` parameter, or the typed keywords; empty for "anything". */
  query: string
  /** Parameters that replace the `[wallhaven]` settings for this row. */
  overrides: Partial<Record<WallhavenParam, string>>
}

export interface RedditListing {
  subreddit: string
  sort: 'hot' | 'new' | 'rising' | 'top' | 'controversial'
  /** The `t` range; only `top` and `controversial` have one. */
  t: string | null
}

const WALLHAVEN_HOSTS = ['wallhaven.cc', 'www.wallhaven.cc']
const REDDIT_BARE_HOSTS = ['reddit.com', 'www.reddit.com', 'old.reddit.com']
const UNSPLASH_HOSTS = ['unsplash.com', 'www.unsplash.com']
const REDDIT_SORTS = ['hot', 'new', 'rising', 'top', 'controversial'] as const
const REDDIT_RANGES = ['hour', 'day', 'week', 'month', 'year', 'all']

/** Rust `url_like`: a URL, `'text'` for plain text, or `'bad'` when it looks like a URL but doesn't parse. */
function urlLike(loc: string, bareHosts: string[]): URL | 'text' | 'bad' {
  const lower = loc.toLowerCase()
  let text: string
  if (lower.startsWith('http://') || lower.startsWith('https://')) text = loc
  else if (bareHosts.some((h) => lower.startsWith(`${h}/`))) text = `https://${loc}`
  else return 'text'
  try {
    return new URL(text)
  } catch {
    return 'bad'
  }
}

const hostIn = (url: URL, hosts: string[]) => hosts.includes(url.hostname.toLowerCase())
const segments = (url: URL) => url.pathname.split('/').filter((s) => s !== '')

/** Rust `percent_decode`: `%XX` escapes decoded, invalid ones kept, invalid UTF-8 replaced. */
function percentDecode(s: string): string {
  const input = new TextEncoder().encode(s)
  const out: number[] = []
  const hex = (b: number | undefined) => {
    const d = b === undefined ? NaN : parseInt(String.fromCharCode(b), 16)
    return Number.isNaN(d) ? null : d
  }
  for (let i = 0; i < input.length; i++) {
    const byte = input[i] ?? 0
    const hi = hex(input[i + 1])
    const lo = hex(input[i + 2])
    if (byte === 0x25 && hi !== null && lo !== null) {
      out.push(hi * 16 + lo)
      i += 2
    } else {
      out.push(byte)
    }
  }
  return new TextDecoder().decode(Uint8Array.from(out))
}

/** Rust `wallhaven_location`. `null` when the daemon rejects the location. */
export function parseWallhaven(location: string): WallhavenSearch | null {
  const loc = location.trim()
  let url: URL
  if (loc.startsWith('/')) {
    try {
      url = new URL(`https://wallhaven.cc${loc}`)
    } catch {
      return null
    }
  } else {
    const parsed = urlLike(loc, WALLHAVEN_HOSTS)
    if (parsed === 'bad') return null
    if (parsed === 'text') return { query: loc, overrides: {} }
    url = parsed
  }
  if (!hostIn(url, WALLHAVEN_HOSTS)) return null

  const search: WallhavenSearch = { query: '', overrides: {} }
  const params: readonly string[] = WALLHAVEN_PARAMS
  for (const [key, raw] of url.searchParams) {
    const value = raw.trim()
    if (value === '') continue
    if (key === 'q') search.query = value
    else if (params.includes(key)) search.overrides[key as WallhavenParam] = value
  }

  const path = segments(url)
  const [first, second] = path
  if (path.length === 1 && first === 'search') return search
  if (path.length === 2 && first === 'tag' && second !== undefined && /^[0-9]+$/.test(second)) {
    search.query = `id:${second}`
    return search
  }
  const listings: Record<string, string> = {
    latest: 'date_added',
    toplist: 'toplist',
    hot: 'hot',
    random: 'random',
  }
  const sorting = path.length === 1 && first !== undefined ? listings[first] : undefined
  if (sorting === undefined) return null
  search.overrides.sorting = sorting
  return search
}

/** Rust `reddit_location` plus `reddit_sort`. `null` when the daemon rejects the location. */
export function parseReddit(location: string): RedditListing | null {
  const loc = location.trim()
  const parsed = urlLike(loc, REDDIT_BARE_HOSTS)
  if (parsed === 'bad') return null
  let name: string
  let listing: Pick<RedditListing, 'sort' | 't'> = { sort: 'hot', t: null }
  if (parsed === 'text') {
    const rest = loc.replace(/^\/+/, '')
    const bare = rest.startsWith('r/') || rest.startsWith('R/') ? rest.slice(2) : rest
    name = bare.replace(/\/+$/, '')
  } else {
    const host = parsed.hostname.toLowerCase()
    if (host !== 'reddit.com' && !host.endsWith('.reddit.com')) return null
    const [r, sub, sort] = segments(parsed)
    if (r?.toLowerCase() !== 'r' || sub === undefined) return null
    name = sub
    listing = redditSort(sort, parsed.searchParams.get('t'))
  }
  if (!/^[A-Za-z0-9_+]{1,50}$/.test(name)) return null
  return { subreddit: name, ...listing }
}

/** Rust `Sort::parse` with its fallback to `hot`. */
function redditSort(segment: string | undefined, t: string | null): Pick<RedditListing, 'sort' | 't'> {
  const sort = REDDIT_SORTS.find((s) => s === segment?.toLowerCase())
  if (sort === undefined) return { sort: 'hot', t: null }
  const range = REDDIT_RANGES.find((r) => r === t?.toLowerCase()) ?? null
  return { sort, t: sort === 'top' || sort === 'controversial' ? range : null }
}

/** Rust `unsplash_location`. `null` when the daemon rejects the location. */
export function parseUnsplash(location: string): string | null {
  const loc = location.trim()
  const parsed = urlLike(loc, UNSPLASH_HOSTS)
  if (parsed === 'bad') return null
  if (parsed === 'text') return loc
  if (!hostIn(parsed, UNSPLASH_HOSTS)) return null
  const path = segments(parsed)
  const keywords = path[2]
  if (path.length !== 3 || path[0] !== 's' || path[1] !== 'photos' || keywords === undefined) return null
  const words = percentDecode(keywords).replaceAll('-', ' ').split(/\s+/).filter(Boolean).join(' ')
  return words === '' ? null : words
}

const SORTINGS: Record<string, string> = {
  toplist: 'Top list',
  date_added: 'Latest',
  hot: 'Hot',
  random: 'Random',
  views: 'Most viewed',
  favorites: 'Most favorited',
  relevance: 'Relevance',
}
const TOP_RANGES: Record<string, string> = {
  '1d': '1 day',
  '3d': '3 days',
  '1w': '1 week',
  '1M': '1 month',
  '3M': '3 months',
  '6M': '6 months',
  '1y': '1 year',
}
const REDDIT_SORT_LABELS: Record<RedditListing['sort'], string> = {
  hot: 'Hot',
  new: 'New',
  rising: 'Rising',
  top: 'Top',
  controversial: 'Controversial',
}
const REDDIT_RANGE_LABELS: Record<string, string> = {
  hour: 'past hour',
  day: 'past day',
  week: 'past week',
  month: 'past month',
  year: 'past year',
  all: 'all time',
}

/** `110` with names `[General, Anime, People]` → `General, Anime`; anything else → `null`. */
function flags(value: string, names: string[]): string | null {
  if (!new RegExp(`^[01]{${names.length}}$`).test(value)) return null
  const on = names.filter((_, i) => value[i] === '1')
  return on.length === 0 ? null : on.join(', ')
}

const dims = (v: string) => v.replaceAll('x', '×')

function summarizeWallhaven(search: WallhavenSearch): string {
  const o = search.overrides
  const parts: string[] = []
  const tag = /^id:([0-9]+)$/.exec(search.query)
  if (tag) parts.push(`Tag ${tag[1]}`)
  else if (search.query !== '') parts.push(`"${search.query}"`)
  if (o.sorting !== undefined) parts.push(SORTINGS[o.sorting] ?? o.sorting)
  // Wallhaven reads the range only with the top list. With no sorting in the URL the row uses
  // `[wallhaven] sorting`, which may be the top list, so the range is shown then too.
  if (o.topRange !== undefined && (o.sorting === undefined || o.sorting === 'toplist')) {
    parts.push(TOP_RANGES[o.topRange] ?? o.topRange)
  }
  if (o.order === 'asc') parts.push('ascending')
  else if (o.order !== undefined && o.order !== 'desc') parts.push(`order ${o.order}`)
  if (o.categories !== undefined) {
    parts.push(flags(o.categories, ['General', 'Anime', 'People']) ?? `categories ${o.categories}`)
  }
  if (o.purity !== undefined) parts.push(flags(o.purity, ['SFW', 'Sketchy', 'NSFW']) ?? `purity ${o.purity}`)
  if (o.atleast !== undefined) parts.push(`at least ${dims(o.atleast)}`)
  if (o.resolutions !== undefined) parts.push(dims(o.resolutions).split(',').join(', '))
  if (o.ratios !== undefined) {
    parts.push(
      o.ratios
        .split(',')
        .map((r) => (/^[0-9]+x[0-9]+$/.test(r) ? r.replace('x', ':') : r))
        .join(', '),
    )
  }
  if (o.colors !== undefined) parts.push(o.colors.startsWith('#') ? o.colors : `#${o.colors}`)
  return parts.length === 0 ? 'Wallhaven settings' : parts.join(' · ')
}

function summarizeReddit(listing: RedditListing): string {
  const parts = [`r/${listing.subreddit}`]
  if (listing.sort !== 'hot') parts.push(REDDIT_SORT_LABELS[listing.sort])
  if (listing.t !== null) parts.push(REDDIT_RANGE_LABELS[listing.t] ?? listing.t)
  return parts.join(' · ')
}

/** Longest web path shown, in characters, `…` included. */
const WEB_PATH_MAX = 40

/**
 * `https://www.example.com/gallery/?page=2` → `example.com/gallery`: the host without `www.`,
 * then the decoded path cut to `WEB_PATH_MAX` characters. The query and fragment are dropped to
 * keep the cell short; the tooltip has the full URL. `null` for anything but a parseable
 * `https://` URL, which the daemon refuses.
 */
function summarizeWeb(loc: string): string | null {
  if (!loc.toLowerCase().startsWith('https://')) return null
  let url: URL
  try {
    url = new URL(loc)
  } catch {
    return null
  }
  // `host` keeps a port other than 443, which the daemon refuses, so it stays visible.
  const host = url.host.toLowerCase().replace(/^www\./, '')
  const path = Array.from(percentDecode(url.pathname).replace(/\/+$/, ''))
  if (path.length === 0) return host
  const shown = path.length > WEB_PATH_MAX ? `${path.slice(0, WEB_PATH_MAX - 1).join('')}…` : path.join('')
  return host + shown
}

/**
 * What the Sources table shows for a row's `location`. A pasted Wallhaven, Reddit or Unsplash
 * URL becomes a short description of what kakemono reads from it (`Top list · 6 months · SFW`);
 * a web link becomes its host and path (`example.com/gallery`).
 * Typed keywords, paths, and locations the daemon rejects are shown as typed, trimmed for the
 * online types; the Status column flags the rejected ones.
 */
export function summarizeLocation(type: SourceType, location: string): string {
  const loc = location.trim()
  switch (type) {
    case 'wallhaven': {
      if (!loc.startsWith('/') && urlLike(loc, WALLHAVEN_HOSTS) === 'text') return loc
      const search = parseWallhaven(loc)
      return search === null ? loc : summarizeWallhaven(search)
    }
    case 'reddit': {
      if (urlLike(loc, REDDIT_BARE_HOSTS) === 'text') return loc
      const listing = parseReddit(loc)
      return listing === null ? loc : summarizeReddit(listing)
    }
    case 'unsplash': {
      if (urlLike(loc, UNSPLASH_HOSTS) === 'text') return loc
      return parseUnsplash(loc) ?? loc
    }
    case 'web':
      return summarizeWeb(loc) ?? loc
    default:
      return location
  }
}
