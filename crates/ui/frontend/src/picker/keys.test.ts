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

  it('uses ↑/↓ for a vertical strip', () => {
    expect(v('ArrowDown')).toEqual({ kind: 'move', to: 3 })
    expect(v('ArrowUp')).toEqual({ kind: 'move', to: 1 })
  })

  it('switches tabs with the cross-axis arrows', () => {
    expect(h('ArrowDown')).toEqual({ kind: 'tab', delta: 1 })
    expect(h('ArrowUp')).toEqual({ kind: 'tab', delta: -1 })
    expect(v('ArrowRight')).toEqual({ kind: 'tab', delta: 1 })
    expect(v('ArrowLeft')).toEqual({ kind: 'tab', delta: -1 })
  })

  it('jumps to a tab with 1–5', () => {
    expect(h('1')).toEqual({ kind: 'tabIndex', index: 0 })
    expect(h('3')).toEqual({ kind: 'tabIndex', index: 2 })
    expect(h('5')).toEqual({ kind: 'tabIndex', index: 4 })
    expect(h('0')).toBeNull()
    expect(h('6')).toBeNull()
  })

  it('cycles the source filter with [ and ]', () => {
    expect(h(']')).toEqual({ kind: 'filter', delta: 1 })
    expect(h('[')).toEqual({ kind: 'filter', delta: -1 })
  })

  it('handles tab keys in an empty strip', () => {
    const empty = (key: string, vertical = false) => keyAction({ key }, vertical, 0, 0)
    expect(empty('ArrowDown')).toEqual({ kind: 'tab', delta: 1 })
    expect(empty('ArrowLeft', true)).toEqual({ kind: 'tab', delta: -1 })
    expect(empty('2')).toEqual({ kind: 'tabIndex', index: 1 })
    expect(empty(']')).toEqual({ kind: 'filter', delta: 1 })
  })

  it('leaves modified tab keys to the system', () => {
    for (const mods of [{ ctrlKey: true }, { metaKey: true }, { altKey: true }]) {
      expect(h('ArrowDown', mods)).toBeNull()
      expect(h('1', mods)).toBeNull()
      expect(h('[', mods)).toBeNull()
      expect(h(']', mods)).toBeNull()
    }
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

  it('only closes, undoes or switches tabs in an empty strip', () => {
    expect(keyAction({ key: 'ArrowRight' }, false, 0, 0)).toBeNull()
    expect(keyAction({ key: 'Home' }, false, 0, 0)).toBeNull()
    expect(keyAction({ key: 'Enter' }, false, 0, 0)).toBeNull()
    expect(keyAction({ key: 'ContextMenu' }, false, 0, 0)).toBeNull()
    expect(keyAction({ key: 'Escape' }, false, 0, 0)).toEqual({ kind: 'close' })
  })
})
