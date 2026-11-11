import { act, cleanup, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { Config } from '../../shared/api'
import * as api from '../../shared/api'
import type { SourceEntry } from '../../shared/bindings/SourceEntry'
import { testConfig } from '../testConfig'
import SourcesTable from './SourcesTable'

vi.mock('../../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../shared/api')>()
  return {
    ...real,
    nextWallpaper: vi.fn(),
    deleteSourceFiles: vi.fn(),
    openSourceFolder: vi.fn(),
    checkSource: vi.fn(),
    selectDirectory: vi.fn(),
    selectFiles: vi.fn(),
  }
})

const SOURCES: SourceEntry[] = [
  { id: 'bing', enabled: true, type: 'bing', location: '' },
  { id: 'wh-nature', enabled: true, type: 'wallhaven', location: 'nature' },
  { id: 'dir-pics', enabled: false, type: 'folder', location: '/home/u/Pictures/Wallpapers' },
  { id: 'apod', enabled: false, type: 'apod', location: '' },
]

beforeEach(() => {
  vi.mocked(api.checkSource).mockResolvedValue({ count: 5 })
  vi.mocked(api.nextWallpaper).mockResolvedValue()
  vi.mocked(api.deleteSourceFiles).mockResolvedValue()
  vi.mocked(api.openSourceFolder).mockResolvedValue()
})
afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

/**
 * Renders the table over a live config. `changes` records `onChange`, `commits` records
 * `onCommit`; `last()` is the newest config either produced.
 */
async function setup({
  sources = SOURCES,
  internet = true,
  commit = 'ok',
}: { sources?: SourceEntry[]; internet?: boolean; commit?: 'ok' | 'fail' | 'none' } = {}) {
  const initial: Config = { ...testConfig(), sources }
  initial.daemon.internet = internet
  const changes: Config[] = []
  const commits: Config[] = []
  const all: Config[] = []
  const order: string[] = []
  function Harness() {
    const [config, setConfig] = useState(initial)
    const onCommit =
      commit === 'none'
        ? undefined
        : async (c: Config) => {
            commits.push(c)
            all.push(c)
            setConfig(c)
            order.push('commit')
            if (commit === 'fail') throw new Error('disk full')
          }
    return (
      <SourcesTable
        config={config}
        onChange={(c) => {
          changes.push(c)
          all.push(c)
          order.push('change')
          setConfig(c)
        }}
        onCommit={onCommit}
      />
    )
  }
  render(<Harness />)
  await act(async () => {})
  const last = (): Config => {
    const c = all[all.length - 1]
    if (!c) throw new Error('the table saved nothing')
    return c
  }
  return { last, changes, commits, order, user: userEvent.setup() }
}

const grid = () => screen.getByRole('grid', { name: 'Image sources' })
/** Body rows, in order (the header row excluded). */
const rows = () => within(grid()).getAllByRole('row').slice(1)
const row = (n: number) => rows()[n]
const button = (name: string | RegExp) => screen.getByRole('button', { name })

