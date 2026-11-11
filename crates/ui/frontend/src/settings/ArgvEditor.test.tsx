import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { afterEach, describe, expect, it } from 'vitest'

import ArgvEditor from './ArgvEditor'

/** Holds the tokens like a panel would and exposes every change. */
function Harness({ initial, seen }: { initial: string[]; seen: string[][] }) {
  const [tokens, setTokens] = useState(initial)
  return (
    <ArgvEditor
      label="Cmd"
      tokens={tokens}
      onChange={(next) => {
        seen.push(next)
        setTokens(next)
      }}
      emptyText="Nothing yet"
    />
  )
}

const rows = () => screen.queryAllByRole('textbox').map((el) => (el as HTMLInputElement).value)

afterEach(cleanup)

describe('ArgvEditor', () => {
  it('keeps a token with spaces and quotes as one row', async () => {
    const seen: string[][] = []
    render(<Harness initial={['setbg']} seen={seen} />)
    await userEvent.click(screen.getByRole('button', { name: 'Add token' }))
    await userEvent.type(screen.getByRole('textbox', { name: 'Cmd, token 2 of 2' }), `it's a "file"`)
    expect(rows()).toEqual(['setbg', `it's a "file"`])
    expect(seen.at(-1)).toEqual(['setbg', `it's a "file"`])
  })

  it('focuses a new row and appends placeholder tokens', async () => {
    const seen: string[][] = []
    render(<Harness initial={[]} seen={seen} />)
    expect(screen.getByText('Nothing yet')).toBeTruthy()
    await userEvent.click(screen.getByRole('button', { name: 'Add token' }))
    expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'Cmd, token 1 of 1' }))
    await userEvent.keyboard('feh')
    await userEvent.click(screen.getByRole('button', { name: 'Add {path} token' }))
    await userEvent.click(screen.getByRole('button', { name: 'Add {fit} token' }))
    expect(seen.at(-1)).toEqual(['feh', '{path}', '{fit}'])
  })

  it('reorders rows and keeps focus on the moved row', async () => {
    const seen: string[][] = []
    render(<Harness initial={['a', 'b', 'c']} seen={seen} />)
    expect(screen.getByRole('button', { name: 'Move token 1 up' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: 'Move token 3 down' })).toHaveProperty('disabled', true)
    await userEvent.click(screen.getByRole('button', { name: 'Move token 3 up' }))
    expect(rows()).toEqual(['a', 'c', 'b'])
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Move token 2 up' }))
    await userEvent.click(screen.getByRole('button', { name: 'Move token 1 down' }))
    expect(rows()).toEqual(['c', 'a', 'b'])
  })

  it('removes rows down to none', async () => {
    const seen: string[][] = []
    render(<Harness initial={['a', 'b']} seen={seen} />)
    await userEvent.click(screen.getByRole('button', { name: 'Remove token 1' }))
    expect(rows()).toEqual(['b'])
    await userEvent.click(screen.getByRole('button', { name: 'Remove token 1' }))
    expect(seen.at(-1)).toEqual([])
    expect(screen.getByText('Nothing yet')).toBeTruthy()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Add token' }))
  })

  it('shows the validation message', () => {
    render(
      <ArgvEditor label="Cmd" tokens={['x', '{bad}']} onChange={() => {}} error="Unknown placeholder" emptyText="" />,
    )
    expect(screen.getByRole('alert').textContent).toBe('Unknown placeholder')
    expect(screen.getByRole('textbox', { name: 'Cmd, token 2 of 2' }).getAttribute('aria-invalid')).toBe('true')
  })
})
