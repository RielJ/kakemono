/**
 * The picker key map (plan §H): pure, so it is tested without a DOM.
 *
 * ←/→ move (↑/↓ when the strip is vertical), Home/End jump, Enter or Space sets, F favourites,
 * Delete or Backspace trashes, Shift+F10 or the Menu key opens the context menu, Esc closes,
 * ⌘Z / Ctrl+Z undoes a pending trash.
 */
export type KeyAction =
  | { kind: 'move'; to: number }
  | { kind: 'set' }
  | { kind: 'favorite' }
  | { kind: 'trash' }
  | { kind: 'menu' }
  | { kind: 'close' }
  | { kind: 'undo' }

export interface KeyInput {
  key: string
  shiftKey?: boolean
  altKey?: boolean
  ctrlKey?: boolean
  metaKey?: boolean
}

/** What `event` does in a strip of `count` items with `active` focused, or `null`. */
export function keyAction(event: KeyInput, vertical: boolean, active: number, count: number): KeyAction | null {
  const { key, shiftKey = false, altKey = false, ctrlKey = false, metaKey = false } = event
  const command = ctrlKey || metaKey
  if (command && !altKey && !shiftKey && key.toLowerCase() === 'z') return { kind: 'undo' }
  if (key === 'Escape') return { kind: 'close' }
  if (key === 'ContextMenu' || (shiftKey && key === 'F10')) return count > 0 ? { kind: 'menu' } : null
  if (command || altKey || count === 0) return null

  const next = vertical ? 'ArrowDown' : 'ArrowRight'
  const prev = vertical ? 'ArrowUp' : 'ArrowLeft'
  const last = count - 1
  const clamp = (i: number) => Math.max(0, Math.min(last, i))
  switch (key) {
    case next:
      return { kind: 'move', to: clamp(active + 1) }
    case prev:
      return { kind: 'move', to: clamp(active - 1) }
    case 'Home':
      return { kind: 'move', to: 0 }
    case 'End':
      return { kind: 'move', to: last }
    case 'Enter':
    case ' ':
      return shiftKey ? null : { kind: 'set' }
    case 'f':
    case 'F':
      return shiftKey && key === 'f' ? null : { kind: 'favorite' }
    case 'Delete':
    case 'Backspace':
      return { kind: 'trash' }
    default:
      return null
  }
}
