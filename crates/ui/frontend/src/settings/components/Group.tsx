import { useId, type ReactNode } from 'react'

/** A titled group of setting rows. */
export function Group({ title, children }: { title: string; children: ReactNode }) {
  const id = useId()
  return (
    <section aria-labelledby={id}>
      <h3 id={id} className="text-xs font-medium text-gray-500 dark:text-gray-400 mb-2">
        {title}
      </h3>
      <div className="rounded-lg divide-y divide-gray-100 dark:divide-gray-700/70 border border-gray-200 dark:border-gray-700 bg-white dark:bg-gray-800/60">
        {children}
      </div>
    </section>
  )
}

/**
 * One setting: label and hint on the left, the control on the right. `children` receives
 * the ids to wire `aria-labelledby` / `aria-describedby` on the control.
 */
export function Row({
  label,
  hint,
  error,
  children,
}: {
  label: string
  hint?: ReactNode
  /** Inline validation message, shown under the hint in the error colour. */
  error?: string | null
  children: (ids: { labelId: string; hintId: string | undefined }) => ReactNode
}) {
  const labelId = useId()
  const hintId = useId()
  const describes = hint !== undefined || error
  return (
    <div className="flex items-center justify-between px-4 py-3 gap-4">
      <div className="flex-1 min-w-0">
        <p id={labelId} className="text-sm font-medium">{label}</p>
        {describes && (
          <div id={hintId} className="mt-0.5 text-xs">
            {hint !== undefined && <p className="text-gray-500 dark:text-gray-400">{hint}</p>}
            {error && (
              <p role="alert" className="text-red-600 dark:text-red-400">
                {error}
              </p>
            )}
          </div>
        )}
      </div>
      <div className="shrink-0">{children({ labelId, hintId: describes ? hintId : undefined })}</div>
    </div>
  )
}
