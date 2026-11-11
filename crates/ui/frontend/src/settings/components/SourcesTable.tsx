import { useRef, useState, type KeyboardEvent, type MouseEvent } from 'react'

import type { Config } from '../../shared/api'
import { deleteSourceFiles, errorMessage, openSourceFolder } from '../../shared/api'
import type { SourceEntry } from '../../shared/bindings/SourceEntry'
import type { SourceType } from '../../shared/bindings/SourceType'
import { MAX_NAME_LEN, SOURCE_TYPES, isOnline, newSourceId, takesInput } from '../sourceTypes'
import { useSourceChecks, type SourceCheck } from '../useSourceChecks'
import AddSourceMenu from './AddSourceMenu'
import { Group, Note } from './Group'
import RemoveSourceDialog from './RemoveSourceDialog'
import SourceDialog from './SourceDialog'

interface Props {
  config: Config
  onChange: (config: Config) => void
  /**
   * Change and write now; resolves once saved, rejects with the reason. Use Only These waits on
   * it to report a failed save; Remove waits on it before deleting files. Without it they fall
   * back to `onChange`, whose write is delayed, so the follow-up can run before the save lands.
   */
  onCommit?: (config: Config) => Promise<void>
}

type Message = { tone: 'danger' | 'muted'; text: string } | null

/** The open add/edit dialog: a new source of `kind`, or the existing source `id`. */
type Editing = { kind: SourceType; id: string | null; initial?: string; name?: string } | null

/** `s` with `name` set, or without the key when `name` is empty (the config writes no `name`). */
const withName = (s: SourceEntry, name: string): SourceEntry => {
  const { name: _old, ...rest } = s
  return name ? { ...rest, name } : rest
}

/** The Status cell's word and tooltip for a row's background check. */
function statusOf(check: SourceCheck | undefined): { tone: string; label: string; detail: string } | null {
  switch (check?.state) {
    case undefined:
      return null
    case 'checking':
      return { tone: 'checking', label: 'Checking…', detail: 'Checking this source for images…' }
    case 'failed':
      return { tone: 'failed', label: 'Invalid', detail: check.message }
    case 'unknown':
      return { tone: 'unknown', label: 'Unknown', detail: check.message }
  }
}

/**
 * The General tab's Images table, after Variety: one row per `[[sources]]` entry with an
 * Enabled box, the name (or the type when unnamed), the location and a Status that flags a source
 * failing its one-time image check (`useSourceChecks`; a good source shows nothing), and Add…,
 * Edit…, Remove…, Open Folder and Use Only These under
 * it. Rows multi-select with shift- and cmd/ctrl-click or the arrow keys. Double-click a name to
 * rename it in place; double-click elsewhere (or Enter) opens Edit.
 */
