import { useId } from 'react'

import type { AppInfo, Config } from '../../shared/api'
import { revealInFinder } from '../../shared/api'
import type { Appearance } from '../../shared/bindings/Appearance'
import type { Fit } from '../../shared/bindings/Fit'
import ArgvEditor from '../ArgvEditor'
import { normalizeArgv } from '../argv'
import DesktopSettings from '../DesktopSettings'
import { HOOK_ENV, PASSED_THROUGH } from '../hookEnv'
import type { ConfigErrors } from '../validate'
import { Group, Row } from './Group'
import PathField from './PathField'
import ProviderKeys from './ProviderKeys'
import Segmented from './Segmented'
import TextField from './TextField'

const code = 'font-mono text-[11px] px-1 py-px rounded bg-pressed'

const FITS: readonly { value: Fit; label: string }[] = [
  { value: 'fill', label: 'Fill' },
  { value: 'fit', label: 'Fit' },
  { value: 'center', label: 'Center' },
  { value: 'stretch', label: 'Stretch' },
  { value: 'tile', label: 'Tile' },
]

const APPEARANCES: readonly { value: Appearance; label: string }[] = [
  { value: 'light', label: 'Light' },
  { value: 'dark', label: 'Dark' },
  { value: 'system', label: 'System' },
]

/** Where `daemon.manual_dir = null` resolves: `<download_dir>/manual`, in the folder's own separator. */
export function defaultManualDir(downloadDir: string): string {
  const sep = downloadDir.includes('\\') && !downloadDir.includes('/') ? '\\' : '/'
  const base = downloadDir.replace(/[\\/]+$/, '')
  return `${base}${sep}manual`
}

