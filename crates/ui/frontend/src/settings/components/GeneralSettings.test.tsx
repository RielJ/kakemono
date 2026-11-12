import { act, cleanup, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { Config, StatusData, Thumbnail } from '../../shared/api'
import * as api from '../../shared/api'
import { testConfig } from '../testConfig'
import { configErrors } from '../validate'
import GeneralSettings from './GeneralSettings'
import NowShowing from './NowShowing'
import { joinInterval, splitInterval } from './SettingLine'

vi.mock('../../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../shared/api')>()
  return {
    ...real,
    getStartAtLogin: vi.fn(),
    setStartAtLogin: vi.fn(),
    selectDirectory: vi.fn(),
    openFolder: vi.fn(),
    showPicker: vi.fn(),
    currentThumbnail: vi.fn(),
    openWelcome: vi.fn(),
  }
})

// The sources table is Task 19's; a marker shows where General mounts it and with what.
const sourcesTable = vi.hoisted(() => ({ props: null as unknown }))
vi.mock('./SourcesTable', () => ({
  default: (props: unknown) => {
    sourcesTable.props = props
    return <div data-testid="sources-table" />
  },
}))

beforeEach(() => {
  vi.mocked(api.getStartAtLogin).mockResolvedValue({
    enabled: false,
    service: { installed: false, running: false, path: null },
    ui_autostart: false,
  })
  vi.mocked(api.currentThumbnail).mockResolvedValue(null)
  vi.mocked(api.showPicker).mockResolvedValue()
  vi.mocked(api.openWelcome).mockResolvedValue()
})
afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

/** Renders General over a live config; `last()` is the newest config it asked to save. */
async function setup({ edit, os = 'macos' }: { edit?: (c: Config) => void; os?: string } = {}) {
  const initial = testConfig()
  edit?.(initial)
  const changes: Config[] = []
  function Harness() {
    const [config, setConfig] = useState(initial)
    return (
      <GeneralSettings
        config={config}
        onChange={(c) => {
          changes.push(c)
          setConfig(c)
        }}
        errors={configErrors(config)}
        status={null}
        daemon="stopped"
        onStatusChange={() => {}}
        fileManager="Finder"
        os={os}
        onResetPosition={() => {}}
      />
    )
  }
  render(<Harness />)
  await act(async () => {})
  const last = (): Config => {
    const c = changes[changes.length - 1]
    if (!c) throw new Error('General saved nothing')
    return c
  }
  return { last, changes, user: userEvent.setup() }
}

