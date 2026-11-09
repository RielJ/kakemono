import { useId, useState } from 'react'

import Icon from './Icon'

/** Editable list of short strings (subreddits, folders, search queries). */
export default function TagInput({
  values,
  onChange,
  label,
  placeholder,
  emptyText,
}: {
  values: string[]
  onChange: (v: string[]) => void
  /** Accessible name of the text field, e.g. "Add subreddit". */
  label: string
  placeholder?: string
  emptyText?: string
}) {
  const [input, setInput] = useState('')
  const listId = useId()
  const add = () => {
    const v = input.trim()
    if (v && !values.includes(v)) onChange([...values, v])
    setInput('')
  }
  return (
    <div className="space-y-2">
      {values.length === 0 && emptyText ? (
        <p className="text-xs text-gray-500 dark:text-gray-400">{emptyText}</p>
      ) : (
        <ul id={listId} className="flex flex-wrap gap-1.5" aria-label={label.replace(/^Add /, '')}>
          {values.map((v) => (
            <li
              key={v}
              className="inline-flex items-center gap-1 pl-2 pr-1 py-0.5 rounded-full text-xs bg-gray-100 text-gray-700 dark:bg-gray-700 dark:text-gray-200 max-w-full"
            >
              <span className="truncate">{v}</span>
              <button
                type="button"
                onClick={() => onChange(values.filter((x) => x !== v))}
                className="rounded-full p-0.5 text-gray-500 hover:text-red-600 dark:text-gray-400 dark:hover:text-red-400 focus-visible:outline-2 focus-visible:outline-accent"
                aria-label={`Remove ${v}`}
              >
                <Icon name="x" className="w-3 h-3" />
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex gap-2">
        <input
          type="text"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              add()
            }
          }}
          placeholder={placeholder ?? 'Add…'}
          aria-label={label}
          aria-controls={listId}
          spellCheck={false}
          className="form-input flex-1 text-xs py-1"
        />
        <button type="button" onClick={add} disabled={input.trim() === ''} className="btn-secondary text-xs py-1 px-2">
          Add
        </button>
      </div>
    </div>
  )
}
