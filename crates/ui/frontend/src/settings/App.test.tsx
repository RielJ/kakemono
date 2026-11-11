import { act, cleanup, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { StatusData } from '../shared/api'
import * as api from '../shared/api'
import type { DaemonEventHandlers } from '../shared/useDaemonEvents'
import App from './App'
import { testConfig } from './testConfig'

vi.mock('../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../shared/api')>()
  return {
    ...real,
    getConfig: vi.fn(),
    getAppInfo: vi.fn(),
    getStatus: vi.fn(),
    fetchDroppedUrl: vi.fn(),
    checkUpdate: vi.fn(),
  }
})

// Captures the handlers the settings window registers, so tests can play daemon events.
const live = vi.hoisted(() => ({ handlers: {} as DaemonEventHandlers }))
vi.mock('../shared/useDaemonEvents', () => ({
  useDaemonEvents: (h: DaemonEventHandlers) => {
    live.handlers = h
  },
}))

const info = (title: string) => ({
  path: `/w/${title}.jpg`,
  source: 'local',
  source_url: '',
  image_url: '',
  title,
  author: '',
  width: 0,
  height: 0,
  set_at: '',
})

const status = (title: string, paused = false): StatusData => ({
  current: info(title),
  paused,
  interval: '30m',
  theme: 'dark',
  history_count: 2,
  history_index: 1,
})

