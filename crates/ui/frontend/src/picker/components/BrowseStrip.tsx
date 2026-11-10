import { useCallback, useEffect, useId, useRef, useState } from 'react'
import {
  browseUse,
  errorMessage,
  isDaemonTimeout,
  resetPickerPosition,
  type BrowseAction,
  type BrowseSourceStatus,
  type RemoteThumb,
} from '../../shared/api'
import { keyAction, type KeyAction } from '../keys'
import { useBrowse, type UseBrowseOptions } from '../useBrowse'
import { useSentinel } from '../useSentinel'
import ContextMenu, { type MenuItem } from './ContextMenu'
import Icon from './Icon'
import { focusIsFree, NEAR_END, type Notices } from './ThumbnailStrip'

interface Props {
  vertical: boolean
  notices: Notices
  /** `[picker] blur_nsfw`: NSFW previews stay blurred until hovered or keyboard-focused. */
  blurNsfw: boolean
  /** Where the browse session outlives the strip (tests pass their own). */
  memory?: UseBrowseOptions['memory']
}

/** What each action says when it is done. */
const DONE_TEXT: Record<BrowseAction, string> = {
  set: 'Set as wallpaper',
  favorite: 'Added to Favorites',
  download: 'Downloaded',
}

/** "title by author — kind", a card's accessible name. A blank title falls back to the id. */
export function remoteLabel(t: RemoteThumb): string {
  const title = t.title || t.remoteId
  return `${title}${t.author ? ` by ${t.author}` : ''} — ${t.kind}`
}

/** "kind (id)", how the filter and the notes name a source. */
const sourceName = (s: BrowseSourceStatus) => `${s.kind} (${s.source_id})`

/** "kind (id): message" for each source that can't give previews right now. */
function problems(statuses: BrowseSourceStatus[]): string[] {
  return statuses.flatMap((s) => ('message' in s.state ? [`${sourceName(s)}: ${s.state.message}`] : []))
}

/** Empty pages asked for in a row before the strip says there is nothing to show. */
const EMPTY_PAGE_TRIES = 3

const itemKey = (t: RemoteThumb) => `${t.sourceId}\u0000${t.remoteId}`

/**
 * The Sources tab: remote previews from the daemon's browse session (`useBrowse`), in a
 * `listbox` with the on-disk strip's key map. Enter, F and the menu download the full image
 * through `browse_use`; there is no trash. With more than one source a filter card comes
 * first, and `[`/`]` cycle it from anywhere in the tab.
 */
