import { act, cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { Config, DoctorReport } from '../shared/api'
import * as api from '../shared/api'
import type { DesktopConfig } from '../shared/bindings/DesktopConfig'
import DesktopSettings from './DesktopSettings'
import { testConfig } from './testConfig'

// Every backend call is mocked: no daemon, no CLI, no config file.
vi.mock('../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../shared/api')>()
  return {
    ...real,
    desktopBackend: vi.fn(),
    testDesktop: vi.fn(),
    detectDesktop: vi.fn(),
    saveConfig: vi.fn(),
    getConfig: vi.fn(),
  }
})

const report = (name: string, detail: string, apply: DoctorReport['apply'] = null): DoctorReport => ({
  version: '2',
  pid: 7,
  backend: { name, detail },
  theme: 'dark',
  apply,
})

const withDesktop = (desktop: Partial<DesktopConfig>): Config => {
  const c = testConfig()
  return { ...c, desktop: { ...c.desktop, ...desktop } }
}

/** The panel with App's state: an edited config, a saved config, and `onWritten`. */
function Harness({ saved, edited, os }: { saved: Config; edited?: Config; os: string }) {
  const [savedConfig, setSaved] = useState(saved)
  const [config, setConfig] = useState(edited ?? saved)
  return (
    <DesktopSettings
      config={config}
      savedConfig={savedConfig}
      onChange={setConfig}
      onWritten={(update) => {
        setSaved(update)
        setConfig(update)
      }}
      os={os}
    />
  )
}

async function renderPanel(props: Parameters<typeof Harness>[0]) {
  render(<Harness {...props} />)
  await act(async () => {})
}

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('DesktopSettings on macOS and Windows', () => {
  it.each([
    ['macos', 'NSWorkspace setDesktopImageURL (current Space of every screen)'],
    ['windows', 'IDesktopWallpaper'],
  ])('%s shows only the native backend and Fit', async (os, detail) => {
    vi.mocked(api.desktopBackend).mockResolvedValue(report(os, detail))
    await renderPanel({ saved: testConfig(), os })
    expect(screen.getByText(os)).toBeTruthy()
    expect(screen.getByText(detail)).toBeTruthy()
    expect(screen.getByRole('combobox', { name: 'Fit' })).toBeTruthy()
    expect(screen.getAllByRole('combobox')).toHaveLength(1)
    expect(screen.queryByRole('button', { name: /Test|Re-detect/ })).toBeNull()
    expect(screen.queryByText('Wallpaper command')).toBeNull()
    expect(screen.queryByRole('switch')).toBeNull()
    expect(api.detectDesktop).not.toHaveBeenCalled()
  })
})

