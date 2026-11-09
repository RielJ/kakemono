import { useState } from 'react'
import type { Config, WallhavenConfig } from '../types/config'

interface Props {
  config: Config
  onChange: (config: Config) => void
}

function Toggle({ id, checked, onChange }: { id: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label htmlFor={id} className="cursor-pointer">
      <input id={id} type="checkbox" checked={checked} onChange={e => onChange(e.target.checked)} className="sr-only" />
      <div className={`relative w-9 h-5 rounded-full transition-colors duration-200 ${checked ? 'bg-accent dark:bg-accent-dark' : 'bg-gray-300 dark:bg-gray-600'}`}>
        <div className={`absolute top-0.5 left-0.5 w-4 h-4 bg-white rounded-full shadow transition-transform duration-200 ${checked ? 'translate-x-4' : ''}`} />
      </div>
    </label>
  )
}

function TagInput({ values, onChange, placeholder }: { values: string[]; onChange: (v: string[]) => void; placeholder?: string }) {
  const [input, setInput] = useState('')
  const add = () => {
    const v = input.trim()
    if (v && !values.includes(v)) onChange([...values, v])
    setInput('')
  }
  return (
    <div className="space-y-1.5">
      <div className="flex flex-wrap gap-1.5 min-h-5">
        {values.map(v => (
          <span key={v} className="inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs bg-gray-100 dark:bg-gray-700 text-gray-700 dark:text-gray-200">
            {v}
            <button onClick={() => onChange(values.filter(x => x !== v))} className="no-drag text-gray-400 hover:text-red-500">×</button>
          </span>
        ))}
      </div>
      <div className="flex gap-2">
        <input type="text" value={input} onChange={e => setInput(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); add() } }}
          placeholder={placeholder ?? 'Add…'} className="form-input flex-1 text-xs py-1" />
        <button onClick={add} className="btn-secondary no-drag text-xs py-1 px-2">Add</button>
      </div>
    </div>
  )
}

function ProviderCard({ id, title, description, enabled, onToggle, children }: {
  id: string; title: string; description: string
  enabled: boolean; onToggle: (v: boolean) => void
  children?: React.ReactNode
}) {
  const [expanded, setExpanded] = useState(false)
  return (
    <div className={`rounded-xl border transition-colors duration-150 ${enabled ? 'border-accent/30 bg-accent/5 dark:border-accent-dark/30 dark:bg-accent-dark/5' : 'border-zinc-200 bg-white dark:border-zinc-700 dark:bg-zinc-800/50'}`}>
      <div className="flex items-start gap-3 px-4 py-3">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <Toggle id={`toggle-${id}`} checked={enabled} onChange={onToggle} />
            <label htmlFor={`toggle-${id}`} className="text-sm font-medium cursor-pointer">{title}</label>
          </div>
          <p className="mt-0.5 ml-11 text-xs text-zinc-400 dark:text-zinc-500">{description}</p>
        </div>
        {children && (
          <button onClick={() => setExpanded(e => !e)}
            className="no-drag text-xs px-2 py-1 rounded text-zinc-500 hover:text-zinc-700 dark:text-zinc-400 dark:hover:text-zinc-200 hover:bg-zinc-100 dark:hover:bg-zinc-700">
            {expanded ? 'Less' : 'Configure'}
          </button>
        )}
      </div>
      {children && expanded && (
        <div className="px-4 pb-4 border-t border-zinc-100 dark:border-zinc-700/50 pt-3 space-y-3">{children}</div>
      )}
    </div>
  )
}

function WallhavenCfg({ cfg, onChange }: { cfg: WallhavenConfig; onChange: (c: WallhavenConfig) => void }) {
  return (
    <>
      <div>
        <label className="form-label">API Key (optional)</label>
        <input type="password" value={cfg.api_key} onChange={e => onChange({ ...cfg, api_key: e.target.value })}
          placeholder="Leave blank for public access" className="form-input text-xs py-1" />
      </div>
      <div className="grid grid-cols-3 gap-3">
        <div>
          <label className="form-label">Purity</label>
          <select value={cfg.purity} onChange={e => onChange({ ...cfg, purity: e.target.value })} className="form-input text-xs py-1">
            <option value="100">SFW only</option>
            <option value="110">SFW + Sketchy</option>
            <option value="111">All (API key required)</option>
          </select>
        </div>
        <div>
          <label className="form-label">Sort by</label>
          <select value={cfg.sorting} onChange={e => onChange({ ...cfg, sorting: e.target.value })} className="form-input text-xs py-1">
            <option value="random">Random</option>
            <option value="toplist">Top list</option>
            <option value="date_added">Date added</option>
            <option value="hot">Hot</option>
          </select>
        </div>
        <div>
          <label className="form-label">Categories</label>
          <select value={cfg.categories} onChange={e => onChange({ ...cfg, categories: e.target.value })} className="form-input text-xs py-1">
            <option value="100">General</option>
            <option value="010">Anime</option>
            <option value="001">People</option>
            <option value="111">All</option>
          </select>
        </div>
      </div>
    </>
  )
}

