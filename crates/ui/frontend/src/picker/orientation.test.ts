import { describe, expect, it } from 'vitest'
import { isVertical, toLayout } from './orientation'

describe('isVertical', () => {
  it('auto follows the edge', () => {
    expect(isVertical({ position: 'bottom', orientation: 'auto' })).toBe(false)
    expect(isVertical({ position: 'top', orientation: 'auto' })).toBe(false)
    expect(isVertical({ position: 'left', orientation: 'auto' })).toBe(true)
    expect(isVertical({ position: 'right', orientation: 'auto' })).toBe(true)
  })

  it('an explicit orientation wins on every edge', () => {
    for (const position of ['bottom', 'top', 'left', 'right'] as const) {
      expect(isVertical({ position, orientation: 'vertical' })).toBe(true)
      expect(isVertical({ position, orientation: 'horizontal' })).toBe(false)
    }
  })
})

describe('toLayout', () => {
  it('reads position and orientation', () => {
    expect(toLayout({ position: 'right', orientation: 'horizontal' })).toEqual({
      position: 'right',
      orientation: 'horizontal',
    })
  })

  it('falls back to bottom / auto for missing or unknown values', () => {
    expect(toLayout(undefined)).toEqual({ position: 'bottom', orientation: 'auto' })
    expect(toLayout({ position: 'left' })).toEqual({ position: 'left', orientation: 'auto' })
    expect(toLayout({ position: 'middle', orientation: 'diagonal' })).toEqual({
      position: 'bottom',
      orientation: 'auto',
    })
  })
})
