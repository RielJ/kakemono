import { emit } from '@tauri-apps/api/event'
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { afterEach, describe, expect, it, vi } from 'vitest'

import {
  browsePage,
  checkUpdate,
  installUpdate,
  openUpdatePage,
  browseUse,
  checkSource,
  currentThumbnail,
  deleteSourceFiles,
  desktopBackend,
  detectDesktop,
  downloadNow,
  errorMessage,
  favoriteWallpaper,
  fetchDroppedUrl,
  importDroppedFile,
  isDaemonNotRunning,
  isDaemonTimeout,
  isDaemonUnreachable,
  listTab,
  onPickerShown,
  openSourceFolder,
  saveConfig,
  selectDirectory,
  selectFiles,
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

/**
 * Wraps the mocked IPC so each call's third `invoke` argument (headers) is recorded too;
 * `mockIPC` drops it. Call after `mockIPC`.
 */
function recordInvokeOptions(): unknown[] {
  const seen: unknown[] = []
  const internals: unknown = Reflect.get(window, '__TAURI_INTERNALS__')
  if (typeof internals !== 'object' || internals === null) throw new Error('mockIPC first')
  const inner: unknown = Reflect.get(internals, 'invoke')
  if (typeof inner !== 'function') throw new Error('mockIPC first')
  Reflect.set(internals, 'invoke', (cmd: string, args: unknown, options: unknown) => {
    seen.push(options)
    return Reflect.apply(inner, internals, [cmd, args, options])
  })
  return seen
}

afterEach(() => clearMocks())

describe('shared/api', () => {
  it('checks for updates and opens the release page', async () => {
    const info = { version: '0.3.0', current: '0.2.0', install: 'in_app', command: null }
    const calls = recordIPC((cmd) => (cmd === 'check_update' ? info : null))
    await expect(checkUpdate()).resolves.toEqual(info)
    await openUpdatePage()
    expect(calls).toEqual([
      { cmd: 'check_update', args: {} },
      { cmd: 'open_update_page', args: {} },
    ])
  })

  it('installs an update with a progress channel', async () => {
    const calls = recordIPC()
    await installUpdate(() => {})
    expect(calls).toHaveLength(1)
    expect(calls[0]?.cmd).toBe('install_update')
    // The channel goes over IPC as a callback reference.
    expect(JSON.parse(JSON.stringify(calls[0]?.args))).toEqual({
      progress: expect.stringMatching(/^__CHANNEL__:\d+$/),
    })
  })

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

  it('opens a multi-select image dialog and always returns a list of paths', async () => {
    let reply: unknown = ['/p/a.jpg', '/p/b.png']
    const calls = recordIPC((cmd) => (cmd === 'plugin:dialog|open' ? reply : undefined))
    await expect(selectFiles()).resolves.toEqual(['/p/a.jpg', '/p/b.png'])
    expect(calls[0]).toMatchObject({
      cmd: 'plugin:dialog|open',
      args: { options: { directory: false, multiple: true } },
    })
    reply = null
    await expect(selectFiles()).resolves.toEqual([])
    reply = '/p/c.webp'
    await expect(selectFiles()).resolves.toEqual(['/p/c.webp'])
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

  it('sends check_source with kind and location and returns the count', async () => {
    const calls = recordIPC(() => ({ count: 24 }))
    await expect(checkSource('wallhaven', 'forest')).resolves.toEqual({ count: 24 })
    expect(calls).toEqual([{ cmd: 'check_source', args: { kind: 'wallhaven', location: 'forest' } }])
  })

  it('tells daemon_unreachable apart from a source the daemon refused', async () => {
    mockIPC(() => {
      throw { kind: 'daemon_unreachable', message: 'cannot reach the kakemono daemon: no CLI' }
    })
    const down: unknown = await checkSource('bing', '').catch((e: unknown) => e)
    expect(isDaemonUnreachable(down)).toBe(true)
    expect(errorMessage(down)).toBe('cannot reach the kakemono daemon: no CLI')

    mockIPC(() => {
      throw { kind: 'daemon', message: 'no images found' }
    })
    const refused: unknown = await checkSource('bing', '').catch((e: unknown) => e)
    expect(isDaemonUnreachable(refused)).toBe(false)
    expect(errorMessage(refused)).toBe('no images found')
    expect(isDaemonUnreachable(new Error('daemon_unreachable'))).toBe(false)
  })

  it('sends download_now with the id, or null for every source without images', async () => {
    const calls = recordIPC(() => ({ path: '/d/wh-1/a.jpg' }))
    await expect(downloadNow('wh-1')).resolves.toEqual({ path: '/d/wh-1/a.jpg' })
    await downloadNow()
    expect(calls).toEqual([
      { cmd: 'download_now', args: { id: 'wh-1' } },
      { cmd: 'download_now', args: { id: null } },
    ])
  })

  it('sends delete_source_files and open_source_folder with the source id', async () => {
    const calls = recordIPC()
    await deleteSourceFiles('wh-1')
    await openSourceFolder('dir-1')
    expect(calls).toEqual([
      { cmd: 'delete_source_files', args: { id: 'wh-1' } },
      { cmd: 'open_source_folder', args: { id: 'dir-1' } },
    ])
  })

  it('sends fetch_dropped_url with the url and returns the new wallpaper', async () => {
    const info = { path: '/m/y.jpg', source: 'manual', title: 'y' }
    const calls = recordIPC(() => info)
    await expect(fetchDroppedUrl('https://x/y.jpg')).resolves.toEqual(info)
    expect(calls).toEqual([{ cmd: 'fetch_dropped_url', args: { url: 'https://x/y.jpg' } }])
  })

  it('sends a dropped file as a raw body with its encoded name in a header', async () => {
    const info = { path: '/m/naïve photo.png', source: 'manual', title: '' }
    const calls = recordIPC(() => info)
    const options = recordInvokeOptions()
    const bytes = new Uint8Array([0x89, 0x50, 0x4e, 0x47])
    await expect(importDroppedFile('naïve photo.png', bytes)).resolves.toEqual(info)
    expect(calls).toEqual([{ cmd: 'import_dropped_file', args: bytes }])
    expect(options).toEqual([{ headers: { 'x-file-name': 'na%C3%AFve%20photo.png' } }])
  })

  it('sends current_thumbnail with no args and returns the entry or null', async () => {
    const entry = {
      path: '/w/a.jpg',
      source: 'wallhaven',
      title: 'a',
      isCurrent: true,
      imageUrl: 'thumb://localhost/1',
    }
    const calls = recordIPC(() => entry)
    await expect(currentThumbnail()).resolves.toEqual(entry)
    expect(calls).toEqual([{ cmd: 'current_thumbnail', args: {} }])
  })

  it('sends tab_thumbnails with the tab and offset and returns the page', async () => {
    const page = { items: [], currentIndex: null, hasMore: true }
    const calls = recordIPC(() => page)
    await expect(listTab('downloaded', 40)).resolves.toEqual(page)
    expect(calls).toEqual([{ cmd: 'tab_thumbnails', args: { tab: 'downloaded', offset: 40 } }])
  })

  it('sends browse_page with null session and source for a fresh, unfiltered page', async () => {
    const listing = { session: 's1', page: 0, restarted: false, done: false, items: [], sources: [] }
    const calls = recordIPC(() => listing)
    await expect(browsePage({ page: 0 })).resolves.toEqual(listing)
    await browsePage({ session: 's1', page: 3, source: 'wh-1' })
    expect(calls).toEqual([
      { cmd: 'browse_page', args: { session: null, page: 0, source: null } },
      { cmd: 'browse_page', args: { session: 's1', page: 3, source: 'wh-1' } },
    ])
  })

  it('sends browse_use with camelCase ids and the action', async () => {
    const data = { path: '/d/wh-1/x.jpg', wallpaper: null }
    const calls = recordIPC(() => data)
    await expect(browseUse({ sourceId: 'wh-1', remoteId: 'x8e1k2', action: 'favorite' })).resolves.toEqual(data)
    expect(calls).toEqual([{ cmd: 'browse_use', args: { sourceId: 'wh-1', remoteId: 'x8e1k2', action: 'favorite' } }])
  })

  it('recognises the browse_use timeout as daemon_timeout', () => {
    expect(isDaemonTimeout({ kind: 'daemon_timeout', message: 'Still downloading in the background…' })).toBe(true)
    expect(isDaemonTimeout({ kind: 'daemon', message: 'x' })).toBe(false)
    expect(isDaemonTimeout(new Error('daemon_timeout'))).toBe(false)
  })

  it('calls the picker-shown handler on each event until unlistened', async () => {
    mockIPC(() => null, { shouldMockEvents: true })
    const shown = vi.fn()
    const unlisten = await onPickerShown(shown)
    await emit('picker-shown')
    await emit('picker-shown')
    expect(shown).toHaveBeenCalledTimes(2)
    unlisten()
    await emit('picker-shown')
    expect(shown).toHaveBeenCalledTimes(2)
  })

  it('recognises a daemon_not_running command error', () => {
    const err = { kind: 'daemon_not_running', message: 'kakemono daemon is not running' }
    expect(isDaemonNotRunning(err)).toBe(true)
    expect(isDaemonNotRunning({ kind: 'daemon', message: 'x' })).toBe(false)
    expect(errorMessage(err)).toBe('kakemono daemon is not running')
    expect(errorMessage(new Error('boom'))).toBe('boom')
  })
})
