import { useRef, useState, type KeyboardEvent } from 'react'

import type { SourceType } from '../../shared/bindings/SourceType'
import { LOCAL_TYPES, ONLINE_TYPES, SOURCE_TYPES } from '../sourceTypes'
import Modal from './Modal'

/**
 * The Add… button and its "Add a source" dialog: local types on the left, online types A–Z
 * on the right, each a button with a bold name and a one-line description. Online types are
 * hidden while Internet access is off. A modal, centred in the window, so the whole list is
 * visible wherever the Sources table is scrolled to.
 *
 * Keys: Up/Down move through the choices (wrapping), Left/Right switch column, Home/End jump
 * to the ends, Enter or Space chooses. Escape, Cancel or a click outside closes it.
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
  const grid = useRef<HTMLDivElement>(null)

  const choose = (kind: SourceType) => {
    setOpen(false)
    onChoose(kind)
  }

  const onGridKeyDown = (e: KeyboardEvent) => {
    const root = grid.current
    if (!root) return
    const columns = [...root.querySelectorAll<HTMLElement>('[data-column]')].map((c) => [
      ...c.querySelectorAll<HTMLElement>('[data-choice]'),
    ])
    const flat = columns.flat()
    const at = flat.indexOf(document.activeElement as HTMLElement)
    if (at < 0) return
    const go = (el: HTMLElement | undefined) => {
      e.preventDefault()
      el?.focus()
    }
    const col = columns.findIndex((c) => c.includes(flat[at]))
    const row = columns[col].indexOf(flat[at])
    switch (e.key) {
      case 'ArrowDown':
        return go(flat[(at + 1) % flat.length])
      case 'ArrowUp':
        return go(flat[(at - 1 + flat.length) % flat.length])
      case 'ArrowRight':
      case 'ArrowLeft': {
        const other = columns[col + (e.key === 'ArrowRight' ? 1 : -1)]
        if (!other) return
        return go(other[Math.min(row, other.length - 1)])
      }
      case 'Home':
        return go(flat[0])
      case 'End':
        return go(flat[flat.length - 1])
    }
  }

  const choice = (kind: SourceType, first: boolean) => {
    const info = SOURCE_TYPES[kind]
    return (
      <button
        key={kind}
        type="button"
        className="add-choice"
        data-choice
        data-autofocus={first || undefined}
        onClick={() => choose(kind)}
      >
        <strong className="font-semibold">{info.menuLabel}</strong>
        <span className="add-choice-desc">{info.description}</span>
      </button>
    )
  }

  return (
    <>
      <button type="button" className="btn" aria-haspopup="dialog" disabled={disabled} onClick={() => setOpen(true)}>
        Add…
      </button>
      {open && (
        <Modal title="Add a source" onClose={() => setOpen(false)} className={internet ? 'modal-wide' : undefined}>
          {/* oxlint-disable-next-line jsx-a11y/no-static-element-interactions -- arrow keys move between the choice buttons inside */}
          <div ref={grid} className="add-grid" data-columns={internet ? 2 : 1} onKeyDown={onGridKeyDown}>
            <section data-column aria-label="On this Mac">
              <h3 className="add-group">On this Mac</h3>
              {LOCAL_TYPES.map((k, i) => choice(k, i === 0))}
            </section>
            {internet && (
              <section data-column aria-label="Online">
                <h3 className="add-group">Online</h3>
                {ONLINE_TYPES.map((k) => choice(k, false))}
              </section>
            )}
          </div>
          <div className="mt-3 flex justify-end">
            <button type="button" className="btn" onClick={() => setOpen(false)}>
              Cancel
            </button>
          </div>
        </Modal>
      )}
    </>
  )
}
