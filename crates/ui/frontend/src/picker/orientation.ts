import type { PickerOrientation } from '../shared/bindings/PickerOrientation'
import type { PickerPosition } from '../shared/bindings/PickerPosition'

/** `[picker]` fields that decide the strip's direction. */
export interface PickerLayout {
  position: PickerPosition
  orientation: PickerOrientation
}

export const DEFAULT_LAYOUT: PickerLayout = { position: 'bottom', orientation: 'auto' }

/**
 * Reads `position` and `orientation` from a config value, falling back to the defaults for
 * missing or unknown values (an older daemon may not send `orientation`).
 */
export function toLayout(picker: { position?: unknown; orientation?: unknown } | undefined): PickerLayout {
  const p = picker?.position
  const o = picker?.orientation
  return {
    position: p === 'top' || p === 'left' || p === 'right' || p === 'bottom' ? p : DEFAULT_LAYOUT.position,
    orientation: o === 'horizontal' || o === 'vertical' || o === 'auto' ? o : DEFAULT_LAYOUT.orientation,
  }
}

/**
 * Whether the strip runs vertically. Same rule as `PickerOrientation::is_vertical` in
 * `crates/config/src/schema.rs`: `auto` is vertical on the left and right edges.
 */
export function isVertical({ position, orientation }: PickerLayout): boolean {
  switch (orientation) {
    case 'vertical':
      return true
    case 'horizontal':
      return false
    case 'auto':
      return position === 'left' || position === 'right'
  }
}