export default function BrowseStrip({ vertical, notices, blurNsfw, memory }: Props) {
  const { say } = notices
  const browse = useBrowse({ memory })
  const { items, loading, loadingMore, done, capped, error, moreError, loadMore, cycleFilter } = browse
  const idPrefix = useId()
  const [active, setActive] = useState(0)
  const [hovered, setHovered] = useState<string | null>(null)
  const [listFocused, setListFocused] = useState(false)
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set())
  const busyRef = useRef(busy)
  busyRef.current = busy
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const sentinelRef = useRef<HTMLDivElement>(null)

  const activeIndex = Math.min(active, Math.max(0, items.length - 1))
  const activeItem: RemoteThumb | undefined = items[activeIndex]
  const optionId = (i: number) => `${idPrefix}-option-${i}`

  // A new session (filter, Reload) starts at the first card.
  useEffect(() => {
    if (loading) setActive(0)
  }, [loading])

  const use = useCallback(
    (item: RemoteThumb, action: BrowseAction) => {
      const k = itemKey(item)
      if (busyRef.current.has(k)) return
      const mark = (on: boolean) =>
        setBusy((prev) => {
          const next = new Set(prev)
          if (on) next.add(k)
          else next.delete(k)
          return next
        })
      mark(true)
      busyRef.current = new Set(busyRef.current).add(k)
      browseUse({ sourceId: item.sourceId, remoteId: item.remoteId, action })
        .then(() => say(DONE_TEXT[action]))
        .catch((err: unknown) => {
          // Past 120 s the daemon keeps downloading: not a failure, and the message says so.
          say(isDaemonTimeout(err) ? errorMessage(err) : `Couldn’t get the image: ${errorMessage(err)}`)
        })
        .finally(() => mark(false))
    },
    [say],
  )

  const openMenuAtActive = () => {
    const rect = document.getElementById(optionId(activeIndex))?.getBoundingClientRect()
    setMenu({ x: (rect?.left ?? 0) + 12, y: (rect?.top ?? 0) + 12 })
  }

  const run = (action: KeyAction) => {
    if (action.kind === 'move') {
      setActive(action.to)
      if (action.to >= items.length - NEAR_END) loadMore()
      return
    }
    if (!activeItem) return
    if (action.kind === 'set') use(activeItem, 'set')
    else if (action.kind === 'favorite') use(activeItem, 'favorite')
    else if (action.kind === 'menu') openMenuAtActive()
    // `trash`: a remote image has no file to trash.
  }

  const onListKeyDown = (e: React.KeyboardEvent) => {
    const action = keyAction(e, vertical, activeIndex, items.length)
    if (!action) return
    switch (action.kind) {
      // Window-wide: Esc and the tab keys (App), and the filter keys (below).
      case 'close':
      case 'undo':
      case 'tab':
      case 'tabIndex':
      case 'filter':
        return
      default:
        e.preventDefault()
        run(action)
    }
  }

  // `[` / `]` cycle the filter from anywhere in the tab, even with nothing loaded.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return
      const action = keyAction(e, vertical, 0, 0)
      if (action?.kind !== 'filter') return
      e.preventDefault()
      cycleFilter(action.delta)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [vertical, cycleFilter])

  const ready = !loading && error === null && items.length > 0
  useEffect(() => {
    const list = listRef.current
    if (ready && list && focusIsFree(list) && !list.contains(document.activeElement)) list.focus()
  }, [ready])

  // A page with nothing on it that isn't the end (every source busy or limited): ask for the
  // next few, then stop, so a limited source isn't asked again and again.
  const [emptyLoads, setEmptyLoads] = useState(0)
  useEffect(() => {
    if (loading) setEmptyLoads(0)
  }, [loading])
  const emptyPage = !loading && error === null && moreError === null && items.length === 0 && !done
  const waiting = emptyPage && (loadingMore || emptyLoads < EMPTY_PAGE_TRIES)
  useEffect(() => {
    if (!emptyPage || loadingMore || emptyLoads >= EMPTY_PAGE_TRIES) return
    setEmptyLoads((n) => n + 1)
    loadMore()
  }, [emptyPage, loadingMore, emptyLoads, loadMore])

  const paging = !done && !capped
  useSentinel(sentinelRef, { onVisible: loadMore, busy: !ready || !paging || loadingMore || moreError !== null })

  useEffect(() => {
    document.getElementById(optionId(activeIndex))?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' })
    // optionId only depends on the stable idPrefix.
  }, [activeIndex, items.length])

  // The wheel scrolls along the strip whichever way it turns.
  useEffect(() => {
    const el = listRef.current
    if (!el) return
    const onWheel = (e: WheelEvent) => {
      e.preventDefault()
      if (vertical) el.scrollTop += e.deltaY + e.deltaX
      else el.scrollLeft += e.deltaY + e.deltaX
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
  }, [vertical, loading])

  const notes = problems(browse.statuses)

  const menuItems: MenuItem[] = activeItem
    ? [
        { label: 'Set as Wallpaper', onSelect: () => use(activeItem, 'set') },
        { label: 'Add to Favorites', onSelect: () => use(activeItem, 'favorite') },
        { label: 'Download', onSelect: () => use(activeItem, 'download') },
        {
          label: 'Reset Position',
          onSelect: () => void resetPickerPosition().catch((err: unknown) => say(errorMessage(err))),
        },
      ]
    : []

  let body: React.ReactNode
  if (loading || waiting) {
    body = <p className="state">Loading previews…</p>
  } else if (error?.kind === 'internet_off') {
    body = <p className="state">{error.message}</p>
  } else if (error) {
    body = (
      <div className="state state-stack">
        <span>Couldn’t browse sources: {error.message}</span>
        <button type="button" className="state-action" onClick={browse.reload}>
          Retry
        </button>
      </div>
    )
  } else if (items.length === 0) {
    body = (
      <div className="state state-stack">
        <span>{browse.sources.length === 0 ? 'No online sources are enabled. Add one in Settings.' : 'No previews to show.'}</span>
        {notes.map((n) => (
          <span key={n} className="state-note">
            {n}
          </span>
        ))}
        {moreError !== null && <span className="state-note">{moreError}</span>}
        {!done && (
          <button type="button" className="state-action" onClick={browse.reload}>
            Retry
          </button>
        )}
      </div>
    )
  }

  const showEnd = body === undefined && (capped || moreError !== null || notes.length > 0)

  return (
    <>
      {browse.sources.length > 1 && <FilterCard sources={browse.sources} filter={browse.filter} onChange={browse.setFilter} />}
      {body}
      <div
        ref={listRef}
        className="strip"
        role="listbox"
        aria-label="Previews"
        aria-orientation={vertical ? 'vertical' : 'horizontal'}
        aria-activedescendant={activeItem ? optionId(activeIndex) : undefined}
        tabIndex={0}
        hidden={body !== undefined}
        onKeyDown={onListKeyDown}
        onFocus={(e) => {
          if (e.target === e.currentTarget) setListFocused(true)
        }}
        onBlur={(e) => {
          if (e.target === e.currentTarget) setListFocused(false)
        }}
      >
        {items.map((item, i) => {
          const k = itemKey(item)
          const revealed = hovered === k || (listFocused && i === activeIndex)
          return (
            <RemoteOption
              key={k}
              id={optionId(i)}
              item={item}
              active={i === activeIndex}
              busy={busy.has(k)}
              blurred={blurNsfw && item.nsfw && !revealed}
              onClick={() => {
                setActive(i)
                use(item, 'set')
              }}
              onContextMenu={(e) => {
                e.preventDefault()
                setActive(i)
                setMenu({ x: e.clientX, y: e.clientY })
              }}
              onHover={(on) => setHovered((cur) => (on ? k : cur === k ? null : cur))}
            />
          )
        })}
        {paging && items.length > 0 && <div ref={sentinelRef} className="option option-skeleton" aria-hidden="true" />}
      </div>
      {showEnd && (
        <div className="browse-end">
          {notes.map((n) => (
            <span key={n} className="browse-note">
              {n}
            </span>
          ))}
          {moreError !== null && (
            <>
              <span className="browse-note">{moreError}</span>
              <button type="button" className="state-action" onClick={loadMore}>
                Retry
              </button>
            </>
          )}
          {capped && (
            <>
              <span className="browse-note">That’s the first 300.</span>
              <button type="button" className="state-action" onClick={browse.reload}>
                Reload
              </button>
            </>
          )}
        </div>
      )}
      {menu && activeItem && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          label={remoteLabel(activeItem)}
          items={menuItems}
          onClose={() => {
            setMenu(null)
            listRef.current?.focus()
          }}
        />
      )}
    </>
  )
}

