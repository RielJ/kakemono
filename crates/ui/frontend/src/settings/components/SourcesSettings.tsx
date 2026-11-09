import { useId, useState, type ReactNode } from 'react'

import type { Config } from '../../shared/api'
import { selectDirectory } from '../../shared/api'
import { SOURCE_LABELS } from '../sections'
import TagInput from './TagInput'
import Toggle from './Toggle'

interface Props {
  config: Config
  onChange: (config: Config) => void
}

type WallhavenConfig = Config['wallhaven']

function SourceCard({
  title,
  description,
  enabled,
  onToggle,
  children,
}: {
  title: string
  description: string
  enabled: boolean
  onToggle: (v: boolean) => void
  children?: ReactNode
}) {
  const [expanded, setExpanded] = useState(false)
  const titleId = useId()
  const descId = useId()
  const panelId = useId()
  return (
    <div className="rounded-lg border border-gray-200 bg-white dark:border-gray-700 dark:bg-gray-800/60">
      <div className="flex items-center gap-3 px-4 py-3">
        <Toggle enabled={enabled} onChange={onToggle} labelledBy={titleId} describedBy={descId} />
        <div className="flex-1 min-w-0">
          <p id={titleId} className="text-sm font-medium">{title}</p>
          <p id={descId} className="mt-0.5 text-xs text-gray-500 dark:text-gray-400">{description}</p>
        </div>
        {children && (
          <button
            type="button"
            onClick={() => setExpanded((e) => !e)}
            aria-expanded={expanded}
            aria-controls={panelId}
            className="btn-secondary text-xs py-1 px-2"
          >
            {expanded ? 'Done' : 'Options'}
          </button>
        )}
      </div>
      {children && expanded && (
        <div id={panelId} className="px-4 pb-4 pt-3 border-t border-gray-100 dark:border-gray-700/70 space-y-3">
          {children}
        </div>
      )}
    </div>
  )
}

function Field({ label, children }: { label: string; children: (id: string) => ReactNode }) {
  const id = useId()
  return (
    <div>
      <label htmlFor={id} className="form-label">
        {label}
      </label>
      {children(id)}
    </div>
  )
}

function WallhavenOptions({ cfg, onChange }: { cfg: WallhavenConfig; onChange: (c: WallhavenConfig) => void }) {
  return (
    <>
      <Field label="API key (optional)">
        {(id) => (
          <input
            id={id}
            type="password"
            value={cfg.api_key}
            onChange={(e) => onChange({ ...cfg, api_key: e.target.value })}
            placeholder="Not needed for safe-for-work images"
            autoComplete="off"
            className="form-input text-xs py-1"
          />
        )}
      </Field>
      <div className="grid grid-cols-3 gap-3">
        <Field label="Purity">
          {(id) => (
            <select id={id} value={cfg.purity} onChange={(e) => onChange({ ...cfg, purity: e.target.value })} className="form-input text-xs py-1">
              <option value="100">Safe for work</option>
              <option value="110">Safe + sketchy</option>
              <option value="111">All (needs API key)</option>
            </select>
          )}
        </Field>
        <Field label="Sort by">
          {(id) => (
            <select id={id} value={cfg.sorting} onChange={(e) => onChange({ ...cfg, sorting: e.target.value })} className="form-input text-xs py-1">
              <option value="random">Random</option>
              <option value="toplist">Top list</option>
              <option value="date_added">Date added</option>
              <option value="hot">Hot</option>
            </select>
          )}
        </Field>
        <Field label="Categories">
          {(id) => (
            <select id={id} value={cfg.categories} onChange={(e) => onChange({ ...cfg, categories: e.target.value })} className="form-input text-xs py-1">
              <option value="100">General</option>
              <option value="010">Anime</option>
              <option value="001">People</option>
              <option value="111">All</option>
            </select>
          )}
        </Field>
      </div>
    </>
  )
}

