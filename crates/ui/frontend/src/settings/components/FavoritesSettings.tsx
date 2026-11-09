import type { Config } from '../../shared/api'
import { openFolder, selectDirectory } from '../../shared/api'
import { Group, Row } from './Group'

interface Props {
  config: Config
  onChange: (config: Config) => void
}

export default function FavoritesSettings({ config, onChange }: Props) {
  const setDir = (dir: string) => onChange({ ...config, favorites: { dir } })

  return (
    <div className="space-y-5 max-w-2xl">
      <Group title="Favorites">
        <Row label="Favorites folder" hint="Wallpapers you favorite are copied here.">
          {({ labelId, hintId }) => (
            <div className="flex items-center gap-2">
              <input
                type="text"
                value={config.favorites.dir}
                onChange={(e) => setDir(e.target.value)}
                placeholder="~/Pictures/Wallpapers/favorites"
                aria-labelledby={labelId}
                aria-describedby={hintId}
                spellCheck={false}
                className="form-input w-48 text-xs py-1 font-mono"
              />
              <button
                type="button"
                onClick={async () => {
                  const p = await selectDirectory()
                  if (p) setDir(p)
                }}
                className="btn-secondary text-xs py-1 px-2"
              >
                Browse…
              </button>
              <button
                type="button"
                onClick={() => void openFolder(config.favorites.dir).catch(() => {})}
                className="btn-secondary text-xs py-1 px-2"
              >
                Open
              </button>
            </div>
          )}
        </Row>
      </Group>

      <div className="text-xs text-gray-600 dark:text-gray-300 space-y-1">
        <p className="font-medium">To favorite the current wallpaper</p>
        <ul className="list-disc pl-4 space-y-0.5 text-gray-500 dark:text-gray-400">
          <li>Use Favorite in the bar at the top of this window or in the tray menu.</li>
          <li>
            Run <code className="font-mono px-1 rounded bg-gray-100 dark:bg-gray-800">kakemono favorite</code> in a terminal.
          </li>
        </ul>
      </div>
    </div>
  )
}
