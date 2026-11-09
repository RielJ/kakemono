import { useCallback, useState, type ReactNode } from 'react'

import type { StatusData } from '../../shared/api'
import {
  errorMessage,
  favoriteWallpaper,
  nextWallpaper,
  pauseWallpaper,
  prevWallpaper,
  resumeWallpaper,
  revealInFinder,
  trashWallpaper,
} from '../../shared/api'
import Icon, { type IconName } from './Icon'

/** What the settings window knows about the daemon. */
export type DaemonState = 'checking' | 'running' | 'stopped'

interface StatusBarProps {
  status: StatusData | null
  daemon: DaemonState
  /** Called after an action so the parent can refresh the status. */
  onStatusChange: () => void
  onStartDaemon: () => Promise<void>
  /** "Finder", "Explorer" or "the file manager". */
  fileManager: string
}

function ActionBtn({
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
      className="p-1.5 rounded-md text-gray-600 hover:text-gray-900 hover:bg-gray-200/70 dark:text-gray-300 dark:hover:text-white dark:hover:bg-gray-700 transition-colors duration-100 motion-reduce:transition-none focus-visible:outline-2 focus-visible:outline-accent disabled:opacity-40 disabled:cursor-not-allowed"
    >
      <Icon name={icon} />
    </button>
  )
}

function fmtDate(iso: string): string {
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  return new Intl.DateTimeFormat(undefined, { timeStyle: 'short', dateStyle: 'short' }).format(d)
}

export default function StatusBar({ status, daemon, onStatusChange, onStartDaemon, fileManager }: StatusBarProps) {
  const [busy, setBusy] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)

  const run = useCallback(
    async (action: () => Promise<unknown>) => {
      if (busy) return
      setBusy(true)
      setActionError(null)
      try {
        await action()
        onStatusChange()
      } catch (err) {
        setActionError(errorMessage(err))
      } finally {
        setBusy(false)
      }
    },
    [busy, onStatusChange],
  )

  const running = daemon === 'running'
  const current = running ? status?.current ?? null : null
  const paused = status?.paused ?? false

  let info: ReactNode
  if (daemon === 'checking') {
    info = <p className="text-xs text-gray-500 dark:text-gray-400">Connecting to the daemon…</p>
  } else if (!running) {
    info = (
      <div className="flex items-center gap-2">
        <p className="text-xs text-gray-600 dark:text-gray-300">Daemon not running</p>
        <button type="button" onClick={() => void run(onStartDaemon)} disabled={busy} className="btn-secondary text-xs py-0.5 px-2">
          {busy ? 'Starting…' : 'Start'}
        </button>
      </div>
    )
  } else if (current) {
    const when = current.set_at ? fmtDate(current.set_at) : ''
    info = (
      <>
        <p className="text-xs font-medium truncate leading-tight">{current.title || current.source || 'Untitled'}</p>
        <p className="text-xs text-gray-500 dark:text-gray-400 truncate leading-tight mt-0.5">
          {[current.source, status?.theme, when].filter(Boolean).join(' · ')}
        </p>
      </>
    )
  } else {
    info = <p className="text-xs text-gray-500 dark:text-gray-400">No wallpaper set yet</p>
  }

  return (
    <header className="flex items-center gap-3 px-4 py-2.5 min-h-14 bg-gray-50 dark:bg-gray-800 border-b border-gray-200 dark:border-gray-700/70">
      <div className="shrink-0 w-14 h-9 rounded-sm overflow-hidden bg-gray-200 dark:bg-gray-700 flex items-center justify-center text-gray-400 dark:text-gray-500">
        <Icon name="image" className="w-5 h-5" />
      </div>

      <div className="flex-1 min-w-0" role="status" aria-live="polite">
        {info}
        {actionError && <p className="text-xs text-red-600 dark:text-red-400 truncate mt-0.5">{actionError}</p>}
      </div>

      {running && status && (
        <span
          className={`hidden sm:inline-flex shrink-0 items-center gap-1.5 text-xs px-2 py-0.5 rounded-full ${
            paused
              ? 'bg-amber-100 text-amber-800 dark:bg-amber-900/40 dark:text-amber-300'
              : 'bg-gray-200/70 text-gray-700 dark:bg-gray-700 dark:text-gray-200'
          }`}
        >
          {paused ? 'Paused' : `Every ${status.interval}`}
        </span>
      )}

      <div className="shrink-0 flex items-center gap-0.5" role="toolbar" aria-label="Wallpaper">
        <ActionBtn
          onClick={() => void run(prevWallpaper)}
          label="Previous wallpaper"
          icon="chevronLeft"
          disabled={busy || !running || (status?.history_index ?? 0) <= 0}
        />
        <ActionBtn onClick={() => void run(nextWallpaper)} label="Next wallpaper" icon="chevronRight" disabled={busy || !running} />
        <div className="w-px h-4 bg-gray-300 dark:bg-gray-600 mx-1" aria-hidden="true" />
        <ActionBtn onClick={() => void run(() => favoriteWallpaper())} label="Favorite" icon="heart" disabled={busy || !current} />
        <ActionBtn onClick={() => void run(trashWallpaper)} label="Move to Trash" icon="trash" disabled={busy || !current} />
        <ActionBtn
          onClick={() => {
            if (current?.path) void run(() => revealInFinder(current.path))
          }}
          label={`Show in ${fileManager}`}
          icon="folderOpen"
          disabled={!current?.path}
        />
        <div className="w-px h-4 bg-gray-300 dark:bg-gray-600 mx-1" aria-hidden="true" />
        <ActionBtn
          onClick={() => void run(paused ? resumeWallpaper : pauseWallpaper)}
          label={paused ? 'Resume rotation' : 'Pause rotation'}
          icon={paused ? 'play' : 'pause'}
          disabled={busy || !running}
        />
      </div>
    </header>
  )
}
