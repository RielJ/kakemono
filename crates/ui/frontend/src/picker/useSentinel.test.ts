import { cleanup, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { useSentinel } from './useSentinel'

/** A controllable stand-in for `IntersectionObserver` (jsdom has none). */
class FakeObserver {
  static instances: FakeObserver[] = []
  readonly targets = new Set<Element>()
  disconnected = false

  constructor(
    readonly callback: IntersectionObserverCallback,
    readonly options: IntersectionObserverInit = {},
  ) {
    FakeObserver.instances.push(this)
  }

  observe(target: Element) {
    this.targets.add(target)
  }

  unobserve(target: Element) {
    this.targets.delete(target)
  }

  disconnect() {
    this.disconnected = true
    this.targets.clear()
  }

  takeRecords(): IntersectionObserverEntry[] {
    return []
  }

  /** Reports `isIntersecting` for every observed target, as the browser would. */
  report(isIntersecting: boolean) {
    const entries = [...this.targets].map((target) => ({ target, isIntersecting }) as IntersectionObserverEntry)
    this.callback(entries, this as unknown as IntersectionObserver)
  }
}

/** The observer that is currently live. */
function live(): FakeObserver {
  const observer = FakeObserver.instances.findLast((o) => !o.disconnected)
  if (!observer) throw new Error('no live observer')
  return observer
}

const el = document.createElement('div')
const ref = { current: el }

beforeEach(() => {
  FakeObserver.instances = []
  vi.stubGlobal('IntersectionObserver', FakeObserver)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('useSentinel', () => {
  it('observes the sentinel with the root margin', () => {
    renderHook(() => useSentinel(ref, { onVisible: () => {}, busy: false, rootMargin: '0px 400px' }))
    expect([...live().targets]).toEqual([el])
    expect(live().options.rootMargin).toBe('0px 400px')
  })

  it('calls onVisible once per intersection', () => {
    const onVisible = vi.fn()
    renderHook(() => useSentinel(ref, { onVisible, busy: false }))
    live().report(false)
    expect(onVisible).not.toHaveBeenCalled()
    live().report(true)
    live().report(true)
    expect(onVisible).toHaveBeenCalledTimes(1)
    live().report(false)
    live().report(true)
    expect(onVisible).toHaveBeenCalledTimes(2)
  })

  it('does not call onVisible while busy, and re-checks when busy clears', () => {
    const onVisible = vi.fn()
    const { rerender } = renderHook(({ busy }) => useSentinel(ref, { onVisible, busy }), {
      initialProps: { busy: true },
    })
    expect(FakeObserver.instances.filter((o) => !o.disconnected)).toEqual([])
    expect(onVisible).not.toHaveBeenCalled()

    // The page rendered but the sentinel is still in view: the fresh observer's first report fires.
    rerender({ busy: false })
    live().report(true)
    expect(onVisible).toHaveBeenCalledTimes(1)
  })

  it('uses the latest onVisible without re-observing', () => {
    const first = vi.fn()
    const second = vi.fn()
    const { rerender } = renderHook(({ onVisible }) => useSentinel(ref, { onVisible, busy: false }), {
      initialProps: { onVisible: first },
    })
    rerender({ onVisible: second })
    expect(FakeObserver.instances).toHaveLength(1)
    live().report(true)
    expect(first).not.toHaveBeenCalled()
    expect(second).toHaveBeenCalledTimes(1)
  })

  it('disconnects on unmount', () => {
    const { unmount } = renderHook(() => useSentinel(ref, { onVisible: () => {}, busy: false }))
    const observer = live()
    unmount()
    expect(observer.disconnected).toBe(true)
  })

  it('does nothing without a sentinel element', () => {
    renderHook(() => useSentinel({ current: null }, { onVisible: () => {}, busy: false }))
    expect(FakeObserver.instances).toEqual([])
  })
})
