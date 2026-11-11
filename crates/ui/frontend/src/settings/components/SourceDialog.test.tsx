import { act, cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import * as api from '../../shared/api'
import type { SourceType } from '../../shared/bindings/SourceType'
import SourceDialog from './SourceDialog'

vi.mock('../../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../shared/api')>()
  return { ...real, checkSource: vi.fn(), selectDirectory: vi.fn(), selectFiles: vi.fn() }
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

async function setup(kind: SourceType, initial?: string, initialName?: string) {
  const onDone = vi.fn()
  const onCancel = vi.fn()
  render(<SourceDialog kind={kind} initial={initial} initialName={initialName} onDone={onDone} onCancel={onCancel} />)
  await act(async () => {})
  return { onDone, onCancel, user: userEvent.setup() }
}

const dialog = () => screen.getByRole('dialog')
/** The location field (the dialog also has an optional Name field). */
const locationField = () => screen.getAllByRole('textbox')[0] as HTMLInputElement
const nameField = () => screen.getByRole('textbox', { name: /^Name/ }) as HTMLInputElement
const ok = () => screen.getByRole('button', { name: 'OK' })

/** A promise and the function that resolves it, for holding `checkSource` open. */
function deferred<T>() {
  let resolve: (v: T) => void = () => {}
  let reject: (e: unknown) => void = () => {}
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

describe('SourceDialog, typed input', () => {
  it('asks for Wallhaven keywords with the type’s prompt; OK is off until something is typed', async () => {
    await setup('wallhaven')
    const field = screen.getByRole('textbox', { name: 'Enter keywords or paste URL here:' })
    expect(document.activeElement).toBe(field)
    expect((ok() as HTMLButtonElement).disabled).toBe(true)
    expect(dialog().getAttribute('aria-modal')).toBe('true')
  })

  it('checks the entry on OK, says so while it waits, and finishes when images are found', async () => {
    const check = deferred<{ count: number }>()
    vi.mocked(api.checkSource).mockReturnValue(check.promise)
    const { onDone, user } = await setup('wallhaven')
    await user.type(locationField(), '  forest  ')
    await user.click(ok())
    expect(api.checkSource).toHaveBeenCalledWith('wallhaven', 'forest')
    expect(screen.getByRole('status').textContent).toMatch('Just a moment to check for images')
    expect((ok() as HTMLButtonElement).disabled).toBe(true)
    await act(async () => check.resolve({ count: 5 }))
    expect(onDone).toHaveBeenCalledWith(['forest'], '')
  })

  it('Enter in the field is OK', async () => {
    vi.mocked(api.checkSource).mockResolvedValue({ count: 3 })
    const { onDone, user } = await setup('reddit')
    await user.type(screen.getByRole('textbox', { name: 'URL or name of a subreddit:' }), 'EarthPorn{Enter}')
    expect(onDone).toHaveBeenCalledWith(['EarthPorn'], '')
  })

  it('shows a check error inline, keeps the dialog open and adds nothing', async () => {
    vi.mocked(api.checkSource).mockRejectedValue({ kind: 'daemon', message: 'no images found' })
    const { onDone, user } = await setup('wallhaven')
    await user.type(locationField(), 'zzzz')
    await user.click(ok())
    expect(screen.getByRole('alert').textContent).toMatch('no images found')
    expect(locationField().getAttribute('aria-invalid')).toBe('true')
    expect(screen.queryByRole('button', { name: 'Add anyway' })).toBeNull()
    expect(onDone).not.toHaveBeenCalled()
    expect(screen.getByRole('dialog')).toBeTruthy()
  })

  it('treats a count of 0 as an error', async () => {
    vi.mocked(api.checkSource).mockResolvedValue({ count: 0 })
    const { onDone, user } = await setup('unsplash')
    await user.type(locationField(), 'nothing')
    await user.click(ok())
    expect(screen.getByRole('alert').textContent).toMatch(/No images found/)
    expect(onDone).not.toHaveBeenCalled()
  })

  it('offers Add anyway only when the daemon can’t be reached', async () => {
    vi.mocked(api.checkSource).mockRejectedValue({ kind: 'daemon_unreachable', message: 'connection refused' })
    const { onDone, user } = await setup('wallhaven')
    await user.type(locationField(), 'forest')
    await user.click(ok())
    expect(screen.getByRole('alert').textContent).toMatch('connection refused')
    expect(onDone).not.toHaveBeenCalled()
    await user.click(screen.getByRole('button', { name: 'Add anyway' }))
    expect(onDone).toHaveBeenCalledWith(['forest'], '')
  })

  it('Cancel and Escape add nothing', async () => {
    const { onDone, onCancel, user } = await setup('wallhaven')
    await user.type(locationField(), 'forest')
    await user.click(screen.getByRole('button', { name: 'Cancel' }))
    await user.keyboard('{Escape}')
    expect(onCancel).toHaveBeenCalled()
    expect(onDone).not.toHaveBeenCalled()
    expect(api.checkSource).not.toHaveBeenCalled()
  })

  it('opens prefilled for Edit and checks again', async () => {
    vi.mocked(api.checkSource).mockResolvedValue({ count: 2 })
    const { onDone, user } = await setup('wallhaven', 'nature')
    const field = locationField()
    expect(field.value).toBe('nature')
    expect(screen.getByRole('heading').textContent).toBe('Edit Wallhaven source')
    await user.clear(field)
    await user.type(field, 'ocean')
    await user.click(ok())
    expect(api.checkSource).toHaveBeenCalledWith('wallhaven', 'ocean')
    expect(onDone).toHaveBeenCalledWith(['ocean'], '')
  })
})

describe('SourceDialog, paths', () => {
  it('opens the folder picker for a new Folders source, then checks and adds the folder', async () => {
    vi.mocked(api.selectDirectory).mockResolvedValue('/home/u/Pictures')
    vi.mocked(api.checkSource).mockResolvedValue({ count: 12 })
    const { onDone } = await setup('folder')
    expect(api.selectDirectory).toHaveBeenCalledTimes(1)
    expect(api.checkSource).toHaveBeenCalledWith('folder', '/home/u/Pictures')
    expect(onDone).toHaveBeenCalledWith(['/home/u/Pictures'], '')
  })

  it('closes without adding when the picker is cancelled', async () => {
    vi.mocked(api.selectDirectory).mockResolvedValue('')
    const { onDone, onCancel } = await setup('album-date')
    expect(onCancel).toHaveBeenCalled()
    expect(onDone).not.toHaveBeenCalled()
  })

  it('Images picks several files and returns one location per file', async () => {
    vi.mocked(api.selectFiles).mockResolvedValue(['/p/a.jpg', '/p/b.png'])
    vi.mocked(api.checkSource).mockResolvedValue({ count: 1 })
    const { onDone } = await setup('images')
    expect(api.selectFiles).toHaveBeenCalledTimes(1)
    expect(api.checkSource).toHaveBeenCalledWith('images', '/p/a.jpg')
    expect(api.checkSource).toHaveBeenCalledWith('images', '/p/b.png')
    expect(onDone).toHaveBeenCalledWith(['/p/a.jpg', '/p/b.png'], '')
  })

  it('keeps the dialog open with the error when a picked folder has no images', async () => {
    vi.mocked(api.selectDirectory).mockResolvedValue('/home/u/empty')
    vi.mocked(api.checkSource).mockRejectedValue({ kind: 'daemon', message: 'no images found' })
    const { onDone, user } = await setup('folder')
    expect(screen.getByRole('alert').textContent).toMatch('no images found')
    expect(onDone).not.toHaveBeenCalled()
    // Choose… picks again.
    vi.mocked(api.selectDirectory).mockResolvedValue('/home/u/full')
    vi.mocked(api.checkSource).mockResolvedValue({ count: 4 })
    await user.click(screen.getByRole('button', { name: 'Choose…' }))
    await user.click(ok())
    expect(onDone).toHaveBeenCalledWith(['/home/u/full'], '')
  })

  it('opens prefilled for Edit without the picker, and checks the path on OK', async () => {
    vi.mocked(api.checkSource).mockResolvedValue({ count: 4 })
    const { onDone, user } = await setup('folder', '/home/u/Pictures')
    expect(api.selectDirectory).not.toHaveBeenCalled()
    const field = screen.getByRole('textbox', { name: 'Folder:' }) as HTMLInputElement
    expect(field.value).toBe('/home/u/Pictures')
    await user.click(ok())
    expect(api.checkSource).toHaveBeenCalledWith('folder', '/home/u/Pictures')
    expect(onDone).toHaveBeenCalledWith(['/home/u/Pictures'], '')
  })
})

describe('SourceDialog, name', () => {
  it('passes a trimmed name with the location', async () => {
    vi.mocked(api.checkSource).mockResolvedValue({ count: 3 })
    const { onDone, user } = await setup('wallhaven')
    await user.type(locationField(), 'https://wallhaven.cc/search?sorting=hot')
    await user.type(nameField(), '  wallhaven-hot  ')
    await user.click(ok())
    expect(onDone).toHaveBeenCalledWith(['https://wallhaven.cc/search?sorting=hot'], 'wallhaven-hot')
  })

  it('Edit shows the current name, and the name alone doesn’t enable OK', async () => {
    const { user } = await setup('reddit', '', 'Earth top')
    expect(nameField().value).toBe('Earth top')
    await user.clear(nameField())
    await user.type(nameField(), 'x')
    expect(ok().hasAttribute('disabled')).toBe(true)
  })

  it('caps the name at 64 characters', async () => {
    await setup('wallhaven')
    expect(nameField().maxLength).toBe(64)
  })
})
