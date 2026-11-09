import { describe, expect, it } from 'vitest'

import type { DesktopConfig } from '../shared/bindings/DesktopConfig'
import { argvError, checkDaemonBackend, normalizeArgv, placeholderNames, shellWordsJoin, usesCommand } from './argv'

const desktop = (over: Partial<DesktopConfig>): DesktopConfig => ({
  backend: 'auto',
  set_command: null,
  persistent: false,
  fit: 'fill',
  theme: 'auto',
  managed_by: 'detect',
  detected_session: null,
  detected_at: null,
  ...over,
})

describe('argvError', () => {
  it('accepts unset and known placeholders, also mid-token', () => {
    expect(argvError(null)).toBeNull()
    expect(argvError(['swaybg', '-i', '{path}', '-m', '{fit}', '--uri={uri}', 'x{theme}y'])).toBeNull()
  })

  it('rejects an empty argv, an empty token and an unknown placeholder', () => {
    expect(argvError([])).toBe('The command is empty.')
    expect(argvError(['feh', ''])).toBe('Token 2 is empty. Fill it in or remove the row.')
    expect(argvError(['setbg', '{file}'])).toBe(
      'Unknown placeholder {file} in token 2. Use {path}, {uri}, {fit} or {theme}.',
    )
  })

  it('scans placeholders like exec::placeholders', () => {
    expect(placeholderNames('{path}{fit}')).toEqual(['path', 'fit'])
    // Not placeholders: empty name, a dash, an unclosed brace, JSON-ish braces.
    expect(placeholderNames('{} {a-b} {path {"k":1}')).toEqual([])
  })
})

describe('normalizeArgv', () => {
  it('stores no rows as null and keeps tokens with spaces as one token', () => {
    expect(normalizeArgv([])).toBeNull()
    expect(normalizeArgv(['setbg', 'a b', '{path}'])).toEqual(['setbg', 'a b', '{path}'])
  })
})

describe('shellWordsJoin', () => {
  it('matches the Rust join_quotes_only_when_needed case', () => {
    expect(shellWordsJoin(['swaymsg', 'output', '*', 'bg', '{path}', 'a b', "it's"])).toBe(
      "swaymsg output * bg {path} 'a b' 'it'\\''s'",
    )
    expect(shellWordsJoin(['x', ''])).toBe("x ''")
  })
})

describe('usesCommand', () => {
  it('is false for D-Bus/gsettings backends and runtime-resolved ones', () => {
    expect(usesCommand('gnome')).toBe(false)
    expect(usesCommand('kde')).toBe(false)
    expect(usesCommand('auto')).toBe(false)
    expect(usesCommand('swaybg')).toBe(true)
    expect(usesCommand('command')).toBe(true)
  })
})

describe('checkDaemonBackend', () => {
  const linux = (saved: Partial<DesktopConfig>, name: string, detail: string) =>
    checkDaemonBackend({ saved: desktop(saved), os: 'linux', daemon: { name, detail } })

  it('cannot compare auto, native, or another OS', () => {
    expect(linux({ backend: 'auto' }, 'awww', 'awww img {path}').kind).toBe('unknown')
    expect(linux({ backend: 'native' }, 'gnome', '').kind).toBe('unknown')
    const mac = checkDaemonBackend({
      saved: desktop({ backend: 'command', set_command: ['x'] }),
      os: 'macos',
      daemon: { name: 'macos', detail: 'NSWorkspace' },
    })
    expect(mac.kind).toBe('unknown')
  })

  it('matches a command backend by its joined argv', () => {
    const saved = { backend: 'command' as const, set_command: ['setbg', 'a b', '{path}'] }
    expect(linux(saved, 'command', "setbg 'a b' {path}")).toEqual({ kind: 'match' })
  })

  it('is stale when the daemon still runs the old argv (no setter rebuild on reload)', () => {
    const saved = { backend: 'command' as const, set_command: ['setbg', '--new', '{path}'] }
    expect(linux(saved, 'command', 'setbg {path}')).toEqual({
      kind: 'stale',
      expected: 'command: setbg --new {path}',
    })
  })

  it('compares the kept-running suffix for persistent commands', () => {
    const argv = ['swaybg', '-i', '{path}']
    expect(linux({ backend: 'swaybg', set_command: argv }, 'swaybg', 'swaybg -i {path} (kept running)').kind).toBe(
      'match',
    )
    expect(
      linux({ backend: 'command', set_command: argv, persistent: true }, 'command', 'swaybg -i {path}').kind,
    ).toBe('stale')
    // `persistent` is ignored by non-command backends.
    expect(linux({ backend: 'gnome', persistent: true }, 'gnome', 'gsettings set x').kind).toBe('match')
  })

  it('compares presets by name and persistence only', () => {
    expect(linux({ backend: 'feh' }, 'feh', 'feh --bg-fill {path}').kind).toBe('match')
    expect(linux({ backend: 'feh', persistent: true }, 'feh', 'feh --bg-fill {path}')).toEqual({
      kind: 'stale',
      expected: 'feh preset (kept running)',
    })
    expect(linux({ backend: 'feh' }, 'xwallpaper', 'xwallpaper --zoom {path}')).toEqual({
      kind: 'stale',
      expected: 'feh preset',
    })
  })

  it('expects unconfigured for command without set_command', () => {
    expect(linux({ backend: 'command' }, 'unconfigured', 'set [desktop] set_command').kind).toBe('match')
    expect(linux({ backend: 'command' }, 'command', 'setbg {path}').kind).toBe('stale')
  })

  it('matches D-Bus backends by name', () => {
    expect(linux({ backend: 'kde' }, 'kde', 'D-Bus …').kind).toBe('match')
    expect(linux({ backend: 'kde' }, 'gnome', 'gsettings …')).toEqual({ kind: 'stale', expected: 'kde' })
  })
})
