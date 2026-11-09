import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it } from 'vitest'

import { errorMessage, favoriteWallpaper, isDaemonNotRunning, saveConfig, selectDirectory, setFile } from './api'

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

  it('recognises a daemon_not_running command error', () => {
    const err = { kind: 'daemon_not_running', message: 'kakemono daemon is not running' }
    expect(isDaemonNotRunning(err)).toBe(true)
    expect(isDaemonNotRunning({ kind: 'daemon', message: 'x' })).toBe(false)
    expect(errorMessage(err)).toBe('kakemono daemon is not running')
    expect(errorMessage(new Error('boom'))).toBe('boom')
  })
})
