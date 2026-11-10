import { useCallback, useEffect, useId, useRef, useState } from 'react'
import {
  errorMessage,
  favoriteWallpaper,
  getStatus,
  listTab,
  resetPickerPosition,
  setFile,
  trashWallpaper,
  type DiskTab,
  type Thumbnail,
} from '../../shared/api'
import { useDaemonEvents } from '../../shared/useDaemonEvents'
import { keyAction, type KeyAction } from '../keys'
import { commitTrash, MESSAGES, UNDO_MS } from '../trash'
import { useActiveInView } from '../useActiveInView'
import { useSentinel } from '../useSentinel'
import ContextMenu, { type MenuItem } from './ContextMenu'
import Icon from './Icon'

/** The picker's one toast, owned by the App so every tab and the image drop share it. */
export interface Notices {
  /** Shows `text`; with `onUndo` it also shows an Undo button and stays until replaced. */
  say: (text: string, onUndo?: () => void) => void
  /** Hides the toast if it is the Undo one (its trash was sent). */
  dismissUndo: () => void
}

interface StripProps {
  /** Which on-disk tab this strip lists; the App remounts the strip for another tab. */
  tab: DiskTab
  vertical: boolean
  notices: Notices
}

/** Moving the keyboard this close to the end of the list loads the next page. */
export const NEAR_END = 3

/** What each tab says when it has nothing to show. */
const EMPTY_TEXT: Record<DiskTab, string> = {
  downloaded: 'Nothing downloaded yet. Pick an image in Sources to download it.',
  local: 'No local images. Add a folder, album or images source in Settings.',
  favorites: 'No favorites yet. Press F on a wallpaper to add it here.',
  cache: 'The cache is empty.',
}

/**
 * True when nothing else holds the keyboard: focus is on the body or already in `list`. The
 * strip takes focus only then, so it never pulls focus out of the tab bar.
 */
export function focusIsFree(list: HTMLElement | null): boolean {
  const el = document.activeElement
  return el === null || el === document.body || (list?.contains(el) ?? false)
}

/** "title — source", the option's accessible name. */
export function optionLabel(thumb: Thumbnail): string {
  const title = thumb.title || thumb.path.split(/[\\/]/).pop() || thumb.path
  return thumb.source ? `${title} — ${thumb.source}` : title
}

/**
 * The strip of one on-disk tab: a `listbox` with `aria-activedescendant`, the §H key map, a
 * context menu and a deferred trash with undo (see `../trash.ts`). Pages of 40 load as the
 * sentinel at the end scrolls into view, or as the keyboard nears the end.
 */
