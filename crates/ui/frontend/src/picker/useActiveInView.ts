import { useEffect, useRef } from 'react'

/**
 * Scrolls the active option (the element with `id`) into view when it becomes a different
 * element: a key move, a click, or the first render of a new list. A page appended to the
 * strip keeps the same keyed element, so the strip stays where the user scrolled it instead
 * of jumping back to the active card.
 */
export function useActiveInView(id: string): void {
  const shown = useRef<Element | null>(null)
  // No deps: a cheap lookup each render, and a re-keyed list swaps the element without
  // changing `id`.
  useEffect(() => {
    const el = document.getElementById(id)
    if (!el || el === shown.current) return
    shown.current = el
    el.scrollIntoView?.({ block: 'nearest', inline: 'nearest' })
  })
}
