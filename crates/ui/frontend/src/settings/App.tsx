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
import DesktopSettings from './DesktopSettings'
import HooksSettings from './HooksSettings'
import FavoritesSettings from './components/FavoritesSettings'
import GeneralSettings from './components/GeneralSettings'
import Icon from './components/Icon'
import Sidebar from './components/Sidebar'
import SourcesSettings from './components/SourcesSettings'
import StatusBar, { type DaemonState } from './components/StatusBar'
import ThemesSettings from './components/ThemesSettings'
import { diffConfig, isEmptyPatch } from './configPatch'
import { fileManagerName, TAB_TITLES, type TabId } from './sections'
import { configErrors, hasErrors } from './validate'
import { useDaemonEvents, type Connection } from '../shared/useDaemonEvents'

// ── Save bar ─────────────────────────────────────────────────────────────────

type SaveState =
  | { kind: 'idle' }
  | { kind: 'saving' }
  | { kind: 'saved'; outcome: SaveOutcome }
  | { kind: 'failed'; message: string }

function savedText(outcome: SaveOutcome): string {
  if (outcome.reloaded) return 'Saved. The daemon is using the new settings.'
  if (outcome.reload_error) return `Saved, but the daemon did not reload: ${outcome.reload_error}`
  return 'Saved. The daemon will use these settings when it starts.'
}

function SaveBar({
  dirty,
  invalid,
  state,
  onSave,
  onDiscard,
}: {
  dirty: boolean
  invalid: boolean
  state: SaveState
  onSave: () => void
  onDiscard: () => void
}) {
  const saving = state.kind === 'saving'
  let message: string | null = null
  let tone = 'text-gray-600 dark:text-gray-300'
  if (state.kind === 'failed') {
    message = `Save failed: ${state.message}`
    tone = 'text-red-700 dark:text-red-300'
  } else if (dirty && invalid) {
    message = 'Fix the highlighted fields to save.'
    tone = 'text-red-700 dark:text-red-300'
  } else if (dirty) {
    message = 'Unsaved changes'
  } else if (state.kind === 'saved') {
    message = savedText(state.outcome)
    tone = state.outcome.reload_error ? 'text-amber-700 dark:text-amber-300' : 'text-gray-600 dark:text-gray-300'
  }

  return (
    <div className="flex items-center justify-between gap-3 px-4 py-2 min-h-11 border-t border-gray-200 dark:border-gray-700/70 bg-gray-50 dark:bg-gray-800">
      <p className={`text-xs flex items-center gap-1.5 min-w-0 ${tone}`} role="status" aria-live="polite">
        {state.kind === 'saved' && !dirty && !state.outcome.reload_error && <Icon name="check" className="w-3.5 h-3.5 shrink-0" />}
        <span className="truncate">{message}</span>
      </p>
      <div className="flex items-center gap-2 shrink-0">
        <button type="button" onClick={onDiscard} disabled={!dirty || saving} className="btn-secondary text-xs py-1">
          Discard
        </button>
        <button type="button" onClick={onSave} disabled={!dirty || invalid || saving} className="btn-primary text-xs py-1">
          {saving ? 'Saving…' : 'Save'}
        </button>
      </div>
    </div>
  )
}

// ── Loading and error panes ──────────────────────────────────────────────────

function LoadingPane() {
  return (
    <div className="flex-1 flex items-center justify-center" role="status">
      <p className="text-sm text-gray-500 dark:text-gray-400">Loading settings…</p>
    </div>
  )
}

function ErrorPane({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <div className="flex-1 flex items-center justify-center p-8">
      <div className="text-center space-y-3 max-w-sm" role="alert">
        <Icon name="alert" className="w-6 h-6 text-red-500 mx-auto" />
        <h3 className="text-sm font-semibold">Could not load settings</h3>
        <p className="text-xs text-gray-600 dark:text-gray-400 break-words">{message}</p>
        <button type="button" onClick={onRetry} className="btn-primary mx-auto">
          Try again
        </button>
      </div>
    </div>
  )
}


// ── Root ─────────────────────────────────────────────────────────────────────

