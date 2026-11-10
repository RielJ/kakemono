/**
 * The picker tabs, in display order. `1`–`5` jump to them in
 * this order, and the cross-axis arrows step through them with wrap-around.
 */
import type { IconName } from './components/Icon'
import type { Step } from './keys'

interface TabSpec {
  readonly id: string
  /** The accessible name and the tooltip; the tab itself shows only the icon. */
  readonly label: string
  readonly icon: IconName
}

export const TABS = [
  { id: 'sources', label: 'Sources', icon: 'cloud' },
  { id: 'downloaded', label: 'Downloaded', icon: 'download' },
  { id: 'local', label: 'Local', icon: 'folder' },
  { id: 'favorites', label: 'Favorites', icon: 'star' },
  { id: 'cache', label: 'Cache', icon: 'archive' },
] as const satisfies readonly TabSpec[]

export type PickerTab = (typeof TABS)[number]
export type PickerTabId = PickerTab['id']

/** The picker always opens on Sources. */
export const DEFAULT_TAB: PickerTabId = 'sources'

/** The tab `delta` steps from `current`, wrapping at both ends. */
export function nextTab(current: PickerTabId, delta: Step): PickerTabId {
  const from = TABS.findIndex((tab) => tab.id === current)
  const to = (from + delta + TABS.length) % TABS.length
  return TABS[to].id
}

/** The tab at 0-based `index` (from the `1`–`5` keys), or `null` when out of range. */
export function tabAt(index: number): PickerTabId | null {
  return TABS[index]?.id ?? null
}
