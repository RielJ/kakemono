import { useCallback, useEffect, useMemo, useState } from 'react'

import type { AppInfo, Config, SaveOutcome, StatusData } from '../shared/api'
import {
  closeWindow,
  errorMessage,
  getAppInfo,
  getConfig,
  getStatus,
  isDaemonNotRunning,
  resetPickerPosition,
  saveConfig,
  startDaemon,
} from '../shared/api'
import { useAppearance } from '../shared/useAppearance'
import { useDaemonEvents, type Connection } from '../shared/useDaemonEvents'
import { useImageDrop, type DropNotice } from '../shared/useImageDrop'
import AdvancedSettings from './components/AdvancedSettings'
import GeneralSettings from './components/GeneralSettings'
import Icon from './components/Icon'
import Tabs, { tabId } from './components/Tabs'
import UpdateBanner from './components/UpdateBanner'
import { fileManagerName, TABS, type DaemonState, type TabId } from './sections'
import { useConfigStore, type SaveStatus } from './useConfigStore'
import { useUpdate } from './useUpdate'
import { configErrors } from './validate'

// ── Save status ──────────────────────────────────────────────────────────────

function savedText(outcome: SaveOutcome): string {
  if (outcome.reloaded) return 'Saved'
  if (outcome.reload_error) return `Saved, but the daemon did not reload: ${outcome.reload_error}`
  return 'Saved. Applies when the daemon starts.'
}

/** Where every change reports back: a quiet line in the section header, never a button bar. */
function SaveIndicator({
  status,
  onRetry,
  onRevert,
}: {
  status: SaveStatus
  onRetry: () => void
  onRevert: () => void
}) {
  // "Saved" fades after a moment; warnings and errors stay until the next change.
  const [fresh, setFresh] = useState(true)
  useEffect(() => {
    setFresh(true)
    if (status.kind !== 'saved' || status.outcome.reload_error) return
    const t = setTimeout(() => setFresh(false), 2000)
    return () => clearTimeout(t)
  }, [status])

  let body: React.ReactNode = null
  if (status.kind === 'saving') body = <span className="text-muted">Saving…</span>
  else if (status.kind === 'saved') {
    const warn = Boolean(status.outcome.reload_error)
    body = (
      <span className={`inline-flex items-center gap-1 ${warn ? 'text-warn' : 'text-muted'}`}>
        {!warn && <Icon name="check" className="w-3.5 h-3.5" />}
        <span className="break-words">{savedText(status.outcome)}</span>
      </span>
    )
  } else if (status.kind === 'invalid')
    body = (
      <span className="inline-flex items-center gap-2 text-danger">
        Fix the highlighted field to save.
        <button type="button" className="btn-plain h-6 px-1.5 text-[12px]" onClick={onRevert}>
          Undo
        </button>
      </span>
    )
  else if (status.kind === 'failed')
    body = (
      <span className="inline-flex items-center gap-2 text-danger">
        <span className="break-words">Not saved: {status.message}</span>
        <button type="button" className="btn-plain h-6 px-1.5 text-[12px]" onClick={onRetry}>
          Retry
        </button>
        <button type="button" className="btn-plain h-6 px-1.5 text-[12px]" onClick={onRevert}>
          Undo
        </button>
      </span>
    )

  const hidden = status.kind === 'idle' || (status.kind === 'saved' && !status.outcome.reload_error && !fresh)
  return (
    <p
      role="status"
      aria-live="polite"
      className={`text-[12px] text-right max-w-72 transition-opacity duration-300 ${hidden ? 'opacity-0' : 'opacity-100'}`}
    >
      {body}
    </p>
  )
}

// ── Dropped image ────────────────────────────────────────────────────────────

/** A dropped image's progress and result, beside the save status in the footer. */
function useDropNotice(): [DropNotice | null, (notice: DropNotice) => void] {
  const [notice, setNotice] = useState<DropNotice | null>(null)
  // "Set from …" fades like "Saved"; errors stay until the next drop.
  useEffect(() => {
    if (notice?.kind !== 'done') return
    const t = setTimeout(() => setNotice(null), 4000)
    return () => clearTimeout(t)
  }, [notice])
  return [notice, setNotice]
}

function DropStatus({ notice }: { notice: DropNotice | null }) {
  return (
    <p role="status" aria-live="polite" className="text-[12px] text-right max-w-72 min-w-0 break-words">
      {notice && <span className={notice.kind === 'error' ? 'text-danger' : 'text-muted'}>{notice.text}</span>}
    </p>
  )
}

