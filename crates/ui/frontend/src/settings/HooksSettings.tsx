import { useId } from 'react'

import type { Config } from '../shared/api'
import ArgvEditor from './ArgvEditor'
import { Group } from './components/Group'
import { normalizeArgv } from './argv'

/** Variables the daemon sets for `on_change` (engine `hooks.rs`). */
export const HOOK_ENV: readonly { name: string; text: string }[] = [
  { name: 'KAKEMONO_PATH', text: 'Absolute path of the new wallpaper' },
  { name: 'KAKEMONO_SOURCE', text: 'Source that supplied it, e.g. bing or local' },
  { name: 'KAKEMONO_TITLE', text: 'Title, when the source has one' },
  { name: 'KAKEMONO_AUTHOR', text: 'Author or photographer, when known' },
  { name: 'KAKEMONO_SOURCE_URL', text: 'Page the image came from' },
  { name: 'KAKEMONO_THEME', text: 'light or dark' },
]

/** Copied from the daemon's environment; everything else is cleared. */
const PASSED_THROUGH = ['PATH', 'HOME', 'DISPLAY', 'WAYLAND_DISPLAY', 'XDG_RUNTIME_DIR', 'DBUS_SESSION_BUS_ADDRESS']

const code = 'font-mono text-[11px] px-1 py-0.5 rounded bg-gray-100 dark:bg-gray-700/70'

export default function HooksSettings({
  config,
  onChange,
  error,
}: {
  config: Config
  onChange: (c: Config) => void
  error?: string | null
}) {
  const hintId = useId()
  const tokens = config.hooks.on_change ?? []
  const setTokens = (next: string[]) => onChange({ ...config, hooks: { ...config.hooks, on_change: normalizeArgv(next) } })

  return (
    <div className="space-y-6 max-w-2xl">
      <Group title="On wallpaper change">
        <div className="px-4 py-3 space-y-3">
          <p id={hintId} className="text-xs text-gray-500 dark:text-gray-400">
            Runs after every change, without a shell, and is stopped after 10 seconds. The wallpaper never waits for
            it; a failing hook is only logged. To use shell syntax, add <span className={code}>sh</span>,{' '}
            <span className={code}>-c</span> and the script as three tokens.
          </p>
          <ArgvEditor
            label="Hook command"
            tokens={tokens}
            onChange={setTokens}
            error={error}
            hintId={hintId}
            emptyText="No hook. Add a token to run a command on every change."
          />
        </div>
      </Group>

      <Group title="Environment">
        <div className="px-4 py-3 space-y-3">
          <table className="w-full text-xs">
            <caption className="sr-only">Variables set for the hook</caption>
            <tbody className="divide-y divide-gray-100 dark:divide-gray-700/70">
              {HOOK_ENV.map((v) => (
                <tr key={v.name}>
                  <th scope="row" className="py-1.5 pr-4 text-left font-mono font-normal whitespace-nowrap">
                    {v.name}
                  </th>
                  <td className="py-1.5 text-gray-600 dark:text-gray-300">{v.text}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <p className="text-xs text-gray-500 dark:text-gray-400">
            Tokens can also use <span className={code}>{'{path}'}</span>, <span className={code}>{'{uri}'}</span>,{' '}
            <span className={code}>{'{fit}'}</span> and <span className={code}>{'{theme}'}</span>; a value never splits
            a token. Other variables are cleared except {PASSED_THROUGH.join(', ')} (and SystemRoot and USERPROFILE on
            Windows).
          </p>
        </div>
      </Group>
    </div>
  )
}
