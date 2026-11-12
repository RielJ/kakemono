import { describe, expect, it } from 'vitest'

import type { SourceType } from '../shared/bindings/SourceType'
import {
  ALL_SOURCE_TYPES,
  isOnline,
  LOCAL_TYPES,
  locationIsPath,
  newSourceId,
  ONLINE_TYPES,
  SOURCE_TYPES,
  takesInput,
} from './sourceTypes'

// The Rust sources of truth, read as text. `sources.rs` holds `is_online`, `takes_input`,
// `prefix` and `label`, plus a test table that `cargo test` checks against those functions;
// `schema.rs` holds `as_str` (variant to config string) and `is_local`.
//
// Read with Node's `fs` rather than a `?raw` import: Vite refuses files outside the frontend
// root. The module name is built at runtime so `tsc` doesn't need `@types/node` in the app
// project, which would let browser code use Node globals unchecked.
const fs: { readFileSync(path: URL, encoding: 'utf8'): string } = await import(/* @vite-ignore */ `node:${'fs'}`)
// `here` is a variable so Vite doesn't rewrite `new URL(…, import.meta.url)` into an asset import.
const here = import.meta.url
const rustFile = (name: string) => fs.readFileSync(new URL(`../../../../config/src/${name}`, here), 'utf8')
const schemaRs = rustFile('schema.rs')
const sourcesRs = rustFile('sources.rs')

/** Body of `fn name` inside `impl SourceType { … }` of `rust`, up to its closing brace (rustfmt indent). */
function fnBody(rust: string, name: string): string {
  const impl = rust.indexOf('impl SourceType {')
  expect(impl, 'impl SourceType').toBeGreaterThanOrEqual(0)
  const start = rust.indexOf(`pub fn ${name}(`, impl)
  expect(start, `fn ${name}`).toBeGreaterThan(impl)
  const end = rust.indexOf('\n    }\n', start)
  expect(end, `end of fn ${name}`).toBeGreaterThan(start)
  return rust.slice(start, end)
}

/** `SourceType::Variant => "text"` arms of a Rust `match`, keyed by variant name. */
function matchArms(body: string): Map<string, string> {
  const arms = new Map<string, string>()
  for (const m of body.matchAll(/SourceType::(\w+)\s*=>\s*"([^"]*)"/g)) arms.set(m[1], m[2])
  return arms
}

interface RustRow {
  kind: string
  online: boolean
  takesInput: boolean
  isLocal: boolean
  prefix: string
  label: string
}

/** The Rust facts per config string, e.g. `album-name`. */
function rustTable(): Map<string, RustRow> {
  const asStr = matchArms(fnBody(schemaRs, 'as_str'))
  const prefix = matchArms(fnBody(sourcesRs, 'prefix'))
  const label = matchArms(fnBody(sourcesRs, 'label'))
  // `matches!(self, SourceType::Folder | …)` in `schema.rs`.
  const local = new Set([...fnBody(schemaRs, 'is_local').matchAll(/SourceType::(\w+)/g)].map((m) => m[1]))
  expect(local.size, 'is_local variants').toBeGreaterThan(0)
  // `(Wallhaven, true, true),` rows of `is_online_and_takes_input_for_every_type`.
  const testStart = sourcesRs.indexOf('fn is_online_and_takes_input_for_every_type')
  expect(testStart, 'Rust is_online/takes_input test').toBeGreaterThan(0)
  const testBody = sourcesRs.slice(testStart, sourcesRs.indexOf('#[test]', testStart))
  const rows = new Map<string, RustRow>()
  for (const m of testBody.matchAll(/\((\w+),\s*(true|false),\s*(true|false)\)/g)) {
    const [, variant, online, input] = m
    const kind = asStr.get(variant)
    expect(kind, `as_str for ${variant}`).toBeDefined()
    if (kind === undefined) continue
    rows.set(kind, {
      kind,
      online: online === 'true',
      takesInput: input === 'true',
      isLocal: local.has(variant),
      prefix: prefix.get(variant) ?? '',
      label: label.get(variant) ?? '',
    })
  }
  return rows
}

describe('sourceTypes', () => {
  it('has an entry for each SourceType', () => {
    const rust = rustTable()
    expect(rust.size).toBe(11)
    expect([...ALL_SOURCE_TYPES].sort()).toEqual([...rust.keys()].sort())
    expect(Object.keys(SOURCE_TYPES).sort()).toEqual([...rust.keys()].sort())
  })

  it('agrees with the Rust is_online, takes_input, is_local, prefix and label', () => {
    for (const row of rustTable().values()) {
      const kind = ALL_SOURCE_TYPES.find((t) => t === row.kind)
      expect(kind, row.kind).toBeDefined()
      if (!kind) continue
      expect({
        kind,
        online: isOnline(kind),
        takesInput: takesInput(kind),
        isLocal: locationIsPath(kind),
        prefix: SOURCE_TYPES[kind].prefix,
        label: SOURCE_TYPES[kind].label,
      }).toEqual(row)
    }
  })

  it('lists online types A–Z: APOD, Bing, Reddit, Unsplash, Wallhaven, Web page', () => {
    expect(ONLINE_TYPES).toEqual(['apod', 'bing', 'reddit', 'unsplash', 'wallhaven', 'web'])
    expect(ONLINE_TYPES.map((t) => SOURCE_TYPES[t].menuLabel)).toEqual([
      'APOD',
      'Bing',
      'Reddit',
      'Unsplash',
      'Wallhaven',
      'Web page',
    ])
  })

  it('lists local types in the spec order', () => {
    expect(LOCAL_TYPES.map((t) => SOURCE_TYPES[t].menuLabel)).toEqual([
      'Images',
      'Folders',
      'Sequential album by file name',
      'Sequential album by file date',
    ])
  })

  it('uses the prompts from the spec', () => {
    const prompt = (t: SourceType) => {
      const { input } = SOURCE_TYPES[t]
      return input.kind === 'none' ? null : input.prompt
    }
    expect(prompt('reddit')).toBe('URL or name of a subreddit:')
    expect(prompt('unsplash')).toBe('Search keyword or URL:')
    expect(prompt('wallhaven')).toBe('Enter keywords or paste URL here:')
    expect(prompt('web')).toBe('Link:')
    expect(prompt('bing')).toBeNull()
    expect(prompt('apod')).toBeNull()
  })

  it('gives every addable type a name and a one-line description', () => {
    for (const t of [...LOCAL_TYPES, ...ONLINE_TYPES]) {
      expect(SOURCE_TYPES[t].menuLabel, t).not.toBe('')
      expect(SOURCE_TYPES[t].description, t).not.toBe('')
      expect(SOURCE_TYPES[t].description, t).not.toContain('\n')
    }
    expect(LOCAL_TYPES).not.toContain('unsupported')
    expect(ONLINE_TYPES).not.toContain('unsupported')
  })

  it('newSourceId(kind) is the type prefix and four hex digits', () => {
    for (const kind of ALL_SOURCE_TYPES) {
      const id = newSourceId(kind)
      expect(id).toMatch(/^[a-z]+-[0-9a-f]{4}$/)
      expect(id.startsWith(`${SOURCE_TYPES[kind].prefix}-`)).toBe(true)
    }
  })

  it('newSourceId avoids ids already taken', () => {
    const taken = new Set<string>()
    for (let i = 0; i < 200; i++) {
      const id = newSourceId('wallhaven', taken)
      expect(taken.has(id)).toBe(false)
      taken.add(id)
    }
  })
})