const box = (name: string | RegExp) => screen.getByRole('checkbox', { name })
const heading = (name: string) => screen.getByRole('heading', { name })
const precedes = (a: Element, b: Element) => (a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0

describe('GeneralSettings', () => {
  it('lists its settings in Variety order', async () => {
    await setup()
    const order = [
      box('Start at login'),
      box('Change wallpaper every'),
      box('Change wallpaper on start'),
      box('Use Internet access'),
      screen.getByTestId('sources-table'),
      heading('When possible, use images that…'),
      heading('Downloading'),
      box('Download a wallpaper in the background every'),
      box(/^Limit download folder to MB, deleting the oldest files when necessary$/),
      screen.getByRole('textbox', { name: 'Favorites folder' }),
      heading('Picker'),
      heading('Shortcuts'),
    ]
    for (let i = 1; i < order.length; i++) expect(precedes(order[i - 1], order[i])).toBe(true)
  })

  it('passes the config and the change function to the sources table', async () => {
    await setup()
    expect(sourcesTable.props).toMatchObject({ config: testConfig() })
    expect(typeof (sourcesTable.props as { onChange: unknown }).onChange).toBe('function')
  })

  it('hands the sources table the save-now callback', async () => {
    const onCommit = vi.fn().mockResolvedValue(undefined)
    render(
      <GeneralSettings
        config={testConfig()}
        onChange={() => {}}
        onCommit={onCommit}
        errors={{}}
        status={null}
        daemon="stopped"
        onStatusChange={() => {}}
        fileManager="Finder"
      />,
    )
    await act(async () => {})
    expect((sourcesTable.props as { onCommit: unknown }).onCommit).toBe(onCommit)
  })

  it('patches the daemon booleans, and the sentence text ticks its box', async () => {
    const { last, user } = await setup()
    await user.click(screen.getByText('Change wallpaper on start'))
    expect(last().daemon.change_on_start).toBe(true)
    await user.click(box('Use Internet access'))
    expect(last().daemon.internet).toBe(false)
    await user.click(box('Download a wallpaper in the background every'))
    expect(last().daemon.download_enabled).toBe(false)
    await user.click(box(/^Limit download folder to/))
    expect(last().daemon.download_limit_mb_enabled).toBe(false)
    await user.click(box('Change wallpaper every'))
    expect(last().daemon.rotation_enabled).toBe(false)
  })

  it('writes the download interval as a Go duration', async () => {
    const { last, user } = await setup()
    const count = screen.getByRole('spinbutton', { name: 'Download a wallpaper in the background every' })
    expect((count as HTMLInputElement).value).toBe('10')
    await user.clear(count)
    await user.type(count, '2{Enter}')
    expect(last().daemon.download_interval).toBe('2m')
    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Download a wallpaper in the background every, unit' }),
      'hours',
    )
    expect(last().daemon.download_interval).toBe('2h')
    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Download a wallpaper in the background every, unit' }),
      'days',
    )
    expect(last().daemon.download_interval).toBe('48h')
  })

  it('shows the rotation interval in its largest whole unit', async () => {
    await setup({ edit: (c) => void (c.daemon.interval = '1h30m') })
    expect((screen.getByRole('spinbutton', { name: 'Change wallpaper every' }) as HTMLInputElement).value).toBe('90')
    expect((screen.getByRole('combobox', { name: 'Change wallpaper every, unit' }) as HTMLSelectElement).value).toBe(
      'minutes',
    )
  })

  it('keeps the download size limit a whole number of at least 10 MB', async () => {
    const { last, user } = await setup()
    const mb = screen.getByRole('spinbutton', { name: 'Download folder limit in MB' })
    await user.clear(mb)
    await user.type(mb, '250.6{Enter}')
    expect(last().daemon.download_limit_mb).toBe(251)
    await user.clear(mb)
    await user.type(mb, '3{Enter}')
    expect(last().daemon.download_limit_mb).toBe(10)
    expect((mb as HTMLInputElement).value).toBe('10')
  })

  it('ignores an empty count and puts the saved value back', async () => {
    const { changes, user } = await setup()
    const mb = screen.getByRole('spinbutton', { name: 'Download folder limit in MB' })
    await user.clear(mb)
    await user.tab()
    expect(changes).toHaveLength(0)
    expect((mb as HTMLInputElement).value).toBe('500')
  })

  it('leaves the inline controls editable while the box is unticked', async () => {
    const { last, user } = await setup({ edit: (c) => void (c.daemon.download_enabled = false) })
    const count = screen.getByRole('spinbutton', { name: 'Download a wallpaper in the background every' })
    expect(count).toHaveProperty('disabled', false)
    await user.clear(count)
    await user.type(count, '5{Enter}')
    expect(last().daemon.download_interval).toBe('5m')
    expect(last().daemon.download_enabled).toBe(false)
  })

  it('says downloads stop while Internet access is off', async () => {
    await setup({ edit: (c) => void (c.daemon.internet = false) })
    expect(screen.getByText('Internet access is off, so nothing is downloaded.')).toBeTruthy()
  })

  it('patches the favorites folder from the text field and from Choose…', async () => {
    const { last, user } = await setup()
    const dir = screen.getByRole('textbox', { name: 'Favorites folder' })
    await user.clear(dir)
    await user.type(dir, '/tmp/favs{Enter}')
    expect(last().favorites.dir).toBe('/tmp/favs')
    vi.mocked(api.selectDirectory).mockResolvedValue('/picked')
    await user.click(screen.getByRole('button', { name: 'Choose…' }))
    expect(last().favorites.dir).toBe('/picked')
  })

  it('opens the favorites folder in the file manager', async () => {
    const { user } = await setup({ edit: (c) => void (c.favorites.dir = '/home/u/favs') })
    vi.mocked(api.openFolder).mockResolvedValue()
    const row = screen.getByRole('textbox', { name: 'Favorites folder' }).parentElement!
    await user.click(within(row).getByRole('button', { name: /^Open/ }))
    expect(api.openFolder).toHaveBeenCalledWith('/home/u/favs')
  })

  it('keeps the picker settings as checkbox lines; XWayland only on Linux', async () => {
    const { last, user } = await setup()
    await user.click(box('Close when it loses focus'))
    expect(last().picker.close_on_blur).toBe(false)
    expect(screen.queryByRole('checkbox', { name: 'Open through XWayland' })).toBeNull()
    cleanup()
    const linux = await setup({ os: 'linux' })
    await linux.user.click(box('Open through XWayland'))
    expect(linux.last().picker.force_x11).toBe(true)
  })
})

