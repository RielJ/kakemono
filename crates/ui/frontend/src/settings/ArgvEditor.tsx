import { useEffect, useId, useRef, useState } from 'react'

import Icon from './components/Icon'
import { PLACEHOLDERS } from './argv'

type Part = 'input' | 'up' | 'down' | 'add'

/**
 * Edits an argv one token per row: add, remove and reorder rows, and append a placeholder
 * token. Tokens are never joined or split, so a token with spaces stays one argument and
 * there is no quoting to get wrong. No rows reads as `[]`; the caller decides what that means.
 */
export default function ArgvEditor({
  label,
  tokens,
  onChange,
  error,
  hintId,
  emptyText,
}: {
  /** Names the list and prefixes each row's accessible name, e.g. "Wallpaper command". */
  label: string
  tokens: readonly string[]
  onChange: (tokens: string[]) => void
  error?: string | null
  /** Id of the text that describes this command, wired to every row. */
  hintId?: string
  emptyText: string
}) {
  const listId = useId()
  const errorId = useId()
  const refs = useRef(new Map<string, HTMLElement>())
  const [focus, setFocus] = useState<{ index: number; part: Part } | null>(null)

  // Move focus after the rows re-render, so keyboard users stay on the row they moved or added.
  useEffect(() => {
    if (!focus) return
    const want = refs.current.get(`${focus.index}:${focus.part}`)
    const target = want && !(want instanceof HTMLButtonElement && want.disabled) ? want : refs.current.get(`${focus.index}:input`)
    target?.focus()
    setFocus(null)
  }, [focus])

  const ref = (index: number, part: Part) => (el: HTMLElement | null) => {
    const key = `${index}:${part}`
    if (el) refs.current.set(key, el)
    else refs.current.delete(key)
  }

  const set = (index: number, token: string) => onChange(tokens.map((t, i) => (i === index ? token : t)))
  const add = (token: string) => {
    onChange([...tokens, token])
    setFocus({ index: tokens.length, part: 'input' })
  }
  const remove = (index: number) => {
    onChange(tokens.filter((_, i) => i !== index))
    if (tokens.length > 1) setFocus({ index: Math.min(index, tokens.length - 2), part: 'input' })
    else setFocus({ index: -1, part: 'add' })
  }
  const move = (index: number, by: -1 | 1) => {
    const to = index + by
    if (to < 0 || to >= tokens.length) return
    const next = [...tokens]
    ;[next[index], next[to]] = [next[to], next[index]]
    onChange(next)
    setFocus({ index: to, part: by < 0 ? 'up' : 'down' })
  }

  const describedBy = [hintId, error ? errorId : undefined].filter(Boolean).join(' ') || undefined
  const iconButton =
    'inline-flex items-center justify-center w-7 h-7 rounded-md text-muted ' +
    'hover:bg-pressed disabled:opacity-30 disabled:cursor-not-allowed ' +
    'focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent'

  return (
    <div className="space-y-2">
      {tokens.length === 0 ? (
        <p className="text-xs text-muted">{emptyText}</p>
      ) : (
        <ol id={listId} aria-label={label} className="space-y-1.5">
          {tokens.map((token, i) => (
            // Index keys: rows are interchangeable inputs, and focus is moved explicitly above.
            <li key={i} className="flex items-center gap-1.5">
              <span aria-hidden="true" className="w-5 text-right text-xs tabular-nums text-faint">
                {i + 1}
              </span>
              <input
                ref={ref(i, 'input')}
                type="text"
                value={token}
                onChange={(e) => set(i, e.target.value)}
                aria-label={`${label}, token ${i + 1} of ${tokens.length}`}
                aria-describedby={describedBy}
                aria-invalid={error ? true : undefined}
                placeholder={i === 0 ? 'program' : 'argument'}
                spellCheck={false}
                autoComplete="off"
                autoCapitalize="off"
                className={`field flex-1 min-w-0 font-mono ${error && token === '' ? 'border-danger!' : ''}`}
              />
              <button
                ref={ref(i, 'up')}
                type="button"
                onClick={() => move(i, -1)}
                disabled={i === 0}
                aria-label={`Move token ${i + 1} up`}
                className={iconButton}
              >
                <Icon name="chevronUp" className="w-3.5 h-3.5" />
              </button>
              <button
                ref={ref(i, 'down')}
                type="button"
                onClick={() => move(i, 1)}
                disabled={i === tokens.length - 1}
                aria-label={`Move token ${i + 1} down`}
                className={iconButton}
              >
                <Icon name="chevronDown" className="w-3.5 h-3.5" />
              </button>
              <button type="button" onClick={() => remove(i)} aria-label={`Remove token ${i + 1}`} className={iconButton}>
                <Icon name="x" className="w-3.5 h-3.5" />
              </button>
            </li>
          ))}
        </ol>
      )}
      {error && (
        <p id={errorId} role="alert" className="text-xs text-danger">
          {error}
        </p>
      )}
      <div className="flex flex-wrap items-center gap-1.5">
        <button ref={ref(-1, 'add')} type="button" onClick={() => add('')} className="btn gap-1">
          <Icon name="plus" className="w-3.5 h-3.5" />
          Add token
        </button>
        {PLACEHOLDERS.map((name) => (
          <button
            key={name}
            type="button"
            onClick={() => add(`{${name}}`)}
            aria-label={`Add {${name}} token`}
            className="btn font-mono"
          >
            {`{${name}}`}
          </button>
        ))}
      </div>
    </div>
  )
}
