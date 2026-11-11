import { act, cleanup, renderHook } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import * as api from '../shared/api'
import type { SourceEntry } from '../shared/bindings/SourceEntry'
import { useSourceChecks } from './useSourceChecks'

vi.mock('../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../shared/api')>()
  return { ...real, checkSource: vi.fn() }
})

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
  it('checks every row in the background and reports each result', async () => {
    vi.mocked(api.checkSource).mockImplementation(async (kind) => {
      if (kind === 'reddit') throw { kind: 'daemon', message: 'no images found' }
      return { count: 24 }
    })
    const sources = [row('wh', 'wallhaven', 'nature'), row('rd', 'reddit', 'zzzz'), row('bing', 'bing', '')]
    const { result } = renderHook(() => useSourceChecks(sources, true))
    expect(result.current.get('wh')).toEqual({ state: 'checking' })
    await flush()
    expect(result.current.get('wh')).toEqual({ state: 'ok', count: 24 })
    expect(result.current.get('rd')).toEqual({ state: 'failed', message: 'no images found' })
    expect(result.current.get('bing')).toEqual({ state: 'ok', count: 24 })
    expect(api.checkSource).toHaveBeenCalledWith('bing', '')
  })

  it('does not check again when only the name or the enabled box changes', async () => {
    vi.mocked(api.checkSource).mockResolvedValue({ count: 5 })
    const { rerender } = renderHook(({ s }) => useSourceChecks(s, true), {
      initialProps: { s: [row('wh', 'wallhaven', 'nature')] },
    })
    await flush()
    rerender({ s: [{ ...row('wh', 'wallhaven', 'nature', 'wallhaven-top'), enabled: false }] })
    await flush()
    expect(api.checkSource).toHaveBeenCalledTimes(1)
  })

  it('checks again when the location changes, and drops the older answer', async () => {
    const first = deferred<{ count: number }>()
    vi.mocked(api.checkSource).mockReturnValueOnce(first.promise).mockResolvedValueOnce({ count: 7 })
    const { result, rerender } = renderHook(({ s }) => useSourceChecks(s, true), {
      initialProps: { s: [row('wh', 'wallhaven', 'nature')] },
    })
    rerender({ s: [row('wh', 'wallhaven', 'ocean')] })
    await flush()
    expect(result.current.get('wh')).toEqual({ state: 'ok', count: 7 })
    first.resolve({ count: 1 })
    await flush()
    expect(result.current.get('wh')).toEqual({ state: 'ok', count: 7 })
    expect(api.checkSource).toHaveBeenLastCalledWith('wallhaven', 'ocean')
  })

  it('skips online rows while Internet access is off, and checks them when it comes back', async () => {
    vi.mocked(api.checkSource).mockResolvedValue({ count: 3 })
    const sources = [row('wh', 'wallhaven', 'nature'), row('dir', 'folder', '/p')]
    const { result, rerender } = renderHook(({ net }) => useSourceChecks(sources, net), {
      initialProps: { net: false },
    })
    await flush()
    expect(result.current.get('wh')).toEqual({ state: 'paused' })
    expect(api.checkSource).toHaveBeenCalledTimes(1)
    expect(api.checkSource).toHaveBeenCalledWith('folder', '/p')
    rerender({ net: true })
    await flush()
    expect(result.current.get('wh')).toEqual({ state: 'ok', count: 3 })
  })

  it('says it could not check when the daemon is not answering', async () => {
    vi.mocked(api.checkSource).mockRejectedValue({ kind: 'daemon_unreachable', message: 'refused' })
    const { result } = renderHook(() => useSourceChecks([row('dir', 'folder', '/p')], true))
    await flush()
    expect(result.current.get('dir')).toEqual({ state: 'unknown', message: 'Couldn’t check: kakemono’s background service isn’t answering.' })
  })
})
