import { useState } from 'react'

import type { Config } from '../../shared/api'
import type { BrightnessFilter } from '../../shared/bindings/BrightnessFilter'
import { Group } from './Group'
import { CheckLine, LineText } from './SettingLine'
import TextField from './TextField'

type Tone = Exclude<BrightnessFilter, 'off'>

const TONES: readonly { value: Tone; label: string }[] = [
  { value: 'dark', label: 'Dark' },
  { value: 'light', label: 'Light' },
  { value: 'system', label: 'Match system appearance' },
]

const SIZES = [50, 80, 100] as const
const RATINGS = [1, 2, 3, 4, 5] as const

const isTone = (v: string): v is Tone => TONES.some((t) => t.value === v)

/**
 * Why `pattern` isn't a valid JavaScript regex, or `null`. The daemon uses Rust `regex`, whose
 * syntax differs a little, so this only warns; the value is saved. A leading inline flag group
 * such as `(?i)` is valid Rust that JavaScript rejects, so it is ignored for the check.
 */
export function regexWarning(pattern: string): string | null {
  try {
    new RegExp(pattern.replace(/^\(\?[a-zA-Z]+\)/, ''))
    return null
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    // "Invalid regular expression: /(x/: Unterminated group" -> "unterminated group"
    const reason = message.slice(message.lastIndexOf(': ') + 2)
    const why = reason ? reason.charAt(0).toLowerCase() + reason.slice(1) : 'syntax error'
    return `This may not be a valid regular expression: ${why}. It is saved anyway; the daemon checks it.`
  }
}

/**
 * General → "When possible, use images that…": Variety's soft filters. A change takes the
 * first local image that passes every ticked filter, or the closest one when none does.
 */
export default function FiltersGroup({ config, onChange }: { config: Config; onChange: (config: Config) => void }) {
  const f = config.filters
  const set = <K extends keyof Config['filters']>(key: K, value: Config['filters'][K]) =>
    onChange({ ...config, filters: { ...f, [key]: value } })

  // `brightness = "off"` stores no tone, so remember the last one picked for when it's ticked.
  const [tone, setTone] = useState<Tone>(f.brightness === 'off' ? 'dark' : f.brightness)
  const shownTone = f.brightness === 'off' ? tone : f.brightness

  return (
    <Group
      title="When possible, use images that…"
      footer="If no image passes, the closest match is used. New downloads that don’t pass are deleted."
    >
      <CheckLine checked={f.landscape} onChange={(v) => set('landscape', v)}>
        <LineText>Have landscape orientation</LineText>
      </CheckLine>

      <CheckLine checked={f.min_size_enabled} onChange={(v) => set('min_size_enabled', v)}>
        <LineText>Are big, at least</LineText>
        <select
          value={f.min_size_percent}
          onChange={(e) => set('min_size_percent', Number(e.target.value))}
          aria-label="Minimum size, percent of the screen"
          className="field w-[4.5rem]"
        >
          {SIZES.map((n) => (
            <option key={n} value={n}>
              {n}
            </option>
          ))}
        </select>
        <LineText>% of the screen resolution</LineText>
      </CheckLine>

      <CheckLine checked={f.brightness !== 'off'} onChange={(v) => set('brightness', v ? shownTone : 'off')}>
        <LineText>Are dark or light</LineText>
        <select
          value={shownTone}
          onChange={(e) => {
            const next = e.target.value
            if (!isTone(next)) return
            setTone(next)
            if (f.brightness !== 'off') set('brightness', next)
          }}
          aria-label="Brightness"
          className="field w-[13rem]"
        >
          {TONES.map((t) => (
            <option key={t.value} value={t.value}>
              {t.label}
            </option>
          ))}
        </select>
      </CheckLine>

      <CheckLine checked={f.color_enabled} onChange={(v) => set('color_enabled', v)}>
        <LineText>Contain this color</LineText>
        <input
          type="color"
          value={f.color}
          onChange={(e) => set('color', e.target.value)}
          aria-label="Color to look for"
          className="h-7 w-10 p-0.5 rounded-md border border-line-strong bg-field cursor-pointer"
        />
      </CheckLine>

      <CheckLine checked={f.rating_enabled} onChange={(v) => set('rating_enabled', v)}>
        <LineText>Have EXIF rating at least</LineText>
        <select
          value={f.min_rating}
          onChange={(e) => set('min_rating', Number(e.target.value))}
          aria-label="Minimum rating"
          className="field w-[3.75rem]"
        >
          {RATINGS.map((n) => (
            <option key={n} value={n}>
              {n}
            </option>
          ))}
        </select>
      </CheckLine>

      <CheckLine
        checked={f.name_regex_enabled}
        onChange={(v) => set('name_regex_enabled', v)}
        warning={regexWarning(f.name_regex)}
      >
        <LineText>File name matches regex</LineText>
        <TextField
          value={f.name_regex}
          onCommit={(v) => set('name_regex', v)}
          placeholder="e.g. ^wh-"
          aria-label="File name regex"
          mono
          className="w-48"
        />
      </CheckLine>
    </Group>
  )
}
