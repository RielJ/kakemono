import { useEffect, useId, useState, type ReactNode } from 'react'

/** How long "Copied" stays on screen. */
const COPIED_MS = 1500

type CopyState = { kind: 'idle' } | { kind: 'copied'; text: string } | { kind: 'failed'; text: string }

/** A command or config line in monospace, with a Copy button. */
function CopyLine({ text, state, onCopy }: { text: string; state: CopyState; onCopy: (text: string) => void }) {
  const message = state.kind !== 'idle' && state.text === text ? state : null
  return (
    <div className="flex items-center gap-2">
      <code className="selectable flex-1 min-w-0 truncate rounded-md bg-hover px-2 py-1 font-mono text-[12px]">
        {text}
      </code>
      {message && (
        <span role="status" className={`text-[12px] ${message.kind === 'copied' ? 'text-ok' : 'text-danger'}`}>
          {message.kind === 'copied' ? 'Copied' : 'Couldn’t copy; select the text instead'}
        </span>
      )}
      <button
        type="button"
        className="btn shrink-0"
        aria-label={`Copy ${text}`}
        onClick={() => onCopy(text)}
      >
        Copy
      </button>
    </div>
  )
}

interface HowTo {
  name: string
  steps: ReactNode
  /** A line to paste into a config file. */
  line?: string
}

const MACOS: readonly HowTo[] = [
  {
    name: 'Shortcuts app',
    steps: (
      <>
        Make a shortcut with a <b>Run Shell Script</b> action that runs the command. In the
        shortcut’s details, choose <b>Add Keyboard Shortcut</b>.
      </>
    ),
  },
  {
    name: 'Raycast',
    steps: (
      <>
        Create a <b>Script Command</b> that runs the command, then give it a hotkey in Raycast’s
        Extensions settings.
      </>
    ),
  },
  {
    name: 'skhd',
    steps: (
      <>
        Add a line like this to <code className="font-mono text-[12px]">~/.skhdrc</code>:
      </>
    ),
    line: 'alt - p : kakemono pick',
  },
]

const LINUX: readonly HowTo[] = [
  {
    name: 'GNOME',
    steps: (
      <>
        Settings → Keyboard → <b>View and Customize Shortcuts</b> → Custom Shortcuts. Add one with
        the command.
      </>
    ),
  },
  {
    name: 'KDE Plasma',
    steps: (
      <>
        System Settings → Keyboard → Shortcuts → <b>Add New</b> → Command or Script. Paste the
        command and record a key.
      </>
    ),
  },
  {
    name: 'Hyprland',
    steps: (
      <>
        Add to <code className="font-mono text-[12px]">hyprland.conf</code>:
      </>
    ),
    line: 'bind = SUPER, P, exec, kakemono pick',
  },
  {
    name: 'Sway',
    steps: (
      <>
        Add to <code className="font-mono text-[12px]">~/.config/sway/config</code>:
      </>
    ),
    line: 'bindsym $mod+p exec kakemono pick',
  },
  {
    name: 'niri',
    steps: (
      <>
        Add inside <code className="font-mono text-[12px]">binds {'{ }'}</code> in{' '}
        <code className="font-mono text-[12px]">config.kdl</code>:
      </>
    ),
    line: 'Mod+P { spawn "kakemono" "pick"; }',
  },
]

const WINDOWS: readonly HowTo[] = [
  {
    name: 'Windows',
    steps: (
      <>
        Make a desktop shortcut that runs the command, then set a <b>Shortcut key</b> in its
        Properties.
      </>
    ),
  },
]

function howTos(os: string | undefined): readonly HowTo[] {
  if (os === 'macos') return MACOS
  if (os === 'linux') return LINUX
  if (os === 'windows') return WINDOWS
  return []
}

/**
 * The Shortcuts hint: kakemono registers no global shortcut, so this shows the two commands to
 * bind and short how-tos for the launchers and compositors common on the user's OS.
 */
export default function ShortcutHint({ os }: { os: string | undefined }) {
  const [copy, setCopy] = useState<CopyState>({ kind: 'idle' })
  const headingBase = useId()

  useEffect(() => {
    if (copy.kind !== 'copied') return
    const t = setTimeout(() => setCopy({ kind: 'idle' }), COPIED_MS)
    return () => clearTimeout(t)
  }, [copy])

  const onCopy = (text: string) => {
    navigator.clipboard
      .writeText(text)
      .then(() => setCopy({ kind: 'copied', text }))
      .catch(() => setCopy({ kind: 'failed', text }))
  }

  return (
    <div className="px-4 py-3 space-y-4">
      <div className="space-y-2">
        <p className="text-muted text-[12px] leading-snug">
          kakemono has no global shortcut of its own. Bind a key to one of these commands:
        </p>
        <dl className="space-y-1.5">
          <div>
            <dt className="text-[12px] text-muted mb-0.5">Open the picker</dt>
            <dd>
              <CopyLine text="kakemono pick" state={copy} onCopy={onCopy} />
            </dd>
          </div>
          <div>
            <dt className="text-[12px] text-muted mb-0.5">Open these settings</dt>
            <dd>
              <CopyLine text="kakemono settings" state={copy} onCopy={onCopy} />
            </dd>
          </div>
        </dl>
      </div>
      {howTos(os).map((h, i) => (
        <section key={h.name} aria-labelledby={`${headingBase}-${i}`} className="space-y-1.5">
          <h4 id={`${headingBase}-${i}`} className="text-[13px] font-medium">
            {h.name}
          </h4>
          <p className="text-[12px] text-muted leading-snug">{h.steps}</p>
          {h.line && <CopyLine text={h.line} state={copy} onCopy={onCopy} />}
        </section>
      ))}
    </div>
  )
}
