import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { DEFAULT_TAB, nextTab, tabAt, TABS, type PickerTabId } from '../tabs'
import TabBar, { PANEL_ID } from './TabBar'

afterEach(cleanup)

/** A TabBar that owns its selection, like the App will. */
function Harness({
  vertical = false,
  onSelect = () => {},
}: {
  vertical?: boolean
  onSelect?: (id: PickerTabId) => void
}) {
  const [active, setActive] = useState<PickerTabId>(DEFAULT_TAB)
  return (
    <TabBar
      active={active}
      vertical={vertical}
      onSelect={(id) => {
        onSelect(id)
        setActive(id)
      }}
    />
  )
}

const LABELS = ['Sources', 'Downloaded', 'Local', 'Favorites', 'Cache']

// `tabs.ts` has no file of its own; its helpers are what the TabBar steps through.
describe('tabs', () => {
  it('lists the tabs in order and opens on Sources', () => {
    expect(TABS.map((t) => t.id)).toEqual(['sources', 'downloaded', 'local', 'favorites', 'cache'])
    expect(DEFAULT_TAB).toBe('sources')
  })

  it('steps with wrap-around', () => {
    expect(nextTab('sources', 1)).toBe('downloaded')
    expect(nextTab('cache', 1)).toBe('sources')
    expect(nextTab('sources', -1)).toBe('cache')
  })

  it('maps 0-based indexes', () => {
    expect(tabAt(0)).toBe('sources')
    expect(tabAt(4)).toBe('cache')
    expect(tabAt(5)).toBeNull()
    expect(tabAt(-1)).toBeNull()
  })
})

describe('TabBar', () => {
  it('renders five labelled tabs with the selected one in the tab order', () => {
    render(<TabBar active="local" vertical={false} onSelect={() => {}} />)
    const tabs = screen.getAllByRole('tab')
    expect(tabs.map((t) => t.getAttribute('aria-label'))).toEqual(LABELS)
    expect(tabs.map((t) => t.getAttribute('title'))).toEqual(LABELS)
    expect(tabs.map((t) => t.getAttribute('aria-selected'))).toEqual(['false', 'false', 'true', 'false', 'false'])
    expect(tabs.map((t) => t.tabIndex)).toEqual([-1, -1, 0, -1, -1])
    for (const tab of tabs) {
      expect(tab.getAttribute('aria-controls')).toBe(PANEL_ID)
      expect(tab.textContent).toBe('')
    }
  })

  it('runs across the strip', () => {
    const { unmount } = render(<TabBar active="sources" vertical={false} onSelect={() => {}} />)
    expect(screen.getByRole('tablist').getAttribute('aria-orientation')).toBe('vertical')
    // The tab column carries the drag handle in a horizontal picker.
    expect(screen.getByRole('img', { name: 'Move picker' })).toBeTruthy()
    unmount()

    render(<TabBar active="sources" vertical onSelect={() => {}} />)
    expect(screen.getByRole('tablist').getAttribute('aria-orientation')).toBe('horizontal')
    expect(screen.queryByRole('img', { name: 'Move picker' })).toBeNull()
  })

  it('selects a tab on click', async () => {
    const onSelect = vi.fn()
    render(<TabBar active="sources" vertical={false} onSelect={onSelect} />)
    await userEvent.setup().click(screen.getByRole('tab', { name: 'Favorites' }))
    expect(onSelect).toHaveBeenCalledWith('favorites')
  })

  it('moves focus and selection with the arrows along the tablist, wrapping', () => {
    const onSelect = vi.fn()
    render(<Harness onSelect={onSelect} />)
    const tab = (name: string) => screen.getByRole('tab', { name })

    fireEvent.keyDown(tab('Sources'), { key: 'ArrowDown' })
    expect(onSelect).toHaveBeenLastCalledWith('downloaded')
    expect(document.activeElement).toBe(tab('Downloaded'))
    expect(tab('Downloaded').tabIndex).toBe(0)

    fireEvent.keyDown(tab('Downloaded'), { key: 'ArrowUp' })
    fireEvent.keyDown(tab('Sources'), { key: 'ArrowUp' })
    expect(onSelect).toHaveBeenLastCalledWith('cache')
    expect(document.activeElement).toBe(tab('Cache'))

    fireEvent.keyDown(tab('Cache'), { key: 'Home' })
    expect(onSelect).toHaveBeenLastCalledWith('sources')
    fireEvent.keyDown(tab('Sources'), { key: 'End' })
    expect(onSelect).toHaveBeenLastCalledWith('cache')

    // The cross axis and modified keys are not the tablist's.
    onSelect.mockClear()
    fireEvent.keyDown(tab('Cache'), { key: 'ArrowRight' })
    fireEvent.keyDown(tab('Cache'), { key: 'ArrowDown', metaKey: true })
    expect(onSelect).not.toHaveBeenCalled()
  })

  it('uses ←/→ in a vertical picker', () => {
    const onSelect = vi.fn()
    render(<Harness vertical onSelect={onSelect} />)
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Sources' }), { key: 'ArrowRight' })
    expect(onSelect).toHaveBeenLastCalledWith('downloaded')
    fireEvent.keyDown(screen.getByRole('tab', { name: 'Downloaded' }), { key: 'ArrowDown' })
    expect(onSelect).toHaveBeenCalledTimes(1)
  })
})
