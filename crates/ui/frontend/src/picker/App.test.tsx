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
    listTab: vi.fn(),
    onPickerShown: vi.fn(),
    setFile: vi.fn(),
    favoriteWallpaper: vi.fn(),
    trashWallpaper: vi.fn(),
    getStatus: vi.fn(),
    hidePicker: vi.fn(),
    resetPickerPosition: vi.fn(),
    fetchDroppedUrl: vi.fn(),
  }
})

// Captures the handlers the strip registers, so tests can play daemon events.
const live = vi.hoisted(() => ({ handlers: {} as import('../shared/useDaemonEvents').DaemonEventHandlers }))
vi.mock('../shared/useDaemonEvents', () => ({
  useDaemonEvents: (h: import('../shared/useDaemonEvents').DaemonEventHandlers) => {
    live.handlers = h
  },
}))

// The `picker-shown` handler App registers, so tests can play a show.
const shown = vi.hoisted(() => ({ handler: null as null | (() => void) }))

const THUMBS: Thumbnail[] = [
  { path: '/w/a.jpg', source: 'unsplash', title: 'Alps', isCurrent: false, imageUrl: 'thumb://localhost/a' },
  { path: '/w/b.jpg', source: 'local', title: 'Beach', isCurrent: true, imageUrl: 'thumb://localhost/b' },
  { path: '/w/c.jpg', source: 'wallhaven', title: '', isCurrent: false, imageUrl: '' },
]

function mockConfig(position: string, orientation?: string) {
  vi.mocked(api.getConfig).mockResolvedValue({ picker: { position, orientation } } as unknown as Awaited<
    ReturnType<typeof api.getConfig>
  >)
}

function mockApi(position = 'bottom', orientation?: string) {
  mockConfig(position, orientation)
  vi.mocked(api.getAppInfo).mockResolvedValue({ os: 'macos', config_path: '', legacy_config: false, version: '0' })
  vi.mocked(api.listTab).mockImplementation(async () => page(THUMBS))
  vi.mocked(api.onPickerShown).mockImplementation(async (handler) => {
    shown.handler = handler
    return () => {}
  })
  vi.mocked(api.setFile).mockResolvedValue(undefined)
  vi.mocked(api.favoriteWallpaper).mockResolvedValue(undefined)
  vi.mocked(api.trashWallpaper).mockResolvedValue(undefined)
  vi.mocked(api.hidePicker).mockResolvedValue(undefined)
  vi.mocked(api.resetPickerPosition).mockResolvedValue(undefined)
}

/** One `listTab` page holding `items` (copied, so a test can't mutate the fixtures). */
function page(items: Thumbnail[], hasMore = false): api.TabListing {
  return { items: items.map((t) => ({ ...t })), currentIndex: items[0]?.isCurrent ? 0 : null, hasMore }
}

/** Opens the picker and switches to the Downloaded tab (it opens on Sources). */
async function renderPicker() {
  render(<App />)
  fireEvent.click(await screen.findByRole('tab', { name: 'Downloaded' }))
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

  it('runs vertically at the bottom when [picker] orientation is vertical', async () => {
    mockApi('bottom', 'vertical')
    const list = await renderPicker()
    await vi.waitFor(() => expect(list.getAttribute('aria-orientation')).toBe('vertical'))
    expect(document.querySelector('main.app-vertical')).not.toBeNull()
  })

  it('runs horizontally on the right when [picker] orientation is horizontal', async () => {
    mockApi('right', 'horizontal')
    const list = await renderPicker()
    // Let the config resolve before asserting the (default) horizontal layout stuck.
    await act(async () => {
      await vi.mocked(api.getConfig).mock.results[0]?.value
    })
    expect(list.getAttribute('aria-orientation')).toBe('horizontal')
    expect(document.querySelector('main.app-horizontal')).not.toBeNull()
  })

  it('re-reads the orientation each time the reused window is shown', async () => {
    const list = await renderPicker()
    expect(list.getAttribute('aria-orientation')).toBe('horizontal')
    mockConfig('bottom', 'vertical')
    act(() => {
      window.dispatchEvent(new Event('focus'))
    })
    await vi.waitFor(() => expect(list.getAttribute('aria-orientation')).toBe('vertical'))
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
    // The daemon re-checks the path atomically (Task 45b).
    expect(api.trashWallpaper).toHaveBeenCalledExactlyOnceWith('/w/b.jpg')
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
    expect(api.listTab).toHaveBeenCalledTimes(1)
  })

  it('reloads the list for a wallpaper it does not have, and on reconnect', async () => {
    await renderPicker()
    await act(async () => live.handlers.onChanged?.({ ...WALLPAPER, path: '/w/new.jpg' }))
    expect(api.listTab).toHaveBeenCalledTimes(2)
    await act(async () => live.handlers.onConnection?.({ state: 'connected', status: null }))
    expect(api.listTab).toHaveBeenCalledTimes(3)
    await act(async () => live.handlers.onConnection?.({ state: 'not_running' }))
    expect(api.listTab).toHaveBeenCalledTimes(3)
    expect(api.listTab).toHaveBeenLastCalledWith('downloaded', 0)
  })
})

