import { useCallback, useEffect, useState } from 'react'

import type { StatusData } from '../../shared/api'
import {
  currentThumbnail,
  errorMessage,
  favoriteWallpaper,
  nextWallpaper,
  pauseWallpaper,
  prevWallpaper,
  resumeWallpaper,
  revealInFinder,
  showPicker,
  trashWallpaper,
} from '../../shared/api'
import Icon, { type IconName } from './Icon'
import type { DaemonState } from '../sections'

function fmtDate(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  return new Intl.DateTimeFormat(undefined, { timeStyle: 'short', dateStyle: 'medium' }).format(d)
}

/** `thumb://` URL of the current wallpaper, or `''` while unknown or outside the admitted folders. */
function useCurrentThumb(path: string | undefined): string {
  const [url, setUrl] = useState('')
  useEffect(() => {
    let live = true
    setUrl('')
    if (!path) return
    currentThumbnail()
      .then((thumb) => {
        if (!live) return
        // The state file can lag the daemon's status by a change; the entry flagged current
        // is still the best picture to show.
        const hit = thumb && (thumb.path === path || thumb.isCurrent) ? thumb : null
        setUrl(hit?.imageUrl ?? '')
      })
      .catch(() => {})
    return () => {
      live = false
    }
  }, [path])
  return url
}

function Action({
  onClick,
  label,
  icon,
  disabled,
}: {
  onClick: () => void
  label: string
  icon: IconName
  disabled?: boolean
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={label}
      aria-label={label}
      disabled={disabled}
      className="w-8 h-8 inline-flex items-center justify-center rounded-md text-muted hover:text-ink hover:bg-hover active:bg-pressed disabled:opacity-35 transition-colors duration-100"
    >
      <Icon name={icon} className="w-[18px] h-[18px]" />
    </button>
  )
}

/**
 * The current wallpaper and the actions the tray menu also offers. Sits at the top of the
 * General section, so the rest of the window is only settings.
 */
export default function NowShowing({
  status,
  daemon,
  onStatusChange,
  fileManager,
}: {
  status: StatusData | null
  daemon: DaemonState
  /** Called after an action so the parent can refresh the status. */
  onStatusChange: () => void
  /** "Finder", "Explorer" or "the file manager". */
  fileManager: string
}) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [broken, setBroken] = useState(false)

  const running = daemon === 'running'
  const current = running ? (status?.current ?? null) : null
  const paused = status?.paused ?? false
  const thumb = useCurrentThumb(current?.path)
  useEffect(() => setBroken(false), [thumb])

  const run = useCallback(
    async (action: () => Promise<unknown>) => {
      if (busy) return
      setBusy(true)
      setError(null)
      try {
        await action()
        onStatusChange()
      } catch (err) {
        setError(errorMessage(err))
      } finally {
        setBusy(false)
      }
    },
    [busy, onStatusChange],
  )

  let title = 'No wallpaper yet'
  let meta = ''
  if (daemon === 'checking') title = 'Connecting to the daemon…'
  else if (!running) {
    title = 'The daemon is not running'
    meta = 'Start it from the sidebar to rotate wallpapers.'
  } else if (current) {
    title = current.title || current.source || 'Untitled'
    meta = [current.source, current.set_at ? fmtDate(current.set_at) : ''].filter(Boolean).join(' · ')
  }

  return (
    <section aria-label="Now showing" className="rounded-xl bg-panel border border-line overflow-hidden">
      <div className="flex gap-4 p-3">
        <div className="shrink-0 w-40 aspect-[16/10] rounded-lg overflow-hidden bg-pressed flex items-center justify-center text-faint">
          {thumb && !broken ? (
            <img
              src={thumb}
              alt=""
              className="w-full h-full object-cover"
              draggable={false}
              onError={() => setBroken(true)}
            />
          ) : (
            <Icon name="image" className="w-6 h-6" />
          )}
        </div>
        <div className="flex-1 min-w-0 flex flex-col py-0.5">
          <p className="text-[11px] font-medium uppercase tracking-[0.06em] text-faint">Now showing</p>
          <p className="mt-1 font-semibold text-[14px] leading-snug line-clamp-2 break-words selectable" title={title}>
            {title}
          </p>
          <div role="status" aria-live="polite" className="mt-0.5 text-[12px] text-muted min-h-4">
            {error ? <span className="text-danger break-words">{error}</span> : meta}
          </div>
          {running && status && (
            <p className="mt-auto pt-2 text-[12px] text-muted flex items-center gap-1.5">
              <span
                className={`inline-block w-1.5 h-1.5 rounded-full ${paused ? 'bg-warn' : 'bg-ok'}`}
                aria-hidden="true"
              />
              {paused ? 'Paused' : `Changes every ${status.interval}`}
              {status.theme && <span className="text-faint">· {status.theme} mode</span>}
            </p>
          )}
        </div>
      </div>
      <div
        className="flex items-center gap-0.5 px-2 py-1 border-t border-line bg-[color-mix(in_srgb,var(--c-panel)_60%,var(--c-window))]"
        role="toolbar"
        aria-label="Wallpaper"
      >
        <Action
          onClick={() => void run(prevWallpaper)}
          label="Previous wallpaper"
          icon="chevronLeft"
          disabled={busy || !running || (status?.history_index ?? 0) <= 0}
        />
        <Action onClick={() => void run(nextWallpaper)} label="Next wallpaper" icon="chevronRight" disabled={busy || !running} />
        <Action
          onClick={() => void run(paused ? resumeWallpaper : pauseWallpaper)}
          label={paused ? 'Resume rotation' : 'Pause rotation'}
          icon={paused ? 'play' : 'pause'}
          disabled={busy || !running}
        />
        <span className="w-px h-4 bg-line mx-1" aria-hidden="true" />
        <Action onClick={() => void run(() => favoriteWallpaper())} label="Favorite" icon="heart" disabled={busy || !current} />
        <Action onClick={() => void run(trashWallpaper)} label="Move to Trash" icon="trash" disabled={busy || !current} />
        <Action
          onClick={() => {
            if (current?.path) void run(() => revealInFinder(current.path))
          }}
          label={`Show in ${fileManager}`}
          icon="folderOpen"
          disabled={!current?.path}
        />
        <span className="flex-1" />
        <button type="button" className="btn-plain" onClick={() => void run(showPicker)}>
          Open picker
        </button>
      </div>
    </section>
  )
}
