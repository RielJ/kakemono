import { useRef, type KeyboardEvent } from 'react'

/**
 * A radio group drawn as a segmented control. Arrow keys move the selection (and focus),
 * as with native radios; only the selected segment is in the tab order.
 */
export default function Segmented<T extends string>({
  value,
  options,
  onChange,
  labelledBy,
  describedBy,
  label,
  disabled,
}: {
  value: T
  options: readonly { value: T; label: string }[]
  onChange: (value: T) => void
  labelledBy?: string
  describedBy?: string
  label?: string
  disabled?: boolean
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([])
  const index = options.findIndex((o) => o.value === value)

  const onKeyDown = (e: KeyboardEvent) => {
    const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0
    if (!step) return
    e.preventDefault()
    const next = (Math.max(index, 0) + step + options.length) % options.length
    onChange(options[next].value)
    refs.current[next]?.focus()
  }

  return (
    <div
      role="radiogroup"
      aria-labelledby={labelledBy}
      aria-label={labelledBy ? undefined : label}
      aria-describedby={describedBy}
      onKeyDown={onKeyDown}
      className={`inline-flex p-0.5 gap-0.5 rounded-lg bg-pressed ${disabled ? 'opacity-45 pointer-events-none' : ''}`}
    >
      {options.map((o, i) => {
        const selected = o.value === value
        return (
          <button
            key={o.value}
            ref={(el) => {
              refs.current[i] = el
            }}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected || (index < 0 && i === 0) ? 0 : -1}
            disabled={disabled}
            onClick={() => onChange(o.value)}
            className={[
              'h-6 px-3 rounded-md text-[12px] font-medium transition-colors duration-100',
              selected ? 'bg-raised shadow-raised text-ink' : 'text-muted hover:text-ink',
            ].join(' ')}
          >
            {o.label}
          </button>
        )
      })}
    </div>
  )
}
