import { useId, type ReactNode } from 'react'

/** Page heading for one sidebar section, with a one-line description of what it controls. */
export function SectionHeader({ id, title, description }: { id: string; title: string; description: string }) {
  return (
    <header className="mb-6">
      <h2 id={id} className="text-[20px] font-semibold tracking-[-0.015em] leading-tight">
        {title}
      </h2>
      <p className="mt-1 text-muted">{description}</p>
    </header>
  )
}

/**
 * A titled panel of setting rows. `footer` is a note under the panel, for context that
 * applies to the whole group.
 */
export function Group({
  title,
  footer,
  action,
  children,
}: {
  title?: string
  footer?: ReactNode
  /** A small control on the title line, e.g. "Open picker". */
  action?: ReactNode
  children: ReactNode
}) {
  const id = useId()
  return (
    <section aria-labelledby={title ? id : undefined} className="mt-7 first:mt-0">
      {(title || action) && (
        <div className="flex items-end justify-between gap-3 mb-2 px-1 min-h-6">
          {title && (
            <h3 id={id} className="text-[13px] font-semibold">
              {title}
            </h3>
          )}
          {action}
        </div>
      )}
      <div className="rounded-xl bg-panel border border-line divide-y divide-line">{children}</div>
      {footer && <div className="mt-2 px-1 text-[12px] text-muted">{footer}</div>}
    </section>
  )
}

/**
 * One setting: label and hint on the left, the control on the right. `children` receives
 * the ids to wire `aria-labelledby` / `aria-describedby` on the control. `stacked` puts a
 * wide control (a list, an editor) under the label instead.
 */
export function Row({
  label,
  hint,
  error,
  stacked = false,
  children,
}: {
  label: string
  hint?: ReactNode
  /** Inline validation message, shown under the hint in the error colour. */
  error?: string | null
  stacked?: boolean
  children: (ids: { labelId: string; hintId: string | undefined }) => ReactNode
}) {
  const labelId = useId()
  const hintId = useId()
  const describes = hint !== undefined || error
  const text = (
    <div className="flex-1 min-w-0">
      <p id={labelId} className="font-medium">
        {label}
      </p>
      {describes && (
        <div id={hintId} className="mt-0.5 text-[12px] leading-snug">
          {hint !== undefined && <p className="text-muted">{hint}</p>}
          {error && (
            <p role="alert" className="text-danger">
              {error}
            </p>
          )}
        </div>
      )}
    </div>
  )
  const control = children({ labelId, hintId: describes ? hintId : undefined })
  if (stacked) {
    return (
      <div className="px-4 py-3 space-y-2.5">
        {text}
        {control}
      </div>
    )
  }
  return (
    <div className="flex items-center justify-between gap-6 px-4 py-2.5 min-h-12">
      {text}
      <div className="shrink-0 flex items-center">{control}</div>
    </div>
  )
}

/** Inline status line inside a panel: a note, a warning, or an error. */
export function Note({ tone = 'muted', children }: { tone?: 'muted' | 'warn' | 'danger'; children: ReactNode }) {
  const cls =
    tone === 'warn' ? 'text-warn bg-warn-soft' : tone === 'danger' ? 'text-danger' : 'text-muted'
  return (
    <p
      role={tone === 'muted' ? undefined : tone === 'danger' ? 'alert' : 'status'}
      className={`text-[12px] leading-snug break-words rounded-md ${tone === 'warn' ? 'px-2.5 py-1.5' : ''} ${cls}`}
    >
      {children}
    </p>
  )
}
