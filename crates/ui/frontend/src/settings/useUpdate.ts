import { useCallback, useEffect, useRef, useState } from 'react'

import { checkUpdate, errorMessage, installUpdate, openUpdatePage } from '../shared/api'
import type { UpdateInfo, UpdateProgress } from '../shared/api'

/** The settings window is hidden, not closed, so it checks again when shown, at most this often. */
export const RECHECK_MS = 60 * 60 * 1000

export type UpdateState =
  | { kind: 'none' }
  | { kind: 'available'; info: UpdateInfo }
  /** `progress` is `null` until the first report arrives. */
  | { kind: 'installing'; info: UpdateInfo; progress: UpdateProgress | null }
  | { kind: 'failed'; info: UpdateInfo; message: string }

export interface Update {
  state: UpdateState
  /** Downloads, installs and restarts. On success the app restarts, so the state stays `installing`. */
  install: () => void
  /** Hides the banner until a newer version than this one is out. */
  dismiss: () => void
  openPage: () => void
}

/**
 * Checks for a newer release when the settings window opens and when it is shown again. A
 * failed check stays quiet: no network is not something the user needs to hear about here.
 */
export function useUpdate(): Update {
  const [state, setState] = useState<UpdateState>({ kind: 'none' })
  const dismissed = useRef<string | null>(null)
  const lastCheck = useRef(0)
  // Not replaced by a check that finishes while an install runs.
  const busy = useRef(false)

  const check = useCallback(async () => {
    lastCheck.current = Date.now()
    try {
      const info = await checkUpdate()
      if (busy.current) return
      setState(info && info.version !== dismissed.current ? { kind: 'available', info } : { kind: 'none' })
    } catch (err) {
      console.warn('update check failed:', errorMessage(err))
    }
  }, [])

  useEffect(() => {
    void check()
    const onVisible = () => {
      if (document.visibilityState === 'visible' && Date.now() - lastCheck.current >= RECHECK_MS) void check()
    }
    document.addEventListener('visibilitychange', onVisible)
    return () => document.removeEventListener('visibilitychange', onVisible)
  }, [check])

  const install = useCallback(() => {
    if (state.kind !== 'available' && state.kind !== 'failed') return
    const { info } = state
    busy.current = true
    setState({ kind: 'installing', info, progress: null })
    installUpdate((progress) => setState({ kind: 'installing', info, progress })).catch((err: unknown) => {
      busy.current = false
      setState({ kind: 'failed', info, message: errorMessage(err) })
    })
  }, [state])

  const dismiss = useCallback(() => {
    if (state.kind === 'available' || state.kind === 'failed') dismissed.current = state.info.version
    setState({ kind: 'none' })
  }, [state])

  const openPage = useCallback(() => {
    openUpdatePage().catch((err: unknown) => console.warn('opening the release page failed:', errorMessage(err)))
  }, [])

  return { state, install, dismiss, openPage }
}
