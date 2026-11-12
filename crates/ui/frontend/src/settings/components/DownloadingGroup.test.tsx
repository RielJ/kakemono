import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'

import { testConfig } from '../testConfig'
import DownloadingGroup from './DownloadingGroup'

afterEach(cleanup)

const LABEL = 'Download a wallpaper in the background every'
const HINT = 'When this is off, Next downloads a new wallpaper each time.'

describe('DownloadingGroup', () => {
  it('says Next downloads on demand while background downloads are off', () => {
    render(<DownloadingGroup config={testConfig()} onChange={() => {}} />)
    expect(screen.getByRole('checkbox', { name: LABEL, description: HINT })).toBeTruthy()
  })
})
