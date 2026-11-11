import type { PointerEvent as ReactPointerEvent } from 'react'

import { pickerResize, pickerResizeBegin, type StripEnd } from '../../shared/api'

/**
 * Thin handles on both ends of the strip that resize the picker along it: left/right when
 * horizontal, top/bottom when vertical. Rust keeps the window on the screen and at least the
 * minimum length (`picker.rs`, `picker_resize`).
 */
export default function ResizeHandles({ vertical }: { vertical: boolean }) {
  return (
    <>
      <Handle end="start" vertical={vertical} />
      <Handle end="end" vertical={vertical} />
    </>
  )
}

function Handle({ end, vertical }: { end: StripEnd; vertical: boolean }) {
  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return
    e.preventDefault()
    // Screen coordinates: the window itself moves while the start end is dragged.
    const at = (ev: PointerEvent | ReactPointerEvent) => (vertical ? ev.screenY : ev.screenX)
    const origin = at(e)
    // One call in flight at a time; the newest delta wins, so a fast drag never queues up.
    let pending: number | null = null
    let busy = true
    const flush = () => {
      if (pending === null) {
        busy = false
        return
      }
      const delta = pending
      pending = null
      busy = true
      void pickerResize(end, delta).catch(() => {}).finally(flush)
    }
    void pickerResizeBegin().catch(() => {}).finally(flush)

    const onMove = (ev: PointerEvent) => {
      pending = at(ev) - origin
      if (!busy) flush()
    }
    const onUp = () => {
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', onUp)
      window.removeEventListener('pointercancel', onUp)
    }
    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', onUp)
    window.addEventListener('pointercancel', onUp)
  }

  const side = vertical ? (end === 'start' ? 'top' : 'bottom') : end === 'start' ? 'left' : 'right'
  return (
    <div
      className={`resize-handle resize-${side}`}
      data-testid={`resize-${side}`}
      aria-hidden="true"
      title="Drag to resize"
      onPointerDown={onPointerDown}
    />
  )
}