/** The Advanced tab: folders, display mode, desktop backend, hook, daemon, appearance and provider keys. */
export default function AdvancedSettings({
  config,
  savedConfig,
  onChange,
  onWritten,
  errors,
  appInfo,
  fileManager,
}: {
  config: Config
  /** The config as last written; the desktop section compares against it. `null` until loaded. */
  savedConfig: Config | null
  onChange: (config: Config) => void
  /** A change already on disk: apply it to both the saved and the edited config. */
  onWritten: (update: (c: Config) => Config) => void
  errors: ConfigErrors
  appInfo: AppInfo | null
  fileManager: string
}) {
  const { daemon } = config
  const setDaemon = <K extends keyof Config['daemon']>(key: K, value: Config['daemon'][K]) =>
    onChange({ ...config, daemon: { ...daemon, [key]: value } })
  const hookHintId = useId()

  return (
    <>
      <Group title="Folders">
        <Row label="Downloads" hint="Where downloaded wallpapers are cached.">
          {({ labelId, hintId }) => (
            <PathField
              value={daemon.download_dir}
              onCommit={(v) => setDaemon('download_dir', v.trim())}
              placeholder="~/Pictures/Wallpapers/cache"
              labelledBy={labelId}
              describedBy={hintId}
              fileManager={fileManager}
            />
          )}
        </Row>
        <Row label="Save manually downloaded wallpapers to" hint="Images you drop on the window or fetch from a link. Empty uses a manual folder inside Downloads.">
          {({ labelId, hintId }) => (
            <PathField
              value={daemon.manual_dir ?? ''}
              onCommit={(v) => setDaemon('manual_dir', v.trim() === '' ? null : v.trim())}
              placeholder={`Default: ${defaultManualDir(daemon.download_dir)}`}
              labelledBy={labelId}
              describedBy={hintId}
              fileManager={fileManager}
            />
          )}
        </Row>
        <Row label="Trash" hint="Trashed wallpapers are moved here.">
          {({ labelId, hintId }) => (
            <PathField
              value={daemon.trash_dir}
              onCommit={(v) => setDaemon('trash_dir', v.trim())}
              placeholder="~/Pictures/Wallpapers/trash"
              labelledBy={labelId}
              describedBy={hintId}
              fileManager={fileManager}
            />
          )}
        </Row>
      </Group>

      <Group title="Wallpaper">
        <Row label="Display mode" hint="How the image fills the screen.">
          {({ labelId, hintId }) => (
            <select
              value={config.desktop.fit}
              onChange={(e) => {
                const fit = FITS.find((f) => f.value === e.target.value)
                if (fit) onChange({ ...config, desktop: { ...config.desktop, fit: fit.value } })
              }}
              aria-labelledby={labelId}
              aria-describedby={hintId}
              className="field w-32"
            >
              {FITS.map((f) => (
                <option key={f.value} value={f.value}>
                  {f.label}
                </option>
              ))}
            </select>
          )}
        </Row>
      </Group>

      {savedConfig && appInfo ? (
        <DesktopSettings
          config={config}
          savedConfig={savedConfig}
          onChange={onChange}
          onWritten={onWritten}
          os={appInfo.os}
          commandError={errors.set_command}
        />
      ) : (
        <Group title="Desktop">
          <p role="status" className="px-4 py-3 text-[12px] text-muted">
            Loading…
          </p>
        </Group>
      )}

      <Group title="Hooks">
        <div className="px-4 py-3 space-y-3">
          <p id={hookHintId} className="text-[12px] text-muted">
            A command run after every change, without a shell, stopped after 10 seconds. The wallpaper never waits for
            it. For shell syntax use three tokens: <span className={code}>sh</span>, <span className={code}>-c</span>{' '}
            and the script.
          </p>
          <ArgvEditor
            label="Hook command"
            tokens={config.hooks.on_change ?? []}
            onChange={(next) => onChange({ ...config, hooks: { ...config.hooks, on_change: normalizeArgv(next) } })}
            error={errors.on_change}
            hintId={hookHintId}
            emptyText="No hook. Add a token to run a command on every change."
          />
          <details className="group text-[12px]">
            <summary className="cursor-pointer select-none text-accent w-fit">Variables and placeholders</summary>
            <table className="mt-2 w-full">
              <caption className="sr-only">Variables set for the hook</caption>
              <tbody className="divide-y divide-line">
                {HOOK_ENV.map((v) => (
                  <tr key={v.name}>
                    <th scope="row" className="py-1.5 pr-4 text-left font-mono font-normal text-[11px] whitespace-nowrap">
                      {v.name}
                    </th>
                    <td className="py-1.5 text-muted">{v.text}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            <p className="mt-2 text-muted">
              Tokens can also use <span className={code}>{'{path}'}</span>, <span className={code}>{'{uri}'}</span>,{' '}
              <span className={code}>{'{fit}'}</span> and <span className={code}>{'{theme}'}</span>; a value never
              splits a token. Other variables are cleared except {PASSED_THROUGH.join(', ')}.
            </p>
          </details>
        </div>
      </Group>

      <Group title="Daemon">
        <Row label="Socket path" hint="Empty uses the default. Applies after the daemon and kakemono restart.">
          {({ labelId, hintId }) => (
            <TextField
              value={daemon.socket_path ?? ''}
              onCommit={(v) => setDaemon('socket_path', v.trim() === '' ? null : v.trim())}
              placeholder="Default"
              aria-labelledby={labelId}
              aria-describedby={hintId}
              mono
              className="w-56"
            />
          )}
        </Row>
        {appInfo && (
          <Row
            label="Config file"
            hint={
              <span className="font-mono text-[11px] break-all selectable">
                {appInfo.config_path}
                {appInfo.legacy_config && ' (shared with the old wallpaperd app)'}
              </span>
            }
          >
            {() => (
              <button type="button" className="btn" onClick={() => void revealInFinder(appInfo.config_path).catch(() => {})}>
                Show in {fileManager}
              </button>
            )}
          </Row>
        )}
      </Group>

      <Group title="Appearance">
        <Row label="Appearance" hint="System follows the light or dark setting of your computer.">
          {({ labelId, hintId }) => (
            <Segmented
              value={config.ui.appearance}
              options={APPEARANCES}
              onChange={(appearance) => onChange({ ...config, ui: { ...config.ui, appearance } })}
              labelledBy={labelId}
              describedBy={hintId}
            />
          )}
        </Row>
      </Group>

      <ProviderKeys config={config} onChange={onChange} />
    </>
  )
}
