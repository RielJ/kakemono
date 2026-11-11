import type { UpdateProgress } from '../../shared/api'
import type { Update } from '../useUpdate'
import Icon from './Icon'

/** Whole percent downloaded, or `null` when the size is unknown. */
export function percent(progress: UpdateProgress | null): number | null {
  if (progress?.step !== 'downloading' || !progress.total) return null
  return Math.min(100, Math.floor((progress.downloaded / progress.total) * 100))
}

function progressText(progress: UpdateProgress | null): string {
  if (progress?.step === 'installing') return 'Installing… kakemono restarts when it’s done.'
  const pct = percent(progress)
  if (pct !== null) return `Downloading… ${pct}%`
  if (progress?.step === 'downloading') return `Downloading… ${(progress.downloaded / 2 ** 20).toFixed(1)} MB`
  return 'Downloading…'
}

/** A newer kakemono is out: install it here, or get it from the release page. */
export default function UpdateBanner({ update }: { update: Update }) {
  const { state } = update
  if (state.kind === 'none') return null
  const { info } = state
  const inApp = info.install === 'in_app'

  let detail: React.ReactNode
  let actions: React.ReactNode = null
  if (state.kind === 'installing') {
    const pct = percent(state.progress)
    detail = (
      <>
        <span>{progressText(state.progress)}</span>
        <span
          role="progressbar"
          aria-label="Update progress"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={pct ?? undefined}
          className="block h-1 mt-1.5 rounded-full bg-line overflow-hidden"
        >
          <span
            className={`block h-full bg-accent transition-[width] duration-200 ${pct === null ? 'w-1/3 animate-pulse' : ''}`}
            style={pct === null ? undefined : { width: `${pct}%` }}
          />
        </span>
      </>
    )
  } else {
    detail =
      state.kind === 'failed' ? (
        <span className="text-danger break-words">Update failed: {state.message}</span>
      ) : (
        <span>
          You have {info.current}.{inApp ? '' : ' Download the new version to update.'}
        </span>
      )
    actions = (
      <div className="flex items-center gap-1 shrink-0">
        <button type="button" className="btn-plain h-6 px-1.5 text-[12px]" onClick={update.openPage}>
          {inApp ? 'What’s new' : 'Download'}
        </button>
        <button type="button" className="btn-plain h-6 px-1.5 text-[12px]" onClick={update.dismiss}>
          Later
        </button>
        {inApp && (
          <button type="button" className="btn btn-accent h-6 px-2.5 text-[12px]" onClick={update.install}>
            {state.kind === 'failed' ? 'Try again' : 'Update'}
          </button>
        )}
      </div>
    )
  }

  return (
    <section
      aria-label="App update"
      aria-live="polite"
      className="mb-5 flex items-start gap-3 rounded-lg border border-line bg-[var(--c-accent-soft)] px-3 py-2.5 text-[12px]"
    >
      <Icon name="download" className="w-4 h-4 mt-0.5 shrink-0 text-accent" />
      <div className="flex-1 min-w-0">
        <p className="font-semibold text-ink">kakemono {info.version} is available</p>
        <div className="text-muted">{detail}</div>
      </div>
      {actions}
    </section>
  )
}
