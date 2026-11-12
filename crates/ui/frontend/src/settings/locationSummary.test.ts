import { describe, expect, it } from 'vitest'

import type { SourceType } from '../shared/bindings/SourceType'
import { parseReddit, parseUnsplash, parseWallhaven, summarizeLocation } from './locationSummary'

// `crates/sources/tests/locations.json` is also checked by the Rust parsers
// (`instance::tests::locations_fixture_matches_the_parsers`), so these parsers can't drift from
// what the daemon actually does. Read with Node's `fs` for the reasons in `sourceTypes.test.ts`.
const fs: { readFileSync(path: URL, encoding: 'utf8'): string } = await import(/* @vite-ignore */ `node:${'fs'}`)
const here = import.meta.url

interface Case<T> {
  location: string
  expect: T | null
}
interface Table {
  wallhaven: Case<Record<string, string>>[]
  reddit: Case<{ subreddit: string; sort: string; t: string | null }>[]
  unsplash: Case<string>[]
}
const table = JSON.parse(fs.readFileSync(new URL('../../../../sources/tests/locations.json', here), 'utf8')) as Table

describe('parsers match the Rust table', () => {
  it.each(table.wallhaven)('wallhaven $location', ({ location, expect: want }) => {
    const parsed = want === null ? null : (({ query, ...overrides }) => ({ query, overrides }))(want)
    expect(parseWallhaven(location)).toEqual(parsed)
  })

  it.each(table.reddit)('reddit $location', ({ location, expect: want }) => {
    expect(parseReddit(location)).toEqual(want)
  })

  it.each(table.unsplash)('unsplash $location', ({ location, expect: want }) => {
    expect(parseUnsplash(location)).toBe(want)
  })
})

describe('summarizeLocation', () => {
  const cases: [SourceType, string, string][] = [
    [
      'wallhaven',
      'https://wallhaven.cc/search?categories=110&purity=100&topRange=6M&sorting=toplist&order=desc&page=2',
      'Top list · 6 months · General, Anime · SFW',
    ],
    ['wallhaven', 'https://wallhaven.cc/search?q=mountains&sorting=random', '"mountains" · Random'],
    ['wallhaven', 'wallhaven.cc/tag/37', 'Tag 37'],
    ['wallhaven', 'https://wallhaven.cc/latest', 'Latest'],
    ['wallhaven', 'https://wallhaven.cc/toplist?topRange=1y', 'Top list · 1 year'],
    ['wallhaven', 'https://wallhaven.cc/search?sorting=views&order=asc', 'Most viewed · ascending'],
    // A range only counts with the top list, so it's left out next to any other sorting.
    ['wallhaven', 'https://wallhaven.cc/search?sorting=random&topRange=1M', 'Random'],
    // Without a sorting the row uses `[wallhaven] sorting`, which may be the top list.
    ['wallhaven', 'https://wallhaven.cc/search?topRange=1M', '1 month'],
    [
      'wallhaven',
      'https://wallhaven.cc/search?q=sky&purity=111&categories=001&atleast=2560x1440&ratios=16x9,landscape&colors=0066cc',
      '"sky" · People · SFW, Sketchy, NSFW · at least 2560×1440 · 16:9, landscape · #0066cc',
    ],
    ['wallhaven', 'https://wallhaven.cc/search?resolutions=1920x1080,2560x1440', '1920×1080, 2560×1440'],
    ['wallhaven', 'https://wallhaven.cc/search?sorting=weird&purity=2', 'weird · purity 2'],
    // A URL that sets nothing uses the [wallhaven] settings; the table says so.
    ['wallhaven', 'https://wallhaven.cc/search?page=3', 'Wallhaven settings'],
    // Typed keywords and rejected URLs are shown as they are.
    ['wallhaven', 'nature', 'nature'],
    ['wallhaven', 'https://wallhaven.cc/w/abc123', 'https://wallhaven.cc/w/abc123'],
    ['reddit', 'https://www.reddit.com/r/EarthPorn/top/?t=month', 'r/EarthPorn · Top · past month'],
    ['reddit', 'https://reddit.com/r/x/controversial?t=all', 'r/x · Controversial · all time'],
    ['reddit', 'https://old.reddit.com/r/wallpapers/new/', 'r/wallpapers · New'],
    ['reddit', 'https://www.reddit.com/r/EarthPorn/', 'r/EarthPorn'],
    ['reddit', 'EarthPorn', 'EarthPorn'],
    ['reddit', 'https://example.com/r/x', 'https://example.com/r/x'],
    ['unsplash', 'https://unsplash.com/s/photos/misty-forest', 'misty forest'],
    ['unsplash', 'mountains', 'mountains'],
    ['folder', '/Users/me/Pictures/https://wallhaven.cc', '/Users/me/Pictures/https://wallhaven.cc'],
  ]

  it.each(cases)('%s %s', (type, location, want) => {
    expect(summarizeLocation(type, location)).toBe(want)
  })

  it('trims typed text', () => {
    expect(summarizeLocation('wallhaven', '  nature  ')).toBe('nature')
  })
})

