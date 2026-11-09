/**
 * Live daemon updates, emitted by `src-tauri/src/events.rs` from one IPC `subscribe` stream.
 * See `.pi/contracts/ui.md` ("Live updates").
 */

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useEffect, useRef } from 'react'

import type { StatusData } from './bindings/StatusData'
import type { WallpaperInfo } from './bindings/WallpaperInfo'

export const CHANGED = 'wallpaper://changed'
export const PAUSED = 'wallpaper://paused'
export const CONNECTION = 'daemon://connection'

/** Mirrors Rust `events::Connection` (its JSON is pinned by `connection_json_matches_the_frontend_type`). */
export type Connection =
  | { state: 'connecting' }
  /** Subscribed; `status` was read just after subscribing (`null` if that failed). Sent on every reconnect. */
  | { state: 'connected'; status: StatusData | null }
  | { state: 'not_running' }
  | { state: 'error'; message: string }

export interface DaemonEventHandlers {
  onChanged?: (info: WallpaperInfo) => void
  onPaused?: (paused: boolean) => void
  /** Also called once on mount with the last known state (`daemon_connection`). */
  onConnection?: (connection: Connection) => void
}

/** The last connection state, for a window that opened after it was emitted. */
export function getDaemonConnection(): Promise<Connection> {
  return invoke<Connection>('daemon_connection')
}

/**
 * Listens for the three daemon events while the component is mounted. Handlers may change
 * on every render; the listeners are registered once.
 */
export function useDaemonEvents(handlers: DaemonEventHandlers): void {
  const ref = useRef(handlers)
  useEffect(() => {
    ref.current = handlers
  })

  useEffect(() => {
    let disposed = false
    let heardConnection = false
    const unlisteners: UnlistenFn[] = []
    const add = (p: Promise<UnlistenFn>) => {
      p.then((un) => (disposed ? un() : unlisteners.push(un))).catch(() => {})
    }

    add(listen<WallpaperInfo>(CHANGED, (e) => ref.current.onChanged?.(e.payload)))
    add(listen<boolean>(PAUSED, (e) => ref.current.onPaused?.(e.payload)))
    add(
      listen<Connection>(CONNECTION, (e) => {
        heardConnection = true
        ref.current.onConnection?.(e.payload)
      }),
    )
    // A live event that beats this reply is newer; don't overwrite it.
    getDaemonConnection()
      .then((c) => {
        if (!disposed && !heardConnection) ref.current.onConnection?.(c)
      })
      .catch(() => {})

    return () => {
      disposed = true
      unlisteners.splice(0).forEach((un) => un())
    }
  }, [])
}
