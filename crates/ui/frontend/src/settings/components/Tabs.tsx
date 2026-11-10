import { useRef, type KeyboardEvent } from 'react'

/** The DOM id of a tab button, for the panel's `aria-labelledby`. */
export const tabId = (id: string) => `tab-${id}`

/**
 * A horizontal tablist drawn as a segmented control (the `Segmented` look, one size up).
 * Left/Right move between tabs and select them, Home/End jump to the ends; only the
 * selected tab is in the tab order. Every tab controls the one panel `panelId`.
 */
export default function Tabs<T extends string>({
  tabs,
  value,
  onChange,
  panelId,
  label,
}: {
  tabs: readonly { id: T; title: string }[]
  value: T
  onChange: (id: T) => void
  panelId: string
  label: string
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([])

  const onKeyDown = (e: KeyboardEvent, index: number) => {
    let next = -1
    if (e.key === 'ArrowRight') next = (index + 1) % tabs.length
    else if (e.key === 'ArrowLeft') next = (index - 1 + tabs.length) % tabs.length
    else if (e.key === 'Home') next = 0
    else if (e.key === 'End') next = tabs.length - 1
    if (next < 0) return
    e.preventDefault()
    onChange(tabs[next].id)
    refs.current[next]?.focus()
  }

  return (
    <div
      role="tablist"
      aria-orientation="horizontal"
      aria-label={label}
      className="inline-flex p-0.5 gap-0.5 rounded-lg bg-pressed"
    >
      {tabs.map((tab, i) => {
        const selected = tab.id === value
        return (
          <button
            key={tab.id}
            ref={(el) => {
              refs.current[i] = el
            }}
            id={tabId(tab.id)}
            type="button"
            role="tab"
            aria-selected={selected}
            aria-controls={panelId}
            tabIndex={selected ? 0 : -1}
            onClick={() => onChange(tab.id)}
            onKeyDown={(e) => onKeyDown(e, i)}
            className={[
              'h-7 min-w-24 px-4 rounded-md text-[13px] font-medium transition-colors duration-100',
              selected ? 'bg-raised shadow-raised text-ink' : 'text-muted hover:text-ink',
            ].join(' ')}
          >
            {tab.title}
          </button>
        )
      })}
    </div>
  )
}
