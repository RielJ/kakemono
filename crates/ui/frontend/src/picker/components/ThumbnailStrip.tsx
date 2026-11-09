import { useCallback, useEffect, useId, useRef, useState } from 'react'
import {
  errorMessage,
  favoriteWallpaper,
  getStatus,
  hidePicker,
  listThumbnails,
  resetPickerPosition,
  setFile,
  trashWallpaper,
  type Thumbnail,
} from '../../shared/api'
import { useDaemonEvents } from '../../shared/useDaemonEvents'
import { keyAction, type KeyAction } from '../keys'
import { commitTrash, MESSAGES, UNDO_MS } from '../trash'
import ContextMenu, { type MenuItem } from './ContextMenu'
import Icon from './Icon'

interface StripProps {
  vertical?: boolean
}

interface Notice {
  text: string
  /** Shows the Undo button (a trash is pending). */
  undo: boolean
}

/** "title — source", the option's accessible name. */
export function optionLabel(thumb: Thumbnail): string {
  const title = thumb.title || thumb.path.split(/[\\/]/).pop() || thumb.path
  return thumb.source ? `${title} — ${thumb.source}` : title
}

/**
 * The wallpaper strip: a `listbox` with `aria-activedescendant`, the §H key map, a context
 * menu, a drag handle and a deferred trash with undo (see `../trash.ts`).
 */
export default function ThumbnailStrip({ vertical = false }: StripProps) {
  const idPrefix = useId()
  const [thumbs, setThumbs] = useState<Thumbnail[]>([])
  const thumbsRef = useRef(thumbs)
  thumbsRef.current = thumbs
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [active, setActive] = useState(0)
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null)
  const [pendingPath, setPendingPath] = useState<string | null>(null)
  const [notice, setNotice] = useState<Notice | null>(null)
  const pending = useRef<{ path: string; timer: ReturnType<typeof setTimeout> } | null>(null)
  const noticeTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const listRef = useRef<HTMLDivElement>(null)

  const visible = pendingPath === null ? thumbs : thumbs.filter((t) => t.path !== pendingPath)
  const activeIndex = Math.min(active, Math.max(0, visible.length - 1))
  const activeThumb: Thumbnail | undefined = visible[activeIndex]
  const optionId = (i: number) => `${idPrefix}-option-${i}`

  const say = useCallback((text: string, undo = false) => {
    clearTimeout(noticeTimer.current)
    setNotice({ text, undo })
    if (!undo) noticeTimer.current = setTimeout(() => setNotice(null), UNDO_MS)
  }, [])

  const load = useCallback(() => {
    listThumbnails()
      .then((list) => {
        setThumbs(list)
        setLoadError(null)
        const shown = list.filter((t) => t.path !== pending.current?.path)
        setActive(Math.max(0, shown.findIndex((t) => t.isCurrent)))
      })
      .catch((err: unknown) => setLoadError(errorMessage(err)))
      .finally(() => setLoading(false))
  }, [])

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
        clearTimeout(noticeTimer.current)
        setNotice((n) => (n?.undo ? null : n))
        load()
        break
      case 'changed':
        say(MESSAGES.changed)
        break
      case 'failed':
        say(errorMessage(outcome.error))
        break
    }
  }, [load, say])

  const undo = useCallback(() => {
    const p = pending.current
    if (!p) return
    pending.current = null
    clearTimeout(p.timer)
    setPendingPath(null)
    say(MESSAGES.undone)
    listRef.current?.focus()
  }, [say])

  const close = useCallback(() => {
    setMenu(null)
    void flushTrash()
    void hidePicker()
  }, [flushTrash])

  const trash = (thumb: Thumbnail) => {
    if (!thumb.isCurrent) {
      say(MESSAGES.notCurrent)
      return
    }
    void flushTrash()
    pending.current = { path: thumb.path, timer: setTimeout(() => void flushTrash(), UNDO_MS) }
    setPendingPath(thumb.path)
    say(MESSAGES.trashed, true)
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
    // Esc and undo are window-wide (below), so they also work from the toast.
    if (!action || action.kind === 'close' || action.kind === 'undo') return
    e.preventDefault()
    run(action)
  }

  // The list is `hidden` while loading, so it can only take focus once the options are in.
  const ready = !loading && loadError === null && visible.length > 0
  useEffect(() => {
    if (ready && !listRef.current?.contains(document.activeElement)) listRef.current?.focus()
  }, [ready])

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
      if (!listRef.current?.contains(document.activeElement)) listRef.current?.focus()
    }
    window.addEventListener('focus', onFocus)
    return () => window.removeEventListener('focus', onFocus)
  }, [load])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return
      const action = keyAction(e, vertical, 0, 0)
      if (action?.kind === 'close') {
        e.preventDefault()
        close()
      } else if (action?.kind === 'undo' && pending.current) {
        e.preventDefault()
        undo()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [vertical, close, undo])

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
      clearTimeout(noticeTimer.current)
    }
  }, [flushTrash])

  useEffect(() => {
    document.getElementById(optionId(activeIndex))?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' })
    // optionId only depends on the stable idPrefix.
  }, [activeIndex, visible.length])

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
    body = <p className="state">No wallpapers yet. Start the daemon, or add a local folder in Settings.</p>
  }

  return (
    <>
      <div
        className="grip"
        data-tauri-drag-region
        role="img"
        aria-label="Move picker"
        title="Move picker"
      >
        <Icon name={vertical ? 'gripHorizontal' : 'gripVertical'} />
      </div>
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
      <div className="toast-region" role="status" aria-live="polite">
        {notice && (
          <div className="toast">
            <span>{notice.text}</span>
            {notice.undo && (
              <button type="button" className="toast-action" onClick={undo}>
                Undo
              </button>
            )}
          </div>
        )}
      </div>
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
      title={label}
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
