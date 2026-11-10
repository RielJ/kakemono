/** The settings window's tabs. */
export type TabId = 'general' | 'advanced'

export const TABS: readonly { id: TabId; title: string }[] = [
  { id: 'general', title: 'General' },
  { id: 'advanced', title: 'Advanced' },
]

/** What the settings window knows about the daemon. */
export type DaemonState = 'checking' | 'running' | 'stopped'

/** What the platform calls its file manager, for "Show in …" labels. */
export function fileManagerName(os: string | undefined): string {
  if (os === 'macos') return 'Finder'
  if (os === 'windows') return 'Explorer'
  return 'the file manager'
}
