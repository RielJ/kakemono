import { useEffect, useId, useRef, useState, type KeyboardEvent } from 'react'

import type { SourceType } from '../../shared/bindings/SourceType'
import { LOCAL_TYPES, ONLINE_TYPES, SOURCE_TYPES } from '../sourceTypes'

/**
 * The Add… button and its menu: local types, a separator, then online types A–Z, each with a
 * bold name and a one-line description. Online types are hidden while Internet access is off.
 * A WAI-ARIA menu button: arrows, Home/End, Enter or Space to choose, Escape or Tab to close.
 */
export default function AddSourceMenu({
  internet,
  onChoose,
  disabled = false,
}: {
  internet: boolean
  onChoose: (kind: SourceType) => void
  disabled?: boolean
}) {
  const [open, setOpen] = useState(false)
  // Which item takes focus when the menu opens: the first, or the last (ArrowUp on the button).
  const [startAt, setStartAt] = useState<'first' | 'last'>('first')
  const menuId = useId()
  const triggerId = useId()
  const wrap = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const menu = useRef<HTMLDivElement>(null)

  const items = () => [...(menu.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])]

  useEffect(() => {
    if (!open) return
    const list = items()
    ;(startAt === 'last' ? list[list.length - 1] : list[0])?.focus({ preventScroll: true })
    menu.current?.scrollIntoView?.({ block: 'nearest' })
    const onDown = (e: MouseEvent) => {
      if (e.target instanceof Node && !wrap.current?.contains(e.target)) setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [open, startAt])

  const show = (at: 'first' | 'last') => {
    setStartAt(at)
    setOpen(true)
  }

  const close = (refocus: boolean) => {
    setOpen(false)
    if (refocus) trigger.current?.focus()
  }

  const choose = (kind: SourceType) => {
    close(true)
    onChoose(kind)
  }

  const onTriggerKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      show(e.key === 'ArrowUp' ? 'last' : 'first')
    }
  }

  const onMenuKeyDown = (e: KeyboardEvent) => {
    const list = items()
    const at = list.indexOf(document.activeElement as HTMLElement)
    const go = (i: number) => {
      e.preventDefault()
      list[(i + list.length) % list.length]?.focus()
    }
    switch (e.key) {
      case 'ArrowDown':
        return go(at + 1)
      case 'ArrowUp':
        return go(at - 1)
      case 'Home':
        return go(0)
      case 'End':
        return go(list.length - 1)
      case 'Escape':
        e.preventDefault()
        e.stopPropagation()
        return close(true)
      case 'Tab':
        return close(false)
    }
  }

  const item = (kind: SourceType) => {
    const info = SOURCE_TYPES[kind]
    return (
      <button
        key={kind}
        type="button"
        role="menuitem"
        tabIndex={-1}
        className="menu-item"
        onClick={() => choose(kind)}
      >
        <strong className="font-semibold">{info.menuLabel}</strong>
        <span className="menu-desc" title={info.description}>
          {info.description}
        </span>
      </button>
    )
  }

  return (
    <div ref={wrap} className="relative">
      <button
        ref={trigger}
        id={triggerId}
        type="button"
        className="btn"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        disabled={disabled}
        onClick={() => (open ? close(false) : show('first'))}
        onKeyDown={onTriggerKeyDown}
      >
        Add…
      </button>
      {open && (
        <div ref={menu} id={menuId} role="menu" aria-label="Add a source" className="menu-popover" onKeyDown={onMenuKeyDown}>
          {LOCAL_TYPES.map(item)}
          {internet && <div role="separator" className="menu-separator" />}
          {internet && ONLINE_TYPES.map(item)}
        </div>
      )}
    </div>
  )
}
