import { useEffect } from 'react'

import type { Appearance } from './bindings/Appearance'

function systemPrefersDark(): boolean {
  return typeof window.matchMedia === 'function' && window.matchMedia('(prefers-color-scheme: dark)').matches
}

/** Sets `<html data-theme>` from `[ui] appearance`, following the system when it is `system`. */
export function useAppearance(appearance: Appearance) {
  useEffect(() => {
    const root = document.documentElement
    const apply = () => {
      const dark = appearance === 'dark' || (appearance === 'system' && systemPrefersDark())
      root.dataset.theme = dark ? 'dark' : 'light'
    }
    apply()
    if (appearance !== 'system' || typeof window.matchMedia !== 'function') return
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    mq.addEventListener('change', apply)
    return () => mq.removeEventListener('change', apply)
  }, [appearance])
}
