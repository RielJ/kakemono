import { act, cleanup, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import * as api from '../shared/api'
import type { SourceEntry } from '../shared/bindings/SourceEntry'
import { useSourceChecks } from './useSourceChecks'

vi.mock('../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../shared/api')>()
  return { ...real, checkSource: vi.fn() }
})

beforeEach(() => localStorage.clear())

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

const row = (id: string, type: SourceEntry['type'], location: string, name?: string): SourceEntry => ({
  id,
  type,
  location,
  enabled: true,
  ...(name ? { name } : {}),
})

function deferred<T>() {
  let resolve: (v: T) => void = () => {}
  let reject: (e: unknown) => void = () => {}
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

const flush = () => act(async () => {})

describe('useSourceChecks', () => {
  it('reports nothing for a good row and flags a bad one', async () => {
    vi.mocked(api.checkSource).mockImplementation(async (kind) => {
      if (kind === 'reddit') throw { kind: 'daemon', message: 'no images found' }
      if (kind === 'apod') return { count: 0 }
      return { count: 24 }
    })
    const sources = [row('wh', 'wallhaven', 'nature'), row('rd', 'reddit', 'zzzz'), row('apod', 'apod', '')]
    const { result } = renderHook(() => useSourceChecks(sources, true))
    expect(result.current.get('wh')).toEqual({ state: 'checking' })
    await flush()
    expect(result.current.has('wh')).toBe(false)
    expect(result.current.get('rd')).toEqual({ state: 'failed', message: 'no images found' })
    expect(result.current.get('apod')).toEqual({ state: 'failed', message: 'No images found.' })
  })

  it('never checks a good source again, even after the window reopens', async () => {
    vi.mocked(api.checkSource).mockResolvedValue({ count: 5 })
    const sources = [row('wh', 'wallhaven', 'nature')]
    renderHook(() => useSourceChecks(sources, true))
    await flush()
    cleanup()
    const { result } = renderHook(() => useSourceChecks(sources, true))
    await flush()
    expect(api.checkSource).toHaveBeenCalledTimes(1)
    expect(result.current.has('wh')).toBe(false)
  })

  it('checks a failed source again when the window reopens', async () => {
    vi.mocked(api.checkSource).mockRejectedValueOnce({ kind: 'daemon', message: 'timed out' })
    vi.mocked(api.checkSource).mockResolvedValueOnce({ count: 5 })
    const sources = [row('wh', 'wallhaven', 'nature')]
    renderHook(() => useSourceChecks(sources, true))
    await flush()
    cleanup()
    const { result } = renderHook(() => useSourceChecks(sources, true))
    await flush()
    expect(api.checkSource).toHaveBeenCalledTimes(2)
    expect(result.current.has('wh')).toBe(false)
  })

  it('does not check again when only the name changes', async () => {
    vi.mocked(api.checkSource).mockRejectedValue({ kind: 'daemon', message: 'bad' })
    const { rerender } = renderHook(({ s }) => useSourceChecks(s, true), {
      initialProps: { s: [row('wh', 'wallhaven', 'nature')] },
    })
    await flush()
    rerender({ s: [row('wh', 'wallhaven', 'nature', 'wallhaven-top')] })
    await flush()
    expect(api.checkSource).toHaveBeenCalledTimes(1)
  })

  it('checks again when the location changes, and drops the older answer', async () => {
    const first = deferred<{ count: number }>()
    vi.mocked(api.checkSource).mockReturnValueOnce(first.promise).mockResolvedValueOnce({ count: 0 })
    const { result, rerender } = renderHook(({ s }) => useSourceChecks(s, true), {
      initialProps: { s: [row('wh', 'wallhaven', 'nature')] },
    })
    rerender({ s: [row('wh', 'wallhaven', 'ocean')] })
    await flush()
    expect(result.current.get('wh')).toEqual({ state: 'failed', message: 'No images found.' })
    first.resolve({ count: 1 })
    await flush()
    expect(result.current.get('wh')).toEqual({ state: 'failed', message: 'No images found.' })
    expect(api.checkSource).toHaveBeenLastCalledWith('wallhaven', 'ocean')
  })

  it('leaves disabled rows unchecked, and checks a row once it is ticked', async () => {
    vi.mocked(api.checkSource).mockResolvedValue({ count: 5 })
    const off = { ...row('wh', 'wallhaven', 'nature'), enabled: false }
    const { result, rerender } = renderHook(({ s }) => useSourceChecks(s, true), {
      initialProps: { s: [off, row('dir', 'folder', '/p')] },
    })
    await flush()
    expect(api.checkSource).toHaveBeenCalledTimes(1)
    expect(api.checkSource).toHaveBeenCalledWith('folder', '/p')
    expect(result.current.has('wh')).toBe(false)
    rerender({ s: [{ ...off, enabled: true }, row('dir', 'folder', '/p')] })
    expect(result.current.get('wh')).toEqual({ state: 'checking' })
    await flush()
    expect(api.checkSource).toHaveBeenCalledTimes(2)
  })

  it('drops an answer that arrives after the row is unticked', async () => {
    const pending = deferred<{ count: number }>()
    vi.mocked(api.checkSource).mockReturnValue(pending.promise)
    const on = row('wh', 'wallhaven', 'nature')
    const { result, rerender } = renderHook(({ s }) => useSourceChecks(s, true), { initialProps: { s: [on] } })
    rerender({ s: [{ ...on, enabled: false }] })
    pending.reject({ kind: 'daemon', message: 'bad' })
    await flush()
    expect(result.current.has('wh')).toBe(false)
  })

  it('skips online rows while Internet access is off, and checks them when it comes back', async () => {
    vi.mocked(api.checkSource).mockResolvedValue({ count: 3 })
    const sources = [row('wh', 'wallhaven', 'nature'), row('dir', 'folder', '/p')]
    const { result, rerender } = renderHook(({ net }) => useSourceChecks(sources, net), {
      initialProps: { net: false },
    })
    await flush()
    expect(result.current.has('wh')).toBe(false)
    expect(api.checkSource).toHaveBeenCalledTimes(1)
    expect(api.checkSource).toHaveBeenCalledWith('folder', '/p')
    rerender({ net: true })
    await flush()
    expect(api.checkSource).toHaveBeenCalledTimes(2)
    expect(api.checkSource).toHaveBeenLastCalledWith('wallhaven', 'nature')
  })

  it('says it could not check when the daemon is not answering, and tries again next time', async () => {
    vi.mocked(api.checkSource).mockRejectedValue({ kind: 'daemon_unreachable', message: 'refused' })
    const sources = [row('dir', 'folder', '/p')]
    const { result } = renderHook(() => useSourceChecks(sources, true))
    await flush()
    expect(result.current.get('dir')).toEqual({
      state: 'unknown',
      message: 'Couldn’t check: kakemono’s background service isn’t answering.',
    })
    cleanup()
    renderHook(() => useSourceChecks(sources, true))
    await flush()
    expect(api.checkSource).toHaveBeenCalledTimes(2)
  })

  it('still works when the stored list is unreadable', async () => {
    localStorage.setItem('kakemono.goodSources', '{not json')
    vi.mocked(api.checkSource).mockResolvedValue({ count: 5 })
    const { result } = renderHook(() => useSourceChecks([row('wh', 'wallhaven', 'nature')], true))
    await flush()
    expect(result.current.has('wh')).toBe(false)
    expect(api.checkSource).toHaveBeenCalledTimes(1)
  })
})
