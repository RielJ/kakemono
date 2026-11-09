/**
 * Argv helpers for `[desktop] set_command` and `[hooks] on_change`, and the check that tells
 * the Desktop panel whether the daemon runs the saved backend.
 *
 * An argv is a list of tokens. Nothing here joins tokens into a shell string except
 * `shellWordsJoin`, which is display-only and mirrors the daemon's `doctor` detail.
 */

import type { Backend } from '../shared/bindings/Backend'
import type { DesktopConfig } from '../shared/bindings/DesktopConfig'
import type { BackendReport } from '../shared/api'

/** Placeholders `kakemono_platform::exec` substitutes inside a token. */
export const PLACEHOLDERS = ['path', 'uri', 'fit', 'theme'] as const

/** `{name}` spans in a token, scanned like `exec::placeholders` (ASCII alphanumerics and `_`). */
export function placeholderNames(token: string): string[] {
  const names: string[] = []
  const re = /\{([A-Za-z0-9_]+)\}/g
  for (let m = re.exec(token); m; m = re.exec(token)) names.push(m[1])
  return names
}

/**
 * Why an argv can't be saved, or `null`. `null` argv means "unset" and is valid. Mirrors
 * `exec::validate`, plus empty tokens, which are almost always an unfinished row.
 */
export function argvError(argv: readonly string[] | null): string | null {
  if (argv === null) return null
  if (argv.length === 0) return 'The command is empty.'
  for (const [i, token] of argv.entries()) {
    if (token === '') return `Token ${i + 1} is empty. Fill it in or remove the row.`
    const known: readonly string[] = PLACEHOLDERS
    const unknown = placeholderNames(token).find((name) => !known.includes(name))
    if (unknown !== undefined) {
      return `Unknown placeholder {${unknown}} in token ${i + 1}. Use {path}, {uri}, {fit} or {theme}.`
    }
  }
  return null
}

/** The value to store for an edited argv: no rows means "unset" (`null`), as the config loads it. */
export function normalizeArgv(argv: readonly string[]): string[] | null {
  return argv.length === 0 ? null : [...argv]
}

/**
 * Display-only join, identical to `platform::linux::backends::command::shell_words_join`:
 * tokens with whitespace, quotes, `\`, `$` or a backtick are single-quoted.
 */
export function shellWordsJoin(argv: readonly string[]): string {
  return argv
    .map((t) => (t !== '' && !/[\s'"\\$`]/.test(t) ? t : `'${t.replaceAll("'", "'\\''")}'`))
    .join(' ')
}

/** Backends that set the wallpaper without an argv (`BackendKind::uses_command` is false). */
const NO_COMMAND: readonly Backend[] = ['gnome', 'kde', 'xfce', 'cinnamon', 'mate']

/** True when `set_command` and `persistent` apply to this backend. */
export function usesCommand(backend: Backend): boolean {
  return !NO_COMMAND.includes(backend) && backend !== 'auto' && backend !== 'native'
}

/** Backends whose command always keeps running (`BackendKind::is_persistent`). */
const ALWAYS_PERSISTENT: readonly Backend[] = ['swaybg', 'wbg']

const KEPT_RUNNING = ' (kept running)'

/** Whether the daemon runs what the saved config selects. */
export type DaemonBackendCheck =
  | { kind: 'match' }
  /** The saved config doesn't name one backend (`auto`, `native`, not Linux): nothing to compare. */
  | { kind: 'unknown' }
  /** The daemon reports something else; `expected` describes the saved backend. */
  | { kind: 'stale'; expected: string }

/**
 * Compares the saved `[desktop]` with the backend the daemon reports in `doctor`
 * (`name` and `detail`). The only input about the daemon is its reply, so once `reload`
 * rebuilds the setter (Task 37b) the two match after Test without any UI change.
 *
 * Rules, from `linux::detect::resolve::from_config` and the setters' `info()`:
 * - `auto` and `native` resolve at runtime: `unknown`. So does any OS but Linux.
 * - `command` without `set_command` resolves to `unconfigured`.
 * - Otherwise the daemon's `name` is the saved backend.
 * - For a command backend, `set_command` (when set) is the detail, joined for display, with
 *   ` (kept running)` when persistent (swaybg, wbg, or `persistent = true`). Without
 *   `set_command` the preset argv is the daemon's to know, so only the suffix is compared.
 */
export function checkDaemonBackend({
  saved,
  os,
  daemon,
}: {
  saved: DesktopConfig
  os: string
  daemon: BackendReport
}): DaemonBackendCheck {
  if (os !== 'linux' || saved.backend === 'auto' || saved.backend === 'native') return { kind: 'unknown' }

  const argv = saved.set_command
  if (saved.backend === 'command' && argv === null) {
    return daemon.name === 'unconfigured' ? { kind: 'match' } : { kind: 'stale', expected: 'command (no set_command yet)' }
  }
  if (!usesCommand(saved.backend)) {
    return daemon.name === saved.backend ? { kind: 'match' } : { kind: 'stale', expected: saved.backend }
  }

  const persistent = ALWAYS_PERSISTENT.includes(saved.backend) || saved.persistent
  const suffix = persistent ? KEPT_RUNNING : ''
  if (argv !== null) {
    const detail = shellWordsJoin(argv) + suffix
    const same = daemon.name === saved.backend && daemon.detail === detail
    return same ? { kind: 'match' } : { kind: 'stale', expected: `${saved.backend}: ${detail}` }
  }
  const same = daemon.name === saved.backend && daemon.detail.endsWith(KEPT_RUNNING) === persistent
  return same ? { kind: 'match' } : { kind: 'stale', expected: `${saved.backend} preset${suffix}` }
}
