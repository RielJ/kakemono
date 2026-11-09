import { useCallback, useEffect, useId, useState } from 'react'

import type { ApplyReport, Config, DoctorReport } from '../shared/api'
import {
  desktopBackend,
  detectDesktop,
  errorMessage,
  getConfig,
  isDaemonNotRunning,
  saveConfig,
  testDesktop,
} from '../shared/api'
import type { Backend } from '../shared/bindings/Backend'
import type { DesktopConfig } from '../shared/bindings/DesktopConfig'
import type { Fit } from '../shared/bindings/Fit'
import type { ThemeOverride } from '../shared/bindings/ThemeOverride'
import ArgvEditor from './ArgvEditor'
import { checkDaemonBackend, normalizeArgv, usesCommand } from './argv'
import { Group, Row } from './components/Group'
import Icon from './components/Icon'
import Toggle from './components/Toggle'

const FITS: readonly { value: Fit; label: string }[] = [
  { value: 'fill', label: 'Fill' },
  { value: 'fit', label: 'Fit' },
  { value: 'center', label: 'Center' },
  { value: 'stretch', label: 'Stretch' },
  { value: 'tile', label: 'Tile' },
]

const THEMES: readonly { value: ThemeOverride; label: string }[] = [
  { value: 'auto', label: 'Automatic' },
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
]

/** `[desktop] backend` values offered on Linux, in `Backend` order. `native` means `auto` there. */
const LINUX_BACKENDS: readonly Backend[] = [
  'auto',
  'gnome',
  'kde',
  'xfce',
  'cinnamon',
  'mate',
  'sway',
  'hyprpaper',
  'awww',
  'swww',
  'swaybg',
  'wbg',
  'xwallpaper',
  'feh',
  'nitrogen',
  'hsetroot',
  'pcmanfm',
  'pcmanfm-qt',
  'command',
]

const backendLabel = (b: Backend) => (b === 'auto' ? 'Automatic (detect)' : b === 'command' ? 'Custom command' : b)

type DaemonView =
  | { kind: 'loading' }
  | { kind: 'ok'; report: DoctorReport }
  | { kind: 'not_running' }
  | { kind: 'error'; message: string }

type Job<T> = { kind: 'idle' } | { kind: 'running' } | { kind: 'done'; value: T } | { kind: 'failed'; message: string }

const pre =
  'text-[11px] leading-relaxed font-mono whitespace-pre-wrap break-words rounded-md px-3 py-2 ' +
  'bg-gray-50 dark:bg-gray-900/60 border border-gray-200 dark:border-gray-700 text-gray-700 dark:text-gray-200'

const sameJson = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b)

function useDaemonBackend(savedDesktop: DesktopConfig) {
  const [view, setView] = useState<DaemonView>({ kind: 'loading' })
  const refresh = useCallback(async () => {
    try {
      setView({ kind: 'ok', report: await desktopBackend() })
    } catch (err) {
      setView(isDaemonNotRunning(err) ? { kind: 'not_running' } : { kind: 'error', message: errorMessage(err) })
    }
  }, [])
  // Re-read after every save: Save asks the daemon to reload.
  const savedKey = JSON.stringify(savedDesktop)
  useEffect(() => {
    void refresh()
  }, [refresh, savedKey])
  return { view, setView, refresh }
}

/** What the daemon is using right now, and whether that is what the saved config selects. */
function InUse({ view, savedDesktop, os }: { view: DaemonView; savedDesktop: DesktopConfig; os: string }) {
  if (view.kind === 'loading') return <p className="text-xs text-gray-500 dark:text-gray-400">Asking the daemon…</p>
  if (view.kind === 'not_running')
    return <p className="text-xs text-gray-500 dark:text-gray-400">The daemon is not running. Start it to see its backend.</p>
  if (view.kind === 'error')
    return (
      <p role="alert" className="text-xs text-red-600 dark:text-red-400 break-words">
        {view.message}
      </p>
    )
  const { backend } = view.report
  const check = checkDaemonBackend({ saved: savedDesktop, os, daemon: backend })
  return (
    <div className="space-y-2">
      <p className="text-sm">
        <span className="font-medium">{backend.name}</span>
      </p>
      {backend.detail && <p className={pre}>{backend.detail}</p>}
      {check.kind === 'stale' && (
        // Disappears on its own once `reload` rebuilds the setter (Task 37b).
        <p role="status" className="text-xs text-amber-700 dark:text-amber-300 flex gap-1.5">
          <Icon name="alert" className="w-3.5 h-3.5 shrink-0 mt-px" />
          <span>
            The saved settings select <span className="font-mono">{check.expected}</span>. Restart the daemon to use the
            edited command.
          </span>
        </p>
      )}
    </div>
  )
}

function ApplyResult({ apply }: { apply: ApplyReport | null }) {
  if (!apply) return <p className="text-xs text-gray-600 dark:text-gray-300">The daemon reported no result.</p>
  if (apply.ok)
    return (
      <p role="status" className="text-xs text-gray-600 dark:text-gray-300 flex gap-1.5 min-w-0">
        <Icon name="check" className="w-3.5 h-3.5 shrink-0 mt-px" />
        <span className="break-all">Wallpaper set{apply.path ? `: ${apply.path}` : '.'}</span>
      </p>
    )
  return (
    <p role="alert" className="text-xs text-red-600 dark:text-red-400 break-words">
      {apply.error ?? 'The daemon could not set the wallpaper.'}
    </p>
  )
}

