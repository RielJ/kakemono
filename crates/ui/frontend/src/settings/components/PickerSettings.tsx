import type { Config } from '../../shared/api'
import { showPicker } from '../../shared/api'
import type { PickerOrientation } from '../../shared/bindings/PickerOrientation'
import type { PickerPosition } from '../../shared/bindings/PickerPosition'
import { isVertical } from '../../picker/orientation'
import { Group, Row } from './Group'
import Segmented from './Segmented'
import { CheckLine, LineText } from './SettingLine'

const POSITIONS: readonly { value: PickerPosition; label: string }[] = [
  { value: 'top', label: 'Top' },
  { value: 'bottom', label: 'Bottom' },
  { value: 'left', label: 'Left' },
  { value: 'right', label: 'Right' },
]

const ORIENTATIONS: readonly { value: PickerOrientation; label: string }[] = [
  { value: 'auto', label: 'Automatic' },
  { value: 'horizontal', label: 'Horizontal' },
  { value: 'vertical', label: 'Vertical' },
]

/** One-line explanation of what the current edge and orientation give. */
export function layoutSummary(position: PickerPosition, orientation: PickerOrientation): string {
  const vertical = isVertical({ position, orientation })
  const along = (position === 'left' || position === 'right') === vertical
  const dir = vertical ? 'A vertical column' : 'A horizontal row'
  return along
    ? `${dir} along the full ${position} edge.`
    : `${dir}, centred on the ${position} edge.`
}

/**
 * A small drawing of a screen with the picker where it will open. Mirrors the Rust placement:
 * on-axis is a strip along the whole edge, off-axis a panel centred on the edge.
 */
function Preview({ position, orientation }: { position: PickerPosition; orientation: PickerOrientation }) {
  const W = 160
  const H = 100
  const vertical = isVertical({ position, orientation })
  const thick = 16
  let x = 0
  let y = 0
  let w = W
  let h = H
  const onAxis = (position === 'left' || position === 'right') === vertical
  if (vertical) {
    w = thick
    h = onAxis ? H : 64
  } else {
    h = thick
    w = onAxis ? W : 100
  }
  if (position === 'right') x = W - w
  else if (position === 'left') x = 0
  else x = (W - w) / 2
  if (position === 'bottom') y = H - h
  else if (position === 'top') y = 0
  else y = (H - h) / 2

  const n = 4
  const cells = Array.from({ length: n }, (_, i) => {
    const pad = 3
    if (vertical) {
      const ch = (h - pad * (n + 1)) / n
      return { x: x + pad, y: y + pad + i * (ch + pad), w: w - pad * 2, h: ch }
    }
    const cw = (w - pad * (n + 1)) / n
    return { x: x + pad + i * (cw + pad), y: y + pad, w: cw, h: h - pad * 2 }
  })

  return (
    <svg
      viewBox={`-2 -2 ${W + 4} ${H + 4}`}
      className="w-44 h-auto shrink-0"
      role="img"
      aria-label={layoutSummary(position, orientation)}
    >
      <rect x={0} y={0} width={W} height={H} rx={6} className="fill-[var(--c-window)] stroke-[var(--c-line-strong)]" strokeWidth={1} />
      <g className="transition-all duration-200 motion-reduce:transition-none">
        <rect x={x} y={y} width={w} height={h} rx={4} className="fill-[var(--c-accent-soft)] stroke-[var(--c-accent)]" strokeWidth={1} />
        {cells.map((c, i) => (
          <rect key={i} x={c.x} y={c.y} width={Math.max(c.w, 0)} height={Math.max(c.h, 0)} rx={1.5} className="fill-[var(--c-accent)]" opacity={i === 1 ? 0.9 : 0.45} />
        ))}
      </g>
    </svg>
  )
}

export default function PickerSettings({
  config,
  onChange,
  os,
  onResetPosition,
}: {
  config: Config
  onChange: (config: Config) => void
  os: string | undefined
  onResetPosition: () => void
}) {
  const { picker } = config
  const set = <K extends keyof Config['picker']>(key: K, value: Config['picker'][K]) =>
    onChange({ ...config, picker: { ...picker, [key]: value } })
  const custom = picker.placement === 'custom'

  return (
    <>
      <Group
        title="Picker"
        action={
          <button type="button" className="btn-plain h-6 text-[12px]" onClick={() => void showPicker().catch(() => {})}>
            Open picker
          </button>
        }
        footer={
          custom
            ? 'You dragged the picker, so it opens where you left it. The edge applies again after Reset position.'
            : undefined
        }
      >
        <div className="flex items-center gap-5 px-4 py-4">
          <Preview position={picker.position} orientation={picker.orientation} />
          <div className="flex-1 min-w-0 space-y-3">
            <Field label="Screen edge">
              {(labelId) => (
                <Segmented value={picker.position} options={POSITIONS} onChange={(v) => set('position', v)} labelledBy={labelId} />
              )}
            </Field>
            <Field label="Orientation">
              {(labelId) => (
                <Segmented
                  value={picker.orientation}
                  options={ORIENTATIONS}
                  onChange={(v) => set('orientation', v)}
                  labelledBy={labelId}
                />
              )}
            </Field>
            <p className="text-[12px] text-muted" aria-live="polite">
              {layoutSummary(picker.position, picker.orientation)}
            </p>
          </div>
        </div>
        {custom && (
          <Row label="Saved position" hint="The picker reopens where you last dragged it.">
            {() => (
              <button type="button" onClick={onResetPosition} className="btn">
                Reset position
              </button>
            )}
          </Row>
        )}
        <CheckLine
          checked={picker.close_on_blur}
          onChange={(v) => set('close_on_blur', v)}
          hint="Clicking anywhere else hides the picker."
        >
          <LineText>Close when it loses focus</LineText>
        </CheckLine>
        <CheckLine
          checked={picker.blur_nsfw}
          onChange={(v) => set('blur_nsfw', v)}
          hint="Online previews marked NSFW stay blurred until you hover or focus them."
        >
          <LineText>Blur NSFW previews</LineText>
        </CheckLine>
        {os === 'linux' && (
          <CheckLine
            checked={picker.force_x11}
            onChange={(v) => set('force_x11', v)}
            hint="Experimental. Lets the picker position itself on Wayland; may look blurry with fractional scaling."
          >
            <LineText>Open through XWayland</LineText>
          </CheckLine>
        )}
      </Group>
    </>
  )
}

function Field({ label, children }: { label: string; children: (labelId: string) => React.ReactNode }) {
  const id = `picker-${label.toLowerCase().replace(/\s+/g, '-')}`
  return (
    <div className="flex items-center justify-between gap-3">
      <span id={id} className="text-[13px] font-medium">
        {label}
      </span>
      {children(id)}
    </div>
  )
}
