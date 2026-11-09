import type { Config } from '../types/config'
import { openFolder, selectDirectory } from '../lib/api'

interface Props {
  config: Config
  onChange: (config: Config) => void
}

export default function FavoritesSettings({ config, onChange }: Props) {
  const setDir = (dir: string) =>
    onChange({ ...config, favorites: { dir } })

  const handleOpen = async () => {
    try {
      await openFolder(config.favorites.dir)
    } catch (err) {
      console.error('Failed to open favorites folder:', err)
    }
  }

  return (
    <div className="space-y-5 max-w-xl">
      <div>
        <h3 className="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400 mb-2">
          Favorites
        </h3>
        <div className="bg-white/80 dark:bg-zinc-800/70 rounded-xl border border-zinc-200 dark:border-zinc-700">
          <div className="flex items-center justify-between px-4 py-3 gap-4">
            <div className="flex-1 min-w-0">
              <p className="text-sm font-medium">Favorites directory</p>
              <p className="text-xs text-zinc-400 dark:text-zinc-500 mt-0.5">Wallpapers you mark as favorites are copied here</p>
            </div>
            <div className="flex items-center gap-2 shrink-0">
              <input
                type="text"
                value={config.favorites.dir}
                onChange={e => setDir(e.target.value)}
                placeholder="~/Pictures/Wallpapers/favorites"
                className="form-input w-44 text-xs py-1 font-mono"
              />
              <button
                onClick={async () => { const p = await selectDirectory(); if (p) setDir(p) }}
                className="btn-secondary no-drag text-xs py-1 px-2"
              >
                Browse
              </button>
              <button
                onClick={handleOpen}
                className="btn-secondary no-drag text-xs py-1 px-2"
                title="Open in Finder"
              >
                Open
              </button>
            </div>
          </div>
        </div>
      </div>

      <div className="rounded-xl bg-blue-50 dark:bg-blue-900/20 border border-blue-200 dark:border-blue-800 px-4 py-3 text-xs text-blue-700 dark:text-blue-300">
        <p className="font-medium mb-1">How to favorite a wallpaper</p>
        <ul className="list-disc list-inside space-y-0.5 text-blue-600 dark:text-blue-400">
          <li>Click the ♥ button in the status bar above</li>
          <li>Run <code className="font-mono bg-blue-100 dark:bg-blue-900 px-1 rounded">wallpaperd favorite</code> in terminal</li>
        </ul>
      </div>
    </div>
  )
}
