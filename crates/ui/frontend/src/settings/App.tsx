import { useState, useEffect, useCallback, useRef } from 'react'
import type { Config, StatusData, TabId } from './types/config'
import { getConfig, saveConfig, getStatus, getThumbnail } from '../shared/api'

import Sidebar from './components/Sidebar'
import StatusBar from './components/StatusBar'
import GeneralSettings from './components/GeneralSettings'
import ProvidersSettings from './components/ProvidersSettings'
import ThemesSettings from './components/ThemesSettings'
import FavoritesSettings from './components/FavoritesSettings'

// ── Unsaved-changes indicator ─────────────────────────────────────────────────

interface SaveBarProps {
  dirty: boolean
  saving: boolean
  saveError: string | null
  onSave: () => void
  onDiscard: () => void
}

function SaveBar({ dirty, saving, saveError, onSave, onDiscard }: SaveBarProps) {
  if (!dirty && !saving && !saveError) return null
  return (
    <div className={`
      no-drag flex items-center justify-between gap-3
      px-4 py-2 border-t
      ${saveError
        ? 'border-red-200 dark:border-red-700/50 bg-red-50 dark:bg-red-900/20'
        : 'border-amber-200 dark:border-amber-700/50 bg-amber-50 dark:bg-amber-900/20'
      }
    `}>
      <span className={`text-xs font-medium ${saveError ? 'text-red-700 dark:text-red-300' : 'text-amber-700 dark:text-amber-300'}`}>
        {saveError ? `Save failed: ${saveError}` : 'You have unsaved changes'}
      </span>
      <div className="flex items-center gap-2">
        <button
          onClick={onDiscard}
          disabled={saving}
          className="btn-secondary no-drag text-xs py-1"
        >
          Discard
        </button>
        <button
          onClick={onSave}
          disabled={saving}
          className="btn-primary no-drag text-xs py-1"
        >
          {saving ? 'Saving…' : 'Save'}
        </button>
      </div>
    </div>
  )
}

// ── Loading skeleton ──────────────────────────────────────────────────────────

function LoadingPane() {
  return (
    <div className="flex-1 flex items-center justify-center">
      <div className="text-center space-y-3">
        <div className="w-8 h-8 border-2 border-accent/30 border-t-accent dark:border-accent-dark/30 dark:border-t-accent-dark rounded-full animate-spin mx-auto" />
        <p className="text-sm text-gray-400 dark:text-gray-500">Loading settings…</p>
      </div>
    </div>
  )
}

// ── Error pane ────────────────────────────────────────────────────────────────

interface ErrorPaneProps {
  message: string
  onRetry: () => void
}

function ErrorPane({ message, onRetry }: ErrorPaneProps) {
  return (
    <div className="flex-1 flex items-center justify-center p-8">
      <div className="text-center space-y-4 max-w-sm">
        <div className="w-12 h-12 rounded-full bg-red-100 dark:bg-red-900/30 flex items-center justify-center mx-auto">
          <svg viewBox="0 0 20 20" fill="currentColor" className="w-6 h-6 text-red-500">
            <path fillRule="evenodd" d="M18 10a8 8 0 11-16 0 8 8 0 0116 0zm-7 4a1 1 0 11-2 0 1 1 0 012 0zm-1-9a1 1 0 00-1 1v4a1 1 0 102 0V6a1 1 0 00-1-1z" clipRule="evenodd" />
          </svg>
        </div>
        <div>
          <h3 className="text-sm font-semibold text-gray-800 dark:text-gray-100">Failed to load settings</h3>
          <p className="mt-1 text-xs text-gray-500 dark:text-gray-400">{message}</p>
        </div>
        <button onClick={onRetry} className="btn-primary no-drag mx-auto">
          Retry
        </button>
      </div>
    </div>
  )
}

// ── Tab content router ────────────────────────────────────────────────────────

interface ContentProps {
  tab: TabId
  config: Config
  onChange: (config: Config) => void
}

function TabContent({ tab, config, onChange }: ContentProps) {
  switch (tab) {
    case 'general':
      return <GeneralSettings config={config} onChange={onChange} />
    case 'providers':
      return <ProvidersSettings config={config} onChange={onChange} />
    case 'themes':
      return <ThemesSettings config={config} onChange={onChange} />
    case 'favorites':
      return <FavoritesSettings config={config} onChange={onChange} />
    default:
      return null
  }
}