/** Shown while an image is dragged over the window; the drop itself is `useImageDrop`. */
function DropOverlay() {
  return (
    <div
      aria-hidden="true"
      className="fixed inset-2 z-50 pointer-events-none flex items-center justify-center gap-2 rounded-xl border-2 border-dashed border-accent bg-[color-mix(in_srgb,var(--c-window)_85%,transparent)] text-ink font-semibold"
    >
      <Icon name="image" className="w-5 h-5 text-accent" />
      Drop an image to set it
    </div>
  )
}

// ── Daemon status ────────────────────────────────────────────────────────────

/** Whether the daemon is running, with a Start button when it is not. */
function DaemonStatus({ daemon, onStart, starting }: { daemon: DaemonState; onStart: () => void; starting: boolean }) {
  const dot = daemon === 'running' ? 'bg-ok' : daemon === 'stopped' ? 'bg-danger' : 'bg-faint'
  const text = starting
    ? 'Starting daemon…'
    : daemon === 'running'
      ? 'Daemon running'
      : daemon === 'stopped'
        ? 'Daemon stopped'
        : 'Connecting…'
  return (
    <div className="flex items-center gap-2 min-w-0" role="status" aria-live="polite">
      <span className={`w-2 h-2 rounded-full shrink-0 ${dot}`} aria-hidden="true" />
      <span className="text-[12px] text-muted truncate">{text}</span>
      {daemon === 'stopped' && !starting && (
        <button type="button" className="btn-plain h-6 px-1.5 text-[12px]" onClick={onStart}>
          Start
        </button>
      )}
    </div>
  )
}

// ── Loading and error panes ──────────────────────────────────────────────────

function LoadingPane() {
  return (
    <div className="py-16 text-center" role="status">
      <p className="text-muted">Loading settings…</p>
    </div>
  )
}

function ErrorPane({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <div className="py-16 flex justify-center">
      <div className="text-center space-y-3 max-w-sm" role="alert">
        <Icon name="alert" className="w-6 h-6 text-danger mx-auto" />
        <h3 className="font-semibold">Could not load settings</h3>
        <p className="text-[12px] text-muted break-words">{message}</p>
        <button type="button" onClick={onRetry} className="btn btn-accent mx-auto">
          Try again
        </button>
      </div>
    </div>
  )
}

// ── Root ─────────────────────────────────────────────────────────────────────

