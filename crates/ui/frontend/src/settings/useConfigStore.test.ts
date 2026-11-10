import { act, renderHook } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { Config, ConfigPatch, SaveOutcome } from '../shared/api'
import { testConfig } from './testConfig'
import { useConfigStore } from './useConfigStore'

const ok: SaveOutcome = { reloaded: true, reload_error: null }

const withInterval = (c: Config, interval: string): Config => ({ ...c, daemon: { ...c.daemon, interval } })
const withRotation = (c: Config, on: boolean): Config => ({ ...c, daemon: { ...c.daemon, rotation_enabled: on } })

function setup(save: (p: ConfigPatch) => Promise<SaveOutcome>) {
  const hook = renderHook(() => useConfigStore({ save, delayMs: 100 }))
  act(() => hook.result.current.load(testConfig()))
  return hook
}

beforeEach(() => vi.useFakeTimers())
afterEach(() => vi.useRealTimers())

describe('useConfigStore', () => {
  it('writes an edit after the delay, as a minimal patch, and merges quick edits', async () => {
    const save = vi.fn().mockResolvedValue(ok)
    const { result } = setup(save)
    act(() => result.current.change(withRotation(result.current.config!, false)))
    act(() => result.current.change(withInterval(result.current.config!, '2h')))
    expect(save).not.toHaveBeenCalled()

    await act(async () => vi.advanceTimersByTimeAsync(100))
    expect(save).toHaveBeenCalledTimes(1)
    expect(save).toHaveBeenCalledWith({ daemon: { rotation_enabled: false, interval: '2h' } })
    expect(result.current.status).toEqual({ kind: 'saved', outcome: ok })
    expect(result.current.savedConfig?.daemon.interval).toBe('2h')
  })

  it('holds an invalid edit back and writes once it is fixed', async () => {
    const save = vi.fn().mockResolvedValue(ok)
    const { result } = setup(save)
    act(() => result.current.change(withInterval(result.current.config!, 'soon')))
    await act(async () => vi.advanceTimersByTimeAsync(100))
    expect(save).not.toHaveBeenCalled()
    expect(result.current.status.kind).toBe('invalid')

    act(() => result.current.change(withInterval(result.current.config!, '45m')))
    await act(async () => vi.advanceTimersByTimeAsync(100))
    expect(save).toHaveBeenCalledWith({ daemon: { interval: '45m' } })
  })

  it('runs one write at a time and sends edits made during it afterwards', async () => {
    let finish: (o: SaveOutcome) => void = () => {}
    const save = vi
      .fn<(p: ConfigPatch) => Promise<SaveOutcome>>()
      .mockImplementationOnce(() => new Promise((r) => (finish = r)))
      .mockResolvedValue(ok)
    const { result } = setup(save)

    act(() => result.current.change(withInterval(result.current.config!, '2h')))
    await act(async () => vi.advanceTimersByTimeAsync(100))
    act(() => result.current.change(withRotation(result.current.config!, false)))
    await act(async () => vi.advanceTimersByTimeAsync(100))
    expect(save).toHaveBeenCalledTimes(1)

    await act(async () => finish(ok))
    expect(save).toHaveBeenCalledTimes(2)
    expect(save).toHaveBeenLastCalledWith({ daemon: { rotation_enabled: false } })
  })

  it('keeps a failed edit on screen; revert restores what is on disk', async () => {
    const save = vi.fn().mockRejectedValue(new Error('disk full'))
    const { result } = setup(save)
    act(() => result.current.change(withInterval(result.current.config!, '2h')))
    await act(async () => vi.advanceTimersByTimeAsync(100))
    expect(result.current.status).toEqual({ kind: 'failed', message: 'disk full' })
    expect(result.current.config?.daemon.interval).toBe('2h')

    act(() => result.current.revert())
    expect(result.current.config?.daemon.interval).toBe(testConfig().daemon.interval)
    expect(result.current.status.kind).toBe('idle')
  })

  it('applies an outside write to both copies without writing it again', async () => {
    const save = vi.fn().mockResolvedValue(ok)
    const { result } = setup(save)
    act(() => result.current.written((c) => ({ ...c, picker: { ...c.picker, placement: 'edge', custom: null } })))
    await act(async () => vi.advanceTimersByTimeAsync(500))
    expect(save).not.toHaveBeenCalled()
    expect(result.current.savedConfig?.picker.placement).toBe('edge')
  })
})

