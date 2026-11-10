import { act, cleanup, renderHook, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { MAX_DROP_BYTES, MESSAGES, useImageDrop, type DropNotice } from './useImageDrop'

const ipc = vi.hoisted(() => ({
  invoke: vi.fn<(cmd: string, args?: unknown, options?: unknown) => Promise<unknown>>(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: ipc.invoke }))

const INFO = { path: '/m/a.jpg', source: 'manual', source_url: '', image_url: '', title: '', author: '', width: 0, height: 0, set_at: '' }

interface FakeTransfer {
  data?: Record<string, string>
  files?: File[]
}

/** A `DataTransfer` stand-in: jsdom has none. `types` follows the data and files given. */
function transfer({ data = {}, files = [] }: FakeTransfer = {}) {
  const types = [...(files.length > 0 ? ['Files'] : []), ...Object.keys(data)]
  return { types, files, getData: (type: string) => data[type] ?? '', dropEffect: 'none' }
}

/** Dispatches a synthetic drag event carrying `dt` on `target`; returns the event. */
function fire(type: string, dt: ReturnType<typeof transfer>, target: EventTarget = document.body): Event {
  const event = new Event(type, { bubbles: true, cancelable: true })
  Object.defineProperty(event, 'dataTransfer', { value: dt })
  act(() => {
    target.dispatchEvent(event)
  })
  return event
}

/** A full drag: enter, over, drop. */
function drop(dt: ReturnType<typeof transfer>, target: EventTarget = document.body): Event {
  fire('dragenter', dt, target)
  fire('dragover', dt, target)
  return fire('drop', dt, target)
}

function setup() {
  const notices: DropNotice[] = []
  const onNotice = vi.fn((n: DropNotice) => notices.push(n))
  const hook = renderHook(() => useImageDrop({ onNotice }))
  const last = () => notices[notices.length - 1]
  return { hook, notices, onNotice, last }
}

const png = (name = 'a.png', bytes = [0x89, 0x50, 0x4e, 0x47]) => new File([new Uint8Array(bytes)], name, { type: 'image/png' })

beforeEach(() => {
  ipc.invoke.mockReset()
  ipc.invoke.mockResolvedValue(INFO)
})
afterEach(() => {
  cleanup()
  document.body.innerHTML = ''
})

describe('useImageDrop', () => {
  it('fetches an https URL from text/uri-list', async () => {
    const { last } = setup()
    const event = drop(transfer({ data: { 'text/uri-list': 'https://images.example.com/full/a.jpg' } }))

    expect(event.defaultPrevented).toBe(true)
    await waitFor(() => expect(last()).toEqual({ kind: 'done', text: 'Set from a.jpg' }))
    expect(ipc.invoke).toHaveBeenCalledTimes(1)
    expect(ipc.invoke).toHaveBeenCalledWith('fetch_dropped_url', { url: 'https://images.example.com/full/a.jpg' })
  })

  it('skips # comment lines in text/uri-list', async () => {
    const { last } = setup()
    drop(transfer({ data: { 'text/uri-list': '# dragged from a page\r\nhttps://x.example/b.png\r\nhttps://x.example/c.png' } }))

    await waitFor(() => expect(last()?.kind).toBe('done'))
    expect(ipc.invoke).toHaveBeenCalledWith('fetch_dropped_url', { url: 'https://x.example/b.png' })
  })

  it('falls back to the <img src> in text/html', async () => {
    const { last } = setup()
    drop(transfer({ data: { 'text/html': '<meta charset="utf-8"><img alt="x" src="https://cdn.example/p/c.webp?w=2">' } }))

    await waitFor(() => expect(last()?.kind).toBe('done'))
    expect(ipc.invoke).toHaveBeenCalledWith('fetch_dropped_url', { url: 'https://cdn.example/p/c.webp?w=2' })
  })

  it('names a URL without a file name by its host', async () => {
    const { last } = setup()
    drop(transfer({ data: { 'text/uri-list': 'https://images.example.com/' } }))

    await waitFor(() => expect(last()).toEqual({ kind: 'done', text: 'Set from images.example.com' }))
  })

  it('ignores data: and blob: URLs with a message', async () => {
    const { last } = setup()
    drop(transfer({ data: { 'text/uri-list': 'data:image/png;base64,AAAA', 'text/html': '<img src="blob:https://x/1">' } }))

    await waitFor(() => expect(last()).toEqual({ kind: 'error', text: MESSAGES.embedded }))
    expect(ipc.invoke).not.toHaveBeenCalled()
  })

  it('imports a dropped image/png File with its name and bytes', async () => {
    const { last } = setup()
    drop(transfer({ files: [png('photo.png', [1, 2, 3])] }))

    await waitFor(() => expect(last()).toEqual({ kind: 'done', text: 'Set from photo.png' }))
    expect(ipc.invoke).toHaveBeenCalledTimes(1)
    const [cmd, body, options] = ipc.invoke.mock.calls[0]
    expect(cmd).toBe('import_dropped_file')
    expect(Array.from(new Uint8Array(body as ArrayBuffer))).toEqual([1, 2, 3])
    expect(options).toEqual({ headers: { 'x-file-name': 'photo.png' } })
  })

  it('prefers an image File over the URLs that come with it', async () => {
    const { last } = setup()
    drop(transfer({ files: [png()], data: { 'text/uri-list': 'https://x.example/a.png' } }))

    await waitFor(() => expect(last()?.kind).toBe('done'))
    expect(ipc.invoke).toHaveBeenCalledTimes(1)
    expect(ipc.invoke.mock.calls[0][0]).toBe('import_dropped_file')
  })

  it('uses the URL when the File is not an image', async () => {
    const { last } = setup()
    const page = new File(['<html>'], 'page.html', { type: 'text/html' })
    drop(transfer({ files: [page], data: { 'text/uri-list': 'https://x.example/a.png' } }))

    await waitFor(() => expect(last()?.kind).toBe('done'))
    expect(ipc.invoke).toHaveBeenCalledWith('fetch_dropped_url', { url: 'https://x.example/a.png' })
  })

  it('adds the extension the backend needs when the File name has none', async () => {
    setup()
    drop(transfer({ files: [new File([new Uint8Array([1])], 'image', { type: 'image/jpeg' })] }))

    await waitFor(() => expect(ipc.invoke).toHaveBeenCalled())
    expect(ipc.invoke.mock.calls[0][2]).toEqual({ headers: { 'x-file-name': 'image.jpg' } })
  })

  it('ignores a non-image file with a message', async () => {
    const { last } = setup()
    drop(transfer({ files: [new File(['hello'], 'notes.txt', { type: 'text/plain' })] }))

    await waitFor(() => expect(last()).toEqual({ kind: 'error', text: MESSAGES.notImage }))
    expect(MESSAGES.notImage).toBe('Drop a JPEG, PNG or WebP image')
    expect(ipc.invoke).not.toHaveBeenCalled()
  })

  it('uses only the first item of a multi-drop', async () => {
    const { last } = setup()
    drop(transfer({ files: [png('first.png'), png('second.png')] }))

    await waitFor(() => expect(last()?.kind).toBe('done'))
    expect(ipc.invoke).toHaveBeenCalledTimes(1)
    expect(ipc.invoke.mock.calls[0][2]).toEqual({ headers: { 'x-file-name': 'first.png' } })
  })

  it('refuses a file over 64 MB without reading it', async () => {
    const { last } = setup()
    const big = png('huge.png')
    Object.defineProperty(big, 'size', { value: MAX_DROP_BYTES + 1 })
    const read = vi.spyOn(big, 'arrayBuffer')
    drop(transfer({ files: [big] }))

    await waitFor(() => expect(last()).toEqual({ kind: 'error', text: MESSAGES.tooBig }))
    expect(read).not.toHaveBeenCalled()
    expect(ipc.invoke).not.toHaveBeenCalled()
  })

  it('surfaces a command error', async () => {
    ipc.invoke.mockRejectedValue({ kind: 'daemon_unreachable', message: 'the daemon is not running' })
    const { last } = setup()
    drop(transfer({ data: { 'text/uri-list': 'https://x.example/a.png' } }))

    await waitFor(() => expect(last()).toEqual({ kind: 'error', text: 'the daemon is not running' }))
  })

  it('reports progress before the result', async () => {
    const { notices } = setup()
    drop(transfer({ data: { 'text/uri-list': 'https://x.example/a.png' } }))

    await waitFor(() => expect(notices.map((n) => n.kind)).toEqual(['busy', 'done']))
    expect(notices[0].text).toBe('Setting a.png…')
  })

  it('shows the overlay on dragover with files', () => {
    const { hook } = setup()
    expect(hook.result.current.over).toBe(false)
    const event = fire('dragover', transfer({ files: [png()] }))
    expect(event.defaultPrevented).toBe(true)
    expect(hook.result.current.over).toBe(true)
  })

  it('shows the overlay on dragenter with a URL and hides it on drop', () => {
    const { hook } = setup()
    const dt = transfer({ data: { 'text/uri-list': 'https://x.example/a.png' } })
    fire('dragenter', dt)
    expect(hook.result.current.over).toBe(true)
    fire('drop', dt)
    expect(hook.result.current.over).toBe(false)
  })

  it('keeps the overlay while the pointer crosses child elements', () => {
    const { hook } = setup()
    const child = document.createElement('div')
    document.body.append(child)
    const dt = transfer({ files: [png()] })

    fire('dragenter', dt, document.body)
    fire('dragenter', dt, child)
    fire('dragleave', dt, document.body)
    expect(hook.result.current.over).toBe(true)
    fire('dragleave', dt, child)
    expect(hook.result.current.over).toBe(false)
  })

  it('ignores drags with neither files nor URLs', () => {
    const { hook, onNotice } = setup()
    const dt = transfer({ data: { 'text/plain': 'hello' } })
    const over = fire('dragover', dt)
    fire('dragenter', dt)
    expect(over.defaultPrevented).toBe(false)
    expect(hook.result.current.over).toBe(false)
    fire('drop', dt)
    expect(onNotice).not.toHaveBeenCalled()
  })

  it('leaves drops on text fields to the field', () => {
    const { hook, onNotice } = setup()
    const input = document.createElement('input')
    document.body.append(input)
    const event = drop(transfer({ data: { 'text/uri-list': 'https://x.example/a.png' } }), input)

    expect(event.defaultPrevented).toBe(false)
    expect(hook.result.current.over).toBe(false)
    expect(onNotice).not.toHaveBeenCalled()
    expect(ipc.invoke).not.toHaveBeenCalled()
  })

  it('ignores a drag that started inside the window', () => {
    const { hook, onNotice } = setup()
    const dt = transfer({ data: { 'text/uri-list': 'https://x.example/a.png' } })
    fire('dragstart', dt)
    const dropped = drop(dt)
    fire('dragend', dt)

    // Still cancelled, so the webview doesn't navigate to the dragged link.
    expect(dropped.defaultPrevented).toBe(true)
    expect(hook.result.current.over).toBe(false)
    expect(onNotice).not.toHaveBeenCalled()

    // The next drag from outside works again.
    fire('dragenter', dt)
    expect(hook.result.current.over).toBe(true)
  })

  it('stops listening on unmount', () => {
    const { hook, onNotice } = setup()
    hook.unmount()
    const event = drop(transfer({ files: [png()] }))
    expect(event.defaultPrevented).toBe(false)
    expect(onNotice).not.toHaveBeenCalled()
  })
})
