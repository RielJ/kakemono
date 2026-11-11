import { useEffect, useId, useRef, useState, type FormEvent } from 'react'

import { selectDirectory, selectFiles } from '../../shared/api'
import type { SourceType } from '../../shared/bindings/SourceType'
import { MAX_NAME_LEN, SOURCE_TYPES } from '../sourceTypes'
import Modal from './Modal'

const baseName = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p

/** Name hints per type; other types get a generic one. */
const NAME_PLACEHOLDER: Partial<Record<SourceType, string>> = {
  wallhaven: 'e.g. wallhaven-hot',
  unsplash: 'e.g. unsplash-mountains',
  reddit: 'e.g. earthporn-top-year',
  folder: 'e.g. my-wallpapers',
  bing: 'e.g. bing-daily',
  apod: 'e.g. nasa-apod',
}

/**
 * Add or edit one source, after Variety: the type's instruction, its field, an optional Name,
 * OK and Cancel. OK finishes at once; the Sources table checks the location for images in the
 * background and shows the result on the row.
 * Path types open the system picker at once when adding (Images picks several files) and then
 * wait for OK, so a name can be typed. Bing and APOD take no location: only the Name shows.
 * `onDone` gets one location per row to add (`['']` for Bing and APOD) and the optional name,
 * trimmed (empty for none); `initial` makes it an edit, and `initialName` fills the name.
 */
export default function SourceDialog({
  kind,
  initial,
  initialName,
  onDone,
  onCancel,
}: {
  kind: SourceType
  initial?: string
  initialName?: string
  onDone: (locations: string[], name: string) => void
  onCancel: () => void
}) {
  const info = SOURCE_TYPES[kind]
  const { input } = info
  const editing = initial !== undefined
  const pick = input.kind === 'path' ? input.pick : null
  const [text, setText] = useState(initial ?? '')
  const [name, setName] = useState(initialName ?? '')
  // Several picked image files; empty when the field holds the one location.
  const [many, setMany] = useState<string[]>([])
  const fieldId = useId()
  const nameId = useId()
  const alive = useRef(true)
  const autoPicked = useRef(false)

  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
    }
  }, [])

  const locations = (): string[] => {
    if (input.kind === 'none') return ['']
    return many.length > 0 ? many : [text.trim()].filter(Boolean)
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
    setText(picked.length === 1 ? picked[0] : '')
    setMany(picked.length > 1 ? picked : [])
  }

  const selectDirOrFile = async (): Promise<string> => {
    if (pick === 'files') return (await selectFiles().catch(() => []))[0] ?? ''
    return selectDirectory().catch(() => '')
  }

  useEffect(() => {
    // Once, on mount, even when React runs effects twice in development.
    if (pick === null || editing || autoPicked.current) return
    autoPicked.current = true
    void choose(true)
    // oxlint-disable-next-line react-hooks/exhaustive-deps -- opens the picker once, on mount
  }, [])

  const submit = (e: FormEvent) => {
    e.preventDefault()
    const locs = locations()
    if (locs.length === 0) return
    onDone(locs, name.trim())
  }

  const prompt = many.length > 0 ? 'Image files:' : input.kind === 'none' ? '' : input.prompt
  const title = `${editing ? 'Edit' : 'Add'} ${info.label} source`

  return (
    <Modal title={title} onClose={onCancel}>
      <form onSubmit={submit} className="mt-3">
        {input.kind !== 'none' && (
          <>
            <label htmlFor={fieldId} className="block mb-1.5">
              {prompt}
            </label>
            <div className="flex items-center gap-1.5">
              {many.length > 0 ? (
                <ul
                  id={fieldId}
                  aria-label={prompt}
                  className="flex-1 min-w-0 max-h-28 overflow-y-auto field h-auto py-1 field-mono"
                >
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
                  onChange={(e) => setText(e.target.value)}
                  className={`field flex-1 ${pick ? 'field-mono' : ''}`}
                />
              )}
              {pick && (
                <button type="button" className="btn" onClick={() => void choose(false)}>
                  Choose…
                </button>
              )}
            </div>
          </>
        )}
        <div className={input.kind === 'none' ? '' : 'mt-2.5'}>
          <label htmlFor={nameId} className="block mb-1.5">
            Name <span className="text-muted">(optional{many.length > 0 ? ', used for each file' : ''})</span>
          </label>
          <input
            id={nameId}
            type="text"
            autoComplete="off"
            maxLength={MAX_NAME_LEN}
            value={name}
            data-autofocus={input.kind === 'none' || undefined}
            placeholder={NAME_PLACEHOLDER[kind] ?? 'e.g. my-wallpapers'}
            onChange={(e) => setName(e.target.value)}
            className="field w-full"
          />
        </div>
        <div className="mt-4 flex justify-end gap-2">
          <button type="button" className="btn" onClick={onCancel}>
            Cancel
          </button>
          <button type="submit" className="btn btn-accent" disabled={locations().length === 0}>
            OK
          </button>
        </div>
      </form>
    </Modal>
  )
}
