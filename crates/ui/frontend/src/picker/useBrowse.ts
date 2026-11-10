import { useCallback, useEffect, useRef, useState } from 'react'
import { browsePage, errorMessage, type BrowseListing, type BrowseSourceStatus, type RemoteThumb } from '../shared/api'

/** The UI keeps at most this many remote items per session; then a Reload card shows. */
export const MAX_ITEMS = 300

/** A remembered session younger than this is resumed when the picker opens again. */
export const RESUME_MS = 10 * 60 * 1000

/** What the Sources tab says when `[daemon] internet = false`. */
export const INTERNET_OFF = 'Browsing is off — internet access is disabled in Settings.'

/** How the daemon words `InternetOff` (`kakemono_protocol` docs). */
const INTERNET_OFF_DAEMON = /internet access is off/i

/** Why the first page failed. `internet_off` is a setting, not a fault, so it reads differently. */
export interface BrowseError {
  kind: 'internet_off' | 'failed'
  message: string
}

/** The session to resume on the next show, and when it was last used. */
export interface BrowseMemory {
  session: string
  filter: string | null
  at: number
}

export interface UseBrowseOptions {
  /** Where the session outlives the strip. Defaults to one module-wide slot. */
  memory?: { current: BrowseMemory | null }
}

export interface Browse {
  items: RemoteThumb[]
  /** The per-source states from the latest page (active, exhausted, rate limited, failed). */
  statuses: BrowseSourceStatus[]
  /** Every source seen in this picker, for the filter (a filtered page lists fewer). */
  sources: BrowseSourceStatus[]
  /** The `[[sources]]` id being browsed, or `null` for all of them. */
  filter: string | null
  /** The first page is loading. */
  loading: boolean
  loadingMore: boolean
  /** Every source ran out. */
  done: boolean
  /** The list reached {@link MAX_ITEMS}. */
  capped: boolean
  /** The first page failed; nothing is shown. */
  error: BrowseError | null
  /** A later page failed; the items stay and `loadMore` retries it. */
  moreError: string | null
  loadMore: () => void
  /** Starts a new session with the same filter. */
  reload: () => void
  setFilter: (filter: string | null) => void
  /** Moves the filter along "All", then each source, wrapping. No-op with one source. */
  cycleFilter: (delta: number) => void
}

const sharedMemory: { current: BrowseMemory | null } = { current: null }

function toError(err: unknown): BrowseError {
  const message = errorMessage(err)
  return INTERNET_OFF_DAEMON.test(message) ? { kind: 'internet_off', message: INTERNET_OFF } : { kind: 'failed', message }
}

/** Adds sources a filtered page didn't list, keeping the order they were first seen in. */
function mergeSources(known: BrowseSourceStatus[], seen: BrowseSourceStatus[]): BrowseSourceStatus[] {
  const have = new Set(known.map((s) => s.source_id))
  const added = seen.filter((s) => !have.has(s.source_id))
  return added.length === 0 ? known : known.concat(added)
}

const key = (t: RemoteThumb) => `${t.sourceId}\u0000${t.remoteId}`

/**
 * The Sources tab's paging over the daemon's browse session (`browse_page`): page 0, then
 * one page per `loadMore`, with calls made while a page is in flight collapsed into it. A
 * session the daemon forgot comes back `restarted` at page 0 and replaces the list. A filter
 * change or `reload` starts a new session; a page asked for before it is dropped.
 */
