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
const menu = () => screen.getByRole('menu', { name: 'Add a source' })

describe('AddSourceMenu', () => {
  it('lists the local types, a separator, then the online types A–Z, each with a bold name and a description', async () => {
    const { user } = setup()
    expect(trigger().getAttribute('aria-haspopup')).toBe('menu')
    expect(trigger().getAttribute('aria-expanded')).toBe('false')
    await user.click(trigger())
    expect(trigger().getAttribute('aria-expanded')).toBe('true')
    const children = [...menu().children].map((el) =>
      el.getAttribute('role') === 'separator' ? '—' : el.querySelector('strong')?.textContent,
    )
    expect(children).toEqual([
      'Images',
      'Folders',
      'Sequential album by file name',
      'Sequential album by file date',
      '—',
      'APOD',
      'Bing',
      'Reddit',
      'Unsplash',
      'Wallhaven',
    ])
    const wallhaven = within(menu()).getByRole('menuitem', { name: /^Wallhaven/ })
    expect(wallhaven.textContent).toContain('Wallpapers from wallhaven.cc for a search or a URL.')
  })

  it('hides the online types when Internet access is off', async () => {
    const { user } = setup(false)
    await user.click(trigger())
    const names = within(menu())
      .getAllByRole('menuitem')
      .map((m) => m.querySelector('strong')?.textContent)
    expect(names).toEqual(['Images', 'Folders', 'Sequential album by file name', 'Sequential album by file date'])
    expect(within(menu()).queryByRole('separator')).toBeNull()
  })

  it('chooses an item by click and closes', async () => {
    const { onChoose, user } = setup()
    await user.click(trigger())
    await user.click(within(menu()).getByRole('menuitem', { name: /^Bing/ }))
    expect(onChoose).toHaveBeenCalledWith('bing')
    expect(screen.queryByRole('menu')).toBeNull()
  })

  it('works from the keyboard: arrows move, Enter chooses, Escape closes and refocuses the button', async () => {
    const { onChoose, user } = setup()
    trigger().focus()
    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(within(menu()).getByRole('menuitem', { name: /^Images/ }))
    await user.keyboard('{End}')
    expect(document.activeElement).toBe(within(menu()).getByRole('menuitem', { name: /^Wallhaven/ }))
    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(within(menu()).getByRole('menuitem', { name: /^Images/ }))
    await user.keyboard('{ArrowUp}{ArrowUp}')
    expect(document.activeElement).toBe(within(menu()).getByRole('menuitem', { name: /^Unsplash/ }))
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('menu')).toBeNull()
    expect(document.activeElement).toBe(trigger())
    await user.keyboard('{Enter}')
    await user.keyboard('{Home}{ArrowDown}{Enter}')
    expect(onChoose).toHaveBeenCalledWith('folder')
  })

  it('closes on a click outside', async () => {
    const { onChoose, user } = setup()
    await user.click(trigger())
    await user.click(document.body)
    expect(screen.queryByRole('menu')).toBeNull()
    expect(onChoose).not.toHaveBeenCalled()
  })
})
