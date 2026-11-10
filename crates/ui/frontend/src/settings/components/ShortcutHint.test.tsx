import { act, cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import ShortcutHint from './ShortcutHint'

const writeText = vi.fn<(text: string) => Promise<void>>()

beforeEach(() => {
  writeText.mockResolvedValue()
  // jsdom has no clipboard; userEvent.setup() installs its own, so define ours after it.
})
afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

function setup(os: string | undefined) {
  const user = userEvent.setup()
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
  render(<ShortcutHint os={os} />)
  return user
}

const text = () => document.body.textContent ?? ''

describe('ShortcutHint', () => {
  it('shows both commands on every OS', () => {
    setup('windows')
    expect(screen.getByText('kakemono pick')).toBeTruthy()
    expect(screen.getByText('kakemono settings')).toBeTruthy()
  })

  it('on macOS explains Shortcuts.app, Raycast and skhd', () => {
    setup('macos')
    expect(screen.getByRole('heading', { name: 'Shortcuts app' })).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'Raycast' })).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'skhd' })).toBeTruthy()
    expect(text()).toContain('alt - p : kakemono pick')
    expect(text()).not.toContain('Hyprland')
  })

  it('on Linux explains GNOME and KDE, and gives Hyprland, Sway and niri bind lines', () => {
    setup('linux')
    expect(screen.getByRole('heading', { name: 'GNOME' })).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'KDE Plasma' })).toBeTruthy()
    expect(text()).toContain('bind = SUPER, P, exec, kakemono pick')
    expect(text()).toContain('bindsym $mod+p exec kakemono pick')
    expect(text()).toContain('Mod+P { spawn "kakemono" "pick"; }')
    expect(text()).not.toContain('Raycast')
  })

  it('copies a command to the clipboard and says so', async () => {
    const user = setup('macos')
    await user.click(screen.getByRole('button', { name: 'Copy kakemono pick' }))
    expect(writeText).toHaveBeenCalledWith('kakemono pick')
    expect(screen.getByRole('status').textContent).toBe('Copied')
  })

  it('says when the clipboard refuses', async () => {
    writeText.mockRejectedValue(new Error('denied'))
    const user = setup('linux')
    await user.click(screen.getByRole('button', { name: 'Copy kakemono settings' }))
    await act(async () => {})
    expect(screen.getByRole('status').textContent).toBe('Couldn’t copy; select the text instead')
  })
})
