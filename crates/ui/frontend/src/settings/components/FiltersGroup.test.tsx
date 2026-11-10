import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { afterEach, describe, expect, it } from 'vitest'

import type { Config } from '../../shared/api'
import { testConfig } from '../testConfig'
import FiltersGroup, { regexWarning } from './FiltersGroup'

afterEach(cleanup)

/** Renders the group over a live config; `last()` is the newest config it asked to save. */
function setup(edit?: (c: Config) => void) {
  const initial = testConfig()
  edit?.(initial)
  const changes: Config[] = []
  function Harness() {
    const [config, setConfig] = useState(initial)
    return (
      <FiltersGroup
        config={config}
        onChange={(c) => {
          changes.push(c)
          setConfig(c)
        }}
      />
    )
  }
  render(<Harness />)
  const last = (): Config['filters'] => {
    const c = changes[changes.length - 1]
    if (!c) throw new Error('the filters group saved nothing')
    return c.filters
  }
  return { last, changes, user: userEvent.setup() }
}

const box = (name: string | RegExp) => screen.getByRole('checkbox', { name })

describe('FiltersGroup', () => {
  it('is headed "When possible, use images that…"', () => {
    setup()
    expect(screen.getByRole('heading', { name: 'When possible, use images that…' })).toBeTruthy()
  })

  it('patches landscape, and the sentence text ticks its box', async () => {
    const { last, user } = setup()
    await user.click(screen.getByText('Have landscape orientation'))
    expect(last().landscape).toBe(true)
  })

  it('patches the minimum size switch and percentage', async () => {
    const { last, user } = setup()
    await user.click(box(/^Are big, at least/))
    expect(last().min_size_enabled).toBe(true)
    const percent = screen.getByRole('combobox', { name: 'Minimum size, percent of the screen' })
    expect([...(percent as HTMLSelectElement).options].map((o) => o.value)).toEqual(['50', '80', '100'])
    await user.selectOptions(percent, '100')
    expect(last().min_size_percent).toBe(100)
  })

  it('patches brightness; unticked means off and ticking restores the choice', async () => {
    const { last, user } = setup()
    const tone = screen.getByRole('combobox', { name: 'Brightness' })
    expect([...(tone as HTMLSelectElement).options].map((o) => o.textContent)).toEqual([
      'Dark',
      'Light',
      'Match system appearance',
    ])
    await user.click(box(/^Are dark or light/))
    expect(last().brightness).toBe('dark')
    await user.selectOptions(tone, 'system')
    expect(last().brightness).toBe('system')
    await user.click(box(/^Are dark or light/))
    expect(last().brightness).toBe('off')
    // Unticked, the choice is kept on screen but not saved until the box is ticked.
    await user.selectOptions(tone, 'light')
    expect(last().brightness).toBe('off')
    await user.click(box(/^Are dark or light/))
    expect(last().brightness).toBe('light')
  })

  it('shows a saved brightness as ticked', () => {
    setup((c) => void (c.filters.brightness = 'light'))
    expect(box(/^Are dark or light/)).toHaveProperty('checked', true)
    expect((screen.getByRole('combobox', { name: 'Brightness' }) as HTMLSelectElement).value).toBe('light')
  })

  it('patches the colour switch and a native colour picker', async () => {
    const { last, user } = setup()
    await user.click(box(/^Contain this color/))
    expect(last().color_enabled).toBe(true)
    const picker = screen.getByLabelText('Color to look for')
    expect(picker.getAttribute('type')).toBe('color')
    // userEvent can't drive a native colour dialog; fire the input as the browser would.
    fireEvent.input(picker, { target: { value: '#ff8800' } })
    expect(last().color).toBe('#ff8800')
  })

  it('patches the EXIF rating switch and a 1–5 minimum', async () => {
    const { last, user } = setup()
    await user.click(box(/^Have EXIF rating at least/))
    expect(last().rating_enabled).toBe(true)
    const rating = screen.getByRole('combobox', { name: 'Minimum rating' })
    expect([...(rating as HTMLSelectElement).options].map((o) => o.value)).toEqual(['1', '2', '3', '4', '5'])
    await user.selectOptions(rating, '2')
    expect(last().min_rating).toBe(2)
  })

  it('patches the file name regex', async () => {
    const { last, user } = setup()
    await user.click(box(/^File name matches regex/))
    expect(last().name_regex_enabled).toBe(true)
    await user.type(screen.getByRole('textbox', { name: 'File name regex' }), '^wh-.*\\.jpg$')
    await user.tab()
    expect(last().name_regex).toBe('^wh-.*\\.jpg$')
    expect(screen.queryByRole('status')).toBeNull()
  })

  it('warns about an invalid regex but still saves it', async () => {
    const { last, user } = setup()
    await user.type(screen.getByRole('textbox', { name: 'File name regex' }), 'wall(paper{Enter}')
    expect(last().name_regex).toBe('wall(paper')
    expect(screen.getByRole('status').textContent).toMatch(/^This may not be a valid regular expression/)
  })
})

describe('regexWarning', () => {
  it('accepts a leading Rust inline flag group the daemon understands', () => {
    expect(regexWarning('(?i)sunset')).toBeNull()
    expect(regexWarning('^IMG_\\d+')).toBeNull()
  })

  it('gives only the reason, without repeating the engine prefix', () => {
    expect(regexWarning('(sunset')).toBe(
      'This may not be a valid regular expression: unterminated group. It is saved anyway; the daemon checks it.',
    )
  })
})
