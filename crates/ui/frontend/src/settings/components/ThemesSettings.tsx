import type { Config, ThemeConfig } from '../types/config'

interface Props {
  config: Config
  onChange: (config: Config) => void
}

// Provider names that can appear in theme sources
const ALL_PROVIDERS = ['bing', 'wallhaven', 'unsplash', 'reddit', 'apod', 'local'] as const
type ProviderName = typeof ALL_PROVIDERS[number]

const PROVIDER_LABELS: Record<ProviderName, string> = {
  bing:      'Bing Daily',
  wallhaven: 'Wallhaven',
  unsplash:  'Unsplash',
  reddit:    'Reddit',
  apod:      'NASA APOD',
  local:     'Local',
}

// ── Source checkboxes ─────────────────────────────────────────────────────────

interface SourcePickerProps {
  selected: string[]
  onChange: (v: string[]) => void
}

function SourcePicker({ selected, onChange }: SourcePickerProps) {
  const toggle = (name: string) => {
    if (selected.includes(name)) {
      onChange(selected.filter(s => s !== name))
    } else {
      onChange([...selected, name])
    }
  }

  return (
    <div className="flex flex-wrap gap-2">
      {ALL_PROVIDERS.map(p => {
        const active = selected.includes(p)
        return (
          <button
            key={p}
            onClick={() => toggle(p)}
            className={`
              no-drag px-3 py-1 rounded-full text-xs font-medium
              border transition-colors duration-100
              focus:outline-none focus:ring-2 focus:ring-accent/40
              ${active
                ? 'bg-accent/10 border-accent/40 text-accent dark:bg-accent-dark/10 dark:border-accent-dark/40 dark:text-accent-dark'
                : 'bg-white border-gray-200 text-gray-500 hover:border-gray-400 dark:bg-gray-800 dark:border-gray-600 dark:text-gray-400 dark:hover:border-gray-400'
              }
            `}
            aria-pressed={active}
          >
            {PROVIDER_LABELS[p]}
          </button>
        )
      })}
    </div>
  )
}

// ── Query tag list ────────────────────────────────────────────────────────────

import { useState } from 'react'

interface QueryListProps {
  queries: string[]
  onChange: (v: string[]) => void
  placeholder?: string
}

function QueryList({ queries, onChange, placeholder }: QueryListProps) {
  const [input, setInput] = useState('')

  const add = () => {
    const v = input.trim()
    if (v && !queries.includes(v)) {
      onChange([...queries, v])
    }
    setInput('')
  }

  const remove = (q: string) => onChange(queries.filter(x => x !== q))

  return (
    <div className="space-y-2">
      <div className="flex flex-wrap gap-1.5 min-h-6">
        {queries.length === 0 && (
          <span className="text-xs text-gray-400 dark:text-gray-500 italic">No queries — all results are used</span>
        )}
        {queries.map(q => (
          <span
            key={q}
            className="inline-flex items-center gap-1 px-2.5 py-0.5 rounded-full text-xs
              bg-gray-100 text-gray-700 dark:bg-gray-700 dark:text-gray-200"
          >
            {q}
            <button
              onClick={() => remove(q)}
              className="no-drag ml-0.5 text-gray-400 hover:text-red-500 dark:hover:text-red-400"
              aria-label={`Remove "${q}"`}
            >
              ×
            </button>
          </span>
        ))}
      </div>
      <div className="flex gap-2">
        <input
          type="text"
          value={input}
          onChange={e => setInput(e.target.value)}
          onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); add() } }}
          placeholder={placeholder ?? 'Add search query…'}
          className="form-input flex-1"
        />
        <button onClick={add} className="btn-secondary no-drag">Add</button>
      </div>
    </div>
  )
}

// ── Theme panel (light or dark) ───────────────────────────────────────────────

interface ThemePanelProps {
  mode: 'light' | 'dark'
  theme: ThemeConfig
  onChange: (t: ThemeConfig) => void
}

function ThemePanel({ mode, theme, onChange }: ThemePanelProps) {
  const icon = mode === 'light' ? '☀️' : '🌙'
  const label = mode === 'light' ? 'Light Mode' : 'Dark Mode'

  return (
    <div className="rounded-lg border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800/50 overflow-hidden">
      <div className="px-4 py-2.5 border-b border-gray-100 dark:border-gray-700/50 bg-gray-50 dark:bg-gray-800">
        <h3 className="text-sm font-semibold text-gray-700 dark:text-gray-200 flex items-center gap-1.5">
          <span>{icon}</span> {label}
        </h3>
      </div>
      <div className="p-4 space-y-4">
        <div>
          <label className="form-label">Active providers</label>
          <SourcePicker
            selected={theme.sources}
            onChange={s => onChange({ ...theme, sources: s })}
          />
          <p className="mt-1.5 text-xs text-gray-400 dark:text-gray-500">
            Selected providers are used when the system is in {label.toLowerCase()}.
          </p>
        </div>
        <div>
          <label className="form-label">Search queries</label>
          <QueryList
            queries={theme.queries}
            onChange={q => onChange({ ...theme, queries: q })}
            placeholder={mode === 'light' ? 'landscape, nature…' : 'space, night sky…'}
          />
          <p className="mt-1.5 text-xs text-gray-400 dark:text-gray-500">
            Passed to providers that support search (Wallhaven, Unsplash, Reddit).
          </p>
        </div>
      </div>
    </div>
  )
}

// ── Main component ────────────────────────────────────────────────────────────

export default function ThemesSettings({ config, onChange }: Props) {
  return (
    <div className="space-y-4">
      <p className="text-xs text-gray-500 dark:text-gray-400">
        wallpaperd detects the current macOS appearance and uses the matching provider list and search queries.
      </p>

      <ThemePanel
        mode="light"
        theme={config.light}
        onChange={t => onChange({ ...config, light: t })}
      />

      <ThemePanel
        mode="dark"
        theme={config.dark}
        onChange={t => onChange({ ...config, dark: t })}
      />
    </div>
  )
}
