import { act, cleanup, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import * as api from '../shared/api'
import type { BrowseListing, BrowseSourceStatus, RemoteThumb } from '../shared/api'
import { INTERNET_OFF, MAX_ITEMS, RESUME_MS, useBrowse, type BrowseMemory } from './useBrowse'

vi.mock('../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../shared/api')>()
  return { ...real, browsePage: vi.fn() }
})

const browsePage = vi.mocked(api.browsePage)

function item(n: number, sourceId = 'wh'): RemoteThumb {
  return {
    sourceId,
    kind: 'wallhaven',
    remoteId: `r${n}`,
    title: `T${n}`,
    author: 'A',
    width: null,
    height: null,
    nsfw: false,
    imageUrl: `thumb://localhost/${n}`,
  }
}

const range = (from: number, n: number, sourceId = 'wh') =>
  Array.from({ length: n }, (_, i) => item(from + i, sourceId))

const ACTIVE = (source_id: string, kind = 'wallhaven'): BrowseSourceStatus => ({
  source_id,
  kind,
  state: { state: 'active' },
})

function listing(over: Partial<BrowseListing> = {}): BrowseListing {
  return { session: 's1', page: 0, restarted: false, done: false, items: [], sources: [ACTIVE('wh')], ...over }
}

/** Answers page `p` of session `s1` with items `10p…10p+9`. */
function pages(lastPage = Infinity) {
  browsePage.mockImplementation(async ({ page }) =>
    listing({ page, items: range(page * 10, 10), done: page >= lastPage }),
  )
}

