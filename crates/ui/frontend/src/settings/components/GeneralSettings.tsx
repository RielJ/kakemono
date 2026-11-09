import type { AppInfo, Config } from '../../shared/api'
import { openFolder, selectDirectory } from '../../shared/api'
import { fileManagerName } from '../sections'
import type { ConfigErrors } from '../validate'
import { Group, Row } from './Group'
import StartAtLogin from './StartAtLogin'
import Toggle from './Toggle'

interface Props {
  config: Config
  onChange: (config: Config) => void
  errors: ConfigErrors
  appInfo: AppInfo | null
  onResetPickerPosition: () => void
}

const INTERVAL_PRESETS = [
  { label: '5 minutes', value: '5m' },
  { label: '15 minutes', value: '15m' },
  { label: '30 minutes', value: '30m' },
  { label: '1 hour', value: '1h' },
  { label: '2 hours', value: '2h' },
  { label: '6 hours', value: '6h' },
  { label: '12 hours', value: '12h' },
  { label: '24 hours', value: '24h' },
]

const POSITIONS = [
  { label: 'Bottom', value: 'bottom' },
  { label: 'Top', value: 'top' },
  { label: 'Left', value: 'left' },
  { label: 'Right', value: 'right' },
] as const


export default function GeneralSettings({ config, onChange, errors, appInfo, onResetPickerPosition }: Props) {
  const { daemon, picker } = config
  const setDaemon = <K extends keyof Config['daemon']>(key: K, value: Config['daemon'][K]) =>
    onChange({ ...config, daemon: { ...daemon, [key]: value } })
  const setPicker = <K extends keyof Config['picker']>(key: K, value: Config['picker'][K]) =>
    onChange({ ...config, picker: { ...picker, [key]: value } })

  const isPreset = INTERVAL_PRESETS.some((p) => p.value === daemon.interval)
  const os = appInfo?.os
  const fileManager = fileManagerName(os)

  const dirRow = (key: 'download_dir' | 'trash_dir', label: string, hint: string, placeholder: string) => (
    <Row label={label} hint={hint}>
      {({ labelId, hintId }) => (
        <div className="flex items-center gap-2">
          <input
            type="text"
            value={daemon[key]}
            onChange={(e) => setDaemon(key, e.target.value)}
            placeholder={placeholder}
            aria-labelledby={labelId}
            aria-describedby={hintId}
            spellCheck={false}
            className="form-input w-48 text-xs py-1 font-mono"
          />
          <button
            type="button"
            onClick={async () => {
              const p = await selectDirectory()
              if (p) setDaemon(key, p)
            }}
            className="btn-secondary text-xs py-1 px-2"
          >
            Browse…
          </button>
          <button
            type="button"
            onClick={() => void openFolder(daemon[key]).catch(() => {})}
            className="btn-secondary text-xs py-1 px-2"
            title={`Open in ${fileManager}`}
          >
            Open
          </button>
        </div>
      )}
    </Row>
  )

  return (
    <div className="space-y-6 max-w-2xl">
      <Group title="Startup">
        <StartAtLogin />
      </Group>

      <Group title="Rotation">
        <Row label="Change wallpaper automatically" hint="Rotate on a timer.">
          {({ labelId, hintId }) => (
            <Toggle
              enabled={daemon.rotation_enabled}
              onChange={(v) => setDaemon('rotation_enabled', v)}
              labelledBy={labelId}
              describedBy={hintId}
            />
          )}
        </Row>
        <Row
          label="Interval"
          hint="A preset, or a duration such as 45m or 1h30m."
          error={errors.interval}
        >
          {({ labelId, hintId }) => (
            <div className="flex items-center gap-2">
              <select
                value={isPreset ? daemon.interval : '__custom'}
                onChange={(e) => {
                  if (e.target.value !== '__custom') setDaemon('interval', e.target.value)
                }}
                aria-labelledby={labelId}
                className="form-input w-36 text-xs py-1"
                disabled={!daemon.rotation_enabled}
              >
                {INTERVAL_PRESETS.map((p) => (
                  <option key={p.value} value={p.value}>
                    {p.label}
                  </option>
                ))}
                {!isPreset && <option value="__custom">Custom</option>}
              </select>
              <input
                type="text"
                value={daemon.interval}
                onChange={(e) => setDaemon('interval', e.target.value)}
                placeholder="30m"
                aria-label="Custom interval"
                aria-describedby={hintId}
                aria-invalid={errors.interval ? true : undefined}
                spellCheck={false}
                autoComplete="off"
                className={`form-input w-24 text-xs py-1 font-mono ${errors.interval ? 'border-red-500!' : ''}`}
                disabled={!daemon.rotation_enabled}
              />
            </div>
          )}
        </Row>
        <Row label="Change with light and dark mode" hint="Pick a new wallpaper when the system appearance switches.">
          {({ labelId, hintId }) => (
            <Toggle
              enabled={daemon.rotate_on_theme_change}
              onChange={(v) => setDaemon('rotate_on_theme_change', v)}
              labelledBy={labelId}
              describedBy={hintId}
            />
          )}
        </Row>
        {os === 'macos' && (
          <Row
            label="Keep wallpaper on every Space"
            hint="Re-applies the wallpaper when you switch Spaces. This turns off 'Show on all Spaces' in System Settings."
          >
            {({ labelId, hintId }) => (
              <Toggle
                enabled={daemon.reapply_on_space_change}
                onChange={(v) => setDaemon('reapply_on_space_change', v)}
                labelledBy={labelId}
                describedBy={hintId}
              />
            )}
          </Row>
        )}
        <Row label="Cache size" hint="Downloaded wallpapers to keep.">
          {({ labelId, hintId }) => (
            <div className="flex items-center gap-2">
              <input
                type="range"
                min={10}
                max={500}
                step={10}
                value={daemon.cache_size}
                onChange={(e) => setDaemon('cache_size', Number(e.target.value))}
                aria-labelledby={labelId}
                aria-describedby={hintId}
                className="w-28 accent-accent dark:accent-accent-dark"
              />
              <span className="text-sm w-10 text-right tabular-nums" aria-hidden="true">
                {daemon.cache_size}
              </span>
            </div>
          )}
        </Row>
      </Group>

      <Group title="Folders">
        {dirRow('download_dir', 'Downloads', 'Where downloaded wallpapers are cached.', '~/Pictures/Wallpapers/cache')}
        {dirRow('trash_dir', 'Trash', 'Where trashed wallpapers are moved.', '~/Pictures/Wallpapers/trash')}
        <Row
          label="Socket path"
          hint="Leave empty for the default. Takes effect after the daemon and kakemono restart."
        >
          {({ labelId, hintId }) => (
            <input
              type="text"
              value={daemon.socket_path ?? ''}
              onChange={(e) => setDaemon('socket_path', e.target.value === '' ? null : e.target.value)}
              placeholder="Default"
              aria-labelledby={labelId}
              aria-describedby={hintId}
              spellCheck={false}
              className="form-input w-56 text-xs py-1 font-mono"
            />
          )}
        </Row>
      </Group>

      <Group title="Picker">
        <Row label="Screen edge" hint="Where the picker opens, on the display under the pointer.">
          {({ labelId, hintId }) => (
            <select
              value={picker.position}
              onChange={(e) => {
                const next = POSITIONS.find((p) => p.value === e.target.value)
                if (next) setPicker('position', next.value)
              }}
              aria-labelledby={labelId}
              aria-describedby={hintId}
              className="form-input w-36 text-xs py-1"
            >
              {POSITIONS.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.label}
                </option>
              ))}
            </select>
          )}
        </Row>
        <Row label="Close when it loses focus">
          {({ labelId }) => (
            <Toggle
              enabled={picker.close_on_blur}
              onChange={(v) => setPicker('close_on_blur', v)}
              labelledBy={labelId}
            />
          )}
        </Row>
        {picker.placement === 'custom' && (
          <Row label="Saved position" hint="The picker reopens where you last dragged it.">
            {() => (
              <button type="button" onClick={onResetPickerPosition} className="btn-secondary text-xs py-1 px-2">
                Reset position
              </button>
            )}
          </Row>
        )}
        {os === 'linux' && (
          <Row
            label="Open the picker through XWayland"
            hint="Experimental. Lets the picker position itself on Wayland; may look blurry with fractional scaling."
          >
            {({ labelId, hintId }) => (
              <Toggle
                enabled={picker.force_x11}
                onChange={(v) => setPicker('force_x11', v)}
                labelledBy={labelId}
                describedBy={hintId}
              />
            )}
          </Row>
        )}
      </Group>

      {appInfo && (
        <p className="text-xs text-gray-500 dark:text-gray-400 break-all">
          Config file: <span className="font-mono select-text">{appInfo.config_path}</span>
          {appInfo.legacy_config && ' (shared with the old wallpaperd app)'}
        </p>
      )}
    </div>
  )
}
