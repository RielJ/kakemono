import { useEffect, useRef, useState, type InputHTMLAttributes } from 'react'

type InputProps = Omit<InputHTMLAttributes<HTMLInputElement>, 'value' | 'onChange' | 'defaultValue'>

/**
 * A text input that applies its value on Enter or when it loses focus, not on every key, so
 * settings that save themselves are not written (and the daemon not reloaded) mid-word.
 * Escape restores the applied value.
 */
export default function TextField({
  value,
  onCommit,
  mono = false,
  className = '',
  ...rest
}: InputProps & {
  value: string
  onCommit: (value: string) => void
  mono?: boolean
}) {
  const [draft, setDraft] = useState(value)
  const focused = useRef(false)

  // Follow outside changes (Browse…, a reload) unless the user is typing.
  useEffect(() => {
    if (!focused.current) setDraft(value)
  }, [value])

  const commit = () => {
    if (draft !== value) onCommit(draft)
  }

  return (
    <input
      type="text"
      spellCheck={false}
      autoComplete="off"
      autoCapitalize="off"
      {...rest}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onFocus={(e) => {
        focused.current = true
        rest.onFocus?.(e)
      }}
      onBlur={(e) => {
        focused.current = false
        commit()
        rest.onBlur?.(e)
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault()
          commit()
        } else if (e.key === 'Escape') {
          setDraft(value)
        }
        rest.onKeyDown?.(e)
      }}
      className={`field ${mono ? 'field-mono' : ''} ${className}`}
    />
  )
}