/** A promise the test resolves by hand. */
function deferred<T>() {
  let resolve!: (v: T) => void
  let reject!: (e: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

beforeEach(() => {
  browsePage.mockReset()
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('useBrowse', () => {
  it('loads page 0, then page 1 on loadMore, collapsing concurrent calls', async () => {
    pages()
    const { result } = renderHook(() => useBrowse({ memory: { current: null } }))
    expect(result.current.loading).toBe(true)
    await waitFor(() => expect(result.current.items).toHaveLength(10))
    expect(browsePage).toHaveBeenCalledExactlyOnceWith({ session: null, page: 0, source: null })
    expect(result.current.loading).toBe(false)

    act(() => {
      result.current.loadMore()
      result.current.loadMore()
      result.current.loadMore()
    })
    expect(result.current.loadingMore).toBe(true)
    await waitFor(() => expect(result.current.items).toHaveLength(20))
    expect(browsePage).toHaveBeenCalledTimes(2)
    expect(browsePage).toHaveBeenLastCalledWith({ session: 's1', page: 1, source: null })
    expect(result.current.items[10].remoteId).toBe('r10')
  })

  it('stops at done and replaces the list when the session restarted', async () => {
    pages(1)
    const { result } = renderHook(() => useBrowse({ memory: { current: null } }))
    await waitFor(() => expect(result.current.items).toHaveLength(10))
    act(() => result.current.loadMore())
    await waitFor(() => expect(result.current.done).toBe(true))
    act(() => result.current.loadMore())
    expect(browsePage).toHaveBeenCalledTimes(2)

    // A session that expired comes back restarted at page 0, with new items.
    pages()
    browsePage.mockResolvedValueOnce(listing({ session: 's2', page: 0, restarted: true, items: range(100, 10) }))
    const { result: second } = renderHook(() => useBrowse({ memory: { current: null } }))
    await waitFor(() => expect(second.current.items).toHaveLength(10))
    browsePage.mockResolvedValueOnce(listing({ session: 's3', page: 0, restarted: true, items: range(200, 3) }))
    act(() => second.current.loadMore())
    await waitFor(() => expect(second.current.items.map((i) => i.remoteId)).toEqual(['r200', 'r201', 'r202']))
    // The next page continues the new session.
    act(() => second.current.loadMore())
    await waitFor(() => expect(browsePage).toHaveBeenLastCalledWith({ session: 's3', page: 1, source: null }))
  })

  it('skips an item a later page repeats', async () => {
    browsePage
      .mockResolvedValueOnce(listing({ items: range(0, 3) }))
      .mockResolvedValueOnce(listing({ page: 1, items: [item(2), item(3)] }))
    const { result } = renderHook(() => useBrowse({ memory: { current: null } }))
    await waitFor(() => expect(result.current.items).toHaveLength(3))
    act(() => result.current.loadMore())
    await waitFor(() => expect(result.current.items).toHaveLength(4))
  })

  it('starts a new session for another source filter, and cycles the filter', async () => {
    browsePage.mockImplementation(async ({ source }) =>
      listing({
        session: source ?? 'all',
        items: [item(1, source ?? 'wh')],
        sources: source ? [ACTIVE(source)] : [ACTIVE('wh'), ACTIVE('bing', 'bing'), ACTIVE('rd', 'reddit')],
      }),
    )
    const { result } = renderHook(() => useBrowse({ memory: { current: null } }))
    await waitFor(() => expect(result.current.sources).toHaveLength(3))
    act(() => result.current.setFilter('bing'))
    expect(result.current.filter).toBe('bing')
    await waitFor(() => expect(browsePage).toHaveBeenLastCalledWith({ session: null, page: 0, source: 'bing' }))
    await waitFor(() => expect(result.current.items[0].sourceId).toBe('bing'))
    // The filter keeps listing every source, not just the filtered one.
    expect(result.current.sources.map((s) => s.source_id)).toEqual(['wh', 'bing', 'rd'])

    act(() => result.current.cycleFilter(1))
    expect(result.current.filter).toBe('rd')
    act(() => result.current.cycleFilter(1))
    expect(result.current.filter).toBeNull()
    act(() => result.current.cycleFilter(-1))
    expect(result.current.filter).toBe('rd')
    await waitFor(() => expect(browsePage).toHaveBeenLastCalledWith({ session: null, page: 0, source: 'rd' }))
  })

  it('has no filter to cycle with a single source', async () => {
    pages()
    const { result } = renderHook(() => useBrowse({ memory: { current: null } }))
    await waitFor(() => expect(result.current.items).toHaveLength(10))
    act(() => result.current.cycleFilter(1))
    expect(result.current.filter).toBeNull()
    expect(browsePage).toHaveBeenCalledTimes(1)
  })

  it('drops a page that a filter change made stale', async () => {
    const slow = deferred<BrowseListing>()
    browsePage.mockReturnValueOnce(slow.promise).mockResolvedValue(listing({ session: 'b', items: [item(9, 'bing')] }))
    const { result } = renderHook(() => useBrowse({ memory: { current: null } }))
    act(() => result.current.setFilter('bing'))
    await waitFor(() => expect(result.current.items).toHaveLength(1))
    await act(async () => slow.resolve(listing({ items: range(0, 10) })))
    expect(result.current.items.map((i) => i.sourceId)).toEqual(['bing'])
  })

  it('surfaces errors: internet off, a failed first page, and a failed later page', async () => {
    browsePage.mockRejectedValueOnce({ kind: 'daemon', message: 'Internet access is off' })
    const { result } = renderHook(() => useBrowse({ memory: { current: null } }))
    await waitFor(() => expect(result.current.error).toEqual({ kind: 'internet_off', message: INTERNET_OFF }))

    browsePage.mockRejectedValueOnce({ kind: 'daemon', message: 'daemon error' })
    act(() => result.current.reload())
    await waitFor(() => expect(result.current.error).toEqual({ kind: 'failed', message: 'daemon error' }))
    expect(result.current.loading).toBe(false)

    browsePage.mockResolvedValueOnce(listing({ items: range(0, 10) }))
    act(() => result.current.reload())
    await waitFor(() => expect(result.current.items).toHaveLength(10))
    expect(result.current.error).toBeNull()

    browsePage.mockRejectedValueOnce({ kind: 'daemon_unreachable', message: 'gone' })
    act(() => result.current.loadMore())
    await waitFor(() => expect(result.current.moreError).toBe('gone'))
    // Kept items stay, and a retry continues from the same page.
    expect(result.current.items).toHaveLength(10)
    browsePage.mockResolvedValueOnce(listing({ page: 1, items: range(10, 10) }))
    act(() => result.current.loadMore())
    await waitFor(() => expect(result.current.items).toHaveLength(20))
    expect(result.current.moreError).toBeNull()
    expect(browsePage).toHaveBeenLastCalledWith({ session: 's1', page: 1, source: null })
  })

  it('surfaces per-source states', async () => {
    const limited: BrowseSourceStatus = {
      source_id: 'un',
      kind: 'unsplash',
      state: { state: 'rate_limited', message: 'Unsplash rate limit reached', retry_after_secs: 600 },
    }
    browsePage.mockResolvedValueOnce(listing({ items: range(0, 2), sources: [ACTIVE('wh'), limited] }))
    const { result } = renderHook(() => useBrowse({ memory: { current: null } }))
    await waitFor(() => expect(result.current.statuses).toEqual([ACTIVE('wh'), limited]))
  })

  it('stops at 300 items until Reload starts a new session', async () => {
    pages()
    const { result } = renderHook(() => useBrowse({ memory: { current: null } }))
    await waitFor(() => expect(result.current.items).toHaveLength(10))
    for (let p = 1; p < MAX_ITEMS / 10; p++) {
      act(() => result.current.loadMore())
      await waitFor(() => expect(result.current.items).toHaveLength((p + 1) * 10))
    }
    expect(result.current.capped).toBe(true)
    act(() => result.current.loadMore())
    expect(browsePage).toHaveBeenCalledTimes(MAX_ITEMS / 10)

    act(() => result.current.reload())
    await waitFor(() => expect(result.current.items).toHaveLength(10))
    expect(result.current.capped).toBe(false)
    expect(browsePage).toHaveBeenLastCalledWith({ session: null, page: 0, source: null })
  })

  it('resumes a remembered session younger than 10 minutes, with its filter', async () => {
    pages()
    const memory: { current: BrowseMemory | null } = { current: null }
    const first = renderHook(() => useBrowse({ memory }))
    await waitFor(() => expect(first.result.current.items).toHaveLength(10))
    expect(memory.current).toMatchObject({ session: 's1', filter: null })
    first.unmount()

    const second = renderHook(() => useBrowse({ memory }))
    await waitFor(() => expect(second.result.current.items).toHaveLength(10))
    expect(browsePage).toHaveBeenLastCalledWith({ session: 's1', page: 0, source: null })
    second.unmount()

    // Too old: a new session.
    if (memory.current) memory.current = { ...memory.current, at: Date.now() - RESUME_MS - 1 }
    const third = renderHook(() => useBrowse({ memory }))
    await waitFor(() => expect(third.result.current.items).toHaveLength(10))
    expect(browsePage).toHaveBeenLastCalledWith({ session: null, page: 0, source: null })
  })
})