describe('useConfigStore commit', () => {
  /** Settles a promise from outside `act`, so a rejection doesn't go unhandled. */
  const settle = (p: Promise<void>) => p.then(() => 'resolved' as const, (e: unknown) => e)

  it('writes at once, without the delay, and resolves after the save with the patch', async () => {
    const save = vi.fn().mockResolvedValue(ok)
    const { result } = setup(save)
    let done: unknown
    await act(async () => {
      done = await settle(result.current.commit(withInterval(result.current.config!, '2h')))
    })
    expect(done).toBe('resolved')
    expect(save).toHaveBeenCalledTimes(1)
    expect(save).toHaveBeenCalledWith({ daemon: { interval: '2h' } })
    expect(result.current.savedConfig?.daemon.interval).toBe('2h')
    // Nothing is written again when the delay passes.
    await act(async () => vi.advanceTimersByTimeAsync(500))
    expect(save).toHaveBeenCalledTimes(1)
  })

  it('waits for a write already in flight, then writes the committed config', async () => {
    let finish: (o: SaveOutcome) => void = () => {}
    const save = vi
      .fn<(p: ConfigPatch) => Promise<SaveOutcome>>()
      .mockImplementationOnce(() => new Promise((r) => (finish = r)))
      .mockResolvedValue(ok)
    const { result } = setup(save)
    act(() => result.current.change(withInterval(result.current.config!, '2h')))
    await act(async () => vi.advanceTimersByTimeAsync(100))
    expect(save).toHaveBeenCalledTimes(1)

    let state = 'pending'
    let pending!: Promise<unknown>
    act(() => {
      pending = settle(result.current.commit(withRotation(result.current.config!, false))).then((s) => (state = String(s)))
    })
    await act(async () => vi.advanceTimersByTimeAsync(0))
    expect(state).toBe('pending')
    expect(save).toHaveBeenCalledTimes(1)

    await act(async () => {
      finish(ok)
      await pending
    })
    expect(state).toBe('resolved')
    expect(save).toHaveBeenCalledTimes(2)
    expect(save).toHaveBeenLastCalledWith({ daemon: { rotation_enabled: false } })
  })

  it('rejects with the message when the save fails', async () => {
    const save = vi.fn().mockRejectedValue({ kind: 'io', message: 'disk full' })
    const { result } = setup(save)
    let done: unknown
    await act(async () => {
      done = await settle(result.current.commit(withInterval(result.current.config!, '2h')))
    })
    expect(done).toBeInstanceOf(Error)
    expect((done as Error).message).toBe('disk full')
    expect(result.current.status).toEqual({ kind: 'failed', message: 'disk full' })
    // The edit stays on screen for Retry or Undo.
    expect(result.current.config?.daemon.interval).toBe('2h')
  })

  it('rejects without writing when the config is invalid', async () => {
    const save = vi.fn().mockResolvedValue(ok)
    const { result } = setup(save)
    let done: unknown
    await act(async () => {
      done = await settle(result.current.commit(withInterval(result.current.config!, 'soon')))
    })
    expect(done).toBeInstanceOf(Error)
    expect((done as Error).message).toMatch(/highlighted field/)
    expect(save).not.toHaveBeenCalled()
    expect(result.current.status.kind).toBe('invalid')
  })

  it('resolves for a config already on disk without writing', async () => {
    const save = vi.fn().mockResolvedValue(ok)
    const { result } = setup(save)
    let done: unknown
    await act(async () => {
      done = await settle(result.current.commit({ ...result.current.config! }))
    })
    expect(done).toBe('resolved')
    expect(save).not.toHaveBeenCalled()
  })
})
