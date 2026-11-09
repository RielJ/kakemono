import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Thumbnail } from '../shared/api'
import * as api from '../shared/api'
import App from './App'

vi.mock('../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../shared/api')>()
  return {
    ...real,
    getConfig: vi.fn(),
    getAppInfo: vi.fn(),
    listThumbnails: vi.fn(),
    setFile: vi.fn(),
    favoriteWallpaper: vi.fn(),
    trashWallpaper: vi.fn(),
    getStatus: vi.fn(),
    hidePicker: vi.fn(),
    resetPickerPosition: vi.fn(),
  }
})

// Captures the handlers the strip registers, so tests can play daemon events.
const live = vi.hoisted(() => ({ handlers: {} as import('../shared/useDaemonEvents').DaemonEventHandlers }))
vi.mock('../shared/useDaemonEvents', () => ({
  useDaemonEvents: (h: import('../shared/useDaemonEvents').DaemonEventHandlers) => {
    live.handlers = h
  },
}))

const THUMBS: Thumbnail[] = [
  { path: '/w/a.jpg', source: 'unsplash', title: 'Alps', isCurrent: false, imageUrl: 'thumb://localhost/a' },
  { path: '/w/b.jpg', source: 'local', title: 'Beach', isCurrent: true, imageUrl: 'thumb://localhost/b' },
  { path: '/w/c.jpg', source: 'wallhaven', title: '', isCurrent: false, imageUrl: '' },
]

function mockApi(position = 'bottom') {
  vi.mocked(api.getConfig).mockResolvedValue({ picker: { position } } as unknown as Awaited<
    ReturnType<typeof api.getConfig>
  >)
  vi.mocked(api.getAppInfo).mockResolvedValue({ os: 'macos', config_path: '', legacy_config: false, version: '0' })
  vi.mocked(api.listThumbnails).mockResolvedValue(THUMBS.map((t) => ({ ...t })))
  vi.mocked(api.setFile).mockResolvedValue(undefined)
  vi.mocked(api.favoriteWallpaper).mockResolvedValue(undefined)
  vi.mocked(api.trashWallpaper).mockResolvedValue(undefined)
  vi.mocked(api.hidePicker).mockResolvedValue(undefined)
  vi.mocked(api.resetPickerPosition).mockResolvedValue(undefined)
}

async function renderPicker() {
  render(<App />)
  const list = await screen.findByRole('listbox', { name: 'Wallpapers' })
  await screen.findAllByRole('option')
  return list
}

function activeLabel(list: HTMLElement): string | null {
  const id = list.getAttribute('aria-activedescendant')
  return id ? (document.getElementById(id)?.getAttribute('aria-label') ?? null) : null
}

beforeEach(() => mockApi())
afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  vi.useRealTimers()
})

