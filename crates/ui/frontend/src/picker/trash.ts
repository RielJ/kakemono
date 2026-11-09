/**
 * Deferred trash with undo. The picker waits out the undo window, then sends `trash` with
 * `expected` = the deleted path: the daemon trashes only if it is still current, checked under
 * its op lock, so rotation or tray Next during the wait can never trash the new wallpaper.
 * The `status` read before it only turns the common "it changed" case into a friendly message
 * instead of the daemon's error.
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
  trash: (expected: string) => Promise<void>
}

export type CommitOutcome = { kind: 'trashed' } | { kind: 'changed' } | { kind: 'failed'; error: unknown }

/** Trashes `path` only if it is still the current wallpaper. */
export async function commitTrash(path: string, deps: TrashDeps): Promise<CommitOutcome> {
  try {
    const status = await deps.getStatus()
    if (status.current?.path !== path) return { kind: 'changed' }
    await deps.trash(path)
    return { kind: 'trashed' }
  } catch (error) {
    return { kind: 'failed', error }
  }
}
