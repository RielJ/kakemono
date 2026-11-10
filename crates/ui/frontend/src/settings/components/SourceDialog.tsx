import { useEffect, useId, useRef, useState, type FormEvent } from 'react'

import { checkSource, errorMessage, isDaemonUnreachable, selectDirectory, selectFiles } from '../../shared/api'
import type { SourceType } from '../../shared/bindings/SourceType'
import { SOURCE_TYPES } from '../sourceTypes'
import Modal from './Modal'

type Phase =
  | { kind: 'idle' }
  | { kind: 'checking' }
  /** `unreachable`: the daemon didn't answer, so the entry couldn't be checked; offer Add anyway. */
  | { kind: 'error'; message: string; unreachable: boolean }

const baseName = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p

/**
 * Add or edit one source's location, after Variety: the type's instruction, one field, OK and
 * Cancel. OK checks the entry with the daemon (`check_source`), shows "Just a moment to check
 * for images…" meanwhile, and finishes only when images were found; an error stays inline.
 * Path types open the system picker at once when adding (Images picks several files).
 * `onDone` gets one location per row to add; `initial` makes it an edit.
 */
export default function SourceDialog({
  kind,
  initial,
  onDone,
  onCancel,
}: {
  kind: SourceType
  initial?: string
  onDone: (locations: string[]) => void
  onCancel: () => void
}) {
  const info = SOURCE_TYPES[kind]
  const { input } = info
  const editing = initial !== undefined
  const pick = input.kind === 'path' ? input.pick : null
  const [text, setText] = useState(initial ?? '')
  // Several picked image files; empty when the field holds the one location.
  const [many, setMany] = useState<string[]>([])
  const [phase, setPhase] = useState<Phase>({ kind: 'idle' })
  const fieldId = useId()
  const messageId = useId()
  const alive = useRef(true)
  const autoPicked = useRef(false)

  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])

  const locations = (): string[] => (many.length > 0 ? many : [text.trim()].filter(Boolean))

  const check = async (locs: string[]) => {
    setPhase({ kind: 'checking' })
    for (const loc of locs) {
      try {
        const { count } = await checkSource(kind, loc)
        if (count < 1) throw new Error('No images found.')
      } catch (err) {
        if (!alive.current) return
        const message = errorMessage(err)
        setPhase({ kind: 'error', message: locs.length > 1 ? `${baseName(loc)}: ${message}` : message, unreachable: isDaemonUnreachable(err) })
        return
      }
      if (!alive.current) return
    }
    onDone(locs)
  }

  /** Opens the system picker; `auto` (Add) closes the dialog when the user cancels it. */
  const choose = async (auto: boolean) => {
    const picked =
      pick === 'files' && !editing ? await selectFiles().catch(() => []) : [await selectDirOrFile()].filter(Boolean)
    if (!alive.current) return
    if (picked.length === 0) {
      if (auto) onCancel()
      return
    }
    setPhase({ kind: 'idle' })
    setText(picked.length === 1 ? picked[0] : '')
    setMany(picked.length > 1 ? picked : [])
    if (auto) await check(picked)
  }

  const selectDirOrFile = async (): Promise<string> => {
    if (pick === 'files') return (await selectFiles().catch(() => []))[0] ?? ''
    return selectDirectory().catch(() => '')
  }

  useEffect(() => {
    // Once, even when React runs effects twice in development.
    if (pick === null || editing || autoPicked.current) return
    autoPicked.current = true
    void choose(true)
  }, [])

  const submit = (e: FormEvent) => {
    e.preventDefault()
    const locs = locations()
    if (locs.length === 0 || phase.kind === 'checking') return
    void check(locs)
  }

  const checking = phase.kind === 'checking'
  const error = phase.kind === 'error' ? phase : null
  const prompt = many.length > 0 ? 'Image files:' : input.kind === 'none' ? 'Location:' : input.prompt
  const title = `${editing ? 'Edit' : 'Add'} ${info.label} source`

  return (
    <Modal title={title} onClose={onCancel}>
      <form onSubmit={submit} className="mt-3">
        <label htmlFor={fieldId} className="block mb-1.5">
          {prompt}
        </label>
        <div className="flex items-center gap-1.5">
          {many.length > 0 ? (
            <ul id={fieldId} aria-label={prompt} className="flex-1 min-w-0 max-h-28 overflow-y-auto field h-auto py-1 field-mono">
              {many.map((p) => (
                <li key={p} className="truncate" title={p}>
                  {baseName(p)}
                </li>
              ))}
            </ul>
          ) : (
            <input
              id={fieldId}
              type="text"
              data-autofocus
              spellCheck={false}
              autoComplete="off"
              autoCapitalize="off"
              value={text}
              placeholder={input.kind === 'text' ? input.placeholder : undefined}
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? messageId : undefined}
              onChange={(e) => {
                setText(e.target.value)
                if (error) setPhase({ kind: 'idle' })
              }}
              className={`field flex-1 ${pick ? 'field-mono' : ''}`}
            />
          )}
          {pick && (
            <button type="button" className="btn" disabled={checking} onClick={() => void choose(false)}>
              Choose…
            </button>
          )}
        </div>
        <div className="mt-2 min-h-[18px] text-[12px] leading-snug">
          <p role="status" className="text-muted">
            {checking ? 'Just a moment to check for images…' : ''}
          </p>
          {error && (
            <div id={messageId}>
              <p role="alert" className="text-danger break-words">
                {error.message}
              </p>
              {error.unreachable && (
                <p className="mt-0.5 text-muted">Kakemono couldn’t check this source because its background service isn’t answering.</p>
              )}
            </div>
          )}
        </div>
        <div className="mt-3 flex justify-end gap-2">
          <button type="button" className="btn" onClick={onCancel}>
            Cancel
          </button>
          {error?.unreachable && (
            <button type="button" className="btn" onClick={() => onDone(locations())}>
              Add anyway
            </button>
          )}
          <button type="submit" className="btn btn-accent" disabled={checking || locations().length === 0}>
            OK
          </button>
        </div>
      </form>
    </Modal>
  )
}