export default function App() {
  const [activeTab, setActiveTab] = useState<TabId>('general')
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null)
  const [status, setStatus] = useState<StatusData | null>(null)
  const [daemon, setDaemon] = useState<DaemonState>('checking')
  const [starting, setStarting] = useState(false)
  const [startError, setStartError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [dropNotice, setDropNotice] = useDropNotice()
  // A dropped image becomes the wallpaper; the daemon's `changed` event updates Now showing.
  const drop = useImageDrop({ onNotice: setDropNotice })
  const update = useUpdate()

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await getStatus())
      setDaemon('running')
    } catch (err) {
      setStatus(null)
      setDaemon(isDaemonNotRunning(err) ? 'stopped' : 'running')
    }
  }, [])

  const store = useConfigStore({
    save: saveConfig,
    onSaved: (outcome) => {
      if (outcome.reloaded) void refreshStatus()
    },
  })
  const { config, savedConfig, load, change, written, revert, flush, setStatus: setSaveStatus } = store

  useAppearance(config?.ui.appearance ?? 'light')

  const loadAll = useCallback(async () => {
    setLoading(true)
    setLoadError(null)
    try {
      const loaded = await getConfig()
      load(loaded)
    } catch (err) {
      setLoadError(errorMessage(err))
    } finally {
      setLoading(false)
    }
  }, [load])

  useEffect(() => {
    void loadAll()
    void refreshStatus()
    getAppInfo()
      .then((info) => {
        setAppInfo(info)
        document.documentElement.dataset.os = info.os
      })
      .catch(() => {})
  }, [loadAll, refreshStatus])

  // Live updates from the daemon's `subscribe` stream (src-tauri/src/events.rs).
  const [connection, setConnection] = useState<Connection['state']>('connecting')
  useDaemonEvents({
    onConnection: (c) => {
      setConnection(c.state)
      if (c.state === 'connected') {
        setDaemon('running')
        if (c.status) setStatus(c.status)
        else void refreshStatus()
      } else if (c.state === 'not_running') {
        setDaemon('stopped')
        setStatus(null)
      } else if (c.state === 'error') {
        // Reachable but not subscribable (e.g. an older daemon): `status` decides.
        void refreshStatus()
      }
    },
    onChanged: (current) => {
      setStatus((s) => (s ? { ...s, current } : s))
      // History position and count moved too.
      void refreshStatus()
    },
    onPaused: (paused) => setStatus((s) => (s ? { ...s, paused } : s)),
  })

  // Without a subscription (an `error` state), fall back to polling.
  useEffect(() => {
    if (connection !== 'error') return
    const timer = setInterval(() => void refreshStatus(), 10_000)
    return () => clearInterval(timer)
  }, [connection, refreshStatus])

  // Write a pending change before the window goes away.
  useEffect(() => {
    const onHide = () => {
      if (document.visibilityState === 'hidden') void flush()
    }
    document.addEventListener('visibilitychange', onHide)
    window.addEventListener('blur', onHide)
    return () => {
      document.removeEventListener('visibilitychange', onHide)
      window.removeEventListener('blur', onHide)
    }
  }, [flush])

  const errors = useMemo(() => (config ? configErrors(config) : {}), [config])

  // Written to disk by the backend; update both copies and keep other edits pending.
  const handleResetPicker = useCallback(async () => {
    try {
      await resetPickerPosition()
      written((c) => ({ ...c, picker: { ...c.picker, placement: 'edge', custom: null } }))
    } catch (err) {
      setSaveStatus({ kind: 'failed', message: errorMessage(err) })
    }
  }, [written, setSaveStatus])

  const handleStartDaemon = useCallback(async () => {
    setStarting(true)
    setStartError(null)
    try {
      const started = await startDaemon()
      await refreshStatus()
      if (!started) setStartError('The daemon did not start. Run "kakemono daemon" in a terminal to see why.')
    } catch (err) {
      setStartError(errorMessage(err))
    } finally {
      setStarting(false)
    }
  }, [refreshStatus])

  // ⌘S / Ctrl+S writes now; ⌘W / Ctrl+W writes, then closes the window (the app stays in the tray).
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return
      const key = e.key.toLowerCase()
      if (key === 's') {
        e.preventDefault()
        void flush()
      } else if (key === 'w') {
        e.preventDefault()
        void flush().finally(() => void closeWindow())
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [flush])

  const fileManager = fileManagerName(appInfo?.os)

  // Each tab mounts the existing sections for now; Tasks 21–23 regroup them.
  let content: React.ReactNode = null
  if (loading) content = <LoadingPane />
  else if (loadError) content = <ErrorPane message={loadError} onRetry={() => void loadAll()} />
  else if (config) {
    switch (activeTab) {
      case 'general':
        content = (
          <GeneralSettings
            config={config}
            onChange={change}
            onCommit={store.commit}
            errors={errors}
            status={status}
            daemon={daemon}
            onStatusChange={() => void refreshStatus()}
            fileManager={fileManager}
            os={appInfo?.os}
            onResetPosition={() => void handleResetPicker()}
          />
        )
        break
      case 'advanced':
        content = (
          <AdvancedSettings
            config={config}
            savedConfig={savedConfig}
            onChange={change}
            onWritten={written}
            errors={errors}
            appInfo={appInfo}
            fileManager={fileManager}
          />
        )
        break
      default: {
        const _exhaustive: never = activeTab
        content = _exhaustive
      }
    }
  }

  return (
    <div className="flex flex-col h-screen bg-window text-ink">
      <header className="shrink-0 flex justify-center px-6 py-3 bg-sidebar border-b border-line">
        <Tabs tabs={TABS} value={activeTab} onChange={setActiveTab} panelId="settings-panel" label="Settings" />
      </header>
      <main
        id="settings-panel"
        role="tabpanel"
        aria-labelledby={tabId(activeTab)}
        className="flex-1 min-h-0 overflow-y-auto"
      >
        <div className="max-w-[640px] mx-auto px-8 pt-6 pb-10">
          <UpdateBanner update={update} />
          {startError && (
            <p role="alert" className="mb-4 text-[12px] text-danger break-words">
              {startError}
            </p>
          )}
          <div className="space-y-7">{content}</div>
        </div>
      </main>
      <footer className="shrink-0 flex items-center justify-between gap-4 px-4 min-h-10 bg-sidebar border-t border-line">
        <DaemonStatus daemon={daemon} onStart={() => void handleStartDaemon()} starting={starting} />
        <div className="flex items-center gap-3 min-w-0">
          <DropStatus notice={dropNotice} />
          {config && <SaveIndicator status={store.status} onRetry={() => void flush()} onRevert={revert} />}
        </div>
      </footer>
      {drop.over && <DropOverlay />}
    </div>
  )
}

export type { Config }
