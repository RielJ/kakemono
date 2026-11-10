import { act, cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { LoginItems } from '../../shared/api'
import * as api from '../../shared/api'
import StartAtLogin from './StartAtLogin'

vi.mock('../../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../shared/api')>()
  return { ...real, getStartAtLogin: vi.fn(), setStartAtLogin: vi.fn() }
})

const items = (installed: boolean, ui: boolean): LoginItems => ({
  enabled: installed && ui,
  service: { installed, running: installed, path: installed ? '/p.plist' : null },
  ui_autostart: ui,
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

const toggle = () => screen.getByRole('switch', { name: 'Start at login' })

describe('StartAtLogin', () => {
  it('is on only when both halves are on', async () => {
    vi.mocked(api.getStartAtLogin).mockResolvedValue(items(true, false))
    render(<StartAtLogin />)
    await act(async () => {})
    expect(toggle().getAttribute('aria-checked')).toBe('false')
    expect(toggle()).toHaveProperty('disabled', false)
  })

  it('switches both on and shows the state read back', async () => {
    vi.mocked(api.getStartAtLogin).mockResolvedValue(items(false, false))
    vi.mocked(api.setStartAtLogin).mockResolvedValue(items(true, true))
    render(<StartAtLogin />)
    await act(async () => {})
    await userEvent.click(toggle())
    expect(api.setStartAtLogin).toHaveBeenCalledWith(true)
    expect(toggle().getAttribute('aria-checked')).toBe('true')
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('shows the CLI stderr and the real state when enabling fails', async () => {
    vi.mocked(api.getStartAtLogin).mockResolvedValue(items(false, false))
    vi.mocked(api.setStartAtLogin).mockRejectedValue({
      kind: 'cli_failed',
      message: '`kakemono service install` failed: launchctl bootstrap failed: 5',
    })
    render(<StartAtLogin />)
    await act(async () => {})
    await userEvent.click(toggle())
    expect(screen.getByRole('alert').textContent).toBe(
      '`kakemono service install` failed: launchctl bootstrap failed: 5',
    )
    expect(toggle().getAttribute('aria-checked')).toBe('false')
    expect(api.getStartAtLogin).toHaveBeenCalledTimes(2)
  })

  it('is disabled with the reason when the state cannot be read', async () => {
    vi.mocked(api.getStartAtLogin).mockRejectedValue({ kind: 'cli_not_found', message: 'cannot find kakemono' })
    render(<StartAtLogin />)
    await act(async () => {})
    expect(toggle()).toHaveProperty('disabled', true)
    expect(screen.getByRole('alert').textContent).toBe('cannot find kakemono')
  })
})

describe('StartAtLogin line variant', () => {
  const box = () => screen.getByRole('checkbox', { name: 'Start at login' })

  it('renders a real checkbox that the sentence text toggles', async () => {
    vi.mocked(api.getStartAtLogin).mockResolvedValue(items(false, false))
    vi.mocked(api.setStartAtLogin).mockResolvedValue(items(true, true))
    render(<StartAtLogin variant="line" />)
    await act(async () => {})
    expect(screen.queryByRole('switch')).toBeNull()
    expect(box()).toHaveProperty('checked', false)
    await userEvent.click(screen.getByText('Start at login'))
    expect(api.setStartAtLogin).toHaveBeenCalledWith(true)
    expect(box()).toHaveProperty('checked', true)
  })

  it('is disabled with the reason when the state cannot be read', async () => {
    vi.mocked(api.getStartAtLogin).mockRejectedValue({ kind: 'cli_not_found', message: 'cannot find kakemono' })
    render(<StartAtLogin variant="line" />)
    await act(async () => {})
    expect(box()).toHaveProperty('disabled', true)
    expect(screen.getByRole('alert').textContent).toBe('cannot find kakemono')
  })
})