describe('NowShowing', () => {
  const status = (path: string): StatusData => ({
    current: {
      path,
      source: 'wallhaven',
      source_url: '',
      image_url: '',
      title: 'Forest',
      author: '',
      width: 0,
      height: 0,
      set_at: '',
    },
    paused: false,
    interval: '30m',
    theme: 'dark',
    history_count: 1,
    history_index: 0,
  })
  const thumb = (path: string): Thumbnail => ({
    path,
    source: 'wallhaven',
    title: 'Forest',
    isCurrent: true,
    imageUrl: `thumb://localhost/${path.length}`,
  })

  async function show(s: StatusData | null, daemon: 'running' | 'stopped' = 'running', rotating = true) {
    const { container } = render(
      <NowShowing status={s} daemon={daemon} rotating={rotating} onStatusChange={() => {}} fileManager="Finder" />,
    )
    await act(async () => {})
    return () => container.querySelector('img')?.getAttribute('src') ?? null
  }

  it('shows the thumb:// image currentThumbnail returns for the current wallpaper', async () => {
    vi.mocked(api.currentThumbnail).mockResolvedValue(thumb('/w/a.jpg'))
    const src = await show(status('/w/a.jpg'))
    expect(api.currentThumbnail).toHaveBeenCalledTimes(1)
    expect(src()).toBe(thumb('/w/a.jpg').imageUrl)
  })

  it('falls back to the entry flagged current when the daemon reports another path', async () => {
    vi.mocked(api.currentThumbnail).mockResolvedValue(thumb('/w/b.jpg'))
    const src = await show(status('/w/a.jpg'))
    expect(src()).toBe(thumb('/w/b.jpg').imageUrl)
  })

  it('shows the placeholder when there is no current thumbnail', async () => {
    const src = await show(status('/w/a.jpg'))
    expect(src()).toBeNull()
  })

  it('says how often the wallpaper changes while rotation is on', async () => {
    vi.mocked(api.currentThumbnail).mockResolvedValue(null)
    await show(status('/w/a.jpg'))
    expect(screen.getByText(/Changes every 30m/)).toBeTruthy()
  })

  it('drops the interval but keeps the theme while rotation is off', async () => {
    vi.mocked(api.currentThumbnail).mockResolvedValue(null)
    await show(status('/w/a.jpg'), 'running', false)
    expect(screen.queryByText(/Changes every/)).toBeNull()
    expect(screen.queryByText(/Paused/)).toBeNull()
    expect(screen.getByText(/dark mode/)).toBeTruthy()
  })

  it('offers pause only while rotation is on', async () => {
    vi.mocked(api.currentThumbnail).mockResolvedValue(null)
    await show(status('/w/a.jpg'))
    expect(screen.getByRole('button', { name: 'Pause rotation' })).toBeTruthy()
    cleanup()
    await show(status('/w/a.jpg'), 'running', false)
    expect(screen.queryByRole('button', { name: /(Pause|Resume) rotation/ })).toBeNull()
    expect(screen.getByRole('button', { name: 'Next wallpaper' })).toBeTruthy()
  })

  it('asks for no thumbnail while the daemon is stopped', async () => {
    const src = await show(status('/w/a.jpg'), 'stopped')
    expect(api.currentThumbnail).not.toHaveBeenCalled()
    expect(src()).toBeNull()
  })
})

describe('interval units', () => {
  it('round-trips minutes, hours and days through Go durations', () => {
    expect(splitInterval('30m')).toEqual({ count: 30, unit: 'minutes' })
    expect(splitInterval('2h')).toEqual({ count: 2, unit: 'hours' })
    expect(splitInterval('48h')).toEqual({ count: 2, unit: 'days' })
    expect(splitInterval('90s')).toEqual({ count: 1.5, unit: 'minutes' })
    expect(splitInterval('nonsense')).toEqual({ count: null, unit: 'minutes' })
    expect(joinInterval(3, 'days')).toBe('72h')
  })
})

describe('welcome tips', () => {
  it('reopens the Welcome window from Show welcome tips, after Shortcuts, writing nothing', async () => {
    const { changes, user } = await setup({ edit: (c) => (c.ui.show_welcome = true) })
    const replay = screen.getByRole('button', { name: 'Show welcome tips' })
    expect(precedes(heading('Shortcuts'), replay)).toBe(true)
    await user.click(replay)
    expect(api.openWelcome).toHaveBeenCalledTimes(1)
    expect(changes).toHaveLength(0)
  })

  it('has no welcome panel, even when show_welcome is set', async () => {
    const { changes } = await setup({ edit: (c) => (c.ui.show_welcome = true) })
    expect(screen.queryByRole('region', { name: 'Welcome to kakemono' })).toBeNull()
    expect(changes).toHaveLength(0)
  })
})
