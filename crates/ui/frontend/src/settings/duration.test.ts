import { describe, expect, it } from 'vitest'

// Shared with `crates/config/tests/duration_cases.rs`; edit the JSON, not this test.
import cases from '../../../../config/tests/fixtures/go_durations.json'
import { intervalError, parseGoDuration } from './duration'

describe('parseGoDuration (shared table with duration.rs)', () => {
  it('has cases', () => {
    expect(cases.valid.length).toBeGreaterThan(0)
    expect(cases.invalid.length).toBeGreaterThan(0)
  })

  it.each(cases.valid)('accepts $input', ({ input, nanos }) => {
    expect(parseGoDuration(input)).toEqual({ ok: true, nanos: BigInt(nanos) })
  })

  it.each(cases.invalid)('rejects $input as $kind', (c) => {
    const got = parseGoDuration(c.input)
    expect(got.ok).toBe(false)
    if (got.ok) return
    expect(got.kind).toBe(c.kind)
    expect(got.unit).toBe('unit' in c ? c.unit : undefined)
  })

  it('words errors like Go', () => {
    const got = parseGoDuration('1d')
    expect(got.ok ? '' : got.message).toBe('time: unknown unit "d" in duration "1d"')
  })
})

describe('intervalError', () => {
  it.each([
    ['30m', null],
    ['1h30m0s', null],
    ['1.5h', null],
  ])('%s is valid', (value, want) => {
    expect(intervalError(value)).toBe(want)
  })

  it.each(['', '0', '0s', '-5m', '30', '1d', ' 30m', 'soon'])('%j is rejected', (value) => {
    expect(intervalError(value)).toEqual(expect.any(String))
  })
})
