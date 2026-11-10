import { act, cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { Config } from '../../shared/api'
import { diffConfig } from '../configPatch'
import { testConfig } from '../testConfig'
import PickerSettings from './PickerSettings'

// "Open picker" calls Tauri; nothing here clicks it, but keep the import inert.
vi.mock('../../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../shared/api')>()
  return { ...real, showPicker: vi.fn() }
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

/** The Picker section over a live config. `patch()` is what auto-save would send. */
async function setup({ saved = testConfig(), os = 'macos' }: { saved?: Config; os?: string } = {}) {
  let current = saved
  function Harness() {
    const [config, setConfig] = useState(saved)
    current = config
    return <PickerSettings config={config} onChange={setConfig} os={os} onResetPosition={() => {}} />
  }
  render(<Harness />)
  await act(async () => {})
  return { patch: () => diffConfig(saved, current), user: userEvent.setup() }
}

const blurBox = () => screen.getByRole<HTMLInputElement>('checkbox', { name: 'Blur NSFW previews' })

describe('PickerSettings', () => {
  it('shows "Blur NSFW previews" ticked by default', async () => {
    await setup()
    expect(blurBox().checked).toBe(true)
  })

  it('unticking "Blur NSFW previews" patches picker.blur_nsfw to false', async () => {
    const { patch, user } = await setup()
    await user.click(blurBox())
    expect(patch()).toEqual({ picker: { blur_nsfw: false } })
  })

  it('ticking "Blur NSFW previews" patches picker.blur_nsfw to true', async () => {
    const saved = testConfig()
    saved.picker.blur_nsfw = false
    const { patch, user } = await setup({ saved })
    expect(blurBox().checked).toBe(false)
    await user.click(blurBox())
    expect(patch()).toEqual({ picker: { blur_nsfw: true } })
  })
})
