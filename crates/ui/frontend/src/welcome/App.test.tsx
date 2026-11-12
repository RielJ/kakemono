import { act, cleanup, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { Config } from '../shared/api'
import * as api from '../shared/api'
import type { DaemonEventHandlers } from '../shared/useDaemonEvents'
import { testConfig } from '../settings/testConfig'
import App, { sourceNames } from './App'

vi.mock('../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../shared/api')>()
  return {
    ...real,
    getConfig: vi.fn(),
    saveConfig: vi.fn(),
    getAppInfo: vi.fn(),
    nextWallpaper: vi.fn(),
    destroyWindow: vi.fn(),
    trayAvailable: vi.fn(),
    getStartAtLogin: vi.fn(),
    setStartAtLogin: vi.fn(),
  }
})

// Captures the window's daemon handlers, so tests can play connection events.
const live = vi.hoisted(() => ({ handlers: {} as DaemonEventHandlers }))
vi.mock('../shared/useDaemonEvents', () => ({
  useDaemonEvents: (h: DaemonEventHandlers) => {
    live.handlers = h
  },
}))

function firstStart(edit?: (c: Config) => void): Config {
  const c = testConfig()
  // What the template gives a new install.
  c.ui.show_welcome = true
  c.daemon.rotation_enabled = false
  edit?.(c)
  return c
}

beforeEach(() => {
  vi.mocked(api.getConfig).mockResolvedValue(firstStart())
  vi.mocked(api.saveConfig).mockResolvedValue({ reloaded: false, reload_error: null })
  vi.mocked(api.getAppInfo).mockResolvedValue({ os: 'macos', config_path: '', version: '0' })
  vi.mocked(api.nextWallpaper).mockResolvedValue()
  vi.mocked(api.destroyWindow).mockResolvedValue()
  vi.mocked(api.trayAvailable).mockResolvedValue(true)
  vi.mocked(api.getStartAtLogin).mockResolvedValue({
    enabled: false,
    service: { installed: false, running: false, path: null },
    ui_autostart: false,
  })
})
afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  live.handlers = {}
  delete document.documentElement.dataset.theme
})

async function setup() {
  render(<App />)
  await act(async () => {})
  return userEvent.setup()
}

/** Plays the daemon's connection state, as `useDaemonEvents` would. */
function daemon(state: 'connected' | 'not_running') {
  act(() => live.handlers.onConnection?.({ state, status: null } as never))
}

const heading = () => screen.getByRole('heading', { level: 1 }).textContent
const button = (name: string) => screen.getByRole('button', { name })
const dots = () => screen.getByRole('list', { name: 'Steps' })

async function goTo(user: ReturnType<typeof userEvent.setup>, step: 'Where kakemono lives' | 'Set it up' | 'Tips') {
  await user.click(button('Get started'))
  if (step === 'Where kakemono lives') return
  await user.click(button('Next'))
  if (step === 'Set it up') return
  await user.click(button('Next'))
}

