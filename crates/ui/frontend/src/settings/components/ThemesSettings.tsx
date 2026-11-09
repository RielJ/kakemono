import type { Config } from '../../shared/api'
import { SOURCE_LABELS, SOURCES } from '../sections'
import Icon from './Icon'
import TagInput from './TagInput'

interface Props {
  config: Config
  onChange: (config: Config) => void
}

type ThemeConfig = Config['light']

function SourcePicker({ selected, onChange, label }: { selected: string[]; onChange: (v: string[]) => void; label: string }) {
  const toggle = (name: string) =>
    onChange(selected.includes(name) ? selected.filter((s) => s !== name) : [...selected, name])

  return (
    <div className="flex flex-wrap gap-2" role="group" aria-label={label}>
      {SOURCES.map((p) => {
        const active = selected.includes(p)
        return (
          <button
            key={p}
            type="button"
            onClick={() => toggle(p)}
            aria-pressed={active}
            className={[
              'inline-flex items-center gap-1 px-3 py-1 rounded-full text-xs font-medium border',
              'transition-colors duration-100 motion-reduce:transition-none',
              'focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent',
              active
                ? 'bg-accent/10 border-accent/40 text-accent dark:bg-accent-dark/15 dark:border-accent-dark/40 dark:text-accent-dark'
                : 'bg-white border-gray-200 text-gray-600 hover:border-gray-400 dark:bg-gray-800 dark:border-gray-600 dark:text-gray-300 dark:hover:border-gray-400',
            ].join(' ')}
          >
            {active && <Icon name="check" className="w-3 h-3" />}
            {SOURCE_LABELS[p]}
          </button>
        )
      })}
    </div>
  )
}

function ThemePanel({ mode, theme, onChange }: { mode: 'light' | 'dark'; theme: ThemeConfig; onChange: (t: ThemeConfig) => void }) {
  const label = mode === 'light' ? 'Light appearance' : 'Dark appearance'
  return (
    <section className="rounded-lg border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800/60" aria-label={label}>
      <h3 className="flex items-center gap-2 px-4 py-2.5 border-b border-gray-100 dark:border-gray-700/70 text-sm font-medium">
        <Icon name={mode === 'light' ? 'sun' : 'moon'} className="w-4 h-4 text-gray-500 dark:text-gray-400" />
        {label}
      </h3>
      <div className="p-4 space-y-4">
        <div>
          <p className="form-label">Sources</p>
          <SourcePicker
            selected={theme.sources}
            onChange={(s) => onChange({ ...theme, sources: s })}
            label={`Sources for ${label.toLowerCase()}`}
          />
        </div>
        <div>
          <p className="form-label">Search terms</p>
          <TagInput
            values={theme.queries}
            onChange={(q) => onChange({ ...theme, queries: q })}
            label={`Add search term for ${label.toLowerCase()}`}
            placeholder={mode === 'light' ? 'landscape' : 'night sky'}
            emptyText="No search terms: sources return their usual picks."
          />
          <p className="mt-1.5 text-xs text-gray-500 dark:text-gray-400">
            Used by sources that support search (Wallhaven, Unsplash, Reddit).
          </p>
        </div>
      </div>
    </section>
  )
}

export default function ThemesSettings({ config, onChange }: Props) {
  return (
    <div className="space-y-4 max-w-2xl">
      <p className="text-xs text-gray-500 dark:text-gray-400">
        kakemono follows the system appearance and uses the matching sources and search terms.
      </p>
      <ThemePanel mode="light" theme={config.light} onChange={(t) => onChange({ ...config, light: t })} />
      <ThemePanel mode="dark" theme={config.dark} onChange={(t) => onChange({ ...config, dark: t })} />
    </div>
  )
}
