import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it } from 'vitest'

import { favoriteWallpaper, selectDirectory, setFile } from './api'

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
})
