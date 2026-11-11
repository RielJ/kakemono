/**
 * Infinite-scroll trigger: calls `onVisible` when the sentinel element (the last slot of a
 * strip) scrolls into view, so the caller can load the next page.
 *
 * - One call per intersection: the sentinel must leave view before it fires again.
 * - Nothing fires while `busy` (a page is loading). When `busy` clears, a fresh observer
 *   reports the sentinel's current state, so a page too short to push the sentinel out of view
 *   still loads the next one. This keeps at most one page request in flight.
 *
 * The sentinel element must be mounted when the effect runs (`ref.current` is read when
 * `busy` or `rootMargin` change, not on every render).
 */
import { useEffect, useRef, type RefObject } from 'react'

export interface SentinelOptions {
  onVisible: () => void
  busy: boolean
  /** Grows the viewport so the next page loads before the sentinel is on screen. */
  rootMargin?: string
}

export function useSentinel(
  ref: RefObject<Element | null>,
  { onVisible, busy, rootMargin = '0px' }: SentinelOptions,
): void {
  const latest = useRef(onVisible)
  useEffect(() => {
    latest.current = onVisible
  })

  useEffect(() => {
    const target = ref.current
    if (busy || !target || typeof IntersectionObserver === 'undefined') return
    let inView = false
    const observer = new IntersectionObserver(
      (entries) => {
        // One target, so the newest entry is its state. (No `Array.at`: older WebKit lacks it.)
        const now = entries[entries.length - 1]?.isIntersecting ?? inView
        if (now && !inView) latest.current()
        inView = now
      },
      { rootMargin },
    )
    observer.observe(target)
    return () => observer.disconnect()
  }, [ref, busy, rootMargin])
}
