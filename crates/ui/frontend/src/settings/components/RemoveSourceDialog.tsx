import { useId, useState } from 'react'

import type { SourceEntry } from '../../shared/bindings/SourceEntry'
import { SOURCE_TYPES, isOnline } from '../sourceTypes'
import Modal from './Modal'

/** "Wallhaven: nature", "Bing", "Folder: ~/Pictures". */
export function sourceName(s: SourceEntry): string {
  const label = SOURCE_TYPES[s.type].label
  return s.location ? `${label}: ${s.location}` : label
}

/**
 * Remove… confirmation for one or more rows. When any row is online it offers "Also delete
 * downloaded files" (`downloads/<id>/`), off by default. Local rows never delete user files.
 */
export default function RemoveSourceDialog({
  sources,
  onConfirm,
  onCancel,
}: {
  sources: readonly SourceEntry[]
  onConfirm: (deleteFiles: boolean) => void
  onCancel: () => void
}) {
  const [deleteFiles, setDeleteFiles] = useState(false)
  const bodyId = useId()
  const online = sources.some((s) => isOnline(s.type))
  const local = sources.some((s) => !isOnline(s.type))
  const title = sources.length === 1 ? `Remove “${sourceName(sources[0])}”?` : `Remove ${sources.length} sources?`

  return (
    <Modal title={title} onClose={onCancel} describedBy={bodyId}>
      <div id={bodyId} className="mt-1.5 space-y-1 text-muted">
        <p>Kakemono will stop showing images from {sources.length === 1 ? 'this source' : 'these sources'}.</p>
        {local && <p>Files in the folder stay where they are.</p>}
      </div>
      {online && (
        <label className="mt-3 flex items-center gap-2">
          <input type="checkbox" checked={deleteFiles} onChange={(e) => setDeleteFiles(e.target.checked)} />
          Also delete downloaded files
        </label>
      )}
      <div className="mt-4 flex justify-end gap-2">
        <button type="button" className="btn" onClick={onCancel}>
          Cancel
        </button>
        <button type="button" className="btn btn-danger" data-autofocus onClick={() => onConfirm(online && deleteFiles)}>
          Remove
        </button>
      </div>
    </Modal>
  )
}
