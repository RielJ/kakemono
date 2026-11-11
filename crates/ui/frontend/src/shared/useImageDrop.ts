import { useEffect, useRef, useState } from 'react'

import { errorMessage, fetchDroppedUrl, importDroppedFile } from './api'

/** The backend's cap (`MAX_DROP_BYTES` in `commands_sources/mod.rs`). Checked before reading. */
export const MAX_DROP_BYTES = 64 * 1024 * 1024

export const MESSAGES = {
  notImage: 'Drop a JPEG, PNG or WebP image',
  tooBig: 'That image is over 64 MB',
  embedded: 'That image is embedded in the page. Save it, then drop the file.',
} as const

/** What a drop reports: `busy` while it is fetched or copied, then `done` or `error`. */
export interface DropNotice {
  kind: 'busy' | 'done' | 'error'
  text: string
}

/** The drag-and-drop types worth showing the overlay for (`getData` is unreadable until the drop). */
const ACCEPTED_TYPES = ['Files', 'text/uri-list', 'text/html']

/** MIME type → the extension `import_dropped_file` requires. */
const IMAGE_TYPES = new Map([
  ['image/jpeg', 'jpg'],
  ['image/png', 'png'],
  ['image/webp', 'webp'],
])
const IMAGE_EXTS = ['jpg', 'jpeg', 'png', 'webp']

const TEXT_INPUT_TYPES = ['text', 'search', 'url', 'email', 'tel', 'password', 'number', '']

/** A field the browser drops text into; drops there are left alone. */
function isTextField(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false
  const field = target.closest('input, textarea, [contenteditable]')
  if (field instanceof HTMLTextAreaElement) return true
  if (field instanceof HTMLInputElement) return TEXT_INPUT_TYPES.includes(field.getAttribute('type') ?? '')
  return field instanceof HTMLElement && field.isContentEditable
}

function accepts(dt: DataTransfer | null): dt is DataTransfer {
  return dt !== null && Array.from(dt.types).some((t) => ACCEPTED_TYPES.includes(t))
}

function extension(name: string): string {
  const dot = name.lastIndexOf('.')
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : ''
}

/** A JPEG, PNG or WebP file, by MIME type, or by extension when the type is empty. */
function isImageFile(file: File): boolean {
  return IMAGE_TYPES.has(file.type) || (file.type === '' && IMAGE_EXTS.includes(extension(file.name)))
}

/** The file name with an extension the backend accepts (browser drags may give `image`). */
function importName(file: File): string {
  const name = file.name.trim() || 'image'
  if (IMAGE_EXTS.includes(extension(name))) return name
  return `${name}.${IMAGE_TYPES.get(file.type) ?? 'jpg'}`
}

/** `uri-list` lines without `#` comments, then the first `<img src>` of `text/html`. */
function candidateUrls(dt: DataTransfer): string[] {
  const list = dt
    .getData('text/uri-list')
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line !== '' && !line.startsWith('#'))
  const html = dt.getData('text/html')
  if (html) {
    // DOMParser documents are inert: nothing loads.
    const src = new DOMParser().parseFromString(html, 'text/html').querySelector('img[src]')?.getAttribute('src')
    if (src) list.push(src.trim())
  }
  return list
}

function parseUrl(raw: string): URL | null {
  try {
    return new URL(raw)
  } catch {
    return null
  }
}

/** Last path segment, or the host when the path has none. */
function urlName(url: URL): string {
  const last = url.pathname.split('/').filter(Boolean).pop()
  if (!last) return url.host || url.href
  try {
    return decodeURIComponent(last)
  } catch {
    return last
  }
}

type Plan = { kind: 'file'; file: File } | { kind: 'url'; url: string; name: string } | { kind: 'refuse'; text: string }

