/**
 * Go `time.ParseDuration`, mirrored from `crates/config/src/duration.rs` (`parse_go`) so the
 * interval field is validated by the same grammar as the daemon. Both are tested against
 * `crates/config/tests/fixtures/go_durations.json`.
 *
 * Grammar: `[-+]?([0-9]*(\.[0-9]*)?(ns|us|µs|μs|ms|s|m|h))+`, or `"0"`. Negative results are
 * rejected, as in Rust (`std::time::Duration` is unsigned).
 */

export type DurationErrorKind = 'invalid' | 'missing_unit' | 'unknown_unit' | 'negative'

export type DurationResult =
  | { ok: true; nanos: bigint }
  | { ok: false; kind: DurationErrorKind; unit?: string; message: string }

const MAX_NANOS = 1n << 63n

// A Map, not an object literal: the two micro signs (U+00B5, U+03BC) are distinct keys.
const UNITS = new Map<string, bigint>([
  ['ns', 1n],
  ['us', 1_000n],
  ['\u00b5s', 1_000n],
  ['\u03bcs', 1_000n],
  ['ms', 1_000_000n],
  ['s', 1_000_000_000n],
  ['m', 60_000_000_000n],
  ['h', 3_600_000_000_000n],
])

const isDigit = (c: string) => c >= '0' && c <= '9'

function fail(kind: DurationErrorKind, input: string, unit?: string): DurationResult {
  const q = JSON.stringify(input)
  const message =
    kind === 'missing_unit'
      ? `time: missing unit in duration ${q}`
      : kind === 'unknown_unit'
        ? `time: unknown unit ${JSON.stringify(unit)} in duration ${q}`
        : kind === 'negative'
          ? `time: duration ${q} is negative`
          : `time: invalid duration ${q}`
  return unit === undefined ? { ok: false, kind, message } : { ok: false, kind, unit, message }
}

/** Parses a Go duration string such as `"30m"`, `"1h30m"` or `"1.5h"`. */
export function parseGoDuration(input: string): DurationResult {
  let s = input
  let neg = false
  if (s.startsWith('-')) {
    neg = true
    s = s.slice(1)
  } else if (s.startsWith('+')) {
    s = s.slice(1)
  }
  if (s === '0') return { ok: true, nanos: 0n }
  if (s === '') return fail('invalid', input)

  let total = 0n
  while (s !== '') {
    if (!(s[0] === '.' || isDigit(s[0]))) return fail('invalid', input)

    // Leading integer; overflow past 2^63 is invalid.
    let i = 0
    let whole = 0n
    while (i < s.length && isDigit(s[i])) {
      whole = whole * 10n + BigInt(s.charCodeAt(i) - 48)
      if (whole > MAX_NANOS) return fail('invalid', input)
      i++
    }
    const hasInt = i > 0
    s = s.slice(i)

    // Fraction; digits past 2^63 precision are dropped, as in Go.
    let frac = 0n
    let scale = 1
    let hasFrac = false
    if (s.startsWith('.')) {
      s = s.slice(1)
      let j = 0
      let overflow = false
      while (j < s.length && isDigit(s[j])) {
        if (!overflow) {
          const next = frac * 10n + BigInt(s.charCodeAt(j) - 48)
          if (next <= MAX_NANOS) {
            frac = next
            scale *= 10
          } else {
            overflow = true
          }
        }
        j++
      }
      hasFrac = j > 0
      s = s.slice(j)
    }
    if (!hasInt && !hasFrac) return fail('invalid', input)

    let end = 0
    while (end < s.length && !(s[end] === '.' || isDigit(s[end]))) end++
    if (end === 0) return fail('missing_unit', input)
    const unitStr = s.slice(0, end)
    s = s.slice(end)
    const unit = UNITS.get(unitStr)
    if (unit === undefined) return fail('unknown_unit', input, unitStr)

    if (whole > MAX_NANOS / unit) return fail('invalid', input)
    let v = whole * unit
    if (frac > 0n) {
      // Same float arithmetic as Go and Rust; truncation is intended.
      const extra = Math.trunc(Number(frac) * (Number(unit) / scale))
      v += BigInt(extra)
      if (v > MAX_NANOS) return fail('invalid', input)
    }
    total += v
    if (total > MAX_NANOS) return fail('invalid', input)
  }

  if (neg) {
    return total === 0n ? { ok: true, nanos: 0n } : fail('negative', input)
  }
  if (total > MAX_NANOS - 1n) return fail('invalid', input)
  return { ok: true, nanos: total }
}

/**
 * Inline validation text for `[daemon] interval`, or `null` when it is valid. The daemon
 * also needs a positive interval, which the save command enforces too.
 */
export function intervalError(value: string): string | null {
  if (value.trim() !== value) return 'Remove the spaces around the interval.'
  const parsed = parseGoDuration(value)
  if (!parsed.ok) {
    switch (parsed.kind) {
      case 'missing_unit':
        return 'Add a unit, for example 30m or 2h.'
      case 'unknown_unit':
        return `Unknown unit “${parsed.unit ?? ''}”. Use h, m, s or ms (for example 1h30m).`
      case 'negative':
        return 'The interval cannot be negative.'
      case 'invalid':
        return 'Enter a duration such as 30m, 1h30m or 1.5h.'
      default: {
        const _exhaustive: never = parsed.kind
        return _exhaustive
      }
    }
  }
  if (parsed.nanos === 0n) return 'The interval must be longer than zero.'
  return null
}

const SECOND = 1_000_000_000n

/**
 * `[daemon] interval` in words for prose, e.g. `90m` → "1 hour 30 minutes". Values that are not
 * a positive whole number of seconds stay as written, so the sentence never hides a typo.
 */
export function formatInterval(value: string): string {
  const parsed = parseGoDuration(value)
  if (!parsed.ok || parsed.nanos <= 0n || parsed.nanos % SECOND !== 0n) return value
  const total = parsed.nanos / SECOND
  const parts: [bigint, string][] = [
    [total / 3600n, 'hour'],
    [(total % 3600n) / 60n, 'minute'],
    [total % 60n, 'second'],
  ]
  return parts
    .filter(([n]) => n > 0n)
    .map(([n, unit]) => `${n} ${unit}${n === 1n ? '' : 's'}`)
    .join(' ')
}
