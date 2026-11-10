/**
 * The picker tab bar (plan "Picker layout and keys"): five icon-only tabs that pick what the
 * strip shows.
 *
 * Horizontal picker: a 32 px column on the left that also carries the drag handle (the column
 * background is the drag region), so the tabs run top to bottom. Vertical picker: a 32 px row
 * under the existing grip row, so the tabs run left to right.
 *
 * Tabs activate on focus: an arrow key inside the tablist moves focus and selection together.
 */
import { useRef } from 'react'

import { nextTab, TABS, type PickerTabId } from '../tabs'
import Icon from './Icon'

/** The `id` of the `role="tabpanel"` that wraps the strip. */
export const PANEL_ID = 'picker-panel'

export interface TabBarProps {
  active: PickerTabId
  onSelect: (id: PickerTabId) => void
  /** The picker strip's orientation; the tabs run across it. */
  vertical: boolean
}

export default function TabBar({ active, onSelect, vertical }: TabBarProps) {
  const buttons = useRef(new Map<PickerTabId, HTMLButtonElement>())
  // The tabs run across the strip: a column beside a horizontal strip, a row above a vertical one.
  const orientation = vertical ? 'horizontal' : 'vertical'
  const next = vertical ? 'ArrowRight' : 'ArrowDown'
  const prev = vertical ? 'ArrowLeft' : 'ArrowUp'

  const select = (id: PickerTabId) => {
    buttons.current.get(id)?.focus()
    onSelect(id)
  }

  const targets: Record<string, PickerTabId> = {
    [next]: nextTab(active, 1),
    [prev]: nextTab(active, -1),
    Home: TABS[0].id,
    End: TABS[TABS.length - 1].id,
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    const to = e.ctrlKey || e.metaKey || e.altKey ? undefined : targets[e.key]
    if (to === undefined) return
    e.preventDefault()
    select(to)
  }

  return (
    <div className={`tabbar ${vertical ? 'tabbar-row' : 'tabbar-column'}`} data-tauri-drag-region>
      {!vertical && (
        <div className="tabbar-grip" data-tauri-drag-region role="img" aria-label="Move picker" title="Move picker">
          <Icon name="gripHorizontal" />
        </div>
      )}
      <div
        className="tablist"
        role="tablist"
        aria-label="Wallpaper sources"
        aria-orientation={orientation}
        data-tauri-drag-region
        onKeyDown={onKeyDown}
      >
        {TABS.map((tab) => {
          const selected = tab.id === active
          return (
            <button
              key={tab.id}
              ref={(el) => {
                if (el) buttons.current.set(tab.id, el)
                else buttons.current.delete(tab.id)
              }}
              type="button"
              className="tab"
              role="tab"
              id={`picker-tab-${tab.id}`}
              aria-label={tab.label}
              title={tab.label}
              aria-selected={selected}
              aria-controls={PANEL_ID}
              tabIndex={selected ? 0 : -1}
              onClick={() => onSelect(tab.id)}
            >
              <Icon name={tab.icon} />
            </button>
          )
        })}
      </div>
    </div>
  )
}