export default function DesktopSettings({
  config,
  savedConfig,
  onChange,
  onWritten,
  os,
  commandError,
}: {
  config: Config
  savedConfig: Config
  onChange: (c: Config) => void
  /** A change already on disk: apply it to both the saved and the edited config. */
  onWritten: (update: (c: Config) => Config) => void
  /** `AppInfo.os`. */
  os: string
  commandError?: string | null
}) {
  const desktop = config.desktop
  const saved = savedConfig.desktop
  const linux = os === 'linux'
  const unsaved = !sameJson(desktop, saved)
  const { view, refresh } = useDaemonBackend(saved)
  const commandHintId = useId()
  const testHintId = useId()

  const [test, setTest] = useState<Job<DoctorReport>>({ kind: 'idle' })
  const [detect, setDetect] = useState<Job<string>>({ kind: 'idle' })
  const [write, setWrite] = useState<Job<string>>({ kind: 'idle' })
  const [pin, setPin] = useState<Job<null>>({ kind: 'idle' })

  const set = <K extends keyof DesktopConfig>(key: K, value: DesktopConfig[K]) =>
    onChange({ ...config, desktop: { ...desktop, [key]: value } })

  const runDetect = useCallback(async () => {
    setDetect({ kind: 'running' })
    try {
      setDetect({ kind: 'done', value: await detectDesktop(false) })
    } catch (err) {
      setDetect({ kind: 'failed', message: errorMessage(err) })
    }
  }, [])

  useEffect(() => {
    if (linux) void runDetect()
  }, [linux, runDetect])

  const runTest = async () => {
    setTest({ kind: 'running' })
    try {
      const report = await testDesktop()
      setTest({ kind: 'done', value: report })
      void refresh()
    } catch (err) {
      setTest({ kind: 'failed', message: errorMessage(err) })
    }
  }

  const writeDetection = async () => {
    setWrite({ kind: 'running' })
    try {
      const text = await detectDesktop(true)
      setWrite({ kind: 'done', value: text })
      const fresh = await getConfig()
      onWritten((c) => ({ ...c, desktop: fresh.desktop }))
      void runDetect()
    } catch (err) {
      setWrite({ kind: 'failed', message: errorMessage(err) })
    }
  }

  const setPinned = async (pinned: boolean) => {
    const managed_by = pinned ? 'user' : 'detect'
    setPin({ kind: 'running' })
    try {
      await saveConfig({ desktop: { managed_by } })
      onWritten((c) => ({ ...c, desktop: { ...c.desktop, managed_by } }))
      setPin({ kind: 'done', value: null })
    } catch (err) {
      setPin({ kind: 'failed', message: errorMessage(err) })
    }
  }

  const fitRow = (
    <Row label="Fit" hint="How the image fills the screen.">
      {({ labelId, hintId }) => (
        <select
          value={desktop.fit}
          onChange={(e) => {
            const fit = FITS.find((f) => f.value === e.target.value)
            if (fit) set('fit', fit.value)
          }}
          aria-labelledby={labelId}
          aria-describedby={hintId}
          className="form-input w-32 text-xs py-1"
        >
          {FITS.map((f) => (
            <option key={f.value} value={f.value}>
              {f.label}
            </option>
          ))}
        </select>
      )}
    </Row>
  )

  if (!linux) {
    return (
      <div className="space-y-6 max-w-2xl">
        <Group title="Backend">
          <div className="px-4 py-3">
            <InUse view={view} savedDesktop={saved} os={os} />
          </div>
        </Group>
        <Group title="Display">{fitRow}</Group>
      </div>
    )
  }

  const pinned = saved.managed_by === 'user'
  const showCommand = usesCommand(desktop.backend) || desktop.backend === 'auto'
  const showPersistent = usesCommand(desktop.backend) && desktop.backend !== 'swaybg' && desktop.backend !== 'wbg'
  const commandHint =
    desktop.backend === 'auto'
      ? 'Used only when detection finds no supported setter.'
      : desktop.backend === 'command'
        ? 'The program and its arguments, one per row.'
        : `Leave empty to use the built-in ${desktop.backend} command; any token replaces it.`
  const backends = LINUX_BACKENDS.includes(desktop.backend) ? LINUX_BACKENDS : [desktop.backend, ...LINUX_BACKENDS]

  return (
    <div className="space-y-6 max-w-2xl">
      <Group title="In use">
        <div className="px-4 py-3 space-y-3">
          <InUse view={view} savedDesktop={saved} os={os} />
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              onClick={() => void runTest()}
              disabled={unsaved || test.kind === 'running' || view.kind === 'not_running'}
              aria-describedby={testHintId}
              className="btn-primary text-xs py-1 px-3 gap-1"
            >
              <Icon name="zap" className="w-3.5 h-3.5" />
              {test.kind === 'running' ? 'Testing…' : 'Test'}
            </button>
            <span id={testHintId} className="text-xs text-gray-500 dark:text-gray-400">
              {unsaved ? 'Save your changes to test them.' : 'Reloads the daemon and sets the current wallpaper again.'}
            </span>
          </div>
          {test.kind === 'done' && <ApplyResult apply={test.value.apply} />}
          {test.kind === 'failed' && (
            <p role="alert" className="text-xs text-red-600 dark:text-red-400 break-words">
              {test.message}
            </p>
          )}
        </div>
      </Group>

      <Group title="Backend">
        <Row label="Backend" hint={pinned ? 'Pinned: detection never changes it.' : 'Detection may replace it when the session changes.'}>
          {({ labelId, hintId }) => (
            <select
              value={desktop.backend}
              onChange={(e) => {
                const b = backends.find((x) => x === e.target.value)
                if (b) set('backend', b)
              }}
              aria-labelledby={labelId}
              aria-describedby={hintId}
              className="form-input w-44 text-xs py-1"
            >
              {backends.map((b) => (
                <option key={b} value={b}>
                  {backendLabel(b)}
                </option>
              ))}
            </select>
          )}
        </Row>
        <Row
          label="Pin"
          hint={
            pinned
              ? 'managed_by = "user". Saved immediately.'
              : 'Keep this backend: re-detection will not overwrite it. Saved immediately.'
          }
          error={pin.kind === 'failed' ? pin.message : null}
        >
          {({ labelId, hintId }) => (
            <Toggle
              enabled={pinned}
              onChange={(v) => void setPinned(v)}
              labelledBy={labelId}
              describedBy={hintId}
              disabled={pin.kind === 'running'}
            />
          )}
        </Row>
        {showCommand && (
          <div className="px-4 py-3 space-y-2">
            <p className="text-sm font-medium">Wallpaper command</p>
            <p id={commandHintId} className="text-xs text-gray-500 dark:text-gray-400">
              {commandHint}
            </p>
            <ArgvEditor
              label="Wallpaper command"
              tokens={desktop.set_command ?? []}
              onChange={(next) => set('set_command', normalizeArgv(next))}
              error={commandError}
              hintId={commandHintId}
              emptyText={desktop.backend === 'command' ? 'No command yet. Add the program first.' : 'No custom command.'}
            />
          </div>
        )}
        {showPersistent && (
          <Row label="Keep running" hint="For setters that must stay open, like swaybg. kakemono restarts it on each change.">
            {({ labelId, hintId }) => (
              <Toggle enabled={desktop.persistent} onChange={(v) => set('persistent', v)} labelledBy={labelId} describedBy={hintId} />
            )}
          </Row>
        )}
      </Group>

      <Group title="Display">
        {fitRow}
        <Row label="Theme" hint="For {theme} and theme rotation when the desktop can't report light or dark.">
          {({ labelId, hintId }) => (
            <select
              value={desktop.theme}
              onChange={(e) => {
                const t = THEMES.find((x) => x.value === e.target.value)
                if (t) set('theme', t.value)
              }}
              aria-labelledby={labelId}
              aria-describedby={hintId}
              className="form-input w-32 text-xs py-1"
            >
              {THEMES.map((t) => (
                <option key={t.value} value={t.value}>
                  {t.label}
                </option>
              ))}
            </select>
          )}
        </Row>
      </Group>

      <Group title="Detection">
        <div className="px-4 py-3 space-y-3">
          {saved.detected_session && (
            <p className="text-xs text-gray-600 dark:text-gray-300">
              Saved for <span className="font-mono">{saved.detected_session}</span>
              {saved.detected_at ? ` on ${saved.detected_at}` : ''}.
            </p>
          )}
          {detect.kind === 'running' && <p className="text-xs text-gray-500 dark:text-gray-400">Detecting…</p>}
          {detect.kind === 'done' && (
            <pre aria-label="Detection report" className={pre}>
              {detect.value.trimEnd()}
            </pre>
          )}
          {detect.kind === 'failed' && (
            <p role="alert" className="text-xs text-red-600 dark:text-red-400 break-words">
              {detect.message}
            </p>
          )}
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              onClick={() => void runDetect()}
              disabled={detect.kind === 'running'}
              className="btn-secondary text-xs py-1 px-2"
            >
              Re-detect
            </button>
            <button
              type="button"
              onClick={() => void writeDetection()}
              disabled={pinned || unsaved || write.kind === 'running'}
              className="btn-secondary text-xs py-1 px-2"
            >
              {write.kind === 'running' ? 'Saving…' : 'Use detected backend'}
            </button>
            <span className="text-xs text-gray-500 dark:text-gray-400">
              {pinned ? 'Unpin to let detection change the backend.' : unsaved ? 'Save or discard your changes first.' : ''}
            </span>
          </div>
          {write.kind === 'done' && <p className={pre}>{write.value.trimEnd()}</p>}
          {write.kind === 'failed' && (
            <p role="alert" className="text-xs text-red-600 dark:text-red-400 break-words">
              {write.message}
            </p>
          )}
        </div>
      </Group>
    </div>
  )
}
