import { act, cleanup, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { StatusData } from '../shared/api'
import * as api from '../shared/api'
import type { DaemonEventHandlers } from '../shared/useDaemonEvents'
import App from './App'

vi.mock('../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../shared/api')>()
  return { ...real, getConfig: vi.fn(), getAppInfo: vi.fn(), getStatus: vi.fn() }
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
  // The status bar renders whatever happens to the config.
  vi.mocked(api.getConfig).mockRejectedValue(new Error('not needed here'))
  vi.mocked(api.getAppInfo).mockResolvedValue({ os: 'macos', config_path: '', legacy_config: false, version: '0' })
  vi.mocked(api.getStatus).mockResolvedValue(status('Alps'))
})
afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('settings status bar, live', () => {
  it('follows changed and paused events without polling', async () => {
    render(<App />)
    expect(await screen.findByText('Alps')).toBeTruthy()

    vi.mocked(api.getStatus).mockResolvedValue(status('Beach'))
    await act(async () => live.handlers.onChanged?.(info('Beach')))
    expect(screen.getByText('Beach')).toBeTruthy()

    await act(async () => live.handlers.onPaused?.(true))
    expect(screen.getByText('Paused')).toBeTruthy()
  })

  it('shows "Daemon not running" and recovers on reconnect', async () => {
    render(<App />)
    await screen.findByText('Alps')

    await act(async () => live.handlers.onConnection?.({ state: 'not_running' }))
    expect(screen.getByText('Daemon not running')).toBeTruthy()

    await act(async () => live.handlers.onConnection?.({ state: 'connected', status: status('Coast') }))
    expect(screen.getByText('Coast')).toBeTruthy()
    expect(screen.queryByText('Daemon not running')).toBeNull()
  })
})
