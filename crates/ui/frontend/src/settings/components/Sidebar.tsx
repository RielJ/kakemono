import type { TabId } from '../sections'
import { TAB_TITLES } from '../sections'
import Icon, { type IconName } from './Icon'

interface SidebarProps {
  activeTab: TabId
  onTabChange: (tab: TabId) => void
}

const NAV_ITEMS: { id: TabId; icon: IconName }[] = [
  { id: 'general', icon: 'settings' },
  { id: 'sources', icon: 'globe' },
  { id: 'themes', icon: 'sunMoon' },
  { id: 'favorites', icon: 'heart' },
  { id: 'desktop', icon: 'image' },
  { id: 'hooks', icon: 'terminal' },
]

export default function Sidebar({ activeTab, onTabChange }: SidebarProps) {
  return (
    <nav
      className="w-44 shrink-0 flex flex-col gap-0.5 p-2 bg-gray-50 dark:bg-gray-900 border-r border-gray-200 dark:border-gray-700/70"
      aria-label="Settings sections"
    >
      {NAV_ITEMS.map((item) => {
        const active = item.id === activeTab
        return (
          <button
            key={item.id}
            type="button"
            onClick={() => onTabChange(item.id)}
            aria-current={active ? 'page' : undefined}
            className={[
              'flex items-center gap-2.5 w-full px-3 py-1.5 rounded-md text-sm text-left',
              'transition-colors duration-100 motion-reduce:transition-none',
              'focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-accent',
              active
                ? 'bg-accent/10 text-accent dark:bg-accent-dark/15 dark:text-accent-dark font-medium'
                : 'text-gray-700 hover:bg-gray-200/70 dark:text-gray-300 dark:hover:bg-gray-800',
            ].join(' ')}
          >
            <Icon name={item.icon} className="w-4 h-4 shrink-0" />
            {TAB_TITLES[item.id]}
          </button>
        )
      })}
    </nav>
  )
}
