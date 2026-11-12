import type { SourceType } from '../shared/bindings/SourceType'

/**
 * What a source's `location` holds, and how the UI asks for it.
 * - `none`: no input (Bing, APOD); `location` stays empty.
 * - `text`: keywords, a URL, a subreddit or a link, typed into the source dialog.
 * - `path`: a file or folder on disk, chosen with the system picker.
 */
export type SourceInput =
  | { kind: 'none' }
  | { kind: 'text'; prompt: string; placeholder: string }
  | { kind: 'path'; prompt: string; pick: 'files' | 'folder' }

export interface SourceTypeInfo {
  /** Name in the sources table. Matches Rust `SourceType::label`. */
  label: string
  /** Bold name in the Add… menu. */
  menuLabel: string
  /** One line under the name in the Add… menu. */
  description: string
  /** Downloads into `downloads/<id>/`. Matches Rust `SourceType::is_online`. */
  online: boolean
  input: SourceInput
  /** Prefix of generated ids, e.g. `wh` in `wh-1a2b`. Matches Rust `SourceType::prefix`. */
  prefix: string
}

/**
 * Per-type metadata, the frontend's single table of it. The Rust side is
 * `crates/config/src/sources.rs`; `sourceTypes.test.ts` reads that file and fails when the two
 * disagree. A `Record` over the generated `SourceType`, so a new variant fails `tsc` here.
 */
export const SOURCE_TYPES: Record<SourceType, SourceTypeInfo> = {
  images: {
    label: 'Images',
    menuLabel: 'Images',
    description: 'Individual image files.',
    online: false,
    input: { kind: 'path', prompt: 'Image file:', pick: 'files' },
    prefix: 'img',
  },
  folder: {
    label: 'Folder',
    menuLabel: 'Folders',
    description: 'Random images from a folder and its subfolders.',
    online: false,
    input: { kind: 'path', prompt: 'Folder:', pick: 'folder' },
    prefix: 'dir',
  },
  'album-name': {
    label: 'Sequential album by file name',
    menuLabel: 'Sequential album by file name',
    description: 'Every image in a folder, in file name order.',
    online: false,
    input: { kind: 'path', prompt: 'Folder:', pick: 'folder' },
    prefix: 'aln',
  },
  'album-date': {
    label: 'Sequential album by file date',
    menuLabel: 'Sequential album by file date',
    description: 'Every image in a folder, oldest file first.',
    online: false,
    input: { kind: 'path', prompt: 'Folder:', pick: 'folder' },
    prefix: 'ald',
  },
  apod: {
    label: 'APOD',
    menuLabel: 'APOD',
    description: "NASA's Astronomy Picture of the Day.",
    online: true,
    input: { kind: 'none' },
    prefix: 'apod',
  },
  bing: {
    label: 'Bing',
    menuLabel: 'Bing',
    description: "Bing's photo of the day.",
    online: true,
    input: { kind: 'none' },
    prefix: 'bing',
  },
  reddit: {
    label: 'Reddit',
    menuLabel: 'Reddit',
    description: 'Images posted to a subreddit.',
    online: true,
    input: { kind: 'text', prompt: 'URL or name of a subreddit:', placeholder: 'EarthPorn' },
    prefix: 'rd',
  },
  unsplash: {
    label: 'Unsplash',
    menuLabel: 'Unsplash',
    description: 'Photos from Unsplash for a search or a collection URL.',
    online: true,
    input: { kind: 'text', prompt: 'Search keyword or URL:', placeholder: 'mountains' },
    prefix: 'us',
  },
  wallhaven: {
    label: 'Wallhaven',
    menuLabel: 'Wallhaven',
    description: 'Wallpapers from wallhaven.cc for a search or a URL.',
    online: true,
    input: { kind: 'text', prompt: 'Enter keywords or paste URL here:', placeholder: 'nature' },
    prefix: 'wh',
  },
  web: {
    label: 'Web page',
    menuLabel: 'Web page',
    description: 'Full-size images from a link: an image, a gallery page or a feed.',
    online: true,
    input: { kind: 'text', prompt: 'Link:', placeholder: 'https://example.com/gallery' },
    prefix: 'web',
  },
  unsupported: {
    label: 'Unknown',
    menuLabel: 'Unknown',
    description: "A source type this version doesn't know.",
    online: false,
    // Rust `takes_input` is true: the row keeps whatever location it had.
    input: { kind: 'text', prompt: 'Location:', placeholder: '' },
    prefix: 'unk',
  },
}

/** Every type, in Add… menu order (local, then online), then `unsupported`. */
export const ALL_SOURCE_TYPES = [
  'images',
  'folder',
  'album-name',
  'album-date',
  'apod',
  'bing',
  'reddit',
  'unsplash',
  'wallhaven',
  'web',
  'unsupported',
] as const satisfies readonly SourceType[]

// Fails to compile when a `SourceType` is missing from `ALL_SOURCE_TYPES`.
type MissingType = Exclude<SourceType, (typeof ALL_SOURCE_TYPES)[number]>
const allListed: [MissingType] extends [never] ? true : MissingType = true
void allListed

export const isOnline = (t: SourceType): boolean => SOURCE_TYPES[t].online
/** Longest source name, as `MAX_NAME_LEN` in kakemono-config; the config cuts longer ones on load. */
export const MAX_NAME_LEN = 64

/** Whether the source needs a `location`. Matches Rust `SourceType::takes_input`. */
export const takesInput = (t: SourceType): boolean => SOURCE_TYPES[t].input.kind !== 'none'
/** Whether `location` is a filesystem path. Matches Rust `SourceType::is_local`. */
export const locationIsPath = (t: SourceType): boolean => SOURCE_TYPES[t].input.kind === 'path'

/** Local types for the Add… menu, in the spec's order. */
export const LOCAL_TYPES: readonly SourceType[] = ALL_SOURCE_TYPES.filter(locationIsPath)

/** Online types for the Add… menu, A–Z by name. */
export const ONLINE_TYPES: readonly SourceType[] = ALL_SOURCE_TYPES.filter(isOnline).sort((a, b) =>
  SOURCE_TYPES[a].menuLabel.localeCompare(SOURCE_TYPES[b].menuLabel, 'en'),
)

/**
 * A random id for a new row, `"{prefix}-{hex4}"` (e.g. `wh-1a2b`), not in `taken`. The daemon
 * generates deterministic ids only when migrating; the UI's Add uses random ones.
 */
export function newSourceId(kind: SourceType, taken: ReadonlySet<string> = new Set()): string {
  const { prefix } = SOURCE_TYPES[kind]
  const buf = new Uint16Array(1)
  for (;;) {
    crypto.getRandomValues(buf)
    const id = `${prefix}-${buf[0].toString(16).padStart(4, '0')}`
    if (!taken.has(id)) return id
  }
}
