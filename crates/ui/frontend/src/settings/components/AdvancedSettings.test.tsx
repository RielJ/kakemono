import { act, cleanup, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { AppInfo, Config } from '../../shared/api'
import * as api from '../../shared/api'
import { diffConfig } from '../configPatch'
import { testConfig } from '../testConfig'
import AdvancedSettings from './AdvancedSettings'

// No daemon, no CLI: the embedded desktop section asks these on mount.
vi.mock('../../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../shared/api')>()
  return {
    ...real,
    desktopBackend: vi.fn(),
    detectDesktop: vi.fn(),
    testDesktop: vi.fn(),
    saveConfig: vi.fn(),
    getConfig: vi.fn(),
    selectDirectory: vi.fn(),
    openFolder: vi.fn(),
    revealInFinder: vi.fn(),
  }
})

const appInfo = (os: string): AppInfo => ({ os, config_path: '/home/u/.config/kakemono/config.toml', version: '1' })

/** The tab with App's state. `latest()` is the edited config, diffed against the saved one by `patch()`. */
function setup({ saved = testConfig(), os = 'macos' }: { saved?: Config; os?: string } = {}) {
  let current = saved
  function Harness() {
    const [savedConfig, setSaved] = useState(saved)
    const [config, setConfig] = useState(saved)
    current = config
    return (
      <AdvancedSettings
        config={config}
        savedConfig={savedConfig}
        onChange={setConfig}
        onWritten={(update) => {
          setSaved(update)
          setConfig(update)
        }}
        errors={{}}
        appInfo={appInfo(os)}
        fileManager={os === 'macos' ? 'Finder' : 'the file manager'}
      />
    )
  }
  return {
    render: async () => {
      render(<Harness />)
      await act(async () => {})
    },
    /** What auto-save would send: the minimal patch from the saved config. */
    patch: () => diffConfig(saved, current),
  }
}

const headings = () => screen.getAllByRole('heading').map((h) => h.textContent)
const group = (name: string) => {
  const section = screen.getByRole('heading', { name }).closest('section')
  if (!section) throw new Error(`no section for ${name}`)
  return within(section)
}
const commit = async (input: HTMLElement, text: string) => {
  await userEvent.clear(input)
  if (text) await userEvent.type(input, text)
  await userEvent.keyboard('{Enter}')
}

