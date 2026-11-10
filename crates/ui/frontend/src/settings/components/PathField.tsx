import { openFolder, selectDirectory } from '../../shared/api'
import Icon from './Icon'
import TextField from './TextField'

/** A folder path: typed, chosen with the system dialog, or opened in the file manager. */
export default function PathField({
  value,
  onCommit,
  placeholder,
  labelledBy,
  describedBy,
  fileManager,
  nullable = false,
}: {
  value: string
  onCommit: (value: string) => void
  placeholder?: string
  labelledBy?: string
  describedBy?: string
  /** "Finder", "Explorer" or "the file manager". */
  fileManager: string
  /** Empty means "default": hide Open when there is no path to open. */
  nullable?: boolean
}) {
  return (
    <div className="flex items-center gap-1.5">
      <TextField
        value={value}
        onCommit={onCommit}
        placeholder={placeholder}
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        mono
        className="w-56"
      />
      <button
        type="button"
        className="btn px-2.5"
        onClick={async () => {
          const p = await selectDirectory().catch(() => '')
          if (p) onCommit(p)
        }}
      >
        Choose…
      </button>
      {!(nullable && value === '') && (
        <button
          type="button"
          className="btn w-7 shrink-0 px-0"
          title={`Open in ${fileManager}`}
          aria-label={`Open in ${fileManager}`}
          disabled={value === ''}
          onClick={() => void openFolder(value).catch(() => {})}
        >
          <Icon name="folderOpen" className="w-4 h-4" />
        </button>
      )}
    </div>
  )
}
