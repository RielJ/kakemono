import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it } from 'vitest'

import {
  desktopBackend,
  detectDesktop,
  errorMessage,
  favoriteWallpaper,
  isDaemonNotRunning,
  saveConfig,
  selectDirectory,
  setFile,
  setStartAtLogin,
  startDaemon,
  testDesktop,
  trashWallpaper,
} from './api'

interface Call {
  cmd: string
  args: unknown
}

function recordIPC(reply: (cmd: string) => unknown = () => null): Call[] {
  const calls: Call[] = []
  mockIPC((cmd, args) => {
    calls.push({ cmd, args })
    return reply(cmd)
  })
  return calls
}

afterEach(() => clearMocks())

describe('shared/api', () => {
  it('starts the daemon through ensure_daemon', async () => {
    const calls = recordIPC(() => true)
    await expect(startDaemon()).resolves.toBe(true)
    expect(calls).toEqual([{ cmd: 'ensure_daemon', args: {} }])
  })

  it('sends set_start_at_login with `enabled` and returns the state read back', async () => {
    const items = {
      enabled: true,
      service: { installed: true, running: true, path: '/p.plist' },
      ui_autostart: true,
    }
    const calls = recordIPC(() => items)
    await expect(setStartAtLogin(true)).resolves.toEqual(items)
    expect(calls).toEqual([{ cmd: 'set_start_at_login', args: { enabled: true } }])
  })

  it('shows the CLI stderr from a failed lifecycle command', async () => {
    mockIPC(() => {
      throw { kind: 'cli_failed', message: '`kakemono service install` failed: denied' }
    })
    const err: unknown = await setStartAtLogin(true).catch((e: unknown) => e)
    expect(errorMessage(err)).toBe('`kakemono service install` failed: denied')
    expect(isDaemonNotRunning(err)).toBe(false)
  })

  it('sends set_file with the path', async () => {
    const calls = recordIPC()
    await setFile('/tmp/a.jpg')
    expect(calls).toEqual([{ cmd: 'set_file', args: { path: '/tmp/a.jpg' } }])
  })

  it('sends favorite with a null path for the current wallpaper', async () => {
    const calls = recordIPC()
    await favoriteWallpaper()
    expect(calls).toEqual([{ cmd: 'favorite', args: { path: null } }])
  })

  it('sends trash with the expected path, or null for no check', async () => {
    const calls = recordIPC()
    await trashWallpaper('/w/a.jpg')
    await trashWallpaper()
    expect(calls).toEqual([
      { cmd: 'trash', args: { expected: '/w/a.jpg' } },
      { cmd: 'trash', args: { expected: null } },
    ])
  })

  it('maps a cancelled directory dialog to an empty string', async () => {
    const calls = recordIPC((cmd) => (cmd === 'plugin:dialog|open' ? null : undefined))
    await expect(selectDirectory()).resolves.toBe('')
    expect(calls.map((c) => c.cmd)).toEqual(['plugin:dialog|open'])
  })

  it('sends save_config with the patch under `patch`', async () => {
    const calls = recordIPC(() => ({ reloaded: true, reload_error: null }))
    await expect(saveConfig({ daemon: { interval: '1h' } })).resolves.toEqual({ reloaded: true, reload_error: null })
    expect(calls).toEqual([{ cmd: 'save_config', args: { patch: { daemon: { interval: '1h' } } } }])
  })

  it('calls the desktop commands with their argument shapes', async () => {
    const report = {
      version: '2',
      pid: 1,
      backend: { name: 'command', detail: 'setbg {path}' },
      theme: 'dark',
      apply: { ok: false, path: null, error: 'setbg: exit status 2' },
    }
    const calls = recordIPC((cmd) => (cmd === 'detect_desktop' ? 'detected: awww\n' : report))
    await expect(desktopBackend()).resolves.toEqual(report)
    await expect(testDesktop()).resolves.toEqual(report)
    await expect(detectDesktop(true)).resolves.toBe('detected: awww\n')
    expect(calls).toEqual([
      { cmd: 'desktop_backend', args: {} },
      { cmd: 'test_desktop', args: {} },
      { cmd: 'detect_desktop', args: { write: true } },
    ])
  })

  it('recognises a daemon_not_running command error', () => {
    const err = { kind: 'daemon_not_running', message: 'kakemono daemon is not running' }
    expect(isDaemonNotRunning(err)).toBe(true)
    expect(isDaemonNotRunning({ kind: 'daemon', message: 'x' })).toBe(false)
    expect(errorMessage(err)).toBe('kakemono daemon is not running')
    expect(errorMessage(new Error('boom'))).toBe('boom')
  })
})