describe('picker listbox', () => {
  it('names options "title — source", marks the current one selected and focuses it', async () => {
    const list = await renderPicker()
    const options = screen.getAllByRole('option')
    expect(options.map((o) => o.getAttribute('aria-label'))).toEqual([
      'Alps — unsplash',
      'Beach — local',
      'c.jpg — wallhaven',
    ])
    expect(options.map((o) => o.getAttribute('aria-selected'))).toEqual(['false', 'true', 'false'])
    expect(document.activeElement).toBe(list)
    expect(activeLabel(list)).toBe('Beach — local')
  })

  it('moves with ←/→ and jumps with Home/End', async () => {
    const user = userEvent.setup()
    const list = await renderPicker()
    await user.keyboard('{ArrowRight}')
    expect(activeLabel(list)).toBe('c.jpg — wallhaven')
    await user.keyboard('{ArrowRight}')
    expect(activeLabel(list)).toBe('c.jpg — wallhaven')
    await user.keyboard('{Home}')
    expect(activeLabel(list)).toBe('Alps — unsplash')
    await user.keyboard('{ArrowLeft}')
    expect(activeLabel(list)).toBe('Alps — unsplash')
    await user.keyboard('{End}')
    expect(activeLabel(list)).toBe('c.jpg — wallhaven')
  })

  it('moves with ↑/↓ when docked to a side', async () => {
    mockApi('left')
    const user = userEvent.setup()
    const list = await renderPicker()
    await screen.findByRole('listbox', { name: 'Wallpapers' })
    await vi.waitFor(() => expect(list.getAttribute('aria-orientation')).toBe('vertical'))
    await user.keyboard('{ArrowUp}')
    expect(activeLabel(list)).toBe('Alps — unsplash')
  })

  it('Enter sets the active wallpaper and moves the selection', async () => {
    const user = userEvent.setup()
    await renderPicker()
    await user.keyboard('{Home}{Enter}')
    expect(api.setFile).toHaveBeenCalledWith('/w/a.jpg')
    expect(screen.getByRole('option', { name: 'Alps — unsplash' }).getAttribute('aria-selected')).toBe('true')
    expect(screen.getByRole('option', { name: 'Beach — local' }).getAttribute('aria-selected')).toBe('false')
  })

  it('a click sets that wallpaper', async () => {
    const user = userEvent.setup()
    await renderPicker()
    await user.click(screen.getByRole('option', { name: 'c.jpg — wallhaven' }))
    expect(api.setFile).toHaveBeenCalledWith('/w/c.jpg')
  })

  it('F favourites the active wallpaper', async () => {
    const user = userEvent.setup()
    await renderPicker()
    await user.keyboard('f')
    expect(api.favoriteWallpaper).toHaveBeenCalledWith('/w/b.jpg')
    expect(await screen.findByText('Added to Favorites')).toBeTruthy()
  })

  it('Esc hides the picker', async () => {
    const user = userEvent.setup()
    await renderPicker()
    await user.keyboard('{Escape}')
    expect(api.hidePicker).toHaveBeenCalledOnce()
  })

  it('has a drag handle labelled "Move picker"', async () => {
    await renderPicker()
    const grip = screen.getByRole('img', { name: 'Move picker' })
    expect(grip.hasAttribute('data-tauri-drag-region')).toBe(true)
    expect(screen.getAllByRole('option').some((o) => o.hasAttribute('data-tauri-drag-region'))).toBe(false)
  })

  it('shows a placeholder, not a broken image, when there is no thumbnail URL', async () => {
    await renderPicker()
    const option = screen.getByRole('option', { name: 'c.jpg — wallhaven' })
    expect(option.querySelector('img')).toBeNull()
    expect(option.querySelector('.placeholder')).not.toBeNull()
  })
})

describe('context menu', () => {
  it('opens with Shift+F10, works from the keyboard and returns focus to the strip', async () => {
    const user = userEvent.setup()
    const list = await renderPicker()
    await user.keyboard('{Shift>}{F10}{/Shift}')
    const menu = screen.getByRole('menu', { name: 'Beach — local' })
    const items = within(menu).getAllByRole('menuitem')
    expect(items.map((i) => i.textContent)).toEqual([
      'Set as Wallpaper',
      'Add to Favorites',
      'Move to Trash',
      'Reset Position',
    ])
    expect(document.activeElement).toBe(items[0])
    await user.keyboard('{End}{Enter}')
    expect(api.resetPickerPosition).toHaveBeenCalledOnce()
    expect(screen.queryByRole('menu')).toBeNull()
    expect(document.activeElement).toBe(list)
  })

  it('Esc closes the menu without hiding the picker', async () => {
    const user = userEvent.setup()
    const list = await renderPicker()
    await user.keyboard('{ContextMenu}')
    expect(screen.getByRole('menu')).toBeTruthy()
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('menu')).toBeNull()
    expect(api.hidePicker).not.toHaveBeenCalled()
    expect(document.activeElement).toBe(list)
  })

  it('disables Move to Trash for a wallpaper that is not current', async () => {
    const user = userEvent.setup()
    await renderPicker()
    await user.keyboard('{Home}{ContextMenu}')
    const trash = screen.getByRole('menuitem', { name: 'Move to Trash' })
    expect(trash.getAttribute('aria-disabled')).toBe('true')
    await user.click(trash)
    expect(api.trashWallpaper).not.toHaveBeenCalled()
  })
})