/** What a drop does: an image File first, then the first http(s) or file URL. */
function planDrop(dt: DataTransfer): Plan {
  const file: File | undefined = dt.files[0]
  if (file && isImageFile(file)) {
    return file.size > MAX_DROP_BYTES ? { kind: 'refuse', text: MESSAGES.tooBig } : { kind: 'file', file }
  }
  const urls = candidateUrls(dt).map(parseUrl)
  const usable = urls.find((u) => u !== null && ['http:', 'https:', 'file:'].includes(u.protocol))
  if (usable) return { kind: 'url', url: usable.href, name: urlName(usable) }
  if (urls.some((u) => u !== null && ['data:', 'blob:'].includes(u.protocol))) {
    return { kind: 'refuse', text: MESSAGES.embedded }
  }
  return { kind: 'refuse', text: MESSAGES.notImage }
}

async function runDrop(plan: Plan, onNotice: (notice: DropNotice) => void): Promise<void> {
  if (plan.kind === 'refuse') {
    onNotice({ kind: 'error', text: plan.text })
    return
  }
  const name = plan.kind === 'file' ? plan.file.name || 'image' : plan.name
  onNotice({ kind: 'busy', text: `Setting ${name}…` })
  try {
    if (plan.kind === 'file') await importDroppedFile(importName(plan.file), await plan.file.arrayBuffer())
    else await fetchDroppedUrl(plan.url)
    onNotice({ kind: 'done', text: `Set from ${name}` })
  } catch (err) {
    onNotice({ kind: 'error', text: errorMessage(err) })
  }
}

/**
 * Window-wide drop target for an image file or image URL: it is copied into `manual_dir` and
 * set as the wallpaper. `over` is true while an acceptable drag is over the window (show the
 * overlay). Progress, success and errors go to `onNotice`. Drops on text fields, and drags
 * that start inside the window, are left alone.
 *
 * Needs the native drag-drop handler off (`disable_drag_drop_handler` on the window builder):
 * with it on, Tauri takes the drop and the page never sees it.
 */
export function useImageDrop({ onNotice }: { onNotice: (notice: DropNotice) => void }): { over: boolean } {
  const [over, setOver] = useState(false)
  const notify = useRef(onNotice)
  notify.current = onNotice

  useEffect(() => {
    // dragenter/dragleave fire for every child crossed; the overlay hides when they balance.
    let depth = 0
    let internal = false

    const onDragStart = () => {
      internal = true
    }
    const onDragEnd = () => {
      internal = false
    }
    const onDragEnter = (e: DragEvent) => {
      if (internal || !accepts(e.dataTransfer)) return
      depth += 1
      setOver(!isTextField(e.target))
    }
    const onDragOver = (e: DragEvent) => {
      if (!accepts(e.dataTransfer) || isTextField(e.target)) {
        if (!internal) setOver(false)
        return
      }
      // Cancelling dragover is what lets the drop happen, and stops the webview from
      // navigating to a dropped link or file.
      e.preventDefault()
      if (internal) {
        e.dataTransfer.dropEffect = 'none'
        return
      }
      e.dataTransfer.dropEffect = 'copy'
      setOver(true)
    }
    const onDragLeave = (e: DragEvent) => {
      if (internal || !accepts(e.dataTransfer)) return
      depth = Math.max(0, depth - 1)
      if (depth === 0) setOver(false)
    }
    const onDrop = (e: DragEvent) => {
      const dt = e.dataTransfer
      if (!accepts(dt) || isTextField(e.target)) return
      e.preventDefault()
      depth = 0
      setOver(false)
      if (internal) return
      void runDrop(planDrop(dt), (n) => notify.current(n))
    }

    window.addEventListener('dragstart', onDragStart)
    window.addEventListener('dragend', onDragEnd)
    window.addEventListener('dragenter', onDragEnter)
    window.addEventListener('dragover', onDragOver)
    window.addEventListener('dragleave', onDragLeave)
    window.addEventListener('drop', onDrop)
    return () => {
      window.removeEventListener('dragstart', onDragStart)
      window.removeEventListener('dragend', onDragEnd)
      window.removeEventListener('dragenter', onDragEnter)
      window.removeEventListener('dragover', onDragOver)
      window.removeEventListener('dragleave', onDragLeave)
      window.removeEventListener('drop', onDrop)
    }
  }, [])

  return { over }
}
