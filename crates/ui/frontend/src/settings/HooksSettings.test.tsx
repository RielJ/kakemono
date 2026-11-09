import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { Config } from '../shared/api'
import HooksSettings from './HooksSettings'
import { testConfig } from './testConfig'

const config = (on_change: string[] | null): Config => ({ ...testConfig(), hooks: { on_change } })

afterEach(cleanup)

describe('HooksSettings', () => {
  it('lists the variables the hook receives', () => {
    render(<HooksSettings config={config(null)} onChange={() => {}} />)
    for (const name of ['KAKEMONO_PATH', 'KAKEMONO_SOURCE', 'KAKEMONO_TITLE', 'KAKEMONO_AUTHOR', 'KAKEMONO_SOURCE_URL', 'KAKEMONO_THEME']) {
      expect(screen.getByRole('rowheader', { name })).toBeTruthy()
    }
  })

  it('stores the argv as tokens, and no rows as null', async () => {
    const onChange = vi.fn()
    const { rerender } = render(<HooksSettings config={config(null)} onChange={onChange} />)
    await userEvent.click(screen.getByRole('button', { name: 'Add {path} token' }))
    expect(onChange).toHaveBeenLastCalledWith(config(['{path}']))

    rerender(<HooksSettings config={config(['notify', '{path}'])} onChange={onChange} />)
    await userEvent.click(screen.getByRole('button', { name: 'Remove token 2' }))
    expect(onChange).toHaveBeenLastCalledWith(config(['notify']))

    rerender(<HooksSettings config={config(['notify'])} onChange={onChange} />)
    await userEvent.click(screen.getByRole('button', { name: 'Remove token 1' }))
    expect(onChange).toHaveBeenLastCalledWith(config(null))
  })
})