export function useBrowse({ memory = sharedMemory }: UseBrowseOptions = {}): Browse {
  // Read at mount and written on every page; a new object each render must not restart paging.
  const memoryRef = useRef(memory)
  memoryRef.current = memory
  const [resume] = useState(() => {
    const m = memory.current
    return m && Date.now() - m.at < RESUME_MS ? m : null
  })
  const [items, setItems] = useState<RemoteThumb[]>([])
  const [statuses, setStatuses] = useState<BrowseSourceStatus[]>([])
  const [sources, setSources] = useState<BrowseSourceStatus[]>([])
  const [filter, setFilterState] = useState<string | null>(resume?.filter ?? null)
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [done, setDone] = useState(false)
  const [error, setError] = useState<BrowseError | null>(null)
  const [moreError, setMoreError] = useState<string | null>(null)

  const session = useRef<string | null>(null)
  const page = useRef(0)
  const count = useRef(0)
  const filterRef = useRef(filter)
  const generation = useRef(0)
  const inFlight = useRef(false)
  const stopped = useRef(true)

  /** Applies one page: a restarted session replaces the list, any other page appends. */
  const apply = useCallback(
    (listing: BrowseListing) => {
      session.current = listing.session
      page.current = listing.page
      memoryRef.current.current = { session: listing.session, filter: filterRef.current, at: Date.now() }
      setItems((prev) => {
        const base = listing.restarted || listing.page === 0 ? [] : prev
        const have = new Set(base.map(key))
        const next = base.concat(listing.items.filter((t) => !have.has(key(t)))).slice(0, MAX_ITEMS)
        count.current = next.length
        return next
      })
      setStatuses(listing.sources)
      setSources((known) => (filterRef.current === null ? mergeSources(listing.sources, known) : mergeSources(known, listing.sources)))
      setDone(listing.done)
      stopped.current = listing.done
    },
    [],
  )

  /** (Re)starts at page 0 of `from` (a session to resume, or `null` for a new one). */
  const start = useCallback(
    (source: string | null, from: string | null) => {
      const gen = ++generation.current
      filterRef.current = source
      session.current = from
      page.current = 0
      count.current = 0
      inFlight.current = true
      stopped.current = true
      setItems([])
      setDone(false)
      setError(null)
      setMoreError(null)
      setLoading(true)
      setLoadingMore(false)
      browsePage({ session: from, page: 0, source })
        .then((listing) => {
          if (gen !== generation.current) return
          apply(listing)
        })
        .catch((err: unknown) => {
          if (gen === generation.current) setError(toError(err))
        })
        .finally(() => {
          if (gen !== generation.current) return
          inFlight.current = false
          setLoading(false)
        })
    },
    [apply],
  )

  useEffect(() => {
    start(resume?.filter ?? null, resume?.session ?? null)
    return () => {
      // Answers after unmount are dropped.
      generation.current++
    }
  }, [start, resume])

  const loadMore = useCallback(() => {
    if (inFlight.current || stopped.current || session.current === null || count.current >= MAX_ITEMS) return
    inFlight.current = true
    setLoadingMore(true)
    setMoreError(null)
    const gen = generation.current
    browsePage({ session: session.current, page: page.current + 1, source: filterRef.current })
      .then((listing) => {
        if (gen === generation.current) apply(listing)
      })
      .catch((err: unknown) => {
        if (gen === generation.current) setMoreError(errorMessage(err))
      })
      .finally(() => {
        if (gen !== generation.current) return
        inFlight.current = false
        setLoadingMore(false)
      })
  }, [apply])

  const reload = useCallback(() => start(filterRef.current, null), [start])

  const setFilter = useCallback(
    (next: string | null) => {
      if (next === filterRef.current) return
      setFilterState(next)
      start(next, null)
    },
    [start],
  )

  const cycleFilter = useCallback(
    (delta: number) => {
      if (sources.length < 2) return
      const options = [null, ...sources.map((s) => s.source_id)]
      const at = Math.max(0, options.indexOf(filterRef.current))
      const to = (((at + delta) % options.length) + options.length) % options.length
      setFilter(options[to] ?? null)
    },
    [sources, setFilter],
  )

  return {
    items,
    statuses,
    sources,
    filter,
    loading,
    loadingMore,
    done,
    capped: items.length >= MAX_ITEMS,
    error,
    moreError,
    loadMore,
    reload,
    setFilter,
    cycleFilter,
  }
}