describe('DesktopSettings on Linux', () => {
  const command = withDesktop({ backend: 'command', set_command: ['setbg', '{path}'], managed_by: 'user' })

  it('shows the detection report with its reasons', async () => {
    vi.mocked(api.desktopBackend).mockResolvedValue(report('command', 'setbg {path}'))
    vi.mocked(api.detectDesktop).mockResolvedValue('detected:  awww\nreasons:\n  - awww-daemon is running\n')
    await renderPanel({ saved: command, os: 'linux' })
    expect(api.detectDesktop).toHaveBeenCalledWith(false)
    expect(screen.getByLabelText('Detection report').textContent).toContain('- awww-daemon is running')

    vi.mocked(api.detectDesktop).mockResolvedValue('detected:  swaybg\n')
    await userEvent.click(screen.getByRole('button', { name: 'Re-detect' }))
    expect(screen.getByLabelText('Detection report').textContent).toBe('detected:  swaybg')
  })

  it('disables Test while the desktop section has unsaved edits', async () => {
    vi.mocked(api.desktopBackend).mockResolvedValue(report('command', 'setbg {path}'))
    vi.mocked(api.detectDesktop).mockResolvedValue('')
    const edited = withDesktop({ ...command.desktop, set_command: ['setbg', '--new', '{path}'] })
    await renderPanel({ saved: command, edited, os: 'linux' })
    expect(screen.getByRole('button', { name: 'Test' })).toHaveProperty('disabled', true)
    expect(screen.getByText('Save your changes to test them.')).toBeTruthy()
  })

  it('Test shows the daemon error text when the re-set fails', async () => {
    vi.mocked(api.desktopBackend).mockResolvedValue(report('command', 'setbg {path}'))
    vi.mocked(api.detectDesktop).mockResolvedValue('')
    vi.mocked(api.testDesktop).mockResolvedValue(
      report('command', 'setbg {path}', { ok: false, path: null, error: 'setbg exited with status 2: bad image' }),
    )
    await renderPanel({ saved: command, os: 'linux' })
    await userEvent.click(screen.getByRole('button', { name: 'Test' }))
    expect(api.testDesktop).toHaveBeenCalledTimes(1)
    expect(screen.getByText('setbg exited with status 2: bad image').getAttribute('role')).toBe('alert')
  })

  it('Test shows a failed reload as the daemon worded it', async () => {
    vi.mocked(api.desktopBackend).mockResolvedValue(report('command', 'setbg {path}'))
    vi.mocked(api.detectDesktop).mockResolvedValue('')
    vi.mocked(api.testDesktop).mockRejectedValue({ kind: 'daemon', message: 'reload: [desktop] set_command: command is empty' })
    await renderPanel({ saved: command, os: 'linux' })
    await userEvent.click(screen.getByRole('button', { name: 'Test' }))
    expect(screen.getByRole('alert').textContent).toBe('reload: [desktop] set_command: command is empty')
  })

  it('shows the restart notice only while the daemon reports another command', async () => {
    const saved = withDesktop({ backend: 'command', set_command: ['setbg', '--new', '{path}'] })
    vi.mocked(api.detectDesktop).mockResolvedValue('')
    vi.mocked(api.desktopBackend).mockResolvedValue(report('command', 'setbg {path}'))
    // Today: reload keeps the old setter, so after Test the daemon still reports the old argv.
    vi.mocked(api.testDesktop).mockResolvedValue(report('command', 'setbg {path}', { ok: true, path: '/w/a.jpg', error: null }))
    await renderPanel({ saved, os: 'linux' })
    expect(screen.getByText(/Restart the daemon to use the edited command/)).toBeTruthy()

    // After Task 37b: reload rebuilds the setter, the reply matches, the notice goes away.
    vi.mocked(api.desktopBackend).mockResolvedValue(report('command', 'setbg --new {path}'))
    await userEvent.click(screen.getByRole('button', { name: 'Test' }))
    expect(screen.getByText('Wallpaper set: /w/a.jpg')).toBeTruthy()
    expect(screen.queryByText(/Restart the daemon/)).toBeNull()
  })

  it('Pin writes managed_by through save_config immediately', async () => {
    vi.mocked(api.desktopBackend).mockResolvedValue(report('awww', 'awww img {path}'))
    vi.mocked(api.detectDesktop).mockResolvedValue('')
    vi.mocked(api.saveConfig).mockResolvedValue({ reloaded: true, reload_error: null })
    await renderPanel({ saved: withDesktop({ backend: 'awww' }), os: 'linux' })
    const pin = screen.getByRole('switch', { name: 'Pin' })
    expect(pin.getAttribute('aria-checked')).toBe('false')
    await userEvent.click(pin)
    expect(api.saveConfig).toHaveBeenCalledWith({ desktop: { managed_by: 'user' } })
    expect(pin.getAttribute('aria-checked')).toBe('true')
    // Pinned: detection may not overwrite it, and the write is not shown as an unsaved edit.
    expect(screen.getByRole('button', { name: 'Use detected backend' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: 'Test' })).toHaveProperty('disabled', false)
  })

  it('shows a failed Pin and stays unpinned', async () => {
    vi.mocked(api.desktopBackend).mockResolvedValue(report('awww', 'awww img {path}'))
    vi.mocked(api.detectDesktop).mockResolvedValue('')
    vi.mocked(api.saveConfig).mockRejectedValue({ kind: 'config', message: 'config.toml: permission denied' })
    await renderPanel({ saved: withDesktop({ backend: 'awww' }), os: 'linux' })
    await userEvent.click(screen.getByRole('switch', { name: 'Pin' }))
    expect(screen.getByRole('alert').textContent).toBe('config.toml: permission denied')
    expect(screen.getByRole('switch', { name: 'Pin' }).getAttribute('aria-checked')).toBe('false')
  })

  it('Use detected backend runs detect --write and reloads [desktop] from disk', async () => {
    vi.mocked(api.desktopBackend).mockResolvedValue(report('feh', 'feh --bg-fill {path}'))
    vi.mocked(api.detectDesktop).mockResolvedValue('detected:  awww\n')
    vi.mocked(api.getConfig).mockResolvedValue(withDesktop({ backend: 'awww', detected_session: 'hyprland:wayland' }))
    await renderPanel({ saved: withDesktop({ backend: 'feh' }), os: 'linux' })
    vi.mocked(api.detectDesktop).mockResolvedValueOnce('Saved backend = "awww"\nRestart the daemon to use it.\n')
    await userEvent.click(screen.getByRole('button', { name: 'Use detected backend' }))
    expect(api.detectDesktop).toHaveBeenCalledWith(true)
    expect(screen.getByRole('combobox', { name: 'Backend' })).toHaveProperty('value', 'awww')
    expect(screen.getByText('hyprland:wayland')).toBeTruthy()
    // The daemon still runs feh: the notice says so until it restarts.
    expect(screen.getByText(/Restart the daemon to use the edited command/).textContent).toContain('awww preset')
  })

  it('hides the command editor for backends that take no argv', async () => {
    vi.mocked(api.desktopBackend).mockResolvedValue(report('gnome', 'gsettings set org.gnome.desktop.background'))
    vi.mocked(api.detectDesktop).mockResolvedValue('')
    await renderPanel({ saved: withDesktop({ backend: 'gnome' }), os: 'linux' })
    expect(screen.queryByText('Wallpaper command')).toBeNull()
    expect(screen.queryByRole('switch', { name: 'Keep running' })).toBeNull()
    expect(screen.queryByText(/Restart the daemon/)).toBeNull()
  })
})
