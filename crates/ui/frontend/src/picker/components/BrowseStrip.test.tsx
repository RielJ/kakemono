import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import * as api from '../../shared/api'
import type { BrowseListing, BrowseSourceStatus, BrowseUseData, RemoteThumb } from '../../shared/api'
import { INTERNET_OFF } from '../useBrowse'
import BrowseStrip, { remoteLabel } from './BrowseStrip'
import type { Notices } from './ThumbnailStrip'

vi.mock('../../shared/api', async (importOriginal) => {
  const real = await importOriginal<typeof import('../../shared/api')>()
  return { ...real, browsePage: vi.fn(), browseUse: vi.fn(), resetPickerPosition: vi.fn() }
})

const browsePage = vi.mocked(api.browsePage)
const browseUse = vi.mocked(api.browseUse)

/** A stand-in for `IntersectionObserver` (jsdom has none) that a test can trigger. */
class FakeObserver {
  static instances: FakeObserver[] = []
  readonly targets = new Set<Element>()
  disconnected = false
  constructor(readonly callback: IntersectionObserverCallback) {
    FakeObserver.instances.push(this)
  }
  observe(target: Element) {
    this.targets.add(target)
  }
  unobserve(target: Element) {
    this.targets.delete(target)
  }
  disconnect() {
    this.disconnected = true
    this.targets.clear()
  }
  takeRecords(): IntersectionObserverEntry[] {
    return []
  }
  /**
   * Waits until a live observer watches a target, then reports it in view. The sentinel's
   * observer is made in a passive effect, which can run after the DOM update a test waited for.
   */
  static async showWhenObserved() {
    await vi.waitFor(() => expect(FakeObserver.instances.some((o) => !o.disconnected && o.targets.size > 0)).toBe(true))
    act(() => FakeObserver.showAll())
  }
  /** Reports every live observer's targets as scrolled into view. */
  static showAll() {
    for (const o of FakeObserver.instances.filter((x) => !x.disconnected)) {
      const entries = [...o.targets].map((target) => ({ target, isIntersecting: true }) as IntersectionObserverEntry)
      o.callback(entries, o as unknown as IntersectionObserver)
    }
  }
}

function item(n: number, over: Partial<RemoteThumb> = {}): RemoteThumb {
  return {
    sourceId: 'wh',
    kind: 'wallhaven',
    remoteId: `r${n}`,
    title: `T${n}`,
    author: 'Ann',
    width: 1920,
    height: 1080,
    nsfw: false,
    imageUrl: `thumb://localhost/${n}`,
    ...over,
  }
}

const range = (from: number, n: number) => Array.from({ length: n }, (_, i) => item(from + i))
const ACTIVE = (source_id: string, kind: string): BrowseSourceStatus => ({ source_id, kind, state: { state: 'active' } })

function listing(over: Partial<BrowseListing> = {}): BrowseListing {
  return { session: 's1', page: 0, restarted: false, done: false, items: range(0, 10), sources: [ACTIVE('wh', 'wallhaven')], ...over }
}