describe('welcome window navigation', () => {
  it('starts on the welcome screen with Get started and Skip, and no Back', async () => {
    await setup()
    expect(heading()).toBe('kakemono changes your desktop wallpaper')
    expect(screen.getByText(/Wallhaven, Reddit, NASA, any web page or your own folders/)).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Back' })).toBeNull()
    expect(button('Skip')).toBeTruthy()
    expect(within(dots()).getAllByRole('listitem')).toHaveLength(4)
    expect(within(dots()).getAllByRole('listitem')[0]?.getAttribute('aria-current')).toBe('step')
  })

  it('says the wallpaper changes only when asked, or the interval once rotation is on', async () => {
    await setup()
    expect(screen.getByText('The wallpaper changes only when you ask, unless you turn on rotation.')).toBeTruthy()
    cleanup()
    vi.mocked(api.getConfig).mockResolvedValue(firstStart((c) => (c.daemon.rotation_enabled = true)))
    await setup()
    expect(screen.getByText('Your wallpaper changes every 30 minutes.')).toBeTruthy()
  })

  it('walks forward with Get started and Next, and back with Back', async () => {
    const user = await setup()
    await user.click(button('Get started'))
    expect(heading()).toBe('Where kakemono lives')
    expect(within(dots()).getAllByRole('listitem')[1]?.getAttribute('aria-current')).toBe('step')
    await user.click(button('Next'))
    expect(heading()).toBe('Set it up')
    await user.click(button('Next'))
    expect(heading()).toBe('Tips')
    expect(screen.queryByRole('button', { name: 'Next' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Skip' })).toBeNull()
    await user.click(button('Back'))
    expect(heading()).toBe('Set it up')
    await user.click(button('Back'))
    await user.click(button('Back'))
    expect(heading()).toBe('kakemono changes your desktop wallpaper')
  })

  it('closes the window from Skip', async () => {
    const user = await setup()
    await user.click(button('Get started'))
    await user.click(button('Skip'))
    expect(api.destroyWindow).toHaveBeenCalledTimes(1)
  })

  it('closes the window from Open Settings on the last screen', async () => {
    const user = await setup()
    await goTo(user, 'Tips')
    await user.click(button('Open Settings'))
    expect(api.destroyWindow).toHaveBeenCalledTimes(1)
  })
})

describe('welcome window keyboard', () => {
  const active = () => document.activeElement?.textContent

  it('focuses Get started on first show, so Enter advances', async () => {
    const user = await setup()
    expect(active()).toBe('Get started')
    await user.keyboard('{Enter}')
    expect(heading()).toBe('Where kakemono lives')
  })

  it('focuses the primary button after each Next and Back', async () => {
    const user = await setup()
    await user.click(button('Get started'))
    expect(active()).toBe('Next')
    await user.click(button('Next'))
    expect(heading()).toBe('Set it up')
    expect(active()).toBe('Next')
    await user.click(button('Next'))
    expect(active()).toBe('Open Settings')
    await user.click(button('Back'))
    expect(active()).toBe('Next')
    await user.click(button('Back'))
    await user.click(button('Back'))
    expect(heading()).toBe('kakemono changes your desktop wallpaper')
    expect(active()).toBe('Get started')
  })

  it('walks every screen with Enter and Space alone', async () => {
    const user = await setup()
    await user.keyboard('{Enter}')
    await user.keyboard(' ')
    await user.keyboard('{Enter}')
    expect(heading()).toBe('Tips')
    await user.keyboard('{Enter}')
    expect(api.destroyWindow).toHaveBeenCalledTimes(1)
  })

  it('refocuses the primary button when the window gains focus with nothing focused', async () => {
    await setup()
    ;(document.activeElement as HTMLElement).blur()
    act(() => {
      window.dispatchEvent(new FocusEvent('focus'))
    })
    expect(active()).toBe('Get started')
  })

  it('closes the window on Escape, like Skip, on any screen', async () => {
    const user = await setup()
    await user.keyboard('{Escape}')
    expect(api.destroyWindow).toHaveBeenCalledTimes(1)
    await goTo(user, 'Tips')
    await user.keyboard('{Escape}')
    expect(api.destroyWindow).toHaveBeenCalledTimes(2)
  })

  it('announces the new screen title', async () => {
    await setup()
    expect(screen.getByRole('heading', { level: 1 }).getAttribute('aria-live')).toBe('polite')
  })
})

describe('welcome window icon', () => {
  it('shows the app icon above the first screen only', async () => {
    const user = await setup()
    const icon = screen.getByRole('presentation', { hidden: true })
    expect(icon.tagName).toBe('IMG')
    expect(icon.getAttribute('src')).toMatch(/128x128\.png$/)
    expect(icon.getAttribute('width')).toBe('64')
    await user.click(button('Get started'))
    expect(screen.queryByRole('presentation', { hidden: true })).toBeNull()
  })
})

describe('welcome window seen marker', () => {
  it('writes show_welcome = false exactly once on the first render', async () => {
    const user = await setup()
    await goTo(user, 'Tips')
    await user.click(button('Back'))
    expect(vi.mocked(api.saveConfig).mock.calls).toEqual([[{ ui: { show_welcome: false } }]])
  })

  it('writes nothing when replayed from Settings', async () => {
    vi.mocked(api.getConfig).mockResolvedValue(testConfig())
    await setup()
    expect(api.saveConfig).not.toHaveBeenCalled()
  })

  it('follows [ui] appearance for the theme', async () => {
    vi.mocked(api.getConfig).mockResolvedValue(firstStart((c) => (c.ui.appearance = 'dark')))
    await setup()
    expect(document.documentElement.dataset.theme).toBe('dark')
  })
})

describe('welcome window: where kakemono lives', () => {
  it.each([
    ['macos', /menu bar, with no Dock icon/],
    ['windows', /notification area/],
    ['linux', /AppIndicator extension/],
  ])('names the place on %s', async (os, text) => {
    vi.mocked(api.getAppInfo).mockResolvedValue({ os, config_path: '', version: '0' })
    const user = await setup()
    await goTo(user, 'Where kakemono lives')
    expect(screen.getByText(text)).toBeTruthy()
    expect(screen.getByText(/Next, Pause, Favorite, Show Picker and Settings/)).toBeTruthy()
  })

  it('says Settings is the way in on Linux without a tray', async () => {
    vi.mocked(api.getAppInfo).mockResolvedValue({ os: 'linux', config_path: '', version: '0' })
    vi.mocked(api.trayAvailable).mockResolvedValue(false)
    const user = await setup()
    await goTo(user, 'Where kakemono lives')
    expect(screen.getByText(/no tray/)).toBeTruthy()
    expect(screen.queryByText(/Next, Pause, Favorite/)).toBeNull()
  })
})

describe('welcome window: set it up', () => {
  it('Change wallpaper now calls next, showing pending then done', async () => {
    let finish = () => {}
    vi.mocked(api.nextWallpaper).mockReturnValue(new Promise<void>((resolve) => (finish = resolve)))
    const user = await setup()
    daemon('connected')
    await goTo(user, 'Set it up')
    await user.click(button('Change wallpaper now'))
    expect(api.nextWallpaper).toHaveBeenCalledTimes(1)
    expect((button('Changing…') as HTMLButtonElement).disabled).toBe(true)
    await act(async () => finish())
    expect((button('Change wallpaper now') as HTMLButtonElement).disabled).toBe(false)
  })

  it('shows the error when next fails', async () => {
    vi.mocked(api.nextWallpaper).mockRejectedValue({ kind: 'daemon', message: 'no images yet' })
    const user = await setup()
    daemon('connected')
    await goTo(user, 'Set it up')
    await user.click(button('Change wallpaper now'))
    expect(await screen.findByText('no images yet')).toBeTruthy()
  })

  it('disables Change wallpaper now while the daemon is down', async () => {
    const user = await setup()
    daemon('not_running')
    await goTo(user, 'Set it up')
    expect((button('Change wallpaper now') as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText('The daemon is not running.')).toBeTruthy()
  })

  it('the rotation checkbox saves rotation_enabled flipped', async () => {
    const user = await setup()
    await goTo(user, 'Set it up')
    const box = screen.getByRole('checkbox', { name: /Change wallpaper every 30 minutes/ })
    expect((box as HTMLInputElement).checked).toBe(false)
    await user.click(box)
    expect(vi.mocked(api.saveConfig).mock.calls.at(-1)).toEqual([{ daemon: { rotation_enabled: true } }])
    expect((box as HTMLInputElement).checked).toBe(true)
    await user.click(box)
    expect(vi.mocked(api.saveConfig).mock.calls.at(-1)).toEqual([{ daemon: { rotation_enabled: false } }])
  })

  it('puts the box back and says so when the rotation save fails', async () => {
    const user = await setup()
    await goTo(user, 'Set it up')
    vi.mocked(api.saveConfig).mockRejectedValue({ kind: 'io', message: 'disk full' })
    const box = screen.getByRole('checkbox', { name: /Change wallpaper every/ })
    await user.click(box)
    expect(await screen.findByText(/disk full/)).toBeTruthy()
    expect((box as HTMLInputElement).checked).toBe(false)
  })

  it('has Start at login and lists the sources by name', async () => {
    const user = await setup()
    await goTo(user, 'Set it up')
    expect(await screen.findByRole('checkbox', { name: /Start at login/ })).toBeTruthy()
    expect(screen.getByText(/Your sources:/).textContent).toContain('Favorites')
  })
})

describe('sourceNames', () => {
  const config = (sources: Config['sources']) => {
    const c = testConfig()
    c.favorites.dir = '/home/u/Pictures/Wallpapers/favorites'
    c.sources = sources
    return c
  }

  it('names the favorites folder Favorites, another folder by its folder name, and keeps given names', () => {
    expect(
      sourceNames(
        config([
          {
            id: 'wh-hot',
            name: 'Wallhaven hot',
            enabled: true,
            type: 'wallhaven',
            location: 'https://wallhaven.cc/hot',
          },
          {
            id: 'favorites',
            name: '',
            enabled: true,
            type: 'folder',
            location: '/home/u/Pictures/Wallpapers/favorites/',
          },
          { id: 'pics', name: '', enabled: true, type: 'folder', location: '/home/u/Pictures/Holidays' },
          { id: 'off', name: 'Off', enabled: false, type: 'folder', location: '/x' },
        ]),
      ),
    ).toEqual(['Wallhaven hot', 'Favorites', 'Holidays'])
  })

  it('names other unnamed sources by type and a short location', () => {
    expect(
      sourceNames(config([{ id: 'r', name: '', enabled: true, type: 'reddit', location: 'r/EarthPorn' }])),
    ).toEqual(['Reddit (r/EarthPorn)'])
    expect(sourceNames(config([{ id: 'b', name: '', enabled: true, type: 'bing', location: '' }]))).toEqual(['Bing'])
  })
})

describe('welcome window: tips', () => {
  it('lists the picker keys, pasting links, dropping images and the shortcut', async () => {
    const user = await setup()
    await goTo(user, 'Tips')
    const text = document.body.textContent ?? ''
    expect(text).toMatch(/arrow keys/i)
    expect(text).toContain('Enter')
    expect(text).toContain('Delete')
    expect(text).toMatch(/search link/)
    expect(text).toMatch(/Drag an image onto Settings/)
    expect(text).toContain('kakemono next')
    expect(text).toMatch(/Settings → General → Shortcuts/)
  })
})
