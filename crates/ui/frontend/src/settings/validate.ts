import type { Config } from '../shared/api'
import { argvError } from './argv'
import { intervalError } from './duration'

/** Field-level problems that block saving, keyed by a stable field name. */
export interface ConfigErrors {
  interval?: string
  set_command?: string
  on_change?: string
}

export function configErrors(cfg: Config): ConfigErrors {
  const errors: ConfigErrors = {}
  const interval = intervalError(cfg.daemon.interval)
  if (interval) errors.interval = interval
  const setCommand = argvError(cfg.desktop.set_command)
  if (setCommand) errors.set_command = setCommand
  const onChange = argvError(cfg.hooks.on_change)
  if (onChange) errors.on_change = onChange
  return errors
}

export function hasErrors(errors: ConfigErrors): boolean {
  return Object.values(errors).some(Boolean)
}
