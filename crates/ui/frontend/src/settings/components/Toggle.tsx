/**
 * Switch for boolean settings. A `role="switch"` button, so it is keyboard and screen-reader
 * operable; `label` names it when no visible label is linked with `labelledBy`.
 */
interface ToggleProps {
  enabled: boolean
  onChange: (enabled: boolean) => void
  label?: string
  labelledBy?: string
  describedBy?: string
  disabled?: boolean
  id?: string
}

export default function Toggle({ enabled, onChange, label, labelledBy, describedBy, disabled, id }: ToggleProps) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={enabled}
      aria-label={labelledBy ? undefined : label}
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
      disabled={disabled}
      onClick={() => onChange(!enabled)}
      className={[
        'relative inline-flex h-5 w-9 shrink-0 rounded-full border-2 border-transparent',
        'transition-colors duration-150 ease-out motion-reduce:transition-none',
        'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
        enabled ? 'bg-accent dark:bg-accent-dark' : 'bg-gray-300 dark:bg-gray-600',
        disabled ? 'opacity-50 cursor-not-allowed' : 'cursor-pointer',
      ].join(' ')}
    >
      <span
        aria-hidden="true"
        className={[
          'pointer-events-none inline-block h-4 w-4 rounded-full bg-white shadow-sm',
          'transition-transform duration-150 ease-out motion-reduce:transition-none',
          enabled ? 'translate-x-4' : 'translate-x-0',
        ].join(' ')}
      />
    </button>
  )
}