export default function SourcesSettings({ config, onChange }: Props) {
  const upd = (patch: Partial<Config>) => onChange({ ...config, ...patch })
  return (
    <div className="space-y-3 max-w-2xl">
      <SourceCard
        title={SOURCE_LABELS.bing}
        description="Bing's photo of the day. No account needed."
        enabled={config.bing.enabled}
        onToggle={(v) => upd({ bing: { ...config.bing, enabled: v } })}
      >
        <Field label="Market">
          {(id) => (
            <input
              id={id}
              type="text"
              value={config.bing.market}
              onChange={(e) => upd({ bing: { ...config.bing, market: e.target.value } })}
              placeholder="en-US"
              spellCheck={false}
              className="form-input max-w-xs text-xs py-1"
            />
          )}
        </Field>
      </SourceCard>

      <SourceCard
        title={SOURCE_LABELS.wallhaven}
        description="Community wallpapers with quality filters."
        enabled={config.wallhaven.enabled}
        onToggle={(v) => upd({ wallhaven: { ...config.wallhaven, enabled: v } })}
      >
        <WallhavenOptions cfg={config.wallhaven} onChange={(c) => upd({ wallhaven: c })} />
      </SourceCard>

      <SourceCard
        title={SOURCE_LABELS.unsplash}
        description="High-resolution photography."
        enabled={config.unsplash.enabled}
        onToggle={(v) => upd({ unsplash: { ...config.unsplash, enabled: v } })}
      >
        <Field label="Access key (optional)">
          {(id) => (
            <input
              id={id}
              type="password"
              value={config.unsplash.access_key}
              onChange={(e) => upd({ unsplash: { ...config.unsplash, access_key: e.target.value } })}
              placeholder="Leave empty for anonymous access"
              autoComplete="off"
              className="form-input text-xs py-1"
            />
          )}
        </Field>
      </SourceCard>

      <SourceCard
        title={SOURCE_LABELS.reddit}
        description="Top images from the subreddits you list."
        enabled={config.reddit.enabled}
        onToggle={(v) => upd({ reddit: { ...config.reddit, enabled: v } })}
      >
        <TagInput
          values={config.reddit.subreddits}
          onChange={(v) => upd({ reddit: { ...config.reddit, subreddits: v } })}
          label="Add subreddit"
          placeholder="EarthPorn"
        />
      </SourceCard>

      <SourceCard
        title={SOURCE_LABELS.apod}
        description="NASA's Astronomy Picture of the Day."
        enabled={config.apod.enabled}
        onToggle={(v) => upd({ apod: { ...config.apod, enabled: v } })}
      >
        <Field label="API key">
          {(id) => (
            <input
              id={id}
              type="text"
              value={config.apod.api_key}
              onChange={(e) => upd({ apod: { ...config.apod, api_key: e.target.value } })}
              placeholder="DEMO_KEY"
              spellCheck={false}
              className="form-input max-w-xs text-xs py-1 font-mono"
            />
          )}
        </Field>
      </SourceCard>

      <SourceCard
        title={SOURCE_LABELS.local}
        description="Images already on this computer."
        enabled={config.local.enabled}
        onToggle={(v) => upd({ local: { ...config.local, enabled: v } })}
      >
        <TagInput
          values={config.local.dirs}
          onChange={(v) => upd({ local: { ...config.local, dirs: v } })}
          label="Add folder"
          placeholder="~/Pictures/Wallpapers"
          emptyText="No folders yet."
        />
        <div className="flex items-center gap-3">
          <button
            type="button"
            onClick={async () => {
              const p = await selectDirectory()
              if (p && !config.local.dirs.includes(p)) {
                upd({ local: { ...config.local, dirs: [...config.local.dirs, p] } })
              }
            }}
            className="btn-secondary text-xs py-1 px-2"
          >
            Choose folder…
          </button>
          <label className="flex items-center gap-2 cursor-pointer select-none">
            <input
              type="checkbox"
              checked={config.local.recursive}
              onChange={(e) => upd({ local: { ...config.local, recursive: e.target.checked } })}
              className="rounded border-gray-300 dark:border-gray-600 accent-accent dark:accent-accent-dark"
            />
            <span className="text-sm">Include subfolders</span>
          </label>
        </div>
      </SourceCard>
    </div>
  )
}
