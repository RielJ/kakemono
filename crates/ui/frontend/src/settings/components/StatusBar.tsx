import { useState, useCallback } from 'react'
import type { StatusData } from '../types/config'
import {
  nextWallpaper,
  prevWallpaper,
  favoriteWallpaper,
  trashWallpaper,
  pauseWallpaper,
  resumeWallpaper,
  revealInFinder,
} from '../../shared/api'

interface StatusBarProps {
  status: StatusData | null
  thumbnail: string            // relative URL (/thumb?path=...) or ''
  onStatusChange: () => void   // called after an action so parent can refresh
}

function ActionBtn({ onClick, title, disabled, children, variant = 'default' }: {
  onClick: () => void; title: string; disabled?: boolean
  children: React.ReactNode; variant?: 'default' | 'red' | 'green'
}) {
  const colors = {
    default: 'text-gray-500 hover:text-gray-800 dark:text-gray-400 dark:hover:text-gray-100',
    red:     'text-gray-500 hover:text-red-500  dark:text-gray-400 dark:hover:text-red-400',
    green:   'text-gray-500 hover:text-green-600 dark:text-gray-400 dark:hover:text-green-400',
  }
  return (
    <button onClick={onClick} title={title} disabled={disabled}
      className={`no-drag p-1.5 rounded-md transition-colors duration-100 focus:outline-none focus:ring-2 focus:ring-accent/40 disabled:opacity-40 disabled:cursor-not-allowed ${colors[variant]}`}>
      {children}
    </button>
  )
}

export default function StatusBar({ status, thumbnail, onStatusChange }: StatusBarProps) {
  const [busy, setBusy] = useState(false)

  const run = useCallback(async (action: () => Promise<void>) => {
    if (busy) return
    setBusy(true)
    try { await action(); onStatusChange() }
    catch (err) { console.error('Action failed:', err) }
    finally { setBusy(false) }
  }, [busy, onStatusChange])

  const current = status?.current
  const paused  = status?.paused ?? false
  const running = status?.running ?? false

  const fmtDate = (iso: string) => {
    try { return new Intl.DateTimeFormat(undefined, { timeStyle: 'short', dateStyle: 'short' }).format(new Date(iso)) }
    catch { return '' }
  }

  return (
    <div className="drag-region flex items-center gap-3 px-4 py-2.5 bg-gray-50/90 dark:bg-gray-800/90 border-b border-gray-200/80 dark:border-gray-700/60 backdrop-blur-sm" style={{ minHeight: 56 }}>
      {/* Thumbnail — served via the /thumb asset handler (FileHandler in app.go) */}
      <div className="no-drag flex-shrink-0 w-14 h-9 rounded overflow-hidden bg-gray-200 dark:bg-gray-700 flex items-center justify-center">
        {thumbnail ? (
          /* thumbnail is a relative URL: /thumb?path=<encoded> */
          <img
            src={thumbnail}
            alt="Current wallpaper"
            className="w-full h-full object-cover"
            onError={e => { (e.currentTarget as HTMLImageElement).style.display = 'none' }}
          />
        ) : current?.path ? (
          /* fallback: construct the URL from the path if GetThumbnail was skipped */
          <img
            src={`/thumb?path=${encodeURIComponent(current.path)}`}
            alt="Current wallpaper"
            className="w-full h-full object-cover"
            onError={e => { (e.currentTarget as HTMLImageElement).style.display = 'none' }}
          />
        ) : (
          <svg viewBox="0 0 20 20" fill="currentColor" className="w-6 h-6 text-gray-300 dark:text-gray-600">
            <path fillRule="evenodd" d="M4 3a2 2 0 00-2 2v10a2 2 0 002 2h12a2 2 0 002-2V5a2 2 0 00-2-2H4zm12 12H4l4-8 3 6 2-4 3 6z" clipRule="evenodd" />
          </svg>
        )}
      </div>

      {/* Info */}
      <div className="no-drag flex-1 min-w-0">
        {!running ? (
          <p className="text-xs text-gray-400 dark:text-gray-500">Daemon not running</p>
        ) : current ? (
          <>
            <p className="text-xs font-medium text-gray-800 dark:text-gray-100 truncate leading-tight">
              {current.title || current.source || 'Untitled'}
            </p>
            <p className="text-xs text-gray-400 dark:text-gray-500 truncate leading-tight mt-0.5">
              {[current.source, status?.theme, status?.interval].filter(Boolean).join(' · ')}
              {current.set_at ? ` · ${fmtDate(current.set_at)}` : ''}
            </p>
          </>
        ) : (
          <p className="text-xs text-gray-400 dark:text-gray-500">No wallpaper set</p>
        )}
      </div>

      {/* Status badge */}
      {status && running && (
        <div className="no-drag flex-shrink-0 hidden sm:flex items-center gap-1.5">
          <span className={`inline-flex items-center gap-1 text-xs px-2 py-0.5 rounded-full font-medium ${paused ? 'bg-yellow-100 text-yellow-700 dark:bg-yellow-900/40 dark:text-yellow-400' : 'bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-400'}`}>
            <span className={`w-1.5 h-1.5 rounded-full ${paused ? 'bg-yellow-500' : 'bg-green-500'}`} />
            {paused ? 'Paused' : status.interval}
          </span>
        </div>
      )}

      {/* Actions */}
      <div className="no-drag flex-shrink-0 flex items-center gap-0.5">
        <ActionBtn onClick={() => run(prevWallpaper)} title="Previous" disabled={busy || !running || (status?.history_index ?? 0) === 0}>‹</ActionBtn>
        <ActionBtn onClick={() => run(nextWallpaper)} title="Next" disabled={busy || !running}>›</ActionBtn>
        <div className="w-px h-4 bg-gray-300 dark:bg-gray-600 mx-1" />
        <ActionBtn onClick={() => run(favoriteWallpaper)} title="Favorite" disabled={busy || !running || !current} variant="green">♥</ActionBtn>
        <ActionBtn onClick={() => run(trashWallpaper)} title="Trash" disabled={busy || !running || !current} variant="red">🗑</ActionBtn>
        <ActionBtn onClick={() => { if (current?.path) revealInFinder(current.path) }} title="Reveal in Finder" disabled={!running || !current}>📂</ActionBtn>
        <div className="w-px h-4 bg-gray-300 dark:bg-gray-600 mx-1" />
        <ActionBtn onClick={() => run(paused ? resumeWallpaper : pauseWallpaper)} title={paused ? 'Resume' : 'Pause'} disabled={busy || !running} variant={paused ? 'green' : 'default'}>
          {paused ? '▶' : '⏸'}
        </ActionBtn>
      </div>
    </div>
  )
}
