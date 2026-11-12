import { useEffect, useRef, useState, type ReactNode } from 'react'

// Bundled by Vite (`server.fs.allow` lets the dev server read it); 2× for the 64 px slot.
import appIcon from '../../../src-tauri/icons/128x128.png'

import type { Config } from '../shared/api'
import {
  destroyWindow,
  errorMessage,
  getAppInfo,
  getConfig,
  nextWallpaper,
  saveConfig,
  trayAvailable,
} from '../shared/api'
import { useAppearance } from '../shared/useAppearance'
import { useDaemonEvents } from '../shared/useDaemonEvents'
import { CheckLine, LineText } from '../settings/components/SettingLine'
import StartAtLogin from '../settings/components/StartAtLogin'
import { formatInterval } from '../settings/duration'
import { summarizeLocation } from '../settings/locationSummary'
import type { DaemonState } from '../settings/sections'
import { SOURCE_TYPES } from '../settings/sourceTypes'

const SCREENS = ['Welcome', 'Where kakemono lives', 'Set it up', 'Tips'] as const

/** A folder path without trailing separators, for comparing two spellings of one folder. */
function trimPath(path: string): string {
  return path.trim().replace(/[/\\]+$/, '')
}

/** The last segment of a folder path, or the path when it has none. */
function folderName(path: string): string {
  return trimPath(path).split(/[/\\]/).pop() || path
}

/**
 * The enabled sources by readable name: the given name; "Favorites" for the favorites folder;
 * another folder's own folder name; else the type plus a short location.
 */
export function sourceNames(config: Config): string[] {
  const favorites = trimPath(config.favorites.dir)
  return config.sources
    .filter((s) => s.enabled)
    .map((s) => {
      if (s.name) return s.name
      if (s.type === 'folder') {
        return trimPath(s.location) === favorites ? 'Favorites' : folderName(s.location)
      }
      const label = SOURCE_TYPES[s.type].label
      const location = s.location.trim() ? summarizeLocation(s.type, s.location) : ''
      return location ? `${label} (${location})` : label
    })
}

/** Where the tray icon is, per `app_info().os`; `tray` is false on a Linux desktop without one. */
function trayPlace(os: string | undefined, tray: boolean): string {
  if (!tray) {
    return 'Your desktop has no tray, so kakemono has no icon. Settings is the way in: open kakemono from your app menu to get back to it.'
  }
  switch (os) {
    case 'macos':
      return 'kakemono runs in the menu bar, with no Dock icon.'
    case 'windows':
      return "kakemono runs in the notification area by the clock. If you don't see it, click the ^ arrow."
    case 'linux':
      return 'kakemono runs in the system tray. On GNOME, install the AppIndicator extension to see it.'
    default:
      return 'kakemono runs in the menu bar or system tray.'
  }
}

const Key = ({ children }: { children: ReactNode }) => (
  <kbd className="font-mono text-[11px] px-1 rounded border border-line bg-panel">{children}</kbd>
)
const Cmd = ({ children }: { children: ReactNode }) => (
  <code className="font-mono text-[12px] text-ink">{children}</code>
)

/** "Change wallpaper now": `next`, with pending and error states; off while the daemon is down. */
function ChangeNow({ daemon }: { daemon: DaemonState }) {
  const [changing, setChanging] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const running = daemon === 'running'

  const change = async () => {
    setChanging(true)
    setError(null)
    try {
      await nextWallpaper()
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setChanging(false)
    }
  }

  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 px-4 py-3">
      <button
        type="button"
        className="btn h-7 px-2.5 text-[12px]"
        disabled={!running || changing}
        onClick={() => void change()}
      >
        {changing ? 'Changing…' : 'Change wallpaper now'}
      </button>
      <span role="status" aria-live="polite" className="text-[12px] min-w-0">
        {error ? (
          <span className="text-danger break-words">{error}</span>
        ) : daemon === 'stopped' ? (
          <span className="text-muted">The daemon is not running.</span>
        ) : null}
      </span>
    </div>
  )
}

