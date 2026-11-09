import { cleanup, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { CHANGED, CONNECTION, PAUSED, useDaemonEvents, type Connection } from './useDaemonEvents'

type Handler = (e: { payload: unknown }) => void

const bus = vi.hoisted(() => ({
  handlers: new Map<string, Handler>(),
  unlistened: [] as string[],
  connection: { state: 'connecting' } as unknown,
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, handler: Handler) => {
    bus.handlers.set(name, handler)
    return () => {
      bus.handlers.delete(name)
      bus.unlistened.push(name)
    }
  }),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    if (cmd !== 'daemon_connection') throw new Error(`unexpected ${cmd}`)
    return bus.connection
  }),
}))

const emit = (name: string, payload: unknown) => bus.handlers.get(name)?.({ payload })

beforeEach(() => {
  bus.handlers.clear()
  bus.unlistened = []
  bus.connection = { state: 'not_running' }
})
afterEach(cleanup)

describe('useDaemonEvents', () => {
  it('delivers the last known state on mount, then live events', async () => {
    const onConnection = vi.fn()
    const onChanged = vi.fn()
    const onPaused = vi.fn()
    renderHook(() => useDaemonEvents({ onConnection, onChanged, onPaused }))

    await waitFor(() => expect(onConnection).toHaveBeenCalledWith({ state: 'not_running' }))
    await waitFor(() => expect(bus.handlers.size).toBe(3))

    const connected: Connection = { state: 'connected', status: null }
    emit(CONNECTION, connected)
    emit(CHANGED, { path: '/w/a.jpg' })
    emit(PAUSED, true)
    expect(onConnection).toHaveBeenLastCalledWith(connected)
    expect(onChanged).toHaveBeenCalledWith({ path: '/w/a.jpg' })
    expect(onPaused).toHaveBeenCalledWith(true)
  })

  it('uses the newest handlers without re-registering', async () => {
    const first = vi.fn()
    const second = vi.fn()
    const { rerender } = renderHook(({ fn }) => useDaemonEvents({ onPaused: fn }), {
      initialProps: { fn: first },
    })
    await waitFor(() => expect(bus.handlers.size).toBe(3))
    rerender({ fn: second })
    emit(PAUSED, false)
    expect(first).not.toHaveBeenCalled()
    expect(second).toHaveBeenCalledWith(false)
    expect(bus.unlistened).toEqual([])
  })

  it('stops listening on unmount', async () => {
    const { unmount } = renderHook(() => useDaemonEvents({}))
    await waitFor(() => expect(bus.handlers.size).toBe(3))
    unmount()
    expect(bus.unlistened.sort()).toEqual([CHANGED, PAUSED, CONNECTION].sort())
  })

  it('unlistens a listener that registers after unmount', async () => {
    const { unmount } = renderHook(() => useDaemonEvents({}))
    unmount() // before the listen() promises resolve
    await waitFor(() => expect(bus.unlistened).toHaveLength(3))
    expect(bus.handlers.size).toBe(0)
  })
})
