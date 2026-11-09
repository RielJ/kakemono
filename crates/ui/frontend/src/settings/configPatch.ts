import type { Config, ConfigPatch } from '../shared/api'

type Json = null | boolean | number | string | Json[] | { [key: string]: Json }
type JsonObject = { [key: string]: Json }

function isObject(v: unknown): v is JsonObject {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
}

/** Changed keys of `b` relative to `a`; `undefined` when nothing changed. */
function diffObject(a: JsonObject, b: JsonObject): JsonObject | undefined {
  const out: JsonObject = {}
  let changed = false
  for (const [key, next] of Object.entries(b)) {
    const prev = a[key]
    if (isObject(prev) && isObject(next)) {
      const child = diffObject(prev, next)
      if (child) {
        out[key] = child
        changed = true
      }
    } else if (JSON.stringify(prev) !== JSON.stringify(next)) {
      // Arrays and scalars replace the whole value; `null` removes the key.
      out[key] = next
      changed = true
    }
  }
  return changed ? out : undefined
}

/**
 * The minimal `save_config` patch that turns `saved` into `next`. Only changed keys are sent,
 * so untouched values (including `~/…` paths the backend expanded on load) stay as written.
 */
export function diffConfig(saved: Config, next: Config): ConfigPatch {
  const a: unknown = saved
  const b: unknown = next
  if (!isObject(a) || !isObject(b)) return {}
  const patch = diffObject(a, b) ?? {}
  // Every key and value in `patch` was copied from `next`, a `Config`, so it is a valid
  // `Patch<Config>`; TypeScript can't follow that through the JSON walk.
  return patch as ConfigPatch
}

/** True when the patch has nothing to write. */
export function isEmptyPatch(patch: ConfigPatch): boolean {
  return Object.keys(patch).length === 0
}