export default function ProvidersSettings({ config, onChange }: Props) {
  const upd = (patch: Partial<Config>) => onChange({ ...config, ...patch })
  return (
    <div className="space-y-3 max-w-xl">
      <ProviderCard id="bing" title="Bing Daily" description="Microsoft Bing photo of the day. No API key required."
        enabled={config.bing.enabled} onToggle={v => upd({ bing: { ...config.bing, enabled: v } })}>
        <div>
          <label className="form-label">Market / region</label>
          <input type="text" value={config.bing.market} onChange={e => upd({ bing: { ...config.bing, market: e.target.value } })}
            placeholder="en-US" className="form-input max-w-xs text-xs py-1" />
        </div>
      </ProviderCard>

      <ProviderCard id="wallhaven" title="Wallhaven" description="Community wallpaper site with quality filtering."
        enabled={config.wallhaven.enabled} onToggle={v => upd({ wallhaven: { ...config.wallhaven, enabled: v } })}>
        <WallhavenCfg cfg={config.wallhaven} onChange={c => upd({ wallhaven: c })} />
      </ProviderCard>

      <ProviderCard id="unsplash" title="Unsplash" description="High-resolution photography."
        enabled={config.unsplash.enabled} onToggle={v => upd({ unsplash: { ...config.unsplash, enabled: v } })}>
        <div>
          <label className="form-label">Access Key (optional)</label>
          <input type="password" value={config.unsplash.access_key} onChange={e => upd({ unsplash: { ...config.unsplash, access_key: e.target.value } })}
            placeholder="Leave blank for anonymous access" className="form-input text-xs py-1" />
        </div>
      </ProviderCard>

      <ProviderCard id="reddit" title="Reddit" description="Top images from any subreddit. No API key needed."
        enabled={config.reddit.enabled} onToggle={v => upd({ reddit: { ...config.reddit, enabled: v } })}>
        <div>
          <label className="form-label">Subreddits</label>
          <TagInput values={config.reddit.subreddits} onChange={v => upd({ reddit: { ...config.reddit, subreddits: v } })} placeholder="EarthPorn" />
        </div>
      </ProviderCard>

      <ProviderCard id="apod" title="NASA APOD" description="NASA Astronomy Picture of the Day."
        enabled={config.apod.enabled} onToggle={v => upd({ apod: { ...config.apod, enabled: v } })}>
        <div>
          <label className="form-label">API Key</label>
          <input type="text" value={config.apod.api_key} onChange={e => upd({ apod: { ...config.apod, api_key: e.target.value } })}
            placeholder="DEMO_KEY" className="form-input max-w-xs text-xs py-1" />
        </div>
      </ProviderCard>

      <ProviderCard id="local" title="Local Directories" description="Rotate through images already on your disk."
        enabled={config.local.enabled} onToggle={v => upd({ local: { ...config.local, enabled: v } })}>
        <div>
          <label className="form-label">Directories</label>
          <TagInput values={config.local.dirs} onChange={v => upd({ local: { ...config.local, dirs: v } })} placeholder="~/Pictures/My Wallpapers" />
        </div>
        <label className="flex items-center gap-2 cursor-pointer select-none mt-2">
          <input type="checkbox" checked={config.local.recursive}
            onChange={e => upd({ local: { ...config.local, recursive: e.target.checked } })}
            className="no-drag rounded border-gray-300 dark:border-gray-600 accent-accent dark:accent-accent-dark" />
          <span className="text-sm">Scan subdirectories recursively</span>
        </label>
      </ProviderCard>
    </div>
  )
}
