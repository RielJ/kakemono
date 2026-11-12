import type { Config, StatusData } from '../../shared/api'
import { openWelcome } from '../../shared/api'
import type { DaemonState } from '../sections'
import type { ConfigErrors } from '../validate'
import DownloadingGroup from './DownloadingGroup'
import FiltersGroup from './FiltersGroup'
import { Group } from './Group'
import NowShowing from './NowShowing'
import PathField from './PathField'
import PickerSettings from './PickerSettings'
import { CheckLine, IntervalField, LineText, PlainLine } from './SettingLine'
import ShortcutHint from './ShortcutHint'
import SourcesTable from './SourcesTable'
import StartAtLogin from './StartAtLogin'

interface Props {
  config: Config
  onChange: (config: Config) => void
  /** Change and write now; resolves once saved. The sources table waits on it before Use and Remove act. */
  onCommit?: (config: Config) => Promise<void>
  errors: ConfigErrors
  status: StatusData | null
  daemon: DaemonState
  onStatusChange: () => void
  fileManager: string
  /** `app_info().os`; picks the Shortcuts how-tos and Linux-only picker options. */
  os?: string
  /** Picker → Reset position. Optional only until App passes it; without it the button does nothing. */
  onResetPosition?: () => void
}

/**
 * The General tab, in Variety's order: a list of sentences with inline controls. Startup and
 * rotation, the sources table, filters, downloading, favorites, the picker, then shortcuts.
 */
export default function GeneralSettings({
  config,
  onChange,
  onCommit,
  errors,
  status,
  daemon,
  onStatusChange,
  fileManager,
  os,
  onResetPosition,
}: Props) {
  const { daemon: d } = config
  const setDaemon = <K extends keyof Config['daemon']>(key: K, value: Config['daemon'][K]) =>
    onChange({ ...config, daemon: { ...d, [key]: value } })

  return (
    <>
      <NowShowing
        status={status}
        daemon={daemon}
        rotating={d.rotation_enabled}
        onStatusChange={onStatusChange}
        fileManager={fileManager}
      />

      <Group>
        <StartAtLogin variant="line" />
        <CheckLine
          checked={d.rotation_enabled}
          onChange={(v) => setDaemon('rotation_enabled', v)}
          error={errors.interval}
        >
          <LineText>Change wallpaper every</LineText>
          <IntervalField value={d.interval} onChange={(v) => setDaemon('interval', v)} label="Change wallpaper every" />
        </CheckLine>
        <CheckLine checked={d.change_on_start} onChange={(v) => setDaemon('change_on_start', v)}>
          <LineText>Change wallpaper on start</LineText>
        </CheckLine>
        <CheckLine checked={d.rotate_on_theme_change} onChange={(v) => setDaemon('rotate_on_theme_change', v)}>
          <LineText>Change wallpaper when the system switches between light and dark</LineText>
        </CheckLine>
        <CheckLine
          checked={d.internet}
          onChange={(v) => setDaemon('internet', v)}
          hint={d.internet ? undefined : 'Only local folders are used; online sources are paused.'}
        >
          <LineText>Use Internet access</LineText>
        </CheckLine>
      </Group>

      <SourcesTable config={config} onChange={onChange} onCommit={onCommit} />

      <FiltersGroup config={config} onChange={onChange} />

      <DownloadingGroup config={config} onChange={onChange} />

      <Group>
        <PlainLine label="Favorites folder" hint="Favorite wallpapers are copied here. They are never deleted.">
          {(labelId) => (
            <PathField
              value={config.favorites.dir}
              onCommit={(dir) => onChange({ ...config, favorites: { ...config.favorites, dir: dir.trim() } })}
              labelledBy={labelId}
              fileManager={fileManager}
            />
          )}
        </PlainLine>
      </Group>

      <PickerSettings config={config} onChange={onChange} os={os} onResetPosition={onResetPosition ?? (() => {})} />

      <Group title="Shortcuts">
        <ShortcutHint os={os} />
      </Group>

      <div className="mt-4 px-1">
        <button
          type="button"
          className="btn-plain h-6 px-1.5 text-[12px]"
          // A failed open has nowhere to show; the button can simply be clicked again.
          onClick={() => void openWelcome().catch(() => {})}
        >
          Show welcome tips
        </button>
      </div>
    </>
  )
}