describe('trash with undo', () => {
  const statusFor = (path: string) =>
    ({ current: { path } }) as unknown as Awaited<ReturnType<typeof api.getStatus>>

  async function trashCurrent() {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime })
    await renderPicker()
    await user.keyboard('{Delete}')
    return user
  }

  it('hides the wallpaper, then trashes it after 4 s if it is still current', async () => {
    vi.mocked(api.getStatus).mockResolvedValue(statusFor('/w/b.jpg'))
    await trashCurrent()
    expect(screen.queryByRole('option', { name: 'Beach — local' })).toBeNull()
    expect(screen.getByRole('status').textContent).toContain('Moved to Trash')
    expect(api.trashWallpaper).not.toHaveBeenCalled()
    await act(() => vi.advanceTimersByTimeAsync(4000))
    expect(api.trashWallpaper).toHaveBeenCalledOnce()
  })

  it('Undo restores it and never trashes', async () => {
    const user = await trashCurrent()
    await user.click(screen.getByRole('button', { name: 'Undo' }))
    expect(screen.getByRole('option', { name: 'Beach — local' })).toBeTruthy()
    await act(() => vi.advanceTimersByTimeAsync(5000))
    expect(api.getStatus).not.toHaveBeenCalled()
    expect(api.trashWallpaper).not.toHaveBeenCalled()
  })

  it('⌘Z undoes too', async () => {
    const user = await trashCurrent()
    await user.keyboard('{Meta>}z{/Meta}')
    expect(screen.getByRole('option', { name: 'Beach — local' })).toBeTruthy()
    await act(() => vi.advanceTimersByTimeAsync(5000))
    expect(api.trashWallpaper).not.toHaveBeenCalled()
  })

  it('does not trash the new wallpaper when it rotated during the undo window', async () => {
    vi.mocked(api.getStatus).mockResolvedValue(statusFor('/w/next.jpg'))
    await trashCurrent()
    await act(() => vi.advanceTimersByTimeAsync(4000))
    expect(api.trashWallpaper).not.toHaveBeenCalled()
    expect(screen.getByRole('status').textContent).toContain('nothing was moved to Trash')
    expect(screen.getByRole('option', { name: 'Beach — local' })).toBeTruthy()
  })

  it('Esc sends the pending trash before hiding (still guarded)', async () => {
    vi.mocked(api.getStatus).mockResolvedValue(statusFor('/w/b.jpg'))
    const user = await trashCurrent()
    await user.keyboard('{Escape}')
    expect(api.hidePicker).toHaveBeenCalledOnce()
    await vi.waitFor(() => expect(api.trashWallpaper).toHaveBeenCalledOnce())
  })

  it('refuses to trash a wallpaper that is not current', async () => {
    const user = userEvent.setup()
    await renderPicker()
    await user.keyboard('{Home}{Delete}')
    expect(screen.getByRole('option', { name: 'Alps — unsplash' })).toBeTruthy()
    expect(screen.getByRole('status').textContent).toContain('Only the current wallpaper')
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(api.trashWallpaper).not.toHaveBeenCalled()
  })
})

describe('live updates', () => {
  const selected = () => screen.getAllByRole('option').map((o) => o.getAttribute('aria-selected'))

  it('moves the current-wallpaper highlight when the daemon changes it', async () => {
    await renderPicker()
    act(() => live.handlers.onChanged?.({ ...WALLPAPER, path: '/w/a.jpg' }))
    expect(selected()).toEqual(['true', 'false', 'false'])
    expect(api.listThumbnails).toHaveBeenCalledTimes(1)
  })

  it('reloads the list for a wallpaper it does not have, and on reconnect', async () => {
    await renderPicker()
    await act(async () => live.handlers.onChanged?.({ ...WALLPAPER, path: '/w/new.jpg' }))
    expect(api.listThumbnails).toHaveBeenCalledTimes(2)
    await act(async () => live.handlers.onConnection?.({ state: 'connected', status: null }))
    expect(api.listThumbnails).toHaveBeenCalledTimes(3)
    await act(async () => live.handlers.onConnection?.({ state: 'not_running' }))
    expect(api.listThumbnails).toHaveBeenCalledTimes(3)
  })
})

const WALLPAPER = {
  path: '',
  source: 'local',
  source_url: '',
  image_url: '',
  title: '',
  author: '',
  width: 0,
  height: 0,
  set_at: '',
}
