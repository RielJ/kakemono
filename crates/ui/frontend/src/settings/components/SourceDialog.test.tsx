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
const ok = () => screen.getByRole('button', { name: 'OK' }) as HTMLButtonElement

describe('SourceDialog, typed input', () => {
  it('asks for Wallhaven keywords with the type’s prompt; OK is off until something is typed', async () => {
    await setup('wallhaven')
    const field = screen.getByRole('textbox', { name: 'Enter keywords or paste URL here:' })
    expect(document.activeElement).toBe(field)
    expect(ok().disabled).toBe(true)
    expect(dialog().getAttribute('aria-modal')).toBe('true')
  })

  it('OK finishes at once with the trimmed entry; the check happens later, on the table', async () => {
    const { onDone, user } = await setup('wallhaven')
    await user.type(locationField(), '  forest  ')
    await user.click(ok())
    expect(onDone).toHaveBeenCalledWith(['forest'], '')
    expect(api.checkSource).not.toHaveBeenCalled()
    expect(screen.queryByText(/Just a moment/)).toBeNull()
  })

  it('Enter in the field is OK', async () => {
    const { onDone, user } = await setup('reddit')
    await user.type(screen.getByRole('textbox', { name: 'URL or name of a subreddit:' }), 'EarthPorn{Enter}')
    expect(onDone).toHaveBeenCalledWith(['EarthPorn'], '')
  })

  it('Cancel and Escape add nothing', async () => {
    const { onDone, onCancel, user } = await setup('wallhaven')
    await user.type(locationField(), 'forest')
    await user.click(screen.getByRole('button', { name: 'Cancel' }))
    await user.keyboard('{Escape}')
    expect(onCancel).toHaveBeenCalled()
    expect(onDone).not.toHaveBeenCalled()
  })

  it('opens prefilled for Edit', async () => {
    const { onDone, user } = await setup('wallhaven', 'nature')
    const field = locationField()
    expect(field.value).toBe('nature')
    expect(screen.getByRole('heading').textContent).toBe('Edit Wallhaven source')
    await user.clear(field)
    await user.type(field, 'ocean')
    await user.click(ok())
    expect(onDone).toHaveBeenCalledWith(['ocean'], '')
  })
})

describe('SourceDialog, paths', () => {
  it('opens the folder picker for a new Folders source, then waits for OK so a name can be typed', async () => {
    vi.mocked(api.selectDirectory).mockResolvedValue('/home/u/Pictures')
    const { onDone, user } = await setup('folder')
    expect(api.selectDirectory).toHaveBeenCalledTimes(1)
    expect(onDone).not.toHaveBeenCalled()
    expect(locationField().value).toBe('/home/u/Pictures')
    await user.type(nameField(), 'my-pics')
    await user.click(ok())
    expect(onDone).toHaveBeenCalledWith(['/home/u/Pictures'], 'my-pics')
    expect(api.checkSource).not.toHaveBeenCalled()
  })

  it('closes without adding when the picker is cancelled', async () => {
    vi.mocked(api.selectDirectory).mockResolvedValue('')
    const { onDone, onCancel } = await setup('album-date')
    expect(onCancel).toHaveBeenCalled()
    expect(onDone).not.toHaveBeenCalled()
  })

  it('Images picks several files, lists them, and returns one location per file with the name', async () => {
    vi.mocked(api.selectFiles).mockResolvedValue(['/p/a.jpg', '/p/b.png'])
    const { onDone, user } = await setup('images')
    expect(api.selectFiles).toHaveBeenCalledTimes(1)
    expect(screen.getByRole('list', { name: 'Image files:' }).textContent).toContain('a.jpg')
    await user.type(nameField(), 'favourites')
    await user.click(ok())
    expect(onDone).toHaveBeenCalledWith(['/p/a.jpg', '/p/b.png'], 'favourites')
  })

  it('Choose… picks again', async () => {
    vi.mocked(api.selectDirectory).mockResolvedValue('/home/u/empty')
    const { onDone, user } = await setup('folder')
    vi.mocked(api.selectDirectory).mockResolvedValue('/home/u/full')
    await user.click(screen.getByRole('button', { name: 'Choose…' }))
    await user.click(ok())
    expect(onDone).toHaveBeenCalledWith(['/home/u/full'], '')
  })

  it('opens prefilled for Edit without the picker', async () => {
    const { onDone, user } = await setup('folder', '/home/u/Pictures')
    expect(api.selectDirectory).not.toHaveBeenCalled()
    const field = screen.getByRole('textbox', { name: 'Folder:' }) as HTMLInputElement
    expect(field.value).toBe('/home/u/Pictures')
    await user.click(ok())
    expect(onDone).toHaveBeenCalledWith(['/home/u/Pictures'], '')
  })
})

describe('SourceDialog, no location (Bing, APOD)', () => {
  it('shows only the Name, focused, and OK works with it empty', async () => {
    const { onDone, user } = await setup('bing')
    expect(screen.getAllByRole('textbox')).toHaveLength(1)
    expect(document.activeElement).toBe(nameField())
    expect(ok().disabled).toBe(false)
    await user.type(nameField(), 'bing-daily{Enter}')
    expect(onDone).toHaveBeenCalledWith([''], 'bing-daily')
  })

  it('Edit renames it', async () => {
    const { onDone, user } = await setup('apod', '', 'space')
    expect(screen.getByRole('heading').textContent).toBe('Edit APOD source')
    expect(nameField().value).toBe('space')
    await user.clear(nameField())
    await user.click(ok())
    expect(onDone).toHaveBeenCalledWith([''], '')
  })
})

describe('SourceDialog, name', () => {
  it('passes a trimmed name with the location', async () => {
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
    expect(ok().disabled).toBe(true)
  })

  it('caps the name at 64 characters', async () => {
    await setup('wallhaven')
    expect(nameField().maxLength).toBe(64)
  })
})
