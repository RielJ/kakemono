import { useCallback, useEffect, useRef, useState } from 'react'

import type { Config, ConfigPatch, SaveOutcome } from '../shared/api'
import { errorMessage } from '../shared/api'
import { diffConfig, isEmptyPatch } from './configPatch'
import { configErrors, hasErrors } from './validate'

/** What the window says about saving. Changes apply on their own; there is no Save button. */
export type SaveStatus =
  | { kind: 'idle' }
  | { kind: 'saving' }
  | { kind: 'saved'; outcome: SaveOutcome }
  /** An edit is held back until the highlighted field is fixed. */
  | { kind: 'invalid' }
  | { kind: 'failed'; message: string }

/** Delay that merges quick successive edits (several checkboxes, a slider drag) into one write. */
export const SAVE_DELAY_MS = 250

interface Options {
  save: (patch: ConfigPatch) => Promise<SaveOutcome>
  /** Called after each successful write. */
  onSaved?: (outcome: SaveOutcome) => void
  delayMs?: number
}

/**
 * The settings window's config: the copy on screen (`config`) and the copy on disk
 * (`savedConfig`). Every `change` is written shortly after, one write at a time; an edit that
 * fails validation stays on screen and is not written until it is fixed.
 */
export function useConfigStore({ save, onSaved, delayMs = SAVE_DELAY_MS }: Options) {
  const [config, setConfig] = useState<Config | null>(null)
  const [savedConfig, setSavedConfig] = useState<Config | null>(null)
  const [status, setStatus] = useState<SaveStatus>({ kind: 'idle' })

  // Refs hold the latest values for the async write loop; state mirrors them for rendering.
  const current = useRef<Config | null>(null)
  const saved = useRef<Config | null>(null)
  const inFlight = useRef(false)
  // Settles when the running write finishes (success or failure); null when none is running.
  const writing = useRef<Promise<void> | null>(null)
  // Why the last write failed; null after a success.
  const lastFailure = useRef<string | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const callbacks = useRef({ save, onSaved })
  callbacks.current = { save, onSaved }

  const clearTimer = () => {
    if (timer.current !== null) clearTimeout(timer.current)
    timer.current = null
  }

  const flush = useCallback(async (): Promise<void> => {
    clearTimer()
    if (inFlight.current) return // The running write re-checks when it finishes.
    const next = current.current
    const base = saved.current
    if (!next || !base) return
    const patch = diffConfig(base, next)
    if (isEmptyPatch(patch)) {
      setStatus((s) => (s.kind === 'invalid' ? { kind: 'idle' } : s))
      return
    }
    if (hasErrors(configErrors(next))) {
      setStatus({ kind: 'invalid' })
      return
    }
    inFlight.current = true
    let release = () => {}
    writing.current = new Promise<void>((r) => (release = r))
    setStatus({ kind: 'saving' })
    let ok = false
    try {
      const outcome = await callbacks.current.save(patch)
      saved.current = next
      lastFailure.current = null
      setSavedConfig(next)
      setStatus({ kind: 'saved', outcome })
      callbacks.current.onSaved?.(outcome)
      ok = true
    } catch (err) {
      lastFailure.current = errorMessage(err)
      setStatus({ kind: 'failed', message: lastFailure.current })
    } finally {
      inFlight.current = false
      writing.current = null
      release()
    }
    // Edits made during the write go out next. After a failure, wait for a new edit or Retry.
    if (ok && current.current !== next) await flush()
  }, [])

  const schedule = useCallback(() => {
    clearTimer()
    timer.current = setTimeout(() => void flush(), delayMs)
  }, [delayMs, flush])

  useEffect(() => clearTimer, [])

  /** A config just read from disk: shown as-is, nothing to write. */
  const load = useCallback((cfg: Config) => {
    clearTimer()
    current.current = cfg
    saved.current = cfg
    setConfig(cfg)
    setSavedConfig(cfg)
    setStatus({ kind: 'idle' })
  }, [])

  /** An edit: shown now, written after `delayMs`. */
  const change = useCallback(
    (next: Config) => {
      current.current = next
      setConfig(next)
      schedule()
    },
    [schedule],
  )

  /**
   * An edit that must be on disk before the caller acts on it (Use, then change now; Remove,
   * then delete the source's files). Shown now and written at once, after any write in flight
   * and the follow-up it starts. Resolves once the saved config matches `next`; rejects with
   * the reason when the write fails or a field is invalid. `change` still applies afterwards:
   * a failed edit stays on screen with Retry and Undo.
   */
  const commit = useCallback(
    async (next: Config): Promise<void> => {
      clearTimer()
      current.current = next
      setConfig(next)
      const settled = async () => {
        // A finishing write may start its follow-up at once, so wait until none is running.
        while (writing.current) await writing.current
      }
      const isSaved = () => saved.current !== null && isEmptyPatch(diffConfig(saved.current, next))
      await settled()
      if (isSaved()) return
      await flush()
      await settled()
      if (isSaved()) return
      const shown = current.current
      if (shown && hasErrors(configErrors(shown))) {
        throw new Error('Fix the highlighted field before this change can be saved.')
      }
      throw new Error(lastFailure.current ?? 'The change was not saved.')
    },
    [flush],
  )

  /** A change another command already wrote to disk (Pin, detect --write, picker reset). */
  const written = useCallback((update: (c: Config) => Config) => {
    if (saved.current) saved.current = update(saved.current)
    if (current.current) current.current = update(current.current)
    setSavedConfig(saved.current)
    setConfig(current.current)
  }, [])

  /** Drop edits that were not written (after a failure or an invalid field). */
  const revert = useCallback(() => {
    clearTimer()
    current.current = saved.current
    setConfig(saved.current)
    setStatus({ kind: 'idle' })
  }, [])

  return { config, savedConfig, status, setStatus, load, change, commit, written, revert, flush }
}