export default function App() {
  const [activeTab, setActiveTab] = useState<TabId>('general')
  const [config, setConfig] = useState<Config | null>(null)
  const [savedConfig, setSavedConfig] = useState<Config | null>(null)
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null)
  const [status, setStatus] = useState<StatusData | null>(null)
  const [daemon, setDaemon] = useState<DaemonState>('checking')
  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [saveState, setSaveState] = useState<SaveState>({ kind: 'idle' })

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await getStatus())
      setDaemon('running')
    } catch (err) {
      setStatus(null)
      setDaemon(isDaemonNotRunning(err) ? 'stopped' : 'running')
    }
  }, [])

  const loadAll = useCallback(async () => {
    setLoading(true)
    setLoadError(null)
    try {
      const cfg = await getConfig()
      setConfig(cfg)
      setSavedConfig(cfg)
    } catch (err) {
      setLoadError(errorMessage(err))
    } finally {
      setLoading(false)
    }
  }, [])

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

  const errors = useMemo(() => (config ? configErrors(config) : {}), [config])
  const invalid = hasErrors(errors)
  const patch = useMemo(() => (config && savedConfig ? diffConfig(savedConfig, config) : {}), [config, savedConfig])
  const dirty = !isEmptyPatch(patch)

  const handleSave = useCallback(async () => {
    if (!config || !dirty || invalid) return
    setSaveState({ kind: 'saving' })
    try {
      const outcome = await saveConfig(patch)
      setSavedConfig(config)
      setSaveState({ kind: 'saved', outcome })
      if (outcome.reloaded) void refreshStatus()
    } catch (err) {
      setSaveState({ kind: 'failed', message: errorMessage(err) })
    }
  }, [config, dirty, invalid, patch, refreshStatus])

  const handleDiscard = useCallback(() => {
    if (savedConfig) setConfig(savedConfig)
    setSaveState({ kind: 'idle' })
  }, [savedConfig])

  const handleChange = useCallback((next: Config) => {
    setConfig(next)
    setSaveState((s) => (s.kind === 'failed' ? { kind: 'idle' } : s))
  }, [])

  // A change another command already wrote to disk (Pin, detect --write): apply it to the saved
  // copy and the edited copy, keeping other edits pending.
  const handleWritten = useCallback((update: (c: Config) => Config) => {
    setSavedConfig((c) => c && update(c))
    setConfig((c) => c && update(c))
  }, [])

  // Written to disk immediately, so update the saved copy too and keep other edits pending.
  const handleResetPicker = useCallback(async () => {
    try {
      await resetPickerPosition()
      const reset = (c: Config | null): Config | null =>
        c && { ...c, picker: { ...c.picker, placement: 'edge', custom: null } }
      setSavedConfig(reset)
      setConfig(reset)
    } catch (err) {
      setSaveState({ kind: 'failed', message: errorMessage(err) })
    }
  }, [])

  const handleStartDaemon = useCallback(async () => {
    setDaemon('checking')
    const started = await startDaemon().catch((err: unknown) => {
      setDaemon('stopped')
      throw err
    })
    await refreshStatus()
    if (!started) throw new Error('The daemon did not start. Run "kakemono daemon" in a terminal to see why.')
  }, [refreshStatus])

  // ⌘S / Ctrl+S saves; ⌘W / Ctrl+W closes the window (the app stays in the tray).
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return
      const key = e.key.toLowerCase()
      if (key === 's') {
        e.preventDefault()
        void handleSave()
      } else if (key === 'w') {
        e.preventDefault()
        void closeWindow()
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [handleSave])

  let content = null
  if (loading) content = <LoadingPane />
  else if (loadError) content = <ErrorPane message={loadError} onRetry={() => void loadAll()} />
  else if (config) {
    switch (activeTab) {
      case 'general':
        content = (
          <GeneralSettings
            config={config}
            onChange={handleChange}
            errors={errors}
            appInfo={appInfo}
            onResetPickerPosition={() => void handleResetPicker()}
          />
        )
        break
      case 'sources':
        content = <SourcesSettings config={config} onChange={handleChange} />
        break
      case 'themes':
        content = <ThemesSettings config={config} onChange={handleChange} />
        break
      case 'favorites':
        content = <FavoritesSettings config={config} onChange={handleChange} />
        break
      case 'desktop':
        content =
          savedConfig && appInfo ? (
            <DesktopSettings
              config={config}
              savedConfig={savedConfig}
              onChange={handleChange}
              onWritten={handleWritten}
              os={appInfo.os}
              commandError={errors.set_command}
            />
          ) : (
            <LoadingPane />
          )
        break
      case 'hooks':
        content = <HooksSettings config={config} onChange={handleChange} error={errors.on_change} />
        break
      default: {
        const _exhaustive: never = activeTab
        content = _exhaustive
      }
    }
  }

  return (
    <div className="flex flex-col h-screen bg-white dark:bg-gray-900 text-gray-900 dark:text-gray-100">
      <StatusBar
        status={status}
        daemon={daemon}
        onStatusChange={() => void refreshStatus()}
        onStartDaemon={handleStartDaemon}
        fileManager={fileManagerName(appInfo?.os)}
      />
      <div className="flex flex-1 min-h-0">
        <Sidebar activeTab={activeTab} onTabChange={setActiveTab} />
        <main className="flex flex-col flex-1 min-w-0" aria-labelledby="section-title">
          <h2 id="section-title" className="px-6 py-3 text-sm font-semibold border-b border-gray-100 dark:border-gray-700/70">
            {TAB_TITLES[activeTab]}
          </h2>
          <div className="flex-1 overflow-y-auto px-6 py-5">{content}</div>
          {config && (
            <SaveBar dirty={dirty} invalid={invalid} state={saveState} onSave={() => void handleSave()} onDiscard={handleDiscard} />
          )}
        </main>
      </div>
    </div>
  )
}
