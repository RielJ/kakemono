import { cleanup, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import AddSourceMenu from './AddSourceMenu'

afterEach(() => cleanup())

function setup(internet = true) {
  const onChoose = vi.fn()
  render(<AddSourceMenu internet={internet} onChoose={onChoose} />)
  return { onChoose, user: userEvent.setup() }
}

const trigger = () => screen.getByRole('button', { name: 'Add…' })
const dialog = () => screen.getByRole('dialog', { name: 'Add a source' })
const choice = (name: RegExp) => within(dialog()).getByRole('button', { name })
const names = (group: string) =>
  [...within(dialog()).getByRole('region', { name: group }).querySelectorAll('[data-choice] strong')].map((s) => s.textContent)

describe('AddSourceMenu', () => {
  it('opens a dialog: local types on the left, online types A–Z on the right, each with a description', async () => {
    const { user } = setup()
    expect(trigger().getAttribute('aria-haspopup')).toBe('dialog')
    expect(screen.queryByRole('dialog')).toBeNull()
    await user.click(trigger())
    expect(names('On this Mac')).toEqual(['Images', 'Folders', 'Sequential album by file name', 'Sequential album by file date'])
    expect(names('Online')).toEqual(['APOD', 'Bing', 'Reddit', 'Unsplash', 'Wallhaven'])
    expect(choice(/^Wallhaven/).textContent).toContain('Wallpapers from wallhaven.cc for a search or a URL.')
    expect(document.activeElement).toBe(choice(/^Images/))
  })

  it('hides the online types when Internet access is off', async () => {
    const { user } = setup(false)
    await user.click(trigger())
    expect(names('On this Mac')).toHaveLength(4)
    expect(within(dialog()).queryByRole('region', { name: 'Online' })).toBeNull()
  })

  it('chooses a type by click and closes', async () => {
    const { onChoose, user } = setup()
    await user.click(trigger())
    await user.click(choice(/^Bing/))
    expect(onChoose).toHaveBeenCalledWith('bing')
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('works from the keyboard: Up/Down wrap, Left/Right switch column, Enter chooses', async () => {
    const { onChoose, user } = setup()
    await user.click(trigger())
    await user.keyboard('{ArrowUp}')
    expect(document.activeElement).toBe(choice(/^Wallhaven/))
    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(choice(/^Images/))
    await user.keyboard('{ArrowDown}{ArrowRight}')
    expect(document.activeElement).toBe(choice(/^Bing/))
    await user.keyboard('{ArrowLeft}{End}')
    expect(document.activeElement).toBe(choice(/^Wallhaven/))
    await user.keyboard('{Home}{ArrowDown}{Enter}')
    expect(onChoose).toHaveBeenCalledWith('folder')
  })

  it('closes on Escape, Cancel or a click outside without choosing, and refocuses the button', async () => {
    const { onChoose, user } = setup()
    await user.click(trigger())
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(document.activeElement).toBe(trigger())
    await user.click(trigger())
    await user.click(within(dialog()).getByRole('button', { name: 'Cancel' }))
    expect(screen.queryByRole('dialog')).toBeNull()
    await user.click(trigger())
    await user.click(document.querySelector('.modal-backdrop') as HTMLElement)
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(onChoose).not.toHaveBeenCalled()
  })
})
