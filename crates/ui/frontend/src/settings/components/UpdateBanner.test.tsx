import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { UpdateInfo } from '../../shared/api'
import type { Update, UpdateState } from '../useUpdate'
import UpdateBanner, { percent } from './UpdateBanner'

const info: UpdateInfo = { version: '0.3.0', current: '0.2.0', install: 'in_app' }

function banner(state: UpdateState) {
  const update: Update = { state, install: vi.fn(), dismiss: vi.fn(), openPage: vi.fn() }
  render(<UpdateBanner update={update} />)
  return update
}

afterEach(cleanup)

describe('UpdateBanner', () => {
  it('renders nothing without an update', () => {
    banner({ kind: 'none' })
    expect(screen.queryByRole('region', { name: 'App update' })).toBeNull()
  })

  it('offers to update in place', async () => {
    const update = banner({ kind: 'available', info })
    expect(screen.getByText('kakemono 0.3.0 is available')).toBeTruthy()
    expect(screen.getByText('You have 0.2.0.')).toBeTruthy()

    await userEvent.click(screen.getByRole('button', { name: 'Update' }))
    await userEvent.click(screen.getByRole('button', { name: 'What’s new' }))
    await userEvent.click(screen.getByRole('button', { name: 'Later' }))
    expect(update.install).toHaveBeenCalledTimes(1)
    expect(update.openPage).toHaveBeenCalledTimes(1)
    expect(update.dismiss).toHaveBeenCalledTimes(1)
  })

  it('links to the download when this copy cannot update itself', async () => {
    const update = banner({ kind: 'available', info: { ...info, install: 'download' } })
    expect(screen.queryByRole('button', { name: 'Update' })).toBeNull()
    expect(screen.getByText(/Download the new version to update/)).toBeTruthy()
    await userEvent.click(screen.getByRole('button', { name: 'Download' }))
    expect(update.openPage).toHaveBeenCalledTimes(1)
  })

  it('shows download progress and hides the buttons while installing', () => {
    banner({ kind: 'installing', info, progress: { step: 'downloading', downloaded: 42, total: 100 } })
    expect(screen.getByText('Downloading… 42%')).toBeTruthy()
    expect(screen.getByRole('progressbar').getAttribute('aria-valuenow')).toBe('42')
    expect(screen.queryAllByRole('button')).toHaveLength(0)
  })

  it('says the app restarts once it is installing', () => {
    banner({ kind: 'installing', info, progress: { step: 'installing' } })
    expect(screen.getByText('Installing… kakemono restarts when it’s done.')).toBeTruthy()
  })

  it('shows the error and offers a retry', async () => {
    const update = banner({ kind: 'failed', info, message: 'signature mismatch' })
    expect(screen.getByText('Update failed: signature mismatch')).toBeTruthy()
    await userEvent.click(screen.getByRole('button', { name: 'Try again' }))
    expect(update.install).toHaveBeenCalledTimes(1)
  })

  it('computes whole percents and copes with unknown sizes', () => {
    expect(percent(null)).toBeNull()
    expect(percent({ step: 'installing' })).toBeNull()
    expect(percent({ step: 'downloading', downloaded: 5, total: null })).toBeNull()
    expect(percent({ step: 'downloading', downloaded: 5, total: 0 })).toBeNull()
    expect(percent({ step: 'downloading', downloaded: 999, total: 1000 })).toBe(99)
    expect(percent({ step: 'downloading', downloaded: 2000, total: 1000 })).toBe(100)
  })
})
