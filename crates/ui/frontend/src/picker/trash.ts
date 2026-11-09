/**
 * Deferred trash with undo. IPC `trash` takes no path: it trashes whatever is current *when
 * it arrives*. So the picker waits out the undo window, then re-reads `status` and sends
 * `trash` only if the wallpaper the user deleted is still the current one. Rotation or tray
 * Next during the wait cancels it instead of trashing the new wallpaper.
 *
 * Follow-up (protocol, not the picker): `trash` should take an optional expected path so the
 * daemon checks this atomically.
 */
import type { StatusData } from '../shared/api'

/** How long the undo toast stays, ms. */
export const UNDO_MS = 4000

export const MESSAGES = {
  trashed: 'Moved to Trash',
  undone: 'Restored',
  changed: 'Wallpaper changed; nothing was moved to Trash',
  notCurrent: 'Only the current wallpaper can be moved to Trash',
} as const

export interface TrashDeps {
  getStatus: () => Promise<StatusData>
  trash: () => Promise<void>
}

export type CommitOutcome = { kind: 'trashed' } | { kind: 'changed' } | { kind: 'failed'; error: unknown }

/** Sends `trash` only if `path` is still the current wallpaper. */
export async function commitTrash(path: string, deps: TrashDeps): Promise<CommitOutcome> {
  try {
    const status = await deps.getStatus()
    if (status.current?.path !== path) return { kind: 'changed' }
    await deps.trash()
    return { kind: 'trashed' }
  } catch (error) {
    return { kind: 'failed', error }
  }
}