export default function ThumbnailStrip({ tab, vertical, notices }: StripProps) {
  const { say, dismissUndo } = notices
  const idPrefix = useId()
  const [thumbs, setThumbs] = useState<Thumbnail[]>([])
  const thumbsRef = useRef(thumbs)
  thumbsRef.current = thumbs
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [hasMore, setHasMore] = useState(false)
  const [loadingMore, setLoadingMore] = useState(false)
  const [active, setActive] = useState(0)
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null)
  const [pendingPath, setPendingPath] = useState<string | null>(null)
  const pending = useRef<{ path: string; timer: ReturnType<typeof setTimeout> } | null>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const sentinelRef = useRef<HTMLDivElement>(null)
  /** Bumped by every first-page load, so a page that was asked for before it is dropped. */
  const generation = useRef(0)
  /** A next page is in flight: further requests collapse into it. */
  const moreInFlight = useRef(false)

  const visible = pendingPath === null ? thumbs : thumbs.filter((t) => t.path !== pendingPath)
  const activeIndex = Math.min(active, Math.max(0, visible.length - 1))
  const activeThumb: Thumbnail | undefined = visible[activeIndex]
  const optionId = (i: number) => `${idPrefix}-option-${i}`

  /** (Re)loads the first page; the current wallpaper is pinned first when the tab holds it. */
  const load = useCallback(() => {
    const gen = ++generation.current
    moreInFlight.current = false
    setLoadingMore(false)
    listTab(tab, 0)
      .then((page) => {
        if (gen !== generation.current) return
        setThumbs(page.items)
        setHasMore(page.hasMore)
        setLoadError(null)
        const shown = page.items.filter((t) => t.path !== pending.current?.path)
        setActive(Math.max(0, shown.findIndex((t) => t.isCurrent)))
      })
      .catch((err: unknown) => {
        if (gen === generation.current) setLoadError(errorMessage(err))
      })
      .finally(() => {
        if (gen === generation.current) setLoading(false)
      })
  }, [tab])

  /** Appends the next page (40 entries) after the ones already loaded. */
  const loadMore = useCallback(() => {
    if (moreInFlight.current) return
    moreInFlight.current = true
    setLoadingMore(true)
    const gen = generation.current
    listTab(tab, thumbsRef.current.length)
      .then((page) => {
        if (gen !== generation.current) return
        // Files can appear between pages and shift the listing: skip what is already shown.
        setThumbs((prev) => {
          const have = new Set(prev.map((t) => t.path))
          return prev.concat(page.items.filter((t) => !have.has(t.path)))
        })
        setHasMore(page.hasMore)
      })
      .catch((err: unknown) => {
        if (gen !== generation.current) return
        // Stop paging; the strip keeps what it has.
        setHasMore(false)
        say(errorMessage(err))
      })
      .finally(() => {
        if (gen !== generation.current) return
        moreInFlight.current = false
        setLoadingMore(false)
      })
  }, [tab, say])

  /** Sends the pending trash now, if the deleted wallpaper is still current. */
  const flushTrash = useCallback(async () => {
    const p = pending.current
    if (!p) return
    pending.current = null
    clearTimeout(p.timer)
    const outcome = await commitTrash(p.path, { getStatus, trash: trashWallpaper })
    setPendingPath((cur) => (cur === p.path ? null : cur))
    switch (outcome.kind) {
      case 'trashed':
        setThumbs((prev) => prev.filter((t) => t.path !== p.path))
        dismissUndo()
        load()
        break
      case 'changed':
        say(MESSAGES.changed)
        break
      case 'failed':
        say(errorMessage(outcome.error))
        break
    }
  }, [load, say, dismissUndo])

  const undo = useCallback(() => {
    const p = pending.current
    if (!p) return
    pending.current = null
    clearTimeout(p.timer)
    setPendingPath(null)
    say(MESSAGES.undone)
    listRef.current?.focus()
  }, [say])

  const trash = (thumb: Thumbnail) => {
    if (!thumb.isCurrent) {
      say(MESSAGES.notCurrent)
      return
    }
    void flushTrash()
    pending.current = { path: thumb.path, timer: setTimeout(() => void flushTrash(), UNDO_MS) }
    setPendingPath(thumb.path)
    say(MESSAGES.trashed, undo)
  }

  const set = (thumb: Thumbnail) => {
    setThumbs((prev) => prev.map((t) => ({ ...t, isCurrent: t.path === thumb.path })))
    setFile(thumb.path).catch((err: unknown) => {
      say(errorMessage(err))
      load()
    })
  }

  const favorite = (thumb: Thumbnail) => {
    favoriteWallpaper(thumb.path)
      .then(() => say('Added to Favorites'))
      .catch((err: unknown) => say(errorMessage(err)))
  }

  const openMenuAtActive = () => {
    const rect = document.getElementById(optionId(activeIndex))?.getBoundingClientRect()
    setMenu({ x: (rect?.left ?? 0) + 12, y: (rect?.top ?? 0) + 12 })
  }

  const run = (action: KeyAction) => {
    if (action.kind === 'move') {
      setActive(action.to)
      if (hasMore && action.to >= visible.length - NEAR_END) loadMore()
      return
    }
    if (!activeThumb) return
    if (action.kind === 'set') set(activeThumb)
    else if (action.kind === 'favorite') favorite(activeThumb)
    else if (action.kind === 'trash') trash(activeThumb)
    else if (action.kind === 'menu') openMenuAtActive()
  }

  const onListKeyDown = (e: React.KeyboardEvent) => {
    const action = keyAction(e, vertical, activeIndex, visible.length)
    if (!action) return
    switch (action.kind) {
      // Window-wide: Esc and undo (below and in the App) also work from the toast, and the tab
      // keys (App) also work from an empty tab.
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

  // The list is `hidden` while loading, so it can only take focus once the options are in.
  const ready = !loading && loadError === null && visible.length > 0
  useEffect(() => {
    const list = listRef.current
    if (ready && list && focusIsFree(list) && !list.contains(document.activeElement)) list.focus()
  }, [ready])

  // The last slot of the list: scrolling it into view loads the next page.
  useSentinel(sentinelRef, { onVisible: loadMore, busy: !ready || !hasMore || loadingMore })

  // Live: the highlight follows the daemon (rotation, tray Next, the CLI); a wallpaper the
  // list doesn't have yet, or a reconnect, reloads it.
  useDaemonEvents({
    onChanged: (info) => {
      if (thumbsRef.current.some((t) => t.path === info.path)) {
        setThumbs((prev) => prev.map((t) => ({ ...t, isCurrent: t.path === info.path })))
      } else {
        load()
      }
    },
    onConnection: (c) => {
      if (c.state === 'connected') load()
    },
  })

  useEffect(() => {
    load()
    // The window is hidden, not closed: refresh and refocus each time it is shown.
    const onFocus = () => {
      load()
      const list = listRef.current
      if (list && focusIsFree(list) && !list.contains(document.activeElement)) list.focus()
    }
    window.addEventListener('focus', onFocus)
    return () => window.removeEventListener('focus', onFocus)
  }, [load])

  useEffect(() => {
    // The App hides the picker on Esc; this sends a pending trash first. Neither handler
    // claims Esc (no preventDefault), so the order they run in doesn't matter.
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return
      const action = keyAction(e, vertical, 0, 0)
      if (action?.kind === 'close') {
        setMenu(null)
        void flushTrash()
      } else if (action?.kind === 'undo' && pending.current) {
        e.preventDefault()
        undo()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [vertical, flushTrash, undo])

  // A pending trash is sent when the picker goes away (hidden by blur, the tray or unmount),
  // never dropped, and never left to a timer the hidden webview may pause.
  useEffect(() => {
    const onVisibility = () => {
      if (document.visibilityState === 'hidden') void flushTrash()
    }
    document.addEventListener('visibilitychange', onVisibility)
    return () => {
      document.removeEventListener('visibilitychange', onVisibility)
      void flushTrash()
    }
  }, [flushTrash])

  useActiveInView(optionId(activeIndex))

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

  const menuItems: MenuItem[] = activeThumb
    ? [
        { label: 'Set as Wallpaper', onSelect: () => set(activeThumb) },
        { label: 'Add to Favorites', onSelect: () => favorite(activeThumb) },
        {
          label: 'Move to Trash',
          onSelect: () => trash(activeThumb),
          disabled: !activeThumb.isCurrent,
          hint: MESSAGES.notCurrent,
        },
        {
          label: 'Reset Position',
          onSelect: () => void resetPickerPosition().catch((err: unknown) => say(errorMessage(err))),
        },
      ]
    : []

  let body: React.ReactNode
  if (loading) {
    body = <p className="state">Loading wallpapers…</p>
  } else if (loadError !== null) {
    body = <p className="state">Couldn’t load wallpapers: {loadError}</p>
  } else if (visible.length === 0) {
    body = <p className="state">{EMPTY_TEXT[tab]}</p>
  }

  return (
    <>
      {body}
      <div
        ref={listRef}
        className="strip"
        role="listbox"
        aria-label="Wallpapers"
        aria-orientation={vertical ? 'vertical' : 'horizontal'}
        aria-activedescendant={activeThumb ? optionId(activeIndex) : undefined}
        tabIndex={0}
        hidden={body !== undefined}
        onKeyDown={onListKeyDown}
      >
        {visible.map((thumb, i) => (
          <Option
            key={thumb.path}
            id={optionId(i)}
            thumb={thumb}
            active={i === activeIndex}
            onClick={() => {
              setActive(i)
              set(thumb)
            }}
            onContextMenu={(e) => {
              e.preventDefault()
              setActive(i)
              setMenu({ x: e.clientX, y: e.clientY })
            }}
          />
        ))}
        {hasMore && <div ref={sentinelRef} className="sentinel" aria-hidden="true" />}
      </div>
      {menu && activeThumb && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          label={optionLabel(activeThumb)}
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

interface OptionProps {
  id: string
  thumb: Thumbnail
  active: boolean
  onClick: () => void
  onContextMenu: (e: React.MouseEvent) => void
}

function Option({ id, thumb, active, onClick, onContextMenu }: OptionProps) {
  const [broken, setBroken] = useState(false)
  const label = optionLabel(thumb)
  return (
    <div
      id={id}
      role="option"
      className="option"
      aria-selected={thumb.isCurrent}
      aria-label={label}
      data-active={active || undefined}
      onClick={onClick}
      onContextMenu={onContextMenu}
    >
      {thumb.imageUrl && !broken ? (
        <img src={thumb.imageUrl} alt="" loading="lazy" decoding="async" draggable={false} onError={() => setBroken(true)} />
      ) : (
        <span className="placeholder">
          <Icon name="image" />
        </span>
      )}
      {thumb.isCurrent && (
        <span className="current-mark">
          <Icon name="check" />
        </span>
      )}
      <span className="caption">{thumb.source}</span>
    </div>
  )
}
