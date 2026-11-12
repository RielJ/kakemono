import type { Config } from '../../shared/api'
import { intervalError } from '../duration'
import { Group } from './Group'
import { CheckLine, IntervalField, LineText, NumberField, wholeAtLeast } from './SettingLine'

/** The smallest download folder limit the UI accepts, in MB. */
export const MIN_DOWNLOAD_LIMIT_MB = 10

/**
 * General → Downloading: whether and how often the background downloader fetches an image for
 * the enabled online sources, and how large its folder may grow. With background downloads
 * off, Next downloads one new image each time instead.
 */
export default function DownloadingGroup({ config, onChange }: { config: Config; onChange: (config: Config) => void }) {
  const d = config.daemon
  const setDaemon = <K extends keyof Config['daemon']>(key: K, value: Config['daemon'][K]) =>
    onChange({ ...config, daemon: { ...d, [key]: value } })

  return (
    <Group title="Downloading" footer={d.internet ? undefined : 'Internet access is off, so nothing is downloaded.'}>
      <CheckLine
        checked={d.download_enabled}
        onChange={(v) => setDaemon('download_enabled', v)}
        error={intervalError(d.download_interval)}
        hint="When this is off, Next downloads a new wallpaper each time."
      >
        <LineText>Download a wallpaper in the background every</LineText>
        <IntervalField
          value={d.download_interval}
          onChange={(v) => setDaemon('download_interval', v)}
          label="Download a wallpaper in the background every"
        />
      </CheckLine>
      <CheckLine
        checked={d.download_limit_mb_enabled}
        onChange={(v) => setDaemon('download_limit_mb_enabled', v)}
        hint="Favorites, the current wallpaper and your own folders are never deleted."
      >
        <LineText>Limit download folder to</LineText>
        <NumberField
          value={d.download_limit_mb}
          onCommit={(v) => setDaemon('download_limit_mb', v)}
          parse={wholeAtLeast(MIN_DOWNLOAD_LIMIT_MB)}
          label="Download folder limit in MB"
          min={MIN_DOWNLOAD_LIMIT_MB}
          className="w-20"
        />
        <LineText>MB, deleting the oldest files when necessary</LineText>
      </CheckLine>
    </Group>
  )
}