describe('summarizeLocation web', () => {
  const cases: [string, string, string][] = [
    ['an https URL shows host and path', 'https://example.com/gallery/2024', 'example.com/gallery/2024'],
    ['a root URL shows just the host', 'https://example.com/', 'example.com'],
    ['a root URL without a slash shows just the host', 'https://example.com', 'example.com'],
    ['www. is dropped from the host', 'https://www.example.com/photos/', 'example.com/photos'],
    ['other subdomains are kept', 'https://images.nasa.gov/feed.xml', 'images.nasa.gov/feed.xml'],
    ['the query and fragment are dropped', 'https://example.com/feed?format=rss&page=2#top', 'example.com/feed'],
    ['a root URL with a query shows just the host', 'https://example.com/?p=1', 'example.com'],
    ['the host is lowercased', 'HTTPS://WWW.Example.COM/Gallery', 'example.com/Gallery'],
    ['escapes in the path are decoded', 'https://example.com/caf%C3%A9/sch%C3%B6n', 'example.com/café/schön'],
    ['a port other than 443 is shown', 'https://example.com:8443/x', 'example.com:8443/x'],
    ['surrounding space is trimmed', '  https://example.com/a  ', 'example.com/a'],
    // Not https: the daemon refuses these, so the table shows them as typed.
    ['plain http passes through', 'http://example.com/gallery', 'http://example.com/gallery'],
    ['a bare host passes through', 'example.com/gallery', 'example.com/gallery'],
    ['garbage passes through', 'not a link', 'not a link'],
    ['an https URL that does not parse passes through', 'https://exa mple.com/x', 'https://exa mple.com/x'],
  ]

  it.each(cases)('%s', (_, location, want) => {
    expect(summarizeLocation('web', location)).toBe(want)
  })

  it('cuts a path over 40 characters to 39 and an ellipsis', () => {
    const path = '/2024/11/a-very-long-gallery-name-that-goes-on-and-on'
    const got = summarizeLocation('web', `https://example.com${path}`)
    expect(got).toBe(`example.com${path.slice(0, 39)}…`)
    expect(Array.from(got.slice('example.com'.length))).toHaveLength(40)
  })

  it('keeps a path of exactly 40 characters whole', () => {
    const path = `/${'a'.repeat(39)}`
    expect(summarizeLocation('web', `https://example.com${path}`)).toBe(`example.com${path}`)
  })

  it('does not split an emoji when cutting', () => {
    const path = `/${'a'.repeat(37)}🌄🌄🌄🌄`
    expect(summarizeLocation('web', `https://example.com${path}`)).toBe(`example.com/${'a'.repeat(37)}🌄…`)
  })
})
