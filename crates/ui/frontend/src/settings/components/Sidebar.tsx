import type { TabId } from '../types/config'

interface SidebarProps {
  activeTab: TabId
  onTabChange: (tab: TabId) => void
}

interface NavItem {
  id: TabId
  label: string
  icon: React.ReactNode
}

// ── Minimal inline SVG icons ──────────────────────────────────────────────────

const IconGear = () => (
  <svg viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
    <path
      fillRule="evenodd"
      d="M11.49 3.17c-.38-1.56-2.6-1.56-2.98 0a1.532 1.532 0 01-2.286.948c-1.372-.836-2.942.734-2.106 2.106.54.886.061 2.042-.947 2.287-1.561.379-1.561 2.6 0 2.978a1.532 1.532 0 01.947 2.287c-.836 1.372.734 2.942 2.106 2.106a1.532 1.532 0 012.287.947c.379 1.561 2.6 1.561 2.978 0a1.533 1.533 0 012.287-.947c1.372.836 2.942-.734 2.106-2.106a1.533 1.533 0 01.947-2.287c1.561-.379 1.561-2.6 0-2.978a1.532 1.532 0 01-.947-2.287c.836-1.372-.734-2.942-2.106-2.106a1.532 1.532 0 01-2.287-.947zM10 13a3 3 0 100-6 3 3 0 000 6z"
      clipRule="evenodd"
    />
  </svg>
)

const IconGlobe = () => (
  <svg viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
    <path
      fillRule="evenodd"
      d="M10 18a8 8 0 100-16 8 8 0 000 16zM4.332 8.027a6.012 6.012 0 011.912-2.706C6.512 5.73 6.974 6 7.5 6A1.5 1.5 0 019 7.5V8a2 2 0 004 0 2 2 0 011.523-1.943A5.977 5.977 0 0116 10c0 .34-.028.675-.083 1H15a2 2 0 00-2 2v2.197A5.973 5.973 0 0110 16v-2a2 2 0 00-2-2 2 2 0 01-2-2 2 2 0 00-1.668-1.973z"
      clipRule="evenodd"
    />
  </svg>
)

const IconSun = () => (
  <svg viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
    <path
      fillRule="evenodd"
      d="M10 2a1 1 0 011 1v1a1 1 0 11-2 0V3a1 1 0 011-1zm4 8a4 4 0 11-8 0 4 4 0 018 0zm-.464 4.95l.707.707a1 1 0 001.414-1.414l-.707-.707a1 1 0 00-1.414 1.414zm2.12-10.607a1 1 0 010 1.414l-.706.707a1 1 0 11-1.414-1.414l.707-.707a1 1 0 011.414 0zM17 11a1 1 0 100-2h-1a1 1 0 100 2h1zm-7 4a1 1 0 011 1v1a1 1 0 11-2 0v-1a1 1 0 011-1zM5.05 6.464A1 1 0 106.465 5.05l-.708-.707a1 1 0 00-1.414 1.414l.707.707zm1.414 8.486l-.707.707a1 1 0 01-1.414-1.414l.707-.707a1 1 0 011.414 1.414zM4 11a1 1 0 100-2H3a1 1 0 000 2h1z"
      clipRule="evenodd"
    />
  </svg>
)

const IconHeart = () => (
  <svg viewBox="0 0 20 20" fill="currentColor" className="w-4 h-4">
    <path
      fillRule="evenodd"
      d="M3.172 5.172a4 4 0 015.656 0L10 6.343l1.172-1.171a4 4 0 115.656 5.656L10 17.657l-6.828-6.829a4 4 0 010-5.656z"
      clipRule="evenodd"
    />
  </svg>
)

const NAV_ITEMS: NavItem[] = [
  { id: 'general',    label: 'General',    icon: <IconGear /> },
  { id: 'providers',  label: 'Providers',  icon: <IconGlobe /> },
  { id: 'themes',     label: 'Themes',     icon: <IconSun /> },
  { id: 'favorites',  label: 'Favorites',  icon: <IconHeart /> },
]

export default function Sidebar({ activeTab, onTabChange }: SidebarProps) {
  return (
    <nav
      className="
        w-44 flex-shrink-0 flex flex-col pt-2 pb-4
        bg-gray-100/80 dark:bg-gray-900/80
        border-r border-gray-200 dark:border-gray-700/60
      "
      aria-label="Settings sections"
    >
      {NAV_ITEMS.map((item) => {
        const active = item.id === activeTab
        return (
          <button
            key={item.id}
            onClick={() => onTabChange(item.id)}
            className={`
              no-drag
              flex items-center gap-2.5 w-full px-3 py-2 mx-1 rounded-lg
              text-sm font-medium transition-colors duration-100
              focus:outline-none focus:ring-2 focus:ring-accent/50
              ${active
                ? 'bg-accent/10 text-accent dark:text-accent-dark'
                : 'text-gray-600 hover:bg-gray-200/70 dark:text-gray-300 dark:hover:bg-gray-700/50'
              }
            `}
            style={{ width: 'calc(100% - 8px)' }}
            aria-current={active ? 'page' : undefined}
          >
            <span className={active ? 'text-accent dark:text-accent-dark' : 'text-gray-400 dark:text-gray-500'}>
              {item.icon}
            </span>
            {item.label}
          </button>
        )
      })}
    </nav>
  )
}
