import type { Config } from '../types/config'
import { selectDirectory, openFolder } from '../lib/api'

interface Props {
  config: Config
  onChange: (config: Config) => void
}

const INTERVAL_PRESETS = [
  { label: '5 minutes',  value: '5m'  },
  { label: '15 minutes', value: '15m' },
  { label: '30 minutes', value: '30m' },
  { label: '1 hour',     value: '1h'  },
  { label: '2 hours',    value: '2h'  },
  { label: '6 hours',    value: '6h'  },
  { label: '12 hours',   value: '12h' },
  { label: '24 hours',   value: '24h' },
]

export default function GeneralSettings({ config, onChange }: Props) {
  const set = <K extends keyof typeof config.daemon>(
    key: K,
    value: typeof config.daemon[K],
  ) => onChange({ ...config, daemon: { ...config.daemon, [key]: value } })

  const { daemon } = config

  return (
    <div className="space-y-6 max-w-xl">
      <div>
        <h3 className="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400 mb-2">
          Rotation
        </h3>
        <div className="bg-white/80 dark:bg-zinc-800/70 rounded-xl divide-y divide-zinc-100 dark:divide-zinc-700 border border-zinc-200 dark:border-zinc-700">
          <Row label="Auto-rotate" hint="Automatically change wallpaper on a timer">
            <label className="cursor-pointer">
              <input type="checkbox" checked={daemon.rotation_enabled} onChange={e => set('rotation_enabled', e.target.checked)} className="sr-only" />
              <div className={`relative w-9 h-5 rounded-full transition-colors duration-200 ${daemon.rotation_enabled ? 'bg-accent dark:bg-accent-dark' : 'bg-gray-300 dark:bg-gray-600'}`}>
                <div className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform duration-200 ${daemon.rotation_enabled ? 'translate-x-4' : ''}`} />
              </div>
            </label>
          </Row>
          <div className={daemon.rotation_enabled ? '' : 'opacity-40 pointer-events-none'}>
            <Row label="Interval" hint="How often to rotate the wallpaper">
              <div className="flex items-center gap-2">
                <select
                  value={INTERVAL_PRESETS.some(p => p.value === daemon.interval) ? daemon.interval : '__custom'}
                  onChange={e => { if (e.target.value !== '__custom') set('interval', e.target.value) }}
                  className="form-input w-36 text-xs py-1"
                  disabled={!daemon.rotation_enabled}
                >
                  {INTERVAL_PRESETS.map(p => (
                    <option key={p.value} value={p.value}>{p.label}</option>
                  ))}
                  {!INTERVAL_PRESETS.some(p => p.value === daemon.interval) && (
                    <option value="__custom">{daemon.interval} (custom)</option>
                  )}
                </select>
                <input
                  type="text"
                  value={daemon.interval}
                  onChange={e => set('interval', e.target.value)}
                  placeholder="30m"
                  title="Custom Go duration (e.g. 45m, 1h30m)"
                  className="form-input w-24 text-xs py-1 font-mono"
                  disabled={!daemon.rotation_enabled}
                />
              </div>
            </Row>
          </div>
          <Row label="Cache size" hint="Max wallpapers to keep downloaded">
            <div className="flex items-center gap-2">
              <input
                type="range"
                min={10} max={500} step={10}
                value={daemon.cache_size}
                onChange={e => set('cache_size', Number(e.target.value))}
                className="no-drag w-28 accent-accent dark:accent-accent-dark"
              />
              <span className="text-sm font-medium w-10 text-right tabular-nums">
                {daemon.cache_size}
              </span>
            </div>
          </Row>
        </div>
      </div>

      <div>
        <h3 className="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400 mb-2">
          Directories
        </h3>
        <div className="bg-white/80 dark:bg-zinc-800/70 rounded-xl divide-y divide-zinc-100 dark:divide-zinc-700 border border-zinc-200 dark:border-zinc-700">
          <Row label="Download directory" hint="Where cached wallpapers are stored">
            <div className="flex items-center gap-2">
              <input
                type="text"
                value={daemon.download_dir}
                onChange={e => set('download_dir', e.target.value)}
                placeholder="~/Pictures/Wallpapers/cache"
                className="form-input w-44 text-xs py-1 font-mono"
              />
              <button onClick={async () => { const p = await selectDirectory(); if (p) set('download_dir', p) }} className="btn-secondary no-drag text-xs py-1 px-2">Browse</button>
              <button onClick={() => openFolder(daemon.download_dir)} className="btn-secondary no-drag text-xs py-1 px-2" title="Open in Finder">Open</button>
            </div>
          </Row>
          <Row label="Trash directory" hint="Where trashed wallpapers are moved">
            <div className="flex items-center gap-2">
              <input
                type="text"
                value={daemon.trash_dir}
                onChange={e => set('trash_dir', e.target.value)}
                placeholder="~/Pictures/Wallpapers/trash"
                className="form-input w-44 text-xs py-1 font-mono"
              />
              <button onClick={async () => { const p = await selectDirectory(); if (p) set('trash_dir', p) }} className="btn-secondary no-drag text-xs py-1 px-2">Browse</button>
              <button onClick={() => openFolder(daemon.trash_dir)} className="btn-secondary no-drag text-xs py-1 px-2" title="Open in Finder">Open</button>
            </div>
          </Row>
          <Row label="Socket path" hint="Unix socket for daemon IPC">
            <input
              type="text"
              value={daemon.socket_path}
              onChange={e => set('socket_path', e.target.value)}
              placeholder="~/.config/wallpaperd/daemon.sock"
              className="form-input w-52 text-xs py-1 font-mono"
            />
          </Row>
        </div>
      </div>

      <div>
        <h3 className="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400 mb-2">
          Picker
        </h3>
        <div className="bg-white/80 dark:bg-zinc-800/70 rounded-xl divide-y divide-zinc-100 dark:divide-zinc-700 border border-zinc-200 dark:border-zinc-700">
          <Row label="Position" hint="Where the floating thumbnail picker appears on screen">
            <select
              value={config.picker.position}
              onChange={e => onChange({ ...config, picker: { ...config.picker, position: e.target.value } })}
              className="form-input w-36 text-xs py-1"
            >
              <option value="bottom">Bottom</option>
              <option value="top">Top</option>
              <option value="left">Left</option>
              <option value="right">Right</option>
            </select>
          </Row>
        </div>
      </div>
    </div>
  )
}

// ── Shared layout primitives (exported for other tabs) ────────────────────────

export function Row({
  label,
  hint,
  children,
}: {
  label: string
  hint?: string
  children: React.ReactNode
}) {
  return (
    <div className="flex items-center justify-between px-4 py-3 gap-4">
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium">{label}</p>
        {hint && <p className="text-xs text-zinc-400 dark:text-zinc-500 mt-0.5">{hint}</p>}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  )
}

/** @deprecated Use Row instead */
export function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div>
      <h3 className="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400 mb-2">
        {title}
      </h3>
      <div className="bg-white/80 dark:bg-zinc-800/70 rounded-xl divide-y divide-zinc-100 dark:divide-zinc-700 border border-zinc-200 dark:border-zinc-700">
        {children}
      </div>
    </div>
  )
}

/** @deprecated Use Row instead */
export function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return <Row label={label} hint={hint}>{children}</Row>
}