function deferred<T>() {
  let resolve!: (v: T) => void
  let reject!: (e: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

/** `commands_sources/browse.rs` `STILL_DOWNLOADING`. */
const STILL_DOWNLOADING = 'Still downloading in the background…'

const notices: Notices = { say: vi.fn(), dismissUndo: vi.fn() }
const USED: BrowseUseData = { path: '/d/wh/r0.jpg', wallpaper: null }

function renderStrip({
  blurNsfw = true,
  vertical = false,
  sourceNames,
}: { blurNsfw?: boolean; vertical?: boolean; sourceNames?: ReadonlyMap<string, string> } = {}) {
  render(
    <BrowseStrip vertical={vertical} notices={notices} blurNsfw={blurNsfw} sourceNames={sourceNames} memory={{ current: null }} />,
  )
}

async function strip() {
  const list = await screen.findByRole('listbox', { name: 'Previews' })
  await vi.waitFor(() => expect(within(list).getAllByRole('option').length).toBeGreaterThan(0))
  return list
}

function activeOption(list: HTMLElement): HTMLElement | null {
  const id = list.getAttribute('aria-activedescendant')
  return id ? document.getElementById(id) : null
}

beforeEach(() => {
  FakeObserver.instances = []
  vi.stubGlobal('IntersectionObserver', FakeObserver)
  browsePage.mockReset()
  browseUse.mockReset()
  vi.mocked(notices.say).mockReset()
  browsePage.mockImplementation(async ({ page }) => listing({ page, items: range(page * 10, 10) }))
})
afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('BrowseStrip', () => {
  describe('paging', () => {
    it('the sentinel loads the next page', async () => {
      renderStrip()
      await strip()
      expect(browsePage).toHaveBeenCalledExactlyOnceWith({ session: null, page: 0, source: null })
      await FakeObserver.showWhenObserved()
      await vi.waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(20))
      expect(browsePage).toHaveBeenLastCalledWith({ session: 's1', page: 1, source: null })
    })

    it('a new page keeps the strip where it was scrolled; a key move still scrolls', async () => {
      const scrolled = vi.fn()
      Element.prototype.scrollIntoView = scrolled
      try {
        const user = userEvent.setup()
        renderStrip()
        const list = await strip()
        scrolled.mockClear()
        await FakeObserver.showWhenObserved()
        await vi.waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(20))
        expect(scrolled).not.toHaveBeenCalled()
        await user.keyboard('{ArrowRight}')
        expect(scrolled).toHaveBeenCalledTimes(1)
        expect(scrolled.mock.contexts[0]).toBe(activeOption(list))
      } finally {
        Reflect.deleteProperty(Element.prototype, 'scrollIntoView')
      }
    })

    it('the next-page slot shows a loading spinner', async () => {
      renderStrip()
      const list = await strip()
      const slot = list.querySelector('.option-skeleton')
      expect(slot?.querySelector('.spinner')).toBeTruthy()
    })

    it('moving the keyboard within 3 items of the end loads more', async () => {
      const user = userEvent.setup()
      renderStrip()
      const list = await strip()
      expect(document.activeElement).toBe(list)
      for (let i = 0; i < 6; i++) await user.keyboard('{ArrowRight}')
      expect(browsePage).toHaveBeenCalledTimes(1)
      await user.keyboard('{ArrowRight}')
      await vi.waitFor(() => expect(browsePage).toHaveBeenCalledTimes(2))
      expect(browsePage).toHaveBeenLastCalledWith({ session: 's1', page: 1, source: null })
    })

    it('shows a Reload card at 300 items that starts a new session', async () => {
      browsePage.mockImplementation(async ({ page }) => listing({ page, items: range(page * 10, 10) }))
      renderStrip()
      await strip()
      for (let p = 1; p < 30; p++) {
        await FakeObserver.showWhenObserved()
        await vi.waitFor(() => expect(screen.getAllByRole('option')).toHaveLength((p + 1) * 10))
      }
      const reload = await screen.findByRole('button', { name: 'Reload' })
      act(() => FakeObserver.showAll())
      expect(browsePage).toHaveBeenCalledTimes(30)
      fireEvent.click(reload)
      await vi.waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(10))
      expect(browsePage).toHaveBeenLastCalledWith({ session: null, page: 0, source: null })
    })
  })

  describe('actions', () => {
    it('Enter sets the image, busy until it is done', async () => {
      const use = deferred<BrowseUseData>()
      browseUse.mockReturnValue(use.promise)
      const user = userEvent.setup()
      renderStrip()
      const list = await strip()
      await user.keyboard('{Enter}')
      expect(browseUse).toHaveBeenCalledExactlyOnceWith({ sourceId: 'wh', remoteId: 'r0', action: 'set' })
      const option = activeOption(list)
      expect(option?.getAttribute('aria-busy')).toBe('true')
      // A second Enter while busy doesn't ask again.
      await user.keyboard('{Enter}')
      expect(browseUse).toHaveBeenCalledTimes(1)
      await act(async () => use.resolve({ path: '/d/wh/r0.jpg', wallpaper: null }))
      expect(option?.getAttribute('aria-busy')).toBeNull()
      expect(notices.say).toHaveBeenLastCalledWith('Set as wallpaper')
    })

    it('f adds to favorites', async () => {
      browseUse.mockResolvedValue(USED)
      const user = userEvent.setup()
      renderStrip()
      await strip()
      await user.keyboard('{ArrowRight}f')
      expect(browseUse).toHaveBeenCalledExactlyOnceWith({ sourceId: 'wh', remoteId: 'r1', action: 'favorite' })
      await vi.waitFor(() => expect(notices.say).toHaveBeenLastCalledWith('Added to Favorites'))
    })

    it('a timeout says the download goes on; another failure says why', async () => {
      browseUse.mockRejectedValueOnce({ kind: 'daemon_timeout', message: STILL_DOWNLOADING })
      const user = userEvent.setup()
      renderStrip()
      const list = await strip()
      await user.keyboard('{Enter}')
      await vi.waitFor(() => expect(notices.say).toHaveBeenLastCalledWith(STILL_DOWNLOADING))
      expect(activeOption(list)?.getAttribute('aria-busy')).toBeNull()

      browseUse.mockRejectedValueOnce({ kind: 'daemon', message: 'image expired' })
      await user.keyboard('{Enter}')
      await vi.waitFor(() => expect(notices.say).toHaveBeenLastCalledWith('Couldn’t get the image: image expired'))
    })

    it('the menu has Set, Favorite, Download and Reset Position, and no Trash', async () => {
      browseUse.mockResolvedValue(USED)
      renderStrip()
      await strip()
      fireEvent.contextMenu(screen.getAllByRole('option')[2], { clientX: 10, clientY: 10 })
      const menu = await screen.findByRole('menu')
      expect(within(menu).getAllByRole('menuitem').map((m) => m.textContent)).toEqual([
        'Set as Wallpaper',
        'Add to Favorites',
        'Download',
        'Reset Position',
      ])
      fireEvent.click(within(menu).getByRole('menuitem', { name: 'Download' }))
      expect(browseUse).toHaveBeenCalledExactlyOnceWith({ sourceId: 'wh', remoteId: 'r2', action: 'download' })
      await vi.waitFor(() => expect(notices.say).toHaveBeenLastCalledWith('Downloaded'))
    })

    it('Delete does nothing on Sources', async () => {
      const user = userEvent.setup()
      renderStrip()
      await strip()
      await user.keyboard('{Delete}')
      expect(browseUse).not.toHaveBeenCalled()
    })

    it('a click sets the image', async () => {
      browseUse.mockResolvedValue(USED)
      renderStrip()
      await strip()
      fireEvent.click(screen.getAllByRole('option')[3])
      expect(browseUse).toHaveBeenCalledExactlyOnceWith({ sourceId: 'wh', remoteId: 'r3', action: 'set' })
    })
  })

  describe('blur', () => {
    const nsfwPage = async () => listing({ items: [item(0, { nsfw: true }), item(1)] })

    it('blurs an NSFW card until hover or keyboard focus when blurNsfw is on', async () => {
      browsePage.mockImplementation(nsfwPage)
      const user = userEvent.setup()
      renderStrip()
      const list = await strip()
      const [nsfw, safe] = within(list).getAllByRole('option')
      expect(safe.hasAttribute('data-blurred')).toBe(false)
      // The list took focus with the NSFW card active: moving off it blurs it.
      await user.keyboard('{ArrowRight}')
      expect(nsfw.hasAttribute('data-blurred')).toBe(true)
      fireEvent.pointerEnter(nsfw)
      expect(nsfw.hasAttribute('data-blurred')).toBe(false)
      fireEvent.pointerLeave(nsfw)
      expect(nsfw.hasAttribute('data-blurred')).toBe(true)
      await user.keyboard('{ArrowLeft}')
      expect(nsfw.hasAttribute('data-blurred')).toBe(false)
      // Focus elsewhere: the active card blurs again.
      act(() => list.blur())
      expect(nsfw.hasAttribute('data-blurred')).toBe(true)
    })

    it('never blurs when blurNsfw is off', async () => {
      browsePage.mockImplementation(nsfwPage)
      const user = userEvent.setup()
      renderStrip({ blurNsfw: false })
      const list = await strip()
      await user.keyboard('{ArrowRight}')
      expect(within(list).getAllByRole('option')[0].hasAttribute('data-blurred')).toBe(false)
    })
  })

  describe('display', () => {
    it('labels a card "title by author — kind" and shows a placeholder without a preview', async () => {
      browsePage.mockResolvedValue(listing({ items: [item(0, { imageUrl: null }), item(1, { author: '' }), item(2, { title: '' })] }))
      renderStrip()
      const list = await strip()
      const options = within(list).getAllByRole('option')
      expect(options.map((o) => o.getAttribute('aria-label'))).toEqual([
        'T0 by Ann — wallhaven',
        'T1 — wallhaven',
        'r2 by Ann — wallhaven',
      ])
      // No native hover tooltip on a wallpaper tile.
      expect(options.every((o) => !o.hasAttribute('title'))).toBe(true)
      expect(options[0].querySelector('img')).toBeNull()
      expect(options[0].querySelector('.placeholder')).not.toBeNull()
      expect(options[1].querySelector('img')?.getAttribute('src')).toBe('thumb://localhost/1')
    })

    it('remoteLabel', () => {
      expect(remoteLabel(item(5))).toBe('T5 by Ann — wallhaven')
      expect(remoteLabel(item(5), new Map([['wh', 'wallhaven-hot']]))).toBe('T5 by Ann — wallhaven-hot')
    })

    it('a named source shows its name on the tile, in the label, the filter and the notes', async () => {
      browsePage.mockResolvedValue(
        listing({
          items: [item(0), item(1, { sourceId: 'un', kind: 'unsplash' })],
          sources: [
            ACTIVE('wh', 'wallhaven'),
            { source_id: 'un', kind: 'unsplash', state: { state: 'rate_limited', message: 'rate limit reached', retry_after_secs: 60 } },
          ],
        }),
      )
      renderStrip({ sourceNames: new Map([['wh', 'wallhaven-hot']]) })
      const list = await strip()
      const options = within(list).getAllByRole('option')
      expect(options.map((o) => o.querySelector('.caption')?.textContent)).toEqual(['wallhaven-hot', 'unsplash'])
      expect(options[0].getAttribute('aria-label')).toBe('T0 by Ann — wallhaven-hot')
      const group = await screen.findByRole('radiogroup', { name: 'Source' })
      expect(within(group).getByRole('radio', { name: 'wallhaven-hot' })).toBeTruthy()
      expect(within(group).getByRole('radio', { name: 'unsplash (un)' })).toBeTruthy()
      // An unnamed source keeps "kind (id)".
      expect(screen.getByText('unsplash (un): rate limit reached')).toBeTruthy()
    })

    it('shows internet-off, a failure with Retry, and an empty session', async () => {
      browsePage.mockRejectedValueOnce({ kind: 'daemon', message: 'Internet access is off' })
      renderStrip()
      expect(await screen.findByText(INTERNET_OFF)).toBeTruthy()
      cleanup()

      browsePage.mockRejectedValueOnce({ kind: 'daemon', message: 'daemon error' })
      renderStrip()
      expect(await screen.findByText('Couldn’t browse sources: daemon error')).toBeTruthy()
      fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
      await strip()
      cleanup()

      browsePage.mockResolvedValueOnce(listing({ items: [], done: true, sources: [] }))
      renderStrip()
      expect(await screen.findByText('No online sources are enabled. Add one in Settings.')).toBeTruthy()
    })

    it('an empty page that is not the end asks for the next few, then stops with Retry', async () => {
      const limited: BrowseSourceStatus = {
        source_id: 'un',
        kind: 'unsplash',
        state: { state: 'rate_limited', message: 'rate limit reached', retry_after_secs: 60 },
      }
      browsePage.mockImplementation(async ({ page }) => listing({ page, items: [], sources: [limited] }))
      renderStrip()
      expect(await screen.findByText('No previews to show.')).toBeTruthy()
      expect(screen.getByText('unsplash (un): rate limit reached')).toBeTruthy()
      expect(browsePage).toHaveBeenCalledTimes(4)
      browsePage.mockImplementation(async ({ page }) => listing({ page }))
      fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
      await strip()
      expect(browsePage).toHaveBeenLastCalledWith({ session: null, page: 0, source: null })
    })

    it('says which sources have a problem', async () => {
      browsePage.mockResolvedValue(
        listing({
          sources: [
            ACTIVE('wh', 'wallhaven'),
            { source_id: 'un', kind: 'unsplash', state: { state: 'rate_limited', message: 'rate limit reached', retry_after_secs: 60 } },
          ],
        }),
      )
      renderStrip()
      await strip()
      expect(screen.getByText('unsplash (un): rate limit reached')).toBeTruthy()
    })

    it('a failed later page keeps the items and offers Retry', async () => {
      renderStrip()
      await strip()
      browsePage.mockRejectedValueOnce({ kind: 'daemon_unreachable', message: 'gone' })
      await FakeObserver.showWhenObserved()
      const retry = await screen.findByRole('button', { name: 'Retry' })
      expect(screen.getByText('gone')).toBeTruthy()
      expect(screen.getAllByRole('option')).toHaveLength(10)
      fireEvent.click(retry)
      await vi.waitFor(() => expect(screen.getAllByRole('option')).toHaveLength(20))
    })
  })

  describe('filter', () => {
    const three = [ACTIVE('wh', 'wallhaven'), ACTIVE('bg', 'bing'), ACTIVE('rd', 'reddit')]

    it('appears only with more than one source', async () => {
      renderStrip()
      await strip()
      expect(screen.queryByRole('radiogroup')).toBeNull()
    })

    it('lists All and each source, and [ / ] cycle it', async () => {
      browsePage.mockImplementation(async ({ source }) => listing({ session: source ?? 'all', sources: source ? three.filter((s) => s.source_id === source) : three }))
      const user = userEvent.setup()
      renderStrip()
      await strip()
      const group = await screen.findByRole('radiogroup', { name: 'Source' })
      expect(within(group).getAllByRole('radio').map((r) => r.getAttribute('aria-label') ?? r.closest('label')?.textContent)).toEqual([
        'All',
        'wallhaven (wh)',
        'bing (bg)',
        'reddit (rd)',
      ])
      expect(within(group).getByRole('radio', { name: 'All' })).toHaveProperty('checked', true)

      await user.keyboard(']')
      await vi.waitFor(() => expect(browsePage).toHaveBeenLastCalledWith({ session: null, page: 0, source: 'wh' }))
      expect(within(group).getByRole('radio', { name: 'wallhaven (wh)' })).toHaveProperty('checked', true)
      // user-event escapes `[` as `[[`: this presses it twice.
      await user.keyboard('[[[[')
      await vi.waitFor(() => expect(browsePage).toHaveBeenLastCalledWith({ session: null, page: 0, source: 'rd' }))

      fireEvent.click(within(group).getByRole('radio', { name: 'bing (bg)' }))
      await vi.waitFor(() => expect(browsePage).toHaveBeenLastCalledWith({ session: null, page: 0, source: 'bg' }))
      // Still every source after a filtered page.
      expect(within(group).getAllByRole('radio')).toHaveLength(4)
    })
  })
})
