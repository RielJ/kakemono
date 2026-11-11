import { useEffect, useState } from 'react'

import { errorMessage, getStartAtLogin, setStartAtLogin } from '../../shared/api'
import { Row } from './Group'
import { CheckLine, LineText } from './SettingLine'
import Toggle from './Toggle'

type State =
  | { kind: 'loading' }
  | { kind: 'ready'; enabled: boolean; busy: boolean; error: string | null }
  | { kind: 'unavailable'; message: string }

/**
 * "Start at login": the daemon's login service plus the UI's autostart entry, switched
 * together by the backend (`set_start_at_login`). Not part of the config file, so it applies
 * immediately rather than on Save. `variant="line"` draws it as a General-tab checkbox line
 * instead of a switch row.
 */
export default function StartAtLogin({ variant = 'row' }: { variant?: 'row' | 'line' } = {}) {
  const [state, setState] = useState<State>({ kind: 'loading' })

  useEffect(() => {
    let live = true
    getStartAtLogin()
      .then((items) => live && setState({ kind: 'ready', enabled: items.enabled, busy: false, error: null }))
      .catch((err: unknown) => live && setState({ kind: 'unavailable', message: errorMessage(err) }))
    return () => {
      live = false
    }
  }, [])

  const change = async (enabled: boolean) => {
    if (state.kind !== 'ready' || state.busy) return
    const before = state.enabled
    setState({ kind: 'ready', enabled, busy: true, error: null })
    try {
      const items = await setStartAtLogin(enabled)
      setState({ kind: 'ready', enabled: items.enabled, busy: false, error: null })
    } catch (err) {
      const error = errorMessage(err)
      setState({ kind: 'ready', enabled: before, busy: false, error })
      // A failed undo can leave one half on; show what is really there.
      getStartAtLogin()
        .then((items) => setState({ kind: 'ready', enabled: items.enabled, busy: false, error }))
        .catch(() => {})
    }
  }

  const error = state.kind === 'unavailable' ? state.message : state.kind === 'ready' ? state.error : null
  const hint = 'Starts the wallpaper daemon and the kakemono tray when you log in. Applies right away.'
  if (variant === 'line') {
    return (
      <CheckLine
        checked={state.kind === 'ready' && state.enabled}
        onChange={(v) => void change(v)}
        disabled={state.kind !== 'ready' || state.busy}
        hint={hint}
        error={error}
      >
        <LineText>Start at login</LineText>
      </CheckLine>
    )
  }
  return (
    <Row label="Start at login" hint={hint} error={error}>
      {({ labelId, hintId }) => (
        <Toggle
          enabled={state.kind === 'ready' && state.enabled}
          onChange={(v) => void change(v)}
          labelledBy={labelId}
          describedBy={hintId}
          disabled={state.kind !== 'ready' || state.busy}
        />
      )}
    </Row>
  )
}