/** A drag event carrying a URL, dispatched on the window (jsdom has no DataTransfer). */
function dragUrl(type: string, url: string) {
  const event = new Event(type, { bubbles: true, cancelable: true })
  const data: Record<string, string> = { 'text/uri-list': url }
  Object.defineProperty(event, 'dataTransfer', {
    value: { types: Object.keys(data), files: [], getData: (t: string) => data[t] ?? '', dropEffect: 'none' },
  })
  act(() => {
    document.body.dispatchEvent(event)
  })
}

describe('picker image drop', () => {
  it('shows the overlay while dragging and reports the result in the toast', async () => {
    vi.mocked(api.fetchDroppedUrl).mockResolvedValue({ ...WALLPAPER, path: '/m/a.jpg' })
    await renderPicker()

    dragUrl('dragenter', 'https://x.example/a.jpg')
    expect(screen.getByText('Drop an image to set it')).toBeTruthy()
    dragUrl('drop', 'https://x.example/a.jpg')
    expect(screen.queryByText('Drop an image to set it')).toBeNull()

    expect(api.fetchDroppedUrl).toHaveBeenCalledWith('https://x.example/a.jpg')
    expect(await within(screen.getByRole('status')).findByText('Set from a.jpg')).toBeTruthy()
  })

  it('shows a failed drop in the toast', async () => {
    vi.mocked(api.fetchDroppedUrl).mockRejectedValue({ kind: 'daemon', message: 'not an image' })
    await renderPicker()
    dragUrl('drop', 'https://x.example/page')
    expect(await within(screen.getByRole('status')).findByText('not an image')).toBeTruthy()
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

/** A stand-in for `IntersectionObserver` (jsdom has none) that a test can trigger. */
class FakeObserver {
  static instances: FakeObserver[] = []
  readonly targets = new Set<Element>()
  disconnected = false
  constructor(readonly callback: IntersectionObserverCallback) {
    FakeObserver.instances.push(this)
  }
  observe(target: Element) {
    this.targets.add(target)
  }
  unobserve(target: Element) {
    this.targets.delete(target)
  }
  disconnect() {
    this.disconnected = true
    this.targets.clear()
  }
  takeRecords(): IntersectionObserverEntry[] {
    return []
  }
  /** Reports every live observer's targets as scrolled into view. */
  static showAll() {
    for (const o of FakeObserver.instances.filter((i) => !i.disconnected)) {
      const entries = [...o.targets].map((target) => ({ target, isIntersecting: true }) as IntersectionObserverEntry)
      o.callback(entries, o as unknown as IntersectionObserver)
    }
  }
}

describe('tabs', () => {
  const tab = (name: string) => screen.getByRole('tab', { name })
  const many = (n: number, from = 0): Thumbnail[] =>
    Array.from({ length: n }, (_, i) => ({
      path: `/w/${from + i}.jpg`,
      source: 'local',
      title: `W${from + i}`,
      isCurrent: from + i === 0,
      imageUrl: '',
    }))

  beforeEach(() => {
    FakeObserver.instances = []
    vi.stubGlobal('IntersectionObserver', FakeObserver)
  })
  afterEach(() => vi.unstubAllGlobals())

  it('opens on Sources inside the tab panel and lists nothing from disk', async () => {
    render(<App />)
    expect((await screen.findByRole('tab', { name: 'Sources' })).getAttribute('aria-selected')).toBe('true')
    const panel = screen.getByRole('tabpanel')
    expect(panel.id).toBe('picker-panel')
    expect(panel.getAttribute('aria-labelledby')).toBe('picker-tab-sources')
    expect(api.listTab).not.toHaveBeenCalled()
  })

  it('Downloaded lists its first page with the current wallpaper first, and the sentinel loads offset 40', async () => {
    vi.mocked(api.listTab).mockImplementation(async (_tab, offset) =>
      offset === 0 ? page(many(40), true) : page(many(5, 40)),
    )
    render(<App />)
    fireEvent.click(await screen.findByRole('tab', { name: 'Downloaded' }))
    const list = await screen.findByRole('listbox', { name: 'Wallpapers' })
    await vi.waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(40))
    expect(api.listTab).toHaveBeenCalledExactlyOnceWith('downloaded', 0)
    expect(activeLabel(list)).toBe('W0 — local')
    expect(screen.getAllByRole('option')[0].getAttribute('aria-selected')).toBe('true')

    act(() => FakeObserver.showAll())
    await vi.waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(45))
    expect(api.listTab).toHaveBeenLastCalledWith('downloaded', 40)
    // The last page said there is no more: the sentinel is gone.
    act(() => FakeObserver.showAll())
    expect(api.listTab).toHaveBeenCalledTimes(2)
  })

  it('moving the keyboard near the end loads the next page', async () => {
    vi.mocked(api.listTab).mockImplementation(async (_tab, offset) =>
      offset === 0 ? page(many(40), true) : page(many(5, 40)),
    )
    const user = userEvent.setup()
    render(<App />)
    fireEvent.click(await screen.findByRole('tab', { name: 'Downloaded' }))
    await screen.findByRole('listbox', { name: 'Wallpapers' })
    await vi.waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(40))
    await user.keyboard('{End}')
    await vi.waitFor(() => expect(api.listTab).toHaveBeenLastCalledWith('downloaded', 40))
  })

  it('picker-shown goes back to Sources; window focus reloads the active tab without switching', async () => {
    await renderPicker()
    expect(api.listTab).toHaveBeenCalledExactlyOnceWith('downloaded', 0)
    act(() => {
      window.dispatchEvent(new Event('focus'))
    })
    await vi.waitFor(() => expect(api.listTab).toHaveBeenCalledTimes(2))
    expect(api.listTab).toHaveBeenLastCalledWith('downloaded', 0)
    expect(tab('Downloaded').getAttribute('aria-selected')).toBe('true')

    await vi.waitFor(() => expect(shown.handler).not.toBeNull())
    act(() => shown.handler?.())
    expect(tab('Sources').getAttribute('aria-selected')).toBe('true')
    expect(screen.queryByRole('listbox', { name: 'Wallpapers' })).toBeNull()
  })

  it('switches with ↑/↓ and 1–5 from the strip, and still from an empty tab', async () => {
    vi.mocked(api.listTab).mockImplementation(async (t) => (t === 'downloaded' ? page(THUMBS) : page([])))
    const user = userEvent.setup()
    await renderPicker()
    await user.keyboard('{ArrowDown}')
    expect(tab('Local').getAttribute('aria-selected')).toBe('true')
    expect(await screen.findByText(/No local images/)).toBeTruthy()
    await user.keyboard('{ArrowDown}')
    expect(tab('Favorites').getAttribute('aria-selected')).toBe('true')
    await user.keyboard('{ArrowUp}{ArrowUp}')
    expect(tab('Downloaded').getAttribute('aria-selected')).toBe('true')
    await user.keyboard('5')
    expect(tab('Cache').getAttribute('aria-selected')).toBe('true')
    await user.keyboard('1')
    expect(tab('Sources').getAttribute('aria-selected')).toBe('true')
    await user.keyboard('{ArrowUp}')
    expect(tab('Cache').getAttribute('aria-selected')).toBe('true')
    expect(api.listTab).toHaveBeenCalledWith('cache', 0)
  })

  it('switches with ←/→ in a vertical picker', async () => {
    mockApi('left')
    const user = userEvent.setup()
    render(<App />)
    await vi.waitFor(() => expect(document.querySelector('main.app-vertical')).not.toBeNull())
    // The vertical picker keeps its grip row above the tab row.
    expect(screen.getByRole('img', { name: 'Move picker' }).className).toBe('grip')
    await user.keyboard('{ArrowRight}')
    expect(tab('Downloaded').getAttribute('aria-selected')).toBe('true')
    await user.keyboard('{ArrowLeft}{ArrowLeft}')
    expect(tab('Cache').getAttribute('aria-selected')).toBe('true')
  })

  it('gives each on-disk tab its own empty state', async () => {
    vi.mocked(api.listTab).mockImplementation(async () => page([]))
    render(<App />)
    const texts: string[] = []
    for (const name of ['Downloaded', 'Local', 'Favorites', 'Cache']) {
      fireEvent.click(await screen.findByRole('tab', { name }))
      await vi.waitFor(() => expect(api.listTab).toHaveBeenLastCalledWith(name.toLowerCase(), 0))
      const text = await vi.waitFor(() => {
        const state = screen.getByRole('tabpanel').querySelector('.state')
        if (!state || state.textContent === 'Loading wallpapers…') throw new Error('still loading')
        return state.textContent ?? ''
      })
      texts.push(text)
    }
    expect(new Set(texts).size).toBe(4)
  })

  it('Esc hides the picker from any tab', async () => {
    const user = userEvent.setup()
    render(<App />)
    await screen.findByRole('tab', { name: 'Sources' })
    await user.keyboard('{Escape}')
    expect(api.hidePicker).toHaveBeenCalledOnce()
  })
})
