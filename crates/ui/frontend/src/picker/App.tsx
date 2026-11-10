import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { getAppInfo, getConfig, hidePicker, onPickerShown } from '../shared/api'
import { useImageDrop } from '../shared/useImageDrop'
import BrowseStrip from './components/BrowseStrip'
import Icon from './components/Icon'
import TabBar, { PANEL_ID } from './components/TabBar'
import ThumbnailStrip, { type Notices } from './components/ThumbnailStrip'
import { keyAction } from './keys'
import { DEFAULT_LAYOUT, isVertical, toLayout } from './orientation'
import { DEFAULT_TAB, nextTab, tabAt, type PickerTabId } from './tabs'
import { UNDO_MS } from './trash'

interface Notice {
  text: string
  /** Shows the Undo button (a trash is pending). */
  onUndo?: () => void
}

/** A form control handles its own arrow keys (the Sources filter is a radio group). */
function isFormControl(target: EventTarget | null): boolean {
  return target instanceof HTMLInputElement || target instanceof HTMLSelectElement || target instanceof HTMLTextAreaElement
}

export default function App() {
  const [layout, setLayout] = useState(DEFAULT_LAYOUT)
  const [blurNsfw, setBlurNsfw] = useState(true)
  const [tab, setTab] = useState<PickerTabId>(DEFAULT_TAB)
  // Bumped on every show: the strip remounts, so it starts again at the first page, scrolled
  // to the start.
  const [shows, setShows] = useState(0)
  const [notice, setNotice] = useState<Notice | null>(null)
  const noticeTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)

  useEffect(() => {
    // The window is hidden, not closed, and Rust re-sizes it from the config on every show:
    // re-read `[picker] position` / `orientation` each time it is shown so the layout matches.
    let current = true
    const loadLayout = () => {
      getConfig()
        .then((cfg) => {
          if (!current) return
          setLayout(toLayout(cfg.picker))
          // Older configs have no key: the default is to blur.
          setBlurNsfw(cfg.picker.blur_nsfw !== false)
        })
        .catch(() => {})
    }
    loadLayout()
    window.addEventListener('focus', loadLayout)
    // 13 px type on macOS (tokens.css keys off data-os).
    getAppInfo()
      .then((info) => {
        document.documentElement.dataset.os = info.os
      })
      .catch(() => {})
    return () => {
      current = false
      window.removeEventListener('focus', loadLayout)
    }
  }, [])

  // The picker always opens on Sources (`picker.rs` emits `picker-shown` on every show). A
  // window `focus` only refreshes the open tab, so a picker kept open is not reset mid-use.
  useEffect(() => {
    let disposed = false
    let unlisten: (() => void) | undefined
    onPickerShown(() => {
      setTab(DEFAULT_TAB)
      setShows((n) => n + 1)
    })
      .then((un) => (disposed ? un() : (unlisten = un)))
      .catch(() => {})
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  const say = useCallback((text: string, onUndo?: () => void) => {
    clearTimeout(noticeTimer.current)
    setNotice({ text, onUndo })
    if (!onUndo) noticeTimer.current = setTimeout(() => setNotice(null), UNDO_MS)
  }, [])
  const dismissUndo = useCallback(() => {
    setNotice((n) => (n?.onUndo ? null : n))
  }, [])
  const notices = useMemo<Notices>(() => ({ say, dismissUndo }), [say, dismissUndo])
  useEffect(() => () => clearTimeout(noticeTimer.current), [])

  // A dropped image file or URL is set as the wallpaper, from any tab; the daemon's `changed`
  // event then reloads an on-disk strip with it.
  const drop = useImageDrop({ onNotice: (n) => say(n.text) })

  const vertical = isVertical(layout)

  // Window-wide keys: Esc hides the picker, and the tab keys work from anywhere, even an empty
  // tab with nothing focused. The strip leaves these keys alone; the tab bar claims its own
  // arrows (`preventDefault`), so they are not handled twice.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return
      const action = keyAction(e, vertical, 0, 0)
      switch (action?.kind) {
        case 'close':
          // Not claimed: the on-disk strip also hears it and sends a pending trash first.
          void hidePicker()
          break
        case 'tab':
          if (isFormControl(e.target)) return
          e.preventDefault()
          setTab((t) => nextTab(t, action.delta))
          break
        case 'tabIndex': {
          const to = tabAt(action.index)
          if (to === null) return
          e.preventDefault()
          setTab(to)
          break
        }
        default:
          break
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [vertical])

  let panel: React.ReactNode
  if (tab === 'sources') {
    panel = <BrowseStrip key={`sources:${shows}`} vertical={vertical} notices={notices} blurNsfw={blurNsfw} />
  } else {
    panel = <ThumbnailStrip key={`${tab}:${shows}`} tab={tab} vertical={vertical} notices={notices} />
  }

  // Hiding on blur is handled in Rust (`picker.rs`, honours `[picker] close_on_blur`).
  return (
    <main className={`app ${vertical ? 'app-vertical' : 'app-horizontal'}`}>
      {/* Horizontal: the tab column carries the drag handle. Vertical: a grip row on top. */}
      {vertical && (
        <div className="grip" data-tauri-drag-region role="img" aria-label="Move picker" title="Move picker">
          <Icon name="gripHorizontal" />
        </div>
      )}
      <TabBar active={tab} vertical={vertical} onSelect={setTab} />
      <div className="panel" role="tabpanel" id={PANEL_ID} aria-labelledby={`picker-tab-${tab}`}>
        {panel}
      </div>
      {drop.over && (
        <div className="drop-overlay" aria-hidden="true">
          <Icon name="image" />
          <span>Drop an image to set it</span>
        </div>
      )}
      <div className="toast-region" role="status" aria-live="polite">
        {notice && (
          <div className="toast">
            <span>{notice.text}</span>
            {notice.onUndo && (
              <button type="button" className="toast-action" onClick={notice.onUndo}>
                Undo
              </button>
            )}
          </div>
        )}
      </div>
    </main>
  )
}
