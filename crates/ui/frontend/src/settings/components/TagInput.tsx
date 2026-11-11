import { useId, useState } from 'react'

import Icon from './Icon'

/**
 * Editable list of short strings (subreddits, folders, search terms) drawn as tokens inside
 * one field: type and press Enter or comma to add, Backspace on an empty field removes the
 * last one.
 */
export default function TagInput({
  values,
  onChange,
  label,
  placeholder,
  describedBy,
  normalize = (v) => v,
}: {
  values: readonly string[]
  onChange: (v: string[]) => void
  /** Accessible name of the text field, e.g. "Add subreddit". */
  label: string
  placeholder?: string
  describedBy?: string
  /** Cleans an entry before it is added, e.g. strips `r/`. */
  normalize?: (v: string) => string
}) {
  const [input, setInput] = useState('')
  const listId = useId()

  const add = () => {
    const v = normalize(input.trim())
    if (v && !values.includes(v)) onChange([...values, v])
    setInput('')
  }

  return (
    <div className="flex flex-wrap items-center gap-1 min-h-8 px-1 py-1 rounded-md bg-field border border-line-strong focus-within:border-accent focus-within:shadow-[0_0_0_3px_var(--c-accent-soft)] transition-[border-color,box-shadow] duration-100">
      {values.length > 0 && (
        <ul id={listId} className="contents" aria-label={label.replace(/^Add /, '')}>
          {values.map((v) => (
            <li
              key={v}
              className="inline-flex items-center gap-0.5 h-6 pl-2 pr-0.5 rounded-[5px] bg-pressed max-w-full"
            >
              <span className="truncate text-[12px]">{v}</span>
              <button
                type="button"
                onClick={() => onChange(values.filter((x) => x !== v))}
                className="inline-flex items-center justify-center w-5 h-5 rounded text-muted hover:text-ink hover:bg-hover"
                aria-label={`Remove ${v}`}
              >
                <Icon name="x" className="w-3 h-3" />
              </button>
            </li>
          ))}
        </ul>
      )}
      <input
        type="text"
        value={input}
        onChange={(e) => setInput(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ',') {
            e.preventDefault()
            add()
          } else if (e.key === 'Backspace' && input === '' && values.length > 0) {
            onChange(values.slice(0, -1))
          }
        }}
        onBlur={add}
        placeholder={values.length === 0 ? (placeholder ?? 'Add…') : 'Add…'}
        aria-label={label}
        aria-describedby={describedBy}
        aria-controls={values.length > 0 ? listId : undefined}
        spellCheck={false}
        autoComplete="off"
        className="flex-1 min-w-24 h-6 px-1.5 bg-transparent outline-none text-[13px] placeholder:text-faint"
      />
    </div>
  )
}
