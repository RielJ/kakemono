import { useEffect, useRef, useState } from 'react'

import { checkSource, errorMessage, isDaemonUnreachable } from '../shared/api'
import type { SourceEntry } from '../shared/bindings/SourceEntry'
import { isOnline } from './sourceTypes'

/**
 * What the check found for one `[[sources]]` row. A row with no entry needs no attention: it
 * passed a check (now or earlier), or it isn't due one.
 */
export type SourceCheck =
  | { state: 'checking' }
  /** The source answered with an error or no images: the user should fix it. */
  | { state: 'failed'; message: string }
  /** The daemon didn't answer, so the row couldn't be checked; it may be fine. */
  | { state: 'unknown'; message: string }

const UNREACHABLE = 'Couldn’t check: kakemono’s background service isn’t answering.'

/** localStorage key for the sources that passed a check, by `identOf`. */
const STORE_KEY = 'kakemono.goodSources'

/** What makes a source the same source to the daemon; the name doesn't count. */
const identOf = (s: SourceEntry) => `${s.type}\n${s.location.trim()}`

function loadGood(): Set<string> {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(STORE_KEY) ?? '[]')
    return new Set(Array.isArray(raw) ? raw.filter((x): x is string => typeof x === 'string') : [])
  } catch {
    return new Set()
  }
}

function saveGood(good: ReadonlySet<string>) {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify([...good]))
  } catch {
    // Storage full or blocked: the source is checked again next time, nothing worse.
  }
}

/**
 * Checks sources with the daemon (`check_source`) in the background, once each, so the user sees
 * which ones need fixing without the settings window fetching from every source each time it
 * opens. A source that passes is remembered (by type and location, across launches) and never
 * checked again; a failure isn't remembered, so a source that was only briefly unreachable is
 * checked again next time. Only enabled rows are checked, and online ones only while Internet
 * access is on. Renaming a row doesn't re-check it; changing its location does. An answer for a
 * state the row is no longer in is dropped.
 */
export function useSourceChecks(sources: readonly SourceEntry[], internet: boolean): ReadonlyMap<string, SourceCheck> {
  const [checks, setChecks] = useState<ReadonlyMap<string, SourceCheck>>(new Map())
  // The key each row was last checked for.
  const keys = useRef(new Map<string, string>())
  const good = useRef<Set<string> | null>(null)

  useEffect(() => {
    good.current ??= loadGood()
    const known = good.current

    // Forget sources no longer in the list, so a removed one is checked again if it comes back.
    const live = new Set(sources.map(identOf))
    let pruned = false
    for (const ident of known)
      if (!live.has(ident)) {
        known.delete(ident)
        pruned = true
      }
    if (pruned) saveGood(known)

    const ids = new Set(sources.map((s) => s.id))
    // Deleting the entry being visited is safe while iterating a Map.
    for (const id of keys.current.keys()) if (!ids.has(id)) keys.current.delete(id)

    const set = (id: string, check: SourceCheck | null) =>
      setChecks((prev) => {
        if (check === null && !prev.has(id)) return prev
        const next = new Map(prev)
        if (check === null) next.delete(id)
        else next.set(id, check)
        return next
      })
    const update = (id: string, key: string, check: SourceCheck | null) => {
      if (keys.current.get(id) === key) set(id, check)
    }

    for (const s of sources) {
      const ident = identOf(s)
      const due = s.enabled && !known.has(ident) && (internet || !isOnline(s.type))
      const key = due ? ident : ''
      if (keys.current.get(s.id) === key) continue
      keys.current.set(s.id, key)
      if (!due) {
        set(s.id, null)
        continue
      }
      set(s.id, { state: 'checking' })
      checkSource(s.type, s.location.trim()).then(
        ({ count }) => {
          if (keys.current.get(s.id) !== key) return
          if (count > 0) {
            known.add(ident)
            saveGood(known)
            update(s.id, key, null)
          } else update(s.id, key, { state: 'failed', message: 'No images found.' })
        },
        (err: unknown) =>
          update(
            s.id,
            key,
            isDaemonUnreachable(err)
              ? { state: 'unknown', message: UNREACHABLE }
              : { state: 'failed', message: errorMessage(err) },
          ),
      )
    }
  }, [sources, internet])

  return checks
}