describe('SourcesTable', () => {
  it('shows one row per source with its enabled box, type and location', async () => {
    await setup()
    expect(rows()).toHaveLength(4)
    const cells = rows().map((r) => within(r).getAllByRole('gridcell').map((c) => c.textContent))
    expect(cells.map((c) => c.slice(1))).toEqual([
      ['Bing', '—', 'OK'],
      ['Wallhaven', 'nature', 'OK'],
      ['Folder', '/home/u/Pictures/Wallpapers', 'OK'],
      ['APOD', '—', 'OK'],
    ])
    const boxes = rows().map((r) => within(r).getByRole('checkbox'))
    expect(boxes.map((b) => (b as HTMLInputElement).checked)).toEqual([true, true, false, false])
    // A long location is cut by CSS; the full text stays in a tooltip.
    expect(within(row(2)).getByText('/home/u/Pictures/Wallpapers').getAttribute('title')).toBe('/home/u/Pictures/Wallpapers')
  })

  it('shows an empty state when there are no sources', async () => {
    await setup({ sources: [] })
    expect(screen.getByText(/No sources yet/)).toBeTruthy()
  })

  it('toggling a box flips only that row', async () => {
    const { last, user } = await setup()
    await user.click(within(row(2)).getByRole('checkbox'))
    expect(last().sources).toEqual([SOURCES[0], SOURCES[1], { ...SOURCES[2], enabled: true }, SOURCES[3]])
    // The box doesn't change the selection.
    expect(rows().filter((r) => r.getAttribute('aria-selected') === 'true')).toHaveLength(0)
  })

  it('selects one row on click, a range with shift-click and toggles with cmd- or ctrl-click', async () => {
    const { user } = await setup()
    const selected = () => rows().map((r) => r.getAttribute('aria-selected') === 'true')
    await user.click(within(row(0)).getByText('Bing'))
    expect(selected()).toEqual([true, false, false, false])
    await user.keyboard('[ShiftLeft>]')
    await user.click(within(row(2)).getByText('Folder'))
    await user.keyboard('[/ShiftLeft]')
    expect(selected()).toEqual([true, true, true, false])
    await user.keyboard('[MetaLeft>]')
    await user.click(within(row(1)).getByText('Wallhaven'))
    await user.keyboard('[/MetaLeft]')
    expect(selected()).toEqual([true, false, true, false])
    await user.keyboard('[ControlLeft>]')
    await user.click(within(row(3)).getByText('APOD'))
    await user.keyboard('[/ControlLeft]')
    expect(selected()).toEqual([true, false, true, true])
    expect(grid().getAttribute('aria-multiselectable')).toBe('true')
  })

  it('moves and extends the selection with the keyboard; space toggles the focused row', async () => {
    const { last, user } = await setup()
    const selected = () => rows().map((r) => r.getAttribute('aria-selected') === 'true')
    await user.tab()
    expect(document.activeElement).toBe(row(0))
    await user.keyboard('{ArrowDown}')
    expect(document.activeElement).toBe(row(1))
    expect(selected()).toEqual([false, true, false, false])
    await user.keyboard('{Shift>}{ArrowDown}{/Shift}')
    expect(selected()).toEqual([false, true, true, false])
    await user.keyboard('{End}')
    expect(selected()).toEqual([false, false, false, true])
    await user.keyboard(' ')
    expect(last().sources[3].enabled).toBe(true)
    await user.keyboard('{Home}')
    expect(document.activeElement).toBe(row(0))
  })

  it('Use Only These enables only the selected rows and saves, leaving the wallpaper alone', async () => {
    const { commits, user } = await setup()
    await user.click(within(row(1)).getByText('Wallhaven'))
    await user.keyboard('[MetaLeft>]')
    await user.click(within(row(2)).getByText('Folder'))
    await user.keyboard('[/MetaLeft]')
    expect(api.nextWallpaper).not.toHaveBeenCalled()
    await user.click(button('Use Only These'))
    expect(commits).toHaveLength(1)
    expect(commits[0].sources.map((s) => s.enabled)).toEqual([false, true, true, false])
    expect(api.nextWallpaper).not.toHaveBeenCalled()
  })

  it('Use Only These is disabled with nothing selected', async () => {
    await setup()
    expect((button('Use Only These') as HTMLButtonElement).disabled).toBe(true)
  })

  it('Use Only These says why when the save fails', async () => {
    const { user } = await setup({ commit: 'fail' })
    await user.click(within(row(1)).getByText('Wallhaven'))
    await user.click(button('Use Only These'))
    expect(api.nextWallpaper).not.toHaveBeenCalled()
    expect(screen.getByRole('alert').textContent).toMatch('disk full')
  })

  it('Use Only These falls back to onChange without onCommit', async () => {
    const { changes, user } = await setup({ commit: 'none' })
    await user.click(within(row(1)).getByText('Wallhaven'))
    await user.click(button('Use Only These'))
    expect(changes[changes.length - 1].sources.map((s) => s.enabled)).toEqual([false, true, false, false])
    expect(api.nextWallpaper).not.toHaveBeenCalled()
  })

  describe('Remove', () => {
    it('for an online row offers to delete its downloads; checked, it deletes after saving', async () => {
      const { commits, order, user } = await setup()
      vi.mocked(api.deleteSourceFiles).mockImplementation(async () => void order.push('delete'))
      await user.click(within(row(1)).getByText('Wallhaven'))
      await user.click(button('Remove…'))
      const dialog = screen.getByRole('dialog', { name: /Remove/ })
      const also = within(dialog).getByRole('checkbox', { name: 'Also delete downloaded files' })
      expect((also as HTMLInputElement).checked).toBe(false)
      await user.click(also)
      await user.click(within(dialog).getByRole('button', { name: 'Remove' }))
      expect(screen.queryByRole('dialog')).toBeNull()
      expect(commits[0].sources.map((s) => s.id)).toEqual(['bing', 'dir-pics', 'apod'])
      expect(api.deleteSourceFiles).toHaveBeenCalledWith('wh-nature')
      expect(order).toEqual(['commit', 'delete'])
      expect(rows()).toHaveLength(3)
    })

    it('unchecked, it removes the row and keeps the files', async () => {
      const { last, user } = await setup()
      await user.click(within(row(0)).getByText('Bing'))
      await user.click(button('Remove…'))
      await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Remove' }))
      expect(last().sources.map((s) => s.id)).toEqual(['wh-nature', 'dir-pics', 'apod'])
      expect(api.deleteSourceFiles).not.toHaveBeenCalled()
    })

    it('for a local row has no delete checkbox, and never deletes', async () => {
      const { last, user } = await setup()
      await user.click(within(row(2)).getByText('Folder'))
      await user.click(button('Remove…'))
      const dialog = screen.getByRole('dialog')
      expect(within(dialog).queryByRole('checkbox')).toBeNull()
      expect(dialog.textContent).toMatch(/files in the folder stay/i)
      await user.click(within(dialog).getByRole('button', { name: 'Remove' }))
      expect(last().sources.map((s) => s.id)).toEqual(['bing', 'wh-nature', 'apod'])
      expect(api.deleteSourceFiles).not.toHaveBeenCalled()
    })

    it('Cancel and Escape keep the row', async () => {
      const { all, user } = await setup().then((s) => ({ ...s, all: s.commits.concat(s.changes) }))
      await user.click(within(row(1)).getByText('Wallhaven'))
      await user.click(button('Remove…'))
      await user.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Cancel' }))
      expect(screen.queryByRole('dialog')).toBeNull()
      expect(document.activeElement).toBe(button('Remove…'))
      await user.click(button('Remove…'))
      await user.keyboard('{Escape}')
      expect(screen.queryByRole('dialog')).toBeNull()
      expect(all).toHaveLength(0)
      expect(rows()).toHaveLength(4)
    })

    it('keeps focus inside the dialog', async () => {
      const { user } = await setup()
      await user.click(within(row(1)).getByText('Wallhaven'))
      await user.click(button('Remove…'))
      const dialog = screen.getByRole('dialog')
      expect(dialog.getAttribute('aria-modal')).toBe('true')
      expect(dialog.contains(document.activeElement)).toBe(true)
      for (let i = 0; i < 5; i++) {
        await user.tab()
        expect(dialog.contains(document.activeElement)).toBe(true)
      }
    })

    it('does not delete files when the save fails; the row stays removed on screen', async () => {
      const { user } = await setup({ commit: 'fail' })
      await user.click(within(row(1)).getByText('Wallhaven'))
      await user.click(button('Remove…'))
      const dialog = screen.getByRole('dialog')
      await user.click(within(dialog).getByRole('checkbox', { name: 'Also delete downloaded files' }))
      await user.click(within(dialog).getByRole('button', { name: 'Remove' }))
      expect(api.deleteSourceFiles).not.toHaveBeenCalled()
      expect(screen.getByRole('alert').textContent).toMatch('disk full')
      expect(rows()).toHaveLength(3)
    })

    it('the Delete key opens the dialog for the selected rows', async () => {
      const { user } = await setup()
      await user.click(within(row(1)).getByText('Wallhaven'))
      await user.keyboard('{Delete}')
      expect(screen.getByRole('dialog', { name: /Remove/ })).toBeTruthy()
    })
  })

  it('Open Folder opens the selected row’s folder', async () => {
    const { user } = await setup()
    expect((button('Open Folder') as HTMLButtonElement).disabled).toBe(true)
    await user.click(within(row(1)).getByText('Wallhaven'))
    await user.click(button('Open Folder'))
    expect(api.openSourceFolder).toHaveBeenCalledWith('wh-nature')
  })

  it('Open Folder shows the error when the folder cannot be opened', async () => {
    vi.mocked(api.openSourceFolder).mockRejectedValue({ kind: 'invalid', message: 'no such folder' })
    const { user } = await setup()
    await user.click(within(row(2)).getByText('Folder'))
    await user.click(button('Open Folder'))
    expect(screen.getByRole('alert').textContent).toMatch('no such folder')
  })

  it('greys online rows when Internet access is off', async () => {
    await setup({ internet: false })
    expect(rows().map((r) => r.getAttribute('aria-disabled'))).toEqual(['true', 'true', null, 'true'])
    expect(screen.getByText(/Internet access is off/)).toBeTruthy()
  })

  it('Edit… on Bing opens a name-only dialog and renames it', async () => {
    const { last, user } = await setup()
    await user.click(within(row(0)).getByText('Bing'))
    await user.click(button('Edit…'))
    const dialog = screen.getByRole('dialog', { name: 'Edit Bing source' })
    expect(within(dialog).getAllByRole('textbox')).toHaveLength(1)
    await user.type(within(dialog).getByRole('textbox', { name: /^Name/ }), 'bing-daily')
    await user.click(within(dialog).getByRole('button', { name: 'OK' }))
    expect(last().sources[0]).toEqual({ ...SOURCES[0], name: 'bing-daily' })
  })

  describe('Status', () => {
    it('checks each row in the background and shows the result', async () => {
      vi.mocked(api.checkSource).mockImplementation(async (kind) => {
        if (kind === 'folder') throw { kind: 'daemon', message: 'no images found' }
        return { count: 24 }
      })
      await setup()
      const status = (n: number) => within(row(n)).getAllByRole('gridcell')[3]
      expect(status(1).textContent).toBe('OK')
      expect(status(1).querySelector('[title]')?.getAttribute('title')).toBe('Found 24 images.')
      expect(status(2).textContent).toBe('Failed')
      expect(status(2).querySelector('[title]')?.getAttribute('title')).toBe('no images found')
    })

    it('spells out a selected row’s failure under the table', async () => {
      vi.mocked(api.checkSource).mockImplementation(async (kind) => {
        if (kind === 'folder') throw { kind: 'daemon', message: 'no images found' }
        return { count: 3 }
      })
      const { user } = await setup()
      expect(screen.queryByText(/no images found/, { selector: '[role="alert"], p' })).toBeNull()
      await user.click(within(row(2)).getByText('Folder'))
      expect(screen.getByText('Folder: no images found')).toBeTruthy()
    })

    it('shows Checking… while the check runs, and doesn’t check again after a rename', async () => {
      let finish: (v: { count: number }) => void = () => {}
      vi.mocked(api.checkSource).mockReturnValue(new Promise((res) => (finish = res)))
      const { user } = await setup({ sources: [SOURCES[1]] })
      expect(within(row(0)).getAllByRole('gridcell')[3].textContent).toBe('Checking…')
      await act(async () => finish({ count: 2 }))
      expect(within(row(0)).getAllByRole('gridcell')[3].textContent).toBe('OK')
      await user.dblClick(within(row(0)).getAllByRole('gridcell')[1])
      await user.keyboard('renamed{Enter}')
      expect(api.checkSource).toHaveBeenCalledTimes(1)
    })
  })

  describe('rename in place', () => {
    const nameCell = (n: number) => within(row(n)).getAllByRole('gridcell')[1]

    it('double-clicking the name edits it there; Enter saves, and no dialog opens', async () => {
      const { last, user } = await setup()
      await user.dblClick(nameCell(1))
      expect(screen.queryByRole('dialog')).toBeNull()
      const field = within(row(1)).getByRole('textbox', { name: 'Name for Wallhaven' }) as HTMLInputElement
      expect(document.activeElement).toBe(field)
      await user.keyboard('wallhaven-top{Enter}')
      expect(last().sources[1]).toEqual({ ...SOURCES[1], name: 'wallhaven-top' })
      expect(within(row(1)).queryByRole('textbox')).toBeNull()
      expect(document.activeElement).toBe(row(1))
    })

    it('Escape cancels, clicking away saves, and an empty name removes it', async () => {
      const named = [{ ...SOURCES[1], name: 'Nature picks' }, SOURCES[2]]
      const { changes, commits, last, user } = await setup({ sources: named })
      await user.dblClick(nameCell(0))
      await user.keyboard('{Control>}a{/Control}zzz{Escape}')
      expect(changes.concat(commits)).toHaveLength(0)
      expect(within(row(0)).getByText('Nature picks')).toBeTruthy()
      await user.dblClick(nameCell(0))
      await user.clear(within(row(0)).getByRole('textbox'))
      await user.click(within(row(1)).getByText('Folder'))
      expect(last().sources[0]).toEqual(SOURCES[1])
      expect('name' in last().sources[0]).toBe(false)
    })

    it('works for Bing too, and keys typed there don’t reach the row', async () => {
      const { last, user } = await setup()
      await user.dblClick(nameCell(0))
      await user.keyboard('a b{Enter}')
      expect(last().sources[0]).toEqual({ ...SOURCES[0], name: 'a b' })
      expect(last().sources[0].enabled).toBe(true)
    })

    it('double-clicking the location opens Edit instead', async () => {
      const { user } = await setup()
      await user.dblClick(within(row(1)).getByText('nature'))
      expect(screen.getByRole('dialog', { name: 'Edit Wallhaven source' })).toBeTruthy()
    })
  })

  describe('Add… and Edit…', () => {
    const choose = async (user: ReturnType<typeof userEvent.setup>, name: RegExp) => {
      await user.click(button('Add…'))
      await user.click(within(screen.getByRole('dialog', { name: 'Add a source' })).getByRole('button', { name }))
    }

    it('adds Bing after a name-only dialog as a new enabled row', async () => {
      const { last, user } = await setup({ sources: SOURCES.slice(1) })
      await choose(user, /^Bing/)
      await user.click(within(screen.getByRole('dialog', { name: 'Add Bing source' })).getByRole('button', { name: 'OK' }))
      const added = last().sources[last().sources.length - 1]
      expect(added).toEqual({ id: expect.stringMatching(/^bing-[0-9a-f]{4}$/), type: 'bing', location: '', enabled: true })
      expect(screen.queryByRole('dialog')).toBeNull()
      expect(rows()).toHaveLength(4)
    })

    it('refuses a second Bing', async () => {
      const { changes, commits, user } = await setup()
      await choose(user, /^Bing/)
      expect(screen.getByRole('alert').textContent).toMatch('Bing is already in the list')
      expect(changes.concat(commits)).toHaveLength(0)
      expect(rows()).toHaveLength(4)
    })

    it('adds a Wallhaven search at OK, checks it in the background, and selects the new row', async () => {
      const { last, user } = await setup()
      await choose(user, /^Wallhaven/)
      const dialog = screen.getByRole('dialog', { name: 'Add Wallhaven source' })
      expect(dialog.textContent).toContain('Enter keywords or paste URL here:')
      await user.type(within(dialog).getAllByRole('textbox')[0], 'forest')
      await user.click(within(dialog).getByRole('button', { name: 'OK' }))
      expect(screen.queryByRole('dialog')).toBeNull()
      const added = last().sources[4]
      expect(added).toEqual({ id: expect.stringMatching(/^wh-[0-9a-f]{4}$/), type: 'wallhaven', location: 'forest', enabled: true })
      expect(row(4).getAttribute('aria-selected')).toBe('true')
      await act(async () => {})
      expect(api.checkSource).toHaveBeenCalledWith('wallhaven', 'forest')
      expect(within(row(4)).getAllByRole('gridcell')[3].textContent).toBe('OK')
    })

    it('adds the row even when its check fails; the row shows Failed', async () => {
      vi.mocked(api.checkSource).mockRejectedValue({ kind: 'daemon', message: 'no images found' })
      const { last, user } = await setup({ sources: [] })
      await choose(user, /^Wallhaven/)
      await user.type(screen.getAllByRole('textbox')[0], 'zzzz')
      await user.click(button('OK'))
      expect(last().sources).toHaveLength(1)
      await act(async () => {})
      expect(within(row(0)).getAllByRole('gridcell')[3].textContent).toBe('Failed')
    })

    it('adds nothing when the dialog is cancelled', async () => {
      const { changes, commits, user } = await setup()
      await choose(user, /^Wallhaven/)
      await user.type(screen.getAllByRole('textbox')[0], 'zzzz')
      await user.click(button('Cancel'))
      expect(screen.queryByRole('dialog')).toBeNull()
      expect(changes.concat(commits)).toHaveLength(0)
    })

    it('adds one Images row per picked file', async () => {
      vi.mocked(api.selectFiles).mockResolvedValue(['/p/a.jpg', '/p/b.png'])
      vi.mocked(api.checkSource).mockResolvedValue({ count: 1 })
      const { last, user } = await setup()
      await choose(user, /^Images/)
      await act(async () => {})
      await user.click(button('OK'))
      expect(last().sources.slice(4).map((s) => [s.type, s.location, s.enabled])).toEqual([
        ['images', '/p/a.jpg', true],
        ['images', '/p/b.png', true],
      ])
      expect(new Set(last().sources.map((s) => s.id)).size).toBe(6)
    })

    it('Edit reopens the dialog with the current value and keeps the id', async () => {
      vi.mocked(api.checkSource).mockResolvedValue({ count: 2 })
      const { last, user } = await setup()
      await user.click(within(row(1)).getByText('Wallhaven'))
      await user.click(button('Edit…'))
      const field = screen.getAllByRole('textbox')[0] as HTMLInputElement
      expect(field.value).toBe('nature')
      await user.clear(field)
      await user.type(field, 'ocean')
      await user.click(button('OK'))
      expect(api.checkSource).toHaveBeenCalledWith('wallhaven', 'ocean')
      expect(last().sources[1]).toEqual({ ...SOURCES[1], location: 'ocean' })
    })

    it('Enter on a row opens Edit', async () => {
      const { user } = await setup()
      await user.click(within(row(1)).getByText('Wallhaven'))
      await user.keyboard('{Enter}')
      expect(screen.getByRole('dialog', { name: 'Edit Wallhaven source' })).toBeTruthy()
    })

    it('saves a name typed in Add, and a named row shows the name with its type', async () => {
      vi.mocked(api.checkSource).mockResolvedValue({ count: 5 })
      const { last, user } = await setup()
      await choose(user, /^Wallhaven/)
      const dialog = screen.getByRole('dialog', { name: 'Add Wallhaven source' })
      await user.type(within(dialog).getAllByRole('textbox')[0], 'https://wallhaven.cc/hot')
      await user.type(within(dialog).getByRole('textbox', { name: /^Name/ }), 'wallhaven-hot')
      await user.click(within(dialog).getByRole('button', { name: 'OK' }))
      expect(last().sources[4]).toMatchObject({ type: 'wallhaven', location: 'https://wallhaven.cc/hot', name: 'wallhaven-hot' })
      expect(within(row(4)).getByText('wallhaven-hot')).toBeTruthy()
      expect(within(row(4)).getByText('Wallhaven')).toBeTruthy()
    })

    it('Edit changes the name; clearing it removes the name', async () => {
      vi.mocked(api.checkSource).mockResolvedValue({ count: 2 })
      const named = [{ ...SOURCES[1], name: 'Nature picks' }]
      const { last, user } = await setup({ sources: named })
      await user.click(within(row(0)).getByText('Nature picks'))
      await user.click(button('Edit…'))
      const name = screen.getByRole('textbox', { name: /^Name/ }) as HTMLInputElement
      expect(name.value).toBe('Nature picks')
      await user.clear(name)
      await user.type(name, 'Forest')
      await user.click(button('OK'))
      expect(last().sources[0]).toEqual({ ...SOURCES[1], name: 'Forest' })
      await user.click(button('Edit…'))
      await user.clear(screen.getByRole('textbox', { name: /^Name/ }))
      await user.click(button('OK'))
      expect(last().sources[0]).toEqual(SOURCES[1])
      expect('name' in last().sources[0]).toBe(false)
    })

    it('hides online types from Add… when Internet access is off', async () => {
      const { user } = await setup({ internet: false })
      await user.click(button('Add…'))
      expect(screen.queryByRole('button', { name: /^Wallhaven/ })).toBeNull()
      expect(screen.getByRole('button', { name: /^Folders/ })).toBeTruthy()
    })
  })
})
