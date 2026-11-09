import { describe, expect, it, vi } from 'vitest'
import type { StatusData } from '../shared/api'
import { commitTrash } from './trash'

function status(path: string | null): StatusData {
  return {
    current:
      path === null
        ? null
        : { path, source: 'local', source_url: '', image_url: '', title: '', author: '', width: 0, height: 0, set_at: '' },
    paused: false,
    interval: '30m',
    theme: 'dark',
    history_count: 1,
    history_index: 0,
  }
}

describe('commitTrash', () => {
  it('trashes when the deleted wallpaper is still current', async () => {
    const trash = vi.fn().mockResolvedValue(undefined)
    const out = await commitTrash('/w/a.jpg', { getStatus: async () => status('/w/a.jpg'), trash })
    expect(out).toEqual({ kind: 'trashed' })
    expect(trash).toHaveBeenCalledOnce()
  })

  it('does not trash a wallpaper that rotated in during the undo window', async () => {
    const trash = vi.fn()
    const out = await commitTrash('/w/a.jpg', { getStatus: async () => status('/w/b.jpg'), trash })
    expect(out).toEqual({ kind: 'changed' })
    expect(trash).not.toHaveBeenCalled()
  })

  it('does not trash when nothing is current', async () => {
    const trash = vi.fn()
    expect(await commitTrash('/w/a.jpg', { getStatus: async () => status(null), trash })).toEqual({ kind: 'changed' })
    expect(trash).not.toHaveBeenCalled()
  })

  it('does not trash when status fails', async () => {
    const trash = vi.fn()
    const error = { kind: 'daemon_not_running', message: 'down' }
    const out = await commitTrash('/w/a.jpg', { getStatus: () => Promise.reject(error), trash })
    expect(out).toEqual({ kind: 'failed', error })
    expect(trash).not.toHaveBeenCalled()
  })
})