beforeEach(() => {
  vi.mocked(api.getConfig).mockResolvedValue(testConfig())
  vi.mocked(api.getAppInfo).mockResolvedValue({ os: 'macos', config_path: '', version: '0' })
  vi.mocked(api.getStatus).mockResolvedValue(status('Alps'))
  vi.mocked(api.checkUpdate).mockResolvedValue(null)
})
afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('settings shell', () => {
  it('shows a banner when a newer version is out', async () => {
    vi.mocked(api.checkUpdate).mockResolvedValue({ version: '9.0.0', current: '0', install: 'in_app' })
    render(<App />)
    const banner = await screen.findByRole('region', { name: 'App update' })
    expect(within(banner).getByText('kakemono 9.0.0 is available')).toBeTruthy()
    expect(within(banner).getByRole('button', { name: 'Update' })).toBeTruthy()
  })

  // Desktop's groups can repeat a title, so count matches instead of asking for exactly one.
  const heading = (name: string) => (screen.queryAllByRole('heading', { name }).length > 0 ? name : null)

  it('has two tabs, General and Advanced, and opens on General', async () => {
    render(<App />)
    await screen.findByText('Alps')

    const tabs = within(screen.getByRole('tablist')).getAllByRole('tab')
    expect(tabs.map((t) => t.textContent)).toEqual(['General', 'Advanced'])
    const general = screen.getByRole('tab', { name: 'General' })
    expect(general.getAttribute('aria-selected')).toBe('true')
    expect(screen.getByRole('tab', { name: 'Advanced' }).getAttribute('aria-selected')).toBe('false')

    const panel = screen.getByRole('tabpanel')
    expect(panel.getAttribute('aria-labelledby')).toBe(general.id)
    // General is the Variety-style list: an untitled top panel, then filters, downloading, picker.
    expect(heading('When possible, use images that…')).toBeTruthy()
    expect(heading('Downloading')).toBeTruthy()
    expect(heading('Picker')).toBeTruthy()
    expect(heading('Folders')).toBeNull()
  })

  it('shows the Advanced content when Advanced is clicked', async () => {
    const user = userEvent.setup()
    render(<App />)
    await screen.findByText('Alps')

    const advanced = screen.getByRole('tab', { name: 'Advanced' })
    await user.click(advanced)

    expect(advanced.getAttribute('aria-selected')).toBe('true')
    expect(screen.getByRole('tab', { name: 'General' }).getAttribute('aria-selected')).toBe('false')
    expect(screen.getByRole('tabpanel').getAttribute('aria-labelledby')).toBe(advanced.id)
    // Advanced holds the desktop backend section along with folders, hooks and provider keys.
    expect(heading('Folders')).toBeTruthy()
    expect(heading('Hooks')).toBeTruthy()
    expect(heading('Desktop')).toBeTruthy()
    expect(heading('Provider keys')).toBeTruthy()
    expect(heading('Rotation')).toBeNull()
  })

  it('moves between tabs with the arrow keys, Home and End; only the selected tab is tabbable', async () => {
    const user = userEvent.setup()
    render(<App />)
    await screen.findByText('Alps')
    const general = screen.getByRole('tab', { name: 'General' })
    const advanced = screen.getByRole('tab', { name: 'Advanced' })
    expect(general.tabIndex).toBe(0)
    expect(advanced.tabIndex).toBe(-1)

    general.focus()
    await user.keyboard('{ArrowRight}')
    expect(advanced.getAttribute('aria-selected')).toBe('true')
    expect(document.activeElement).toBe(advanced)
    expect(advanced.tabIndex).toBe(0)
    expect(general.tabIndex).toBe(-1)

    await user.keyboard('{ArrowRight}')
    expect(general.getAttribute('aria-selected')).toBe('true')
    expect(document.activeElement).toBe(general)

    await user.keyboard('{ArrowLeft}')
    expect(document.activeElement).toBe(advanced)
    await user.keyboard('{Home}')
    expect(document.activeElement).toBe(general)
    await user.keyboard('{End}')
    expect(document.activeElement).toBe(advanced)
    expect(heading('Folders')).toBeTruthy()
  })

  it('keeps the daemon status footer, with Start when the daemon is stopped', async () => {
    render(<App />)
    await screen.findByText('Alps')
    expect(screen.getByText('Daemon running')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Start' })).toBeNull()

    await act(async () => live.handlers.onConnection?.({ state: 'not_running' }))
    expect(screen.getByText('Daemon stopped')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Start' })).toBeTruthy()
  })
})

describe('settings now-showing card, live', () => {
  it('follows changed and paused events without polling', async () => {
    render(<App />)
    expect(await screen.findByText('Alps')).toBeTruthy()

    vi.mocked(api.getStatus).mockResolvedValue(status('Beach'))
    await act(async () => live.handlers.onChanged?.(info('Beach')))
    expect(screen.getByText('Beach')).toBeTruthy()

    await act(async () => live.handlers.onPaused?.(true))
    expect(screen.getByText('Paused')).toBeTruthy()
  })

  it('shows the daemon is not running and recovers on reconnect', async () => {
    render(<App />)
    await screen.findByText('Alps')

    await act(async () => live.handlers.onConnection?.({ state: 'not_running' }))
    expect(screen.getByText('The daemon is not running')).toBeTruthy()

    await act(async () => live.handlers.onConnection?.({ state: 'connected', status: status('Coast') }))
    expect(screen.getByText('Coast')).toBeTruthy()
    expect(screen.queryByText('The daemon is not running')).toBeNull()
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

describe('settings image drop', () => {
  it('shows the overlay while dragging and reports the result in the footer', async () => {
    vi.mocked(api.fetchDroppedUrl).mockResolvedValue(info('a'))
    render(<App />)
    await screen.findByText('Alps')

    dragUrl('dragenter', 'https://x.example/a.jpg')
    expect(screen.getByText('Drop an image to set it')).toBeTruthy()
    dragUrl('drop', 'https://x.example/a.jpg')
    expect(screen.queryByText('Drop an image to set it')).toBeNull()

    expect(api.fetchDroppedUrl).toHaveBeenCalledWith('https://x.example/a.jpg')
    const footer = screen.getByRole('contentinfo')
    expect(await within(footer).findByText('Set from a.jpg')).toBeTruthy()
  })

  it('keeps a failed drop in the footer', async () => {
    vi.mocked(api.fetchDroppedUrl).mockRejectedValue({ kind: 'daemon', message: 'not an image' })
    render(<App />)
    await screen.findByText('Alps')
    dragUrl('drop', 'https://x.example/page')
    expect(await within(screen.getByRole('contentinfo')).findByText('not an image')).toBeTruthy()
  })
})