interface FilterCardProps {
  sources: BrowseSourceStatus[]
  filter: string | null
  onChange: (filter: string | null) => void
}

/** "All" plus one radio per source; changing it starts a new browse session. */
function FilterCard({ sources, filter, onChange }: FilterCardProps) {
  const name = useId()
  const options: { id: string | null; label: string }[] = [
    { id: null, label: 'All' },
    ...sources.map((s) => ({ id: s.source_id, label: sourceName(s) })),
  ]
  return (
    <div className="filter-card" role="radiogroup" aria-label="Source">
      {options.map((o) => (
        <label key={o.id ?? ''} className="filter-option">
          <input type="radio" name={name} checked={filter === o.id} onChange={() => onChange(o.id)} />
          <span>{o.label}</span>
        </label>
      ))}
    </div>
  )
}

interface OptionProps {
  id: string
  item: RemoteThumb
  active: boolean
  busy: boolean
  blurred: boolean
  onClick: () => void
  onContextMenu: (e: React.MouseEvent) => void
  onHover: (on: boolean) => void
}

function RemoteOption({ id, item, active, busy, blurred, onClick, onContextMenu, onHover }: OptionProps) {
  const [broken, setBroken] = useState(false)
  const label = remoteLabel(item)
  return (
    <div
      id={id}
      role="option"
      className="option"
      aria-selected={active}
      aria-label={label}
      aria-busy={busy || undefined}
      data-active={active || undefined}
      data-blurred={blurred || undefined}
      title={label}
      onClick={onClick}
      onContextMenu={onContextMenu}
      onPointerEnter={() => onHover(true)}
      onPointerLeave={() => onHover(false)}
    >
      {item.imageUrl && !broken ? (
        <img src={item.imageUrl} alt="" loading="lazy" decoding="async" draggable={false} onError={() => setBroken(true)} />
      ) : (
        <span className="placeholder">
          <Icon name="image" />
        </span>
      )}
      {busy && (
        <span className="busy-mark" aria-hidden="true">
          <Icon name="download" />
        </span>
      )}
      <span className="caption">{item.kind}</span>
    </div>
  )
}