const TAB_TITLES: Record<TabId, string> = {
  general:    'General',
  providers:  'Providers',
  themes:     'Themes',
  favorites:  'Favorites',
}

// ── Root App ──────────────────────────────────────────────────────────────────

export default function App() {
  const [activeTab, setActiveTab]   = useState<TabId>('general')
  const [config, setConfig]         = useState<Config | null>(null)
  const [savedConfig, setSavedConfig] = useState<Config | null>(null)
  const [status, setStatus]         = useState<StatusData | null>(null)
  const [thumbnail, setThumbnail]   = useState<string>('')
  const [loading, setLoading]       = useState(true)
  const [loadError, setLoadError]   = useState<string | null>(null)
  const [saving, setSaving]         = useState(false)
  const [saveError, setSaveError]   = useState<string | null>(null)
  const statusPollRef               = useRef<ReturnType<typeof setInterval> | null>(null)

  // ── Load initial data ──────────────────────────────────────────────────────

  const loadAll = useCallback(async () => {
    setLoading(true)
    setLoadError(null)
    try {
      const [cfg, st, thumb] = await Promise.all([
        getConfig(),
        getStatus().catch(() => null),
        getThumbnail().catch(() => ''),
      ])
      setConfig(cfg)
      setSavedConfig(cfg)
      setStatus(st)
      setThumbnail(thumb)
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { loadAll() }, [loadAll])

  // ── Poll status every 10 s ─────────────────────────────────────────────────

  const refreshStatus = useCallback(async () => {
    try {
      const [st, thumb] = await Promise.all([
        getStatus(),
        getThumbnail().catch(() => thumbnail),
      ])
      setStatus(st)
      setThumbnail(thumb)
    } catch {
      // daemon may be stopped — silently ignore
    }
  }, [thumbnail])

  useEffect(() => {
    statusPollRef.current = setInterval(refreshStatus, 10_000)
    return () => {
      if (statusPollRef.current) clearInterval(statusPollRef.current)
    }
  }, [refreshStatus])

  // ── Save handler ───────────────────────────────────────────────────────────

  const handleSave = useCallback(async () => {
    if (!config) return
    setSaving(true)
    try {
      await saveConfig(config)
      setSavedConfig(config)
      setSaveError(null)
    } catch (err) {
      console.error('Save failed:', err)
      setSaveError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }, [config])

  // ── Discard handler ────────────────────────────────────────────────────────

  const handleDiscard = useCallback(() => {
    if (savedConfig) setConfig(savedConfig)
  }, [savedConfig])

  // ── Dirty check ────────────────────────────────────────────────────────────

  const isDirty = config !== null &&
    savedConfig !== null &&
    JSON.stringify(config) !== JSON.stringify(savedConfig)

  // ── Keyboard shortcut: ⌘S to save ─────────────────────────────────────────

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 's') {
        e.preventDefault()
        if (isDirty && !saving) handleSave()
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [isDirty, saving, handleSave])

  // ── Render ─────────────────────────────────────────────────────────────────

  return (
    <div className="flex flex-col h-screen bg-white dark:bg-gray-900 text-gray-900 dark:text-gray-100">
      {/* Top status/action bar — draggable titlebar area */}
      <StatusBar
        status={status}
        thumbnail={thumbnail}
        onStatusChange={refreshStatus}
      />

      {/* Main layout: sidebar + content */}
      <div className="flex flex-1 min-h-0">
        <Sidebar activeTab={activeTab} onTabChange={setActiveTab} />

        {/* Right content area */}
        <main className="flex flex-col flex-1 min-w-0">
          {/* Section header */}
          <div className="px-6 py-3 border-b border-gray-100 dark:border-gray-700/50 bg-white dark:bg-gray-900">
            <h2 className="text-sm font-semibold text-gray-700 dark:text-gray-200">
              {TAB_TITLES[activeTab]}
            </h2>
          </div>

          {/* Scrollable content */}
          <div className="flex-1 overflow-y-auto px-6 py-5">
            {loading ? (
              <LoadingPane />
            ) : loadError ? (
              <ErrorPane message={loadError} onRetry={loadAll} />
            ) : config ? (
              <TabContent
                tab={activeTab}
                config={config}
                onChange={setConfig}
              />
            ) : null}
          </div>

          {/* Save bar */}
          <SaveBar
            dirty={isDirty}
            saving={saving}
            saveError={saveError}
            onSave={handleSave}
            onDiscard={handleDiscard}
          />
        </main>
      </div>
    </div>
  )
}
