import { useId, useState } from 'react'

import type { Config } from '../../shared/api'
import { Group, Row } from './Group'
import Icon from './Icon'
import TextField from './TextField'

/** Display labels for Wallhaven's option codes; the Sources table summary uses them too. */
export const PURITY: Record<string, string> = { '100': 'SFW', '110': 'SFW + sketchy', '111': 'All' }
export const SORTING: Record<string, string> = { random: 'Random', toplist: 'Top list', date_added: 'Newest', hot: 'Hot' }
export const CATEGORIES: Record<string, string> = { '100': 'General', '010': 'Anime', '001': 'People', '111': 'All categories' }

type WallhavenChoice = 'purity' | 'sorting' | 'categories'

/** A key typed as a password, with a toggle that shows it. The toggle's name is fixed; `aria-pressed` is the state. */
function SecretField({
  name,
  value,
  onCommit,
  placeholder,
  labelledBy,
  describedBy,
}: {
  /** Names the toggle: "Show <name>". */
  name: string
  value: string
  onCommit: (value: string) => void
  placeholder: string
  labelledBy: string
  describedBy?: string
}) {
  const [shown, setShown] = useState(false)
  const inputId = useId()
  const label = `Show ${name}`
  return (
    <div className="flex items-center gap-1.5">
      <TextField
        id={inputId}
        type={shown ? 'text' : 'password'}
        value={value}
        onCommit={onCommit}
        placeholder={placeholder}
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        mono
        className="w-56"
      />
      <button
        type="button"
        className="btn w-7 shrink-0 px-0"
        aria-label={label}
        title={label}
        aria-pressed={shown}
        aria-controls={inputId}
        onClick={() => setShown((s) => !s)}
      >
        <Icon name={shown ? 'eyeOff' : 'eye'} className="w-4 h-4" />
      </button>
    </div>
  )
}

/** Credentials and options for the online providers. Moved here from the old Sources detail panel. */
export default function ProviderKeys({ config, onChange }: { config: Config; onChange: (config: Config) => void }) {
  const { wallhaven, unsplash, apod, bing } = config

  /** A Wallhaven option as a `Row` child. An unknown value from the file stays selectable. */
  const choice =
    (key: WallhavenChoice, options: Record<string, string>) =>
    ({ labelId, hintId }: { labelId: string; hintId: string | undefined }) => (
      <select
        value={wallhaven[key]}
        onChange={(e) => onChange({ ...config, wallhaven: { ...wallhaven, [key]: e.target.value } })}
        aria-labelledby={labelId}
        aria-describedby={hintId}
        className="field w-48"
      >
        {!(wallhaven[key] in options) && <option value={wallhaven[key]}>{wallhaven[key]}</option>}
        {Object.entries(options).map(([v, l]) => (
          <option key={v} value={v}>
            {l}
          </option>
        ))}
      </select>
    )

  return (
    <Group title="Provider keys" footer="Keys stay in config.toml on this computer and go only to their provider.">
      <Row label="Wallhaven API key" hint="Optional. Without one, kakemono reads the public site.">
        {({ labelId, hintId }) => (
          <SecretField
            name="Wallhaven API key"
            value={wallhaven.api_key}
            onCommit={(api_key) => onChange({ ...config, wallhaven: { ...wallhaven, api_key: api_key.trim() } })}
            placeholder="Not set"
            labelledBy={labelId}
            describedBy={hintId}
          />
        )}
      </Row>
      <Row label="Wallhaven sorting">{choice('sorting', SORTING)}</Row>
      <Row label="Wallhaven categories">{choice('categories', CATEGORIES)}</Row>
      <Row
        label="Wallhaven purity"
        hint={wallhaven.purity === '111' && !wallhaven.api_key ? 'Showing NSFW needs an API key.' : undefined}
      >
        {choice('purity', PURITY)}
      </Row>
      <Row label="Unsplash access key" hint="From unsplash.com/developers.">
        {({ labelId, hintId }) => (
          <SecretField
            name="Unsplash access key"
            value={unsplash.access_key}
            onCommit={(access_key) => onChange({ ...config, unsplash: { ...unsplash, access_key: access_key.trim() } })}
            placeholder="Not set"
            labelledBy={labelId}
            describedBy={hintId}
          />
        )}
      </Row>
      <Row label="NASA APOD API key" hint="DEMO_KEY works for light use; a free key from api.nasa.gov lifts the limit.">
        {({ labelId, hintId }) => (
          <SecretField
            name="NASA APOD API key"
            value={apod.api_key}
            onCommit={(api_key) => onChange({ ...config, apod: { ...apod, api_key: api_key.trim() || 'DEMO_KEY' } })}
            placeholder="DEMO_KEY"
            labelledBy={labelId}
            describedBy={hintId}
          />
        )}
      </Row>
      <Row label="Bing market" hint="Region code, e.g. en-US or ja-JP.">
        {({ labelId, hintId }) => (
          <TextField
            value={bing.market}
            onCommit={(market) => onChange({ ...config, bing: { ...bing, market: market.trim() } })}
            placeholder="en-US"
            aria-labelledby={labelId}
            aria-describedby={hintId}
            mono
            className="w-32"
          />
        )}
      </Row>
    </Group>
  )
}
