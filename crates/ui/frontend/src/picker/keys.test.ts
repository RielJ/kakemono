import { describe, expect, it } from 'vitest'
import { keyAction } from './keys'

const h = (key: string, mods: Partial<Record<'shiftKey' | 'altKey' | 'ctrlKey' | 'metaKey', boolean>> = {}) =>
  keyAction({ key, ...mods }, false, 2, 5)
const v = (key: string) => keyAction({ key }, true, 2, 5)

describe('keyAction', () => {
  it('moves along the strip and clamps at both ends', () => {
    expect(h('ArrowRight')).toEqual({ kind: 'move', to: 3 })
    expect(h('ArrowLeft')).toEqual({ kind: 'move', to: 1 })
    expect(keyAction({ key: 'ArrowRight' }, false, 4, 5)).toEqual({ kind: 'move', to: 4 })
    expect(keyAction({ key: 'ArrowLeft' }, false, 0, 5)).toEqual({ kind: 'move', to: 0 })
    expect(h('Home')).toEqual({ kind: 'move', to: 0 })
    expect(h('End')).toEqual({ kind: 'move', to: 4 })
  })

  it('uses ↑/↓ for a vertical strip and ignores the cross axis', () => {
    expect(v('ArrowDown')).toEqual({ kind: 'move', to: 3 })
    expect(v('ArrowUp')).toEqual({ kind: 'move', to: 1 })
    expect(v('ArrowRight')).toBeNull()
    expect(h('ArrowDown')).toBeNull()
  })

  it('maps the action keys', () => {
    expect(h('Enter')).toEqual({ kind: 'set' })
    expect(h(' ')).toEqual({ kind: 'set' })
    expect(h('f')).toEqual({ kind: 'favorite' })
    expect(h('F', { shiftKey: true })).toEqual({ kind: 'favorite' })
    expect(h('Delete')).toEqual({ kind: 'trash' })
    expect(h('Backspace')).toEqual({ kind: 'trash' })
    expect(h('ContextMenu')).toEqual({ kind: 'menu' })
    expect(h('F10', { shiftKey: true })).toEqual({ kind: 'menu' })
    expect(h('F10')).toBeNull()
    expect(h('Escape')).toEqual({ kind: 'close' })
    expect(h('z', { metaKey: true })).toEqual({ kind: 'undo' })
    expect(h('z', { ctrlKey: true })).toEqual({ kind: 'undo' })
  })

  it('leaves modified keys to the system', () => {
    expect(h('f', { metaKey: true })).toBeNull()
    expect(h('ArrowRight', { altKey: true })).toBeNull()
    expect(h('z', { metaKey: true, shiftKey: true })).toBeNull()
  })

  it('only closes or undoes in an empty strip', () => {
    expect(keyAction({ key: 'ArrowRight' }, false, 0, 0)).toBeNull()
    expect(keyAction({ key: 'Enter' }, false, 0, 0)).toBeNull()
    expect(keyAction({ key: 'ContextMenu' }, false, 0, 0)).toBeNull()
    expect(keyAction({ key: 'Escape' }, false, 0, 0)).toEqual({ kind: 'close' })
  })
})
