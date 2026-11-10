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
        'relative inline-flex h-[22px] w-[38px] shrink-0 rounded-full p-0.5',
        'transition-colors duration-150 ease-out',
        enabled ? 'bg-accent' : 'bg-line-strong',
        disabled ? 'opacity-45' : '',
      ].join(' ')}
    >
      <span
        aria-hidden="true"
        className={[
          'pointer-events-none inline-block h-[18px] w-[18px] rounded-full bg-white',
          'shadow-[0_1px_2px_rgba(0,0,0,0.25)] transition-transform duration-150 ease-out',
          enabled ? 'translate-x-4' : 'translate-x-0',
        ].join(' ')}
      />
    </button>
  )
}