function SetUp({
  config,
  daemon,
  onRotationChange,
  rotationError,
}: {
  config: Config
  daemon: DaemonState
  onRotationChange: (enabled: boolean) => void
  rotationError: string | null
}) {
  const sources = sourceNames(config)
  return (
    <>
      <div className="rounded-xl bg-panel border border-line divide-y divide-line">
        <ChangeNow daemon={daemon} />
        <CheckLine
          checked={config.daemon.rotation_enabled}
          onChange={onRotationChange}
          error={rotationError ? `Not saved: ${rotationError}` : null}
        >
          <LineText>Change wallpaper every {formatInterval(config.daemon.interval)}</LineText>
        </CheckLine>
        <StartAtLogin variant="line" />
      </div>
      <p className="mt-3 text-muted">
        {sources.length > 0 ? `Your sources: ${sources.join(', ')}.` : 'No source is turned on.'} Change them under
        Sources in Settings.
      </p>
    </>
  )
}

/**
 * The Welcome window: four short screens on the very first start, and again from Settings →
 * "Show welcome tips". The backend opens Settings when this window closes, however it closes
 * (Skip, Open Settings or the close button). Loading a config that still has
 * `[ui] show_welcome = true` writes `false` once; a replay finds it false and writes nothing.
 */
export default function App() {
  const [step, setStep] = useState(0)
  const [config, setConfig] = useState<Config | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [os, setOs] = useState<string | undefined>()
  const [tray, setTray] = useState(true)
  const [daemon, setDaemon] = useState<DaemonState>('checking')
  const [rotationError, setRotationError] = useState<string | null>(null)
  const recorded = useRef(false)
  const primaryRef = useRef<HTMLButtonElement>(null)

  useAppearance(config?.ui.appearance ?? 'system')

  useEffect(() => {
    getConfig()
      .then((loaded) => {
        setConfig(loaded)
        if (!loaded.ui.show_welcome || recorded.current) return
        recorded.current = true
        setConfig({ ...loaded, ui: { ...loaded.ui, show_welcome: false } })
        // A missing config is created from the template before the write (`write_patch`), so this
        // does not race the daemon creating it. A failure leaves the window to show next start.
        saveConfig({ ui: { show_welcome: false } }).catch(() => {})
      })
      .catch((err: unknown) => setLoadError(errorMessage(err)))
    getAppInfo()
      .then((info) => {
        setOs(info.os)
        document.documentElement.dataset.os = info.os
      })
      .catch(() => {})
    trayAvailable()
      .then(setTray)
      .catch(() => {})
  }, [])

  useDaemonEvents({
    onConnection: (c) => {
      // `error`: reachable but not subscribable (an older daemon), so `next` still works.
      setDaemon(c.state === 'connecting' ? 'checking' : c.state === 'not_running' ? 'stopped' : 'running')
    },
  })

  // Each screen's primary button (Get started, Next, Open Settings) takes focus, so Enter and
  // Space advance. The title is a live region, so a screen reader still hears the new screen.
  useEffect(() => {
    primaryRef.current?.focus()
  }, [step])

  // Settings opens from the backend once the window is gone.
  const close = () => void destroyWindow().catch(() => {})

  useEffect(() => {
    // The webview can take window focus after the mount-time focus call; don't leave it on <body>.
    const onFocus = () => {
      if (!document.activeElement || document.activeElement === document.body) primaryRef.current?.focus()
    }
    // Escape acts like Skip, on every screen.
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.isComposing || e.defaultPrevented) return
      e.preventDefault()
      close()
    }
    window.addEventListener('focus', onFocus)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('focus', onFocus)
      window.removeEventListener('keydown', onKey)
    }
  }, [])

  const go = (to: number) => setStep(to)

  const changeRotation = (enabled: boolean) => {
    if (!config) return
    const before = config.daemon.rotation_enabled
    setRotationError(null)
    setConfig({ ...config, daemon: { ...config.daemon, rotation_enabled: enabled } })
    saveConfig({ daemon: { rotation_enabled: enabled } }).catch((err: unknown) => {
      setRotationError(errorMessage(err))
      setConfig((c) => (c ? { ...c, daemon: { ...c.daemon, rotation_enabled: before } } : c))
    })
  }

  const interval = config ? formatInterval(config.daemon.interval) : ''
  const last = SCREENS.length - 1

  let title: string
  let body: ReactNode
  switch (step) {
    case 0:
      title = 'kakemono changes your desktop wallpaper'
      body = (
        <>
          <p>It finds images on Wallhaven, Reddit, NASA, any web page or your own folders.</p>
          <p className="text-muted">
            {config?.daemon.rotation_enabled
              ? `Your wallpaper changes every ${interval}.`
              : 'The wallpaper changes only when you ask, unless you turn on rotation.'}
          </p>
        </>
      )
      break
    case 1:
      title = 'Where kakemono lives'
      body = (
        <>
          <p>{trayPlace(os, tray)}</p>
          {tray && <p className="text-muted">Click the icon for Next, Pause, Favorite, Show Picker and Settings.</p>}
        </>
      )
      break
    case 2:
      title = 'Set it up'
      body = config ? (
        <SetUp config={config} daemon={daemon} onRotationChange={changeRotation} rotationError={rotationError} />
      ) : (
        <p className={loadError ? 'text-danger break-words' : 'text-muted'}>
          {loadError ? `Cannot read the config: ${loadError}` : 'Loading…'}
        </p>
      )
      break
    default:
      title = 'Tips'
      body = (
        <ul className="list-disc pl-5 space-y-2">
          <li>
            In the picker, the arrow keys move, <Key>Enter</Key> sets the wallpaper, <Key>F</Key> favorites it and{' '}
            <Key>Delete</Key> moves it to the trash.
          </li>
          <li>Paste a wallpaper site's search link as a source, under Sources in Settings.</li>
          <li>Drag an image onto Settings to make it your wallpaper.</li>
          <li>
            kakemono has no built-in shortcut. Bind <Cmd>kakemono next</Cmd> to a key; Settings → General → Shortcuts
            shows how.
          </li>
        </ul>
      )
  }

  return (
    <main className="h-screen flex flex-col bg-window text-ink text-[13px] select-none">
      {/* Every screen fits at 560×440; a long error or source list scrolls rather than clips. */}
      <section className="flex-1 min-h-0 px-8 pt-8 pb-4 space-y-3 overflow-y-auto">
        {step === 0 && <img src={appIcon} alt="" width={64} height={64} className="size-16 mb-4" draggable={false} />}
        <h1 aria-live="polite" className="text-[20px] font-semibold leading-tight mb-4">
          {title}
        </h1>
        {body}
      </section>
      <footer className="shrink-0 flex items-center gap-2 px-6 py-4 border-t border-line">
        <ol aria-label="Steps" className="flex gap-1.5 mr-auto">
          {SCREENS.map((name, i) => (
            <li
              key={name}
              aria-current={i === step ? 'step' : undefined}
              aria-label={`${name}, step ${i + 1} of ${SCREENS.length}`}
              className={`size-2 rounded-full ${i === step ? 'bg-accent' : 'bg-line-strong'}`}
            />
          ))}
        </ol>
        {step < last && (
          <button type="button" className="btn-plain h-7 px-2.5 text-[12px]" onClick={close}>
            Skip
          </button>
        )}
        {step > 0 && (
          <button type="button" className="btn h-7 px-3 text-[12px]" onClick={() => go(step - 1)}>
            Back
          </button>
        )}
        {step < last ? (
          <button
            ref={primaryRef}
            type="button"
            className="btn btn-accent h-7 px-3 text-[12px]"
            onClick={() => go(step + 1)}
          >
            {step === 0 ? 'Get started' : 'Next'}
          </button>
        ) : (
          <button ref={primaryRef} type="button" className="btn btn-accent h-7 px-3 text-[12px]" onClick={close}>
            Open Settings
          </button>
        )}
      </footer>
    </main>
  )
}