beforeEach(() => {
  vi.mocked(api.desktopBackend).mockResolvedValue({
    version: '2',
    pid: 1,
    backend: { name: 'macos', detail: '' },
    theme: 'light',
    apply: null,
  })
  vi.mocked(api.detectDesktop).mockResolvedValue('')
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('AdvancedSettings', () => {
  it('shows Folders, Wallpaper, Desktop, Hooks, Daemon, Appearance and Provider keys, in that order', async () => {
    await setup().render()
    expect(headings()).toEqual(['Folders', 'Wallpaper', 'Desktop', 'Hooks', 'Daemon', 'Appearance', 'Provider keys'])
  })

  it('lists the download, manual and trash folders, without the old image-count slider or favorites', async () => {
    await setup().render()
    const folders = group('Folders')
    const labels = folders.getAllByRole('textbox').map((t) => t.getAttribute('aria-labelledby'))
    const names = labels.map((id) => (id ? document.getElementById(id)?.textContent : null))
    expect(names).toEqual(['Downloads', 'Save manually downloaded wallpapers to', 'Trash'])
    expect(screen.queryByRole('slider')).toBeNull()
    expect(screen.queryByText(/Keep up to/)).toBeNull()
    expect(screen.queryByText('Favorites')).toBeNull()
  })

  it('labels the open-folder button next to Choose…', async () => {
    await setup().render()
    const open = group('Folders').getAllByRole('button', { name: 'Open in Finder' })[0]
    expect(open.getAttribute('title')).toBe('Open in Finder')
    expect(open.querySelector('svg')?.getAttribute('class')).toContain('w-4')
  })

  it('manual folder: empty shows the default under the download folder and patches null; a path patches the path', async () => {
    const s = setup()
    await s.render()
    const manual = screen.getByRole('textbox', { name: 'Save manually downloaded wallpapers to' })
    expect(manual.getAttribute('placeholder')).toBe('Default: /home/u/Pictures/Wallpapers/cache/manual')
    expect(manual).toHaveProperty('value', '')
    await commit(manual, '/home/u/Drops')
    expect(s.patch()).toEqual({ daemon: { manual_dir: '/home/u/Drops' } })

    const saved = testConfig()
    saved.daemon.manual_dir = '/home/u/Drops'
    cleanup()
    const t = setup({ saved })
    await t.render()
    await commit(screen.getByRole('textbox', { name: 'Save manually downloaded wallpapers to' }), '')
    expect(t.patch()).toEqual({ daemon: { manual_dir: null } })
  })

  it.each([
    ['Downloads', '/d', { daemon: { download_dir: '/d' } }],
    ['Trash', '/t', { daemon: { trash_dir: '/t' } }],
    ['Socket path', '/run/k.sock', { daemon: { socket_path: '/run/k.sock' } }],
    ['Bing market', 'ja-JP', { bing: { market: 'ja-JP' } }],
    ['Wallhaven API key', 'wk', { wallhaven: { api_key: 'wk' } }],
    ['Unsplash access key', 'uk', { unsplash: { access_key: 'uk' } }],
    ['NASA APOD API key', 'nk', { apod: { api_key: 'nk' } }],
  ])('%s patches its key', async (name, text, expected) => {
    const s = setup()
    await s.render()
    await commit(screen.getByLabelText(name), text)
    expect(s.patch()).toEqual(expected)
  })

  it.each([
    ['Display mode', 'center', { desktop: { fit: 'center' } }],
    ['Wallhaven sorting', 'toplist', { wallhaven: { sorting: 'toplist' } }],
    ['Wallhaven categories', '010', { wallhaven: { categories: '010' } }],
    ['Wallhaven purity', '110', { wallhaven: { purity: '110' } }],
  ])('%s patches its key', async (name, value, expected) => {
    const s = setup()
    await s.render()
    await userEvent.selectOptions(screen.getByRole('combobox', { name }), value)
    expect(s.patch()).toEqual(expected)
  })

  it('an empty APOD key falls back to DEMO_KEY', async () => {
    const saved = testConfig()
    saved.apod.api_key = 'mine'
    const s = setup({ saved })
    await s.render()
    await commit(screen.getByLabelText('NASA APOD API key'), '')
    expect(s.patch()).toEqual({ apod: { api_key: 'DEMO_KEY' } })
  })

  it('Appearance offers Light, Dark and System and patches ui.appearance', async () => {
    const s = setup()
    await s.render()
    const appearance = screen.getByRole('radiogroup', { name: 'Appearance' })
    expect(
      within(appearance)
        .getAllByRole('radio')
        .map((r) => r.textContent),
    ).toEqual(['Light', 'Dark', 'System'])
    await userEvent.click(within(appearance).getByRole('radio', { name: 'System' }))
    expect(s.patch()).toEqual({ ui: { appearance: 'system' } })
  })

  it('renders API keys as password fields with a show/hide toggle', async () => {
    await setup().render()
    for (const name of ['Wallhaven API key', 'Unsplash access key', 'NASA APOD API key']) {
      const input = screen.getByLabelText(name)
      expect(input.getAttribute('type')).toBe('password')
      const toggle = screen.getByRole('button', { name: `Show ${name}` })
      expect(toggle.getAttribute('aria-pressed')).toBe('false')
      expect(toggle.getAttribute('aria-controls')).toBe(input.id)
      await userEvent.click(toggle)
      expect(input.getAttribute('type')).toBe('text')
      expect(toggle.getAttribute('aria-pressed')).toBe('true')
      // The name stays put; aria-pressed carries the state.
      expect(toggle.getAttribute('aria-label')).toBe(`Show ${name}`)
      await userEvent.click(toggle)
      expect(input.getAttribute('type')).toBe('password')
    }
  })

  it('macOS: the Desktop group holds the backend and the Space switch, which patches daemon.reapply_on_space_change', async () => {
    const s = setup()
    await s.render()
    const desktop = group('Desktop')
    expect(desktop.getByText('macos')).toBeTruthy()
    await userEvent.click(desktop.getByRole('switch', { name: 'Keep wallpaper on every Space' }))
    expect(s.patch()).toEqual({ daemon: { reapply_on_space_change: true } })
  })

  it('Linux: the Desktop group holds the backend and the custom command; Detection follows it', async () => {
    const saved = testConfig()
    saved.desktop.backend = 'command'
    await setup({ saved, os: 'linux' }).render()
    const desktop = group('Desktop')
    expect(desktop.getByRole('combobox', { name: 'Backend' })).toBeTruthy()
    expect(desktop.getByText('Wallpaper command', { selector: 'p' })).toBeTruthy()
    expect(headings()).toEqual([
      'Folders',
      'Wallpaper',
      'Desktop',
      'Detection',
      'Hooks',
      'Daemon',
      'Appearance',
      'Provider keys',
    ])
  })
})
