import { useEffect, useRef, useState } from 'react'

import { checkSource, errorMessage, isDaemonUnreachable } from '../shared/api'
import type { SourceEntry } from '../shared/bindings/SourceEntry'
import { isOnline } from './sourceTypes'

/** What the background check found for one `[[sources]]` row. */
export type SourceCheck =
  | { state: 'checking' }
  | { state: 'ok'; count: number }
  | { state: 'failed'; message: string }
  /** The daemon didn't answer, so the row couldn't be checked; it may be fine. */
  | { state: 'unknown'; message: string }
  /** Online row while `[daemon] internet` is off: not checked. */
  | { state: 'paused' }

const UNREACHABLE = 'Couldn’t check: kakemono’s background service isn’t answering.'

/** What a check depends on; the name and the enabled box don't count. */
const keyOf = (s: SourceEntry, internet: boolean) =>
  `${s.type}\n${s.location.trim()}\n${internet || !isOnline(s.type) ? 'on' : 'off'}`

/**
 * Checks each source with the daemon (`check_source`) in the background, so saving a row never
 * waits on it. A row is checked when it first appears and again only when its type or location
 * changes (or Internet access turns back on); renaming it or ticking its box doesn't re-check.
 * An answer for a location the row no longer has is dropped.
 */
export function useSourceChecks(sources: readonly SourceEntry[], internet: boolean): ReadonlyMap<string, SourceCheck> {
  const [checks, setChecks] = useState<ReadonlyMap<string, SourceCheck>>(new Map())
  // The key each row was last checked for.
  const keys = useRef(new Map<string, string>())

  useEffect(() => {
    const live = new Set(sources.map((s) => s.id))
    // Deleting the entry being visited is safe while iterating a Map.
    for (const id of keys.current.keys()) if (!live.has(id)) keys.current.delete(id)

    const update = (id: string, key: string, check: SourceCheck) => {
      if (keys.current.get(id) !== key) return
      setChecks((prev) => new Map(prev).set(id, check))
    }

    for (const s of sources) {
      const key = keyOf(s, internet)
      if (keys.current.get(s.id) === key) continue
      keys.current.set(s.id, key)
      if (!internet && isOnline(s.type)) {
        update(s.id, key, { state: 'paused' })
        continue
      }
      update(s.id, key, { state: 'checking' })
      checkSource(s.type, s.location.trim()).then(
        ({ count }) =>
          update(s.id, key, count > 0 ? { state: 'ok', count } : { state: 'failed', message: 'No images found.' }),
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
