import { useEffect, useLayoutEffect, useRef, useState } from 'react'

export interface MenuItem {
  label: string
  onSelect: () => void
  disabled?: boolean
  /** Why it is disabled; shown as a tooltip and read as the description. */
  hint?: string
}

interface Props {
  /** Where the menu opens (viewport px); it is moved inward to fit the window. */
  x: number
  y: number
  label: string
  items: MenuItem[]
  /** Called on Esc, Tab, a click outside, or after an item runs. */
  onClose: () => void
}

const MARGIN = 4

/**
 * A keyboard-operable menu: focus starts on the first enabled item, ↑/↓ wrap, Home/End jump,
 * Enter or Space runs an item, Esc or Tab closes. The caller restores focus on close.
 */
export default function ContextMenu({ x, y, label, items, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState({ left: x, top: y })

  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const { width, height } = el.getBoundingClientRect()
    const left = Math.max(MARGIN, Math.min(x, window.innerWidth - width - MARGIN))
    const top = Math.max(MARGIN, Math.min(y, window.innerHeight - height - MARGIN))
    setPos({ left, top })
    el.querySelector<HTMLButtonElement>('[role="menuitem"]:not([aria-disabled="true"])')?.focus()
  }, [x, y])

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && e.target instanceof Node && !ref.current.contains(e.target)) onClose()
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [onClose])

  const onKeyDown = (e: React.KeyboardEvent) => {
    const buttons = Array.from(ref.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [])
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement)
    const focus = (i: number) => buttons[(i + buttons.length) % buttons.length]?.focus()
    switch (e.key) {
      case 'ArrowDown':
        focus(at + 1)
        break
      case 'ArrowUp':
        focus(at < 0 ? -1 : at - 1)
        break
      case 'Home':
        focus(0)
        break
      case 'End':
        focus(-1)
        break
      case 'Escape':
      case 'Tab':
        onClose()
        break
      default:
        return
    }
    // Keys the menu handles never reach the strip or the window's Esc handler.
    e.preventDefault()
    e.stopPropagation()
  }

  return (
    <div
      ref={ref}
      className="menu"
      role="menu"
      aria-label={label}
      style={{ left: pos.left, top: pos.top }}
      onKeyDown={onKeyDown}
    >
      {items.map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          className="menu-item"
          aria-disabled={item.disabled ? true : undefined}
          title={item.disabled ? item.hint : undefined}
          onClick={() => {
            if (item.disabled) return
            onClose()
            item.onSelect()
          }}
        >
          {item.label}
        </button>
      ))}
    </div>
  )
}