export default function SourcesTable({ config, onChange, onCommit }: Props) {
  const sources = config.sources
  const internet = config.daemon.internet
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set())
  const [anchor, setAnchor] = useState<string | null>(null)
  const [active, setActive] = useState<string | null>(null)
  const [removing, setRemoving] = useState<SourceEntry[] | null>(null)
  const [editing, setEditing] = useState<Editing>(null)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<Message>(null)
  // The row whose name is being edited in place.
  const [renaming, setRenaming] = useState<string | null>(null)
  const renameDone = useRef(false)
  const checks = useSourceChecks(sources, internet)
  const rowRefs = useRef(new Map<string, HTMLTableRowElement>())

  // Ids that left the config (removed here or elsewhere) drop out of the selection.
  const ids = sources.map((s) => s.id)
  const chosen = sources.filter((s) => selected.has(s.id))
  const focusId = active !== null && ids.includes(active) ? active : (ids[0] ?? null)
  const single = chosen.length === 1 ? chosen[0] : null

  const setSources = (next: SourceEntry[]) => onChange({ ...config, sources: next })

  /** Writes `next` now when possible; false (with the reason shown) when the save failed. */
  const commit = async (next: Config): Promise<boolean> => {
    if (!onCommit) {
      onChange(next)
      return true
    }
    try {
      await onCommit(next)
      return true
    } catch (err) {
      setMessage({ tone: 'danger', text: `Couldn't save the change: ${errorMessage(err)}` })
      return false
    }
  }

  const toggle = (id: string) => setSources(sources.map((s) => (s.id === id ? { ...s, enabled: !s.enabled } : s)))

  const focusRow = (id: string) => {
    setActive(id)
    rowRefs.current.get(id)?.focus()
  }

  /** Selects `id` the way a click or arrow key with these modifiers does. */
  const select = (id: string, { range, add }: { range: boolean; add: boolean }) => {
    const from = anchor !== null && ids.includes(anchor) ? anchor : id
    if (range) {
      const [a, b] = [ids.indexOf(from), ids.indexOf(id)].sort((x, y) => x - y)
      setSelected(new Set(ids.slice(a, b + 1)))
      setAnchor(from)
    } else if (add) {
      const next = new Set(selected)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      setSelected(next)
      setAnchor(id)
    } else {
      setSelected(new Set([id]))
      setAnchor(id)
    }
    setActive(id)
  }

  const onRowClick = (e: MouseEvent, id: string) => select(id, { range: e.shiftKey, add: e.metaKey || e.ctrlKey })

  const onRowKeyDown = (e: KeyboardEvent, id: string) => {
    const at = ids.indexOf(id)
    const move = (to: number) => {
      const target = ids[Math.max(0, Math.min(ids.length - 1, to))]
      if (target === undefined) return
      e.preventDefault()
      select(target, { range: e.shiftKey, add: false })
      focusRow(target)
    }
    switch (e.key) {
      case 'ArrowDown':
        return move(at + 1)
      case 'ArrowUp':
        return move(at - 1)
      case 'Home':
        return move(0)
      case 'End':
        return move(ids.length - 1)
      case ' ':
        e.preventDefault()
        return toggle(id)
      case 'Enter':
        e.preventDefault()
        return openEdit()
      case 'a':
        if (e.metaKey || e.ctrlKey) {
          e.preventDefault()
          setSelected(new Set(ids))
        }
        return
      case 'Delete':
      case 'Backspace':
        if (chosen.length > 0) {
          e.preventDefault()
          openRemove()
        }
        return
    }
  }

  /** Opens the Add dialog. Bing and APOD take no location, so each can be added only once. */
  const add = (kind: SourceType) => {
    setMessage(null)
    if (!takesInput(kind) && sources.some((s) => s.type === kind)) {
      setMessage({ tone: 'danger', text: `${SOURCE_TYPES[kind].label} is already in the list.` })
      return
    }
    setEditing({ kind, id: null })
  }

  /** Appends one enabled row per location and selects the new rows. */
  const addRows = (kind: SourceType, locations: string[], name = '') => {
    const taken = new Set(ids)
    const added = locations.map((location) => {
      const id = newSourceId(kind, taken)
      taken.add(id)
      return withName({ id, type: kind, location, enabled: true }, name)
    })
    setSources([...sources, ...added])
    setSelected(new Set(added.map((a) => a.id)))
    setAnchor(added[0]?.id ?? null)
    setActive(added[0]?.id ?? null)
  }

  const openEditFor = (s: SourceEntry) => {
    setMessage(null)
    setEditing({ kind: s.type, id: s.id, initial: s.location, name: s.name })
  }

  const openEdit = () => {
    if (single) openEditFor(single)
  }

  const startRename = (id: string) => {
    select(id, { range: false, add: false })
    renameDone.current = false
    setRenaming(id)
  }

  /** Ends the in-place rename; `name` saves it (trimmed, empty removes it), `null` cancels. */
  const endRename = (id: string, name: string | null) => {
    if (renameDone.current) return
    renameDone.current = true
    setRenaming(null)
    if (name !== null) {
      const next = name.trim()
      const cur = sources.find((s) => s.id === id)
      if (cur && (cur.name ?? '') !== next) setSources(sources.map((s) => (s.id === id ? withName(s, next) : s)))
    }
    focusRow(id)
  }

  const finishEdit = (locations: string[], name: string) => {
    const current = editing
    setEditing(null)
    if (!current) return
    if (current.id === null) return addRows(current.kind, locations, name)
    const location = locations[0] ?? ''
    setSources(sources.map((s) => (s.id === current.id ? withName({ ...s, location }, name) : s)))
    focusRow(current.id)
  }

  const openRemove = () => {
    setMessage(null)
    setRemoving(chosen)
  }

  const confirmRemove = async (deleteFiles: boolean) => {
    const gone = removing ?? []
    setRemoving(null)
    const goneIds = new Set(gone.map((s) => s.id))
    const at = ids.findIndex((id) => goneIds.has(id))
    const remaining = sources.filter((s) => !goneIds.has(s.id))
    // The row next in line takes the selection, so Remove can be pressed again.
    const after = remaining[Math.min(at, remaining.length - 1)]
    setSelected(after ? new Set([after.id]) : new Set())
    setAnchor(after?.id ?? null)
    setActive(after?.id ?? null)
    setBusy(true)
    try {
      // The row leaves the saved config before its folder goes, so the daemon can't refill it.
      if (!(await commit({ ...config, sources: remaining }))) return
      if (!deleteFiles) return
      const failed: string[] = []
      for (const s of gone.filter((g) => isOnline(g.type))) {
        await deleteSourceFiles(s.id).catch((err: unknown) => failed.push(errorMessage(err)))
      }
      if (failed.length > 0) {
        setMessage({
          tone: 'danger',
          text: `Removed, but the downloaded files couldn't be deleted: ${failed.join('; ')}`,
        })
      }
    } finally {
      setBusy(false)
    }
  }

  const use = async () => {
    setMessage(null)
    setBusy(true)
    try {
      // Switches sources only; the current wallpaper stays until the next change.
      const next = sources.map((s) => ({ ...s, enabled: selected.has(s.id) }))
      await commit({ ...config, sources: next })
    } finally {
      setBusy(false)
    }
  }

  const openFolder = async () => {
    if (!single) return
    setMessage(null)
    await openSourceFolder(single.id).catch((err: unknown) => setMessage({ tone: 'danger', text: errorMessage(err) }))
  }

  // The selected row's failed check, spelled out under the table.
  const singleCheck = single ? checks.get(single.id) : undefined
  const problem =
    single && singleCheck?.state === 'failed'
      ? `${single.name || SOURCE_TYPES[single.type].label}: ${singleCheck.message}`
      : null

  return (
    <Group
      title="Images"
      footer={
        (message || problem || !internet) && (
          <div className="space-y-1">
            {message && <Note tone={message.tone}>{message.text}</Note>}
            {problem && <Note tone="danger">{problem}</Note>}
            {!internet && <Note>Internet access is off, so online sources are paused.</Note>}
          </div>
        )
      }
    >
      <div className="sources-scroll">
        {/* oxlint-disable-next-line jsx-a11y/no-noninteractive-element-to-interactive-role -- an ARIA grid: rows select, arrow keys move */}
        <table role="grid" aria-label="Image sources" aria-multiselectable="true" className="sources-table">
          <colgroup>
            <col className="w-[72px]" />
            <col className="w-[32%]" />
            <col />
            <col className="w-[96px]" />
          </colgroup>
          <thead>
            <tr>
              <th scope="col">Enabled</th>
              <th scope="col">Name</th>
              <th scope="col">Location</th>
              <th scope="col">Status</th>
            </tr>
          </thead>
          <tbody>
            {sources.map((s) => {
              const info = SOURCE_TYPES[s.type]
              const offline = !internet && info.online
              const location = takesInput(s.type) ? s.location : ''
              const status = statusOf(checks.get(s.id))
              return (
                <tr
                  key={s.id}
                  ref={(el) => {
                    if (el) rowRefs.current.set(s.id, el)
                    else rowRefs.current.delete(s.id)
                  }}
                  tabIndex={s.id === focusId ? 0 : -1}
                  aria-selected={selected.has(s.id)}
                  aria-disabled={offline || undefined}
                  onClick={(e) => onRowClick(e, s.id)}
                  onDoubleClick={() => {
                    select(s.id, { range: false, add: false })
                    openEditFor(s)
                  }}
                  onFocus={() => setActive(s.id)}
                  onKeyDown={(e) => onRowKeyDown(e, s.id)}
                >
                  <td role="gridcell">
                    <input
                      type="checkbox"
                      tabIndex={-1}
                      checked={s.enabled}
                      aria-label={`Enabled: ${s.name || info.label}${location ? ` ${location}` : ''}`}
                      onClick={(e) => e.stopPropagation()}
                      onChange={() => toggle(s.id)}
                    />
                  </td>
                  <td
                    role="gridcell"
                    title={renaming === s.id ? undefined : 'Double-click to rename'}
                    onDoubleClick={(e) => {
                      e.stopPropagation()
                      startRename(s.id)
                    }}
                  >
                    {renaming === s.id ? (
                      <input
                        type="text"
                        aria-label={`Name for ${info.label}`}
                        className="field sources-rename"
                        // oxlint-disable-next-line jsx-a11y/no-autofocus -- opened by a double-click on this cell, so focus belongs here
                        autoFocus
                        autoComplete="off"
                        maxLength={MAX_NAME_LEN}
                        defaultValue={s.name ?? ''}
                        placeholder={info.label}
                        onFocus={(e) => e.currentTarget.select()}
                        onClick={(e) => e.stopPropagation()}
                        onKeyDown={(e) => {
                          e.stopPropagation()
                          if (e.key === 'Enter') {
                            e.preventDefault()
                            endRename(s.id, e.currentTarget.value)
                          } else if (e.key === 'Escape') {
                            e.preventDefault()
                            endRename(s.id, null)
                          }
                        }}
                        onBlur={(e) => endRename(s.id, e.currentTarget.value)}
                      />
                    ) : s.name ? (
                      <span className="sources-name" title={`${s.name} (${info.label})`}>
                        <span className="truncate">{s.name}</span>
                        <span className="text-muted text-[11px]">{info.label}</span>
                      </span>
                    ) : (
                      info.label
                    )}
                  </td>
                  <td role="gridcell">
                    {location ? (
                      <span className="sources-location" title={location}>
                        {location}
                      </span>
                    ) : (
                      <span className="text-faint" aria-label="none">
                        —
                      </span>
                    )}
                  </td>
                  <td role="gridcell">
                    {status && (
                      <span className={`sources-status sources-status-${status.tone}`} title={status.detail}>
                        <span className="sources-dot" aria-hidden="true" />
                        {status.label}
                      </span>
                    )}
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
        {sources.length === 0 && (
          <p className="px-4 py-6 text-center text-muted">
            No sources yet. Choose Add… to pick images or an online source.
          </p>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-1.5 px-2.5 py-2">
        <AddSourceMenu internet={internet} onChoose={add} disabled={busy} />
        <button type="button" className="btn" disabled={!single || busy} onClick={openEdit}>
          Edit…
        </button>
        <button type="button" className="btn" disabled={chosen.length === 0 || busy} onClick={openRemove}>
          Remove…
        </button>
        <span className="flex-1" />
        <button type="button" className="btn" disabled={!single} onClick={() => void openFolder()}>
          Open Folder
        </button>
        <button
          type="button"
          className="btn"
          disabled={chosen.length === 0 || busy}
          title="Turn on only the selected sources. The wallpaper changes at the next change, not now."
          onClick={() => void use()}
        >
          Use Only These
        </button>
      </div>
      {editing && (
        <SourceDialog
          kind={editing.kind}
          initial={editing.initial}
          initialName={editing.name}
          onDone={finishEdit}
          onCancel={() => {
            const id = editing.id
            setEditing(null)
            if (id) focusRow(id)
          }}
        />
      )}
      {removing && (
        <RemoveSourceDialog
          sources={removing}
          onCancel={() => setRemoving(null)}
          onConfirm={(d) => void confirmRemove(d)}
        />
      )}
    </Group>
  )
}
