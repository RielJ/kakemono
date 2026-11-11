import { act, cleanup, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import * as api from '../shared/api'
import type { UpdateInfo, UpdateProgress } from '../shared/api'
import { RECHECK_MS, useUpdate } from './useUpdate'

vi.mock('../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../shared/api')>()
  return { ...real, checkUpdate: vi.fn(), installUpdate: vi.fn(), openUpdatePage: vi.fn() }
})

const update: UpdateInfo = { version: '0.3.0', current: '0.2.0', install: 'in_app' }

function setVisibility(state: DocumentVisibilityState) {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state })
  document.dispatchEvent(new Event('visibilitychange'))
}

beforeEach(() => {
  vi.mocked(api.checkUpdate).mockResolvedValue(update)
  vi.mocked(api.openUpdatePage).mockResolvedValue()
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.clearAllMocks()
  setVisibility('visible')
})

describe('useUpdate', () => {
  it('reports a newer version', async () => {
    const { result } = renderHook(() => useUpdate())
    await waitFor(() => expect(result.current.state).toEqual({ kind: 'available', info: update }))
    expect(api.checkUpdate).toHaveBeenCalledTimes(1)
  })

  it('stays quiet when up to date or when the check fails', async () => {
    vi.mocked(api.checkUpdate).mockResolvedValueOnce(null)
    const first = renderHook(() => useUpdate())
    await waitFor(() => expect(api.checkUpdate).toHaveBeenCalledTimes(1))
    expect(first.result.current.state).toEqual({ kind: 'none' })

    vi.mocked(api.checkUpdate).mockRejectedValueOnce({ kind: 'update', message: 'offline' })
    const second = renderHook(() => useUpdate())
    await waitFor(() => expect(api.checkUpdate).toHaveBeenCalledTimes(2))
    expect(second.result.current.state).toEqual({ kind: 'none' })
  })

  it('checks again when the window comes back, at most once an hour', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    renderHook(() => useUpdate())
    await waitFor(() => expect(api.checkUpdate).toHaveBeenCalledTimes(1))

    setVisibility('hidden')
    setVisibility('visible')
    expect(api.checkUpdate).toHaveBeenCalledTimes(1)

    vi.advanceTimersByTime(RECHECK_MS)
    setVisibility('hidden')
    setVisibility('visible')
    await waitFor(() => expect(api.checkUpdate).toHaveBeenCalledTimes(2))
  })

  it('shows progress while installing', async () => {
    let report: (p: UpdateProgress) => void = () => {}
    vi.mocked(api.installUpdate).mockImplementation((onProgress) => {
      report = onProgress
      // Success restarts the app, so the call never settles.
      return new Promise(() => {})
    })
    const { result } = renderHook(() => useUpdate())
    await waitFor(() => expect(result.current.state.kind).toBe('available'))

    act(() => result.current.install())
    expect(result.current.state).toEqual({ kind: 'installing', info: update, progress: null })
    act(() => report({ step: 'downloading', downloaded: 50, total: 100 }))
    expect(result.current.state).toEqual({
      kind: 'installing',
      info: update,
      progress: { step: 'downloading', downloaded: 50, total: 100 },
    })
    act(() => report({ step: 'installing' }))
    expect(result.current.state).toMatchObject({ progress: { step: 'installing' } })
  })

  it('shows why an install failed and can try again', async () => {
    vi.mocked(api.installUpdate).mockRejectedValueOnce({ kind: 'update', message: 'signature mismatch' })
    const { result } = renderHook(() => useUpdate())
    await waitFor(() => expect(result.current.state.kind).toBe('available'))

    act(() => result.current.install())
    await waitFor(() =>
      expect(result.current.state).toEqual({ kind: 'failed', info: update, message: 'signature mismatch' }),
    )

    vi.mocked(api.installUpdate).mockReturnValueOnce(new Promise(() => {}))
    act(() => result.current.install())
    expect(result.current.state.kind).toBe('installing')
    expect(api.installUpdate).toHaveBeenCalledTimes(2)
  })

  it('hides the banner until a newer version comes out', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const { result } = renderHook(() => useUpdate())
    await waitFor(() => expect(result.current.state.kind).toBe('available'))

    act(() => result.current.dismiss())
    expect(result.current.state).toEqual({ kind: 'none' })

    // The same version again stays hidden; a newer one shows.
    vi.advanceTimersByTime(RECHECK_MS)
    setVisibility('hidden')
    setVisibility('visible')
    await waitFor(() => expect(api.checkUpdate).toHaveBeenCalledTimes(2))
    expect(result.current.state).toEqual({ kind: 'none' })

    const newer = { ...update, version: '0.4.0' }
    vi.mocked(api.checkUpdate).mockResolvedValueOnce(newer)
    vi.advanceTimersByTime(RECHECK_MS)
    setVisibility('hidden')
    setVisibility('visible')
    await waitFor(() => expect(result.current.state).toEqual({ kind: 'available', info: newer }))
  })

  it('opens the release page', async () => {
    const { result } = renderHook(() => useUpdate())
    act(() => result.current.openPage())
    await waitFor(() => expect(api.openUpdatePage).toHaveBeenCalledTimes(1))
  })
})
