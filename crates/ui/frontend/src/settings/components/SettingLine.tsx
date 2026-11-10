import { createContext, useContext, useEffect, useId, useRef, useState, type ReactNode } from 'react'

import { parseGoDuration } from '../duration'

/** The id of the checkbox that a line's sentence text labels. */
const LineCheckbox = createContext<string | null>(null)

/** Hint, error and warning text under a line, linked to its control with `aria-describedby`. */
function LineNotes({ id, hint, error, warning }: { id: string; hint?: ReactNode; error?: string | null; warning?: string | null }) {
  return (
    <div id={id} className="mt-0.5 text-[12px] leading-snug">
      {hint !== undefined && <p className="text-muted">{hint}</p>}
      {warning && (
        <p role="status" className="text-warn">
          {warning}
        </p>
      )}
      {error && (
        <p role="alert" className="text-danger">
          {error}
        </p>
      )}
    </div>
  )
}

/**
 * One General-tab setting written as a sentence: a checkbox, then text (`LineText`) and inline
 * controls in reading order, e.g. "☑ Change wallpaper every [30] [minutes]". Every `LineText`
 * is a `<label>` for the checkbox, so clicking the sentence ticks it. The inline controls stay
 * editable while the box is unticked; they only change their own key.
 */
export function CheckLine({
  checked,
  onChange,
  disabled = false,
  hint,
  error,
  warning,
  children,
}: {
  checked: boolean
  onChange: (checked: boolean) => void
  disabled?: boolean
  hint?: ReactNode
  /** Shown in the danger colour, e.g. a failed command. */
  error?: string | null
  /** Shown in the warning colour; the value is still saved. */
  warning?: string | null
  children: ReactNode
}) {
  const id = useId()
  const notesId = useId()
  const describes = hint !== undefined || Boolean(error) || Boolean(warning)
  return (
    <div className="flex gap-2.5 px-4 py-2 min-h-11">
      <div className="h-7 flex items-center shrink-0">
        <input
          id={id}
          type="checkbox"
          checked={checked}
          disabled={disabled}
          onChange={(e) => onChange(e.target.checked)}
          aria-describedby={describes ? notesId : undefined}
          className="size-4 m-0 disabled:opacity-45"
        />
      </div>
      <div className="flex-1 min-w-0">
        <LineCheckbox.Provider value={id}>
          <div className="min-h-7 flex flex-wrap items-center gap-x-2 gap-y-1">{children}</div>
        </LineCheckbox.Provider>
        {describes && <LineNotes id={notesId} hint={hint} error={error} warning={warning} />}
      </div>
    </div>
  )
}

/** Sentence text in a `CheckLine`: a label for its checkbox. */
export function LineText({ children }: { children: ReactNode }) {
  const id = useContext(LineCheckbox)
  if (id === null) return <span>{children}</span>
  return <label htmlFor={id}>{children}</label>
}

/**
 * A setting line without a checkbox, e.g. "Favorites folder [path] Choose… Open".
 * `children` receives the label id for the control's `aria-labelledby`.
 */
export function PlainLine({
  label,
  hint,
  children,
}: {
  label: string
  hint?: ReactNode
  children: (labelId: string) => ReactNode
}) {
  const labelId = useId()
  const notesId = useId()
  return (
    <div className="px-4 py-2 min-h-11">
      <div className="min-h-7 flex flex-wrap items-center gap-x-2 gap-y-1">
        <span id={labelId}>{label}</span>
        {children(labelId)}
      </div>
      {hint !== undefined && <LineNotes id={notesId} hint={hint} />}
    </div>
  )
}

/**
 * A number typed into a sentence. Applies on Enter or blur, not on every key, so a half-typed
 * value is never saved; `parse` turns the text into the value to save, or `null` to put the
 * last saved value back.
 */
export function NumberField({
  value,
  onCommit,
  parse,
  label,
  min,
  className = 'w-16',
}: {
  value: number | null
  onCommit: (value: number) => void
  parse: (text: string) => number | null
  label: string
  min?: number
  className?: string
}) {
  const shown = value === null ? '' : String(value)
  const [draft, setDraft] = useState(shown)
  const focused = useRef(false)

  // Follow outside changes (a reload, a unit switch) unless the user is typing.
  useEffect(() => {
    if (!focused.current) setDraft(shown)
  }, [shown])

  const commit = () => {
    if (draft === shown) return
    const next = parse(draft)
    if (next === null) setDraft(shown)
    else {
      setDraft(String(next))
      if (next !== value) onCommit(next)
    }
  }

  return (
    <input
      type="number"
      inputMode="numeric"
      min={min}
      aria-label={label}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onFocus={() => {
        focused.current = true
      }}
      onBlur={() => {
        focused.current = false
        commit()
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault()
          commit()
        } else if (e.key === 'Escape') setDraft(shown)
      }}
      className={`field tabular-nums text-right ${className}`}
    />
  )
}

/** A whole number at least `min`, rounded; anything else is rejected. */
export function wholeAtLeast(min: number) {
  return (text: string): number | null => {
    const n = Number(text)
    if (text.trim() === '' || !Number.isFinite(n)) return null
    return Math.max(min, Math.round(n))
  }
}

export type IntervalUnit = 'minutes' | 'hours' | 'days'

const MINUTE_NS = 60_000_000_000n
const UNIT_NS: Record<IntervalUnit, bigint> = {
  minutes: MINUTE_NS,
  hours: 60n * MINUTE_NS,
  days: 24n * 60n * MINUTE_NS,
}

/**
 * A Go duration as a count and a unit: the largest unit that divides it evenly. A duration
 * that isn't whole minutes (e.g. `"90s"`) shows as fractional minutes; an invalid one as no
 * count.
 */
export function splitInterval(value: string): { count: number | null; unit: IntervalUnit } {
  const parsed = parseGoDuration(value)
  if (!parsed.ok || parsed.nanos === 0n) return { count: null, unit: 'minutes' }
  const { nanos } = parsed
  for (const unit of ['days', 'hours', 'minutes'] as const) {
    if (nanos % UNIT_NS[unit] === 0n) return { count: Number(nanos / UNIT_NS[unit]), unit }
  }
  return { count: Number(nanos) / Number(MINUTE_NS), unit: 'minutes' }
}

/** The Go duration for a count and unit. Days become hours: Go durations have no day unit. */
export function joinInterval(count: number, unit: IntervalUnit): string {
  switch (unit) {
    case 'minutes':
      return `${count}m`
    case 'hours':
      return `${count}h`
    case 'days':
      return `${count * 24}h`
    default: {
      const _exhaustive: never = unit
      return _exhaustive
    }
  }
}

const UNITS: readonly IntervalUnit[] = ['minutes', 'hours', 'days']

/** "[N] [minutes|hours|days]" for a Go duration key such as `daemon.interval`. */
export function IntervalField({ value, onChange, label }: { value: string; onChange: (value: string) => void; label: string }) {
  const { count, unit } = splitInterval(value)
  return (
    <span className="inline-flex items-center gap-1.5">
      <NumberField
        value={count}
        onCommit={(n) => onChange(joinInterval(n, unit))}
        parse={wholeAtLeast(1)}
        label={label}
        min={1}
      />
      <select
        value={unit}
        onChange={(e) => {
          const next = UNITS.find((u) => u === e.target.value)
          if (next) onChange(joinInterval(Math.max(1, Math.round(count ?? 1)), next))
        }}
        aria-label={`${label}, unit`}
        className="field w-[6.5rem]"
      >
        {UNITS.map((u) => (
          <option key={u} value={u}>
            {u}
          </option>
        ))}
      </select>
    </span>
  )
}
