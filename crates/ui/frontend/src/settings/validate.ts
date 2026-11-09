import type { Config } from '../shared/api'
import { intervalError } from './duration'

/** Field-level problems that block saving, keyed by a stable field name. */
export interface ConfigErrors {
  interval?: string
}

export function configErrors(cfg: Config): ConfigErrors {
  const interval = intervalError(cfg.daemon.interval)
  return interval ? { interval } : {}
}

export function hasErrors(errors: ConfigErrors): boolean {
  return Object.values(errors).some(Boolean)
}
