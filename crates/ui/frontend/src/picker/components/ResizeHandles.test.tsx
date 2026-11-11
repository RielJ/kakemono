import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import * as api from '../../shared/api'
import ResizeHandles from './ResizeHandles'

vi.mock('../../shared/api', () => ({
  pickerResizeBegin: vi.fn(),
  pickerResize: vi.fn(),
}))

beforeEach(() => {
  vi.mocked(api.pickerResizeBegin).mockResolvedValue(undefined)
  vi.mocked(api.pickerResize).mockResolvedValue(undefined)
})

afterEach(() => {
  // Ends any drag a test left open, so its window listeners don't reach the next test.
  fireEvent.pointerUp(window)
  cleanup()
  vi.clearAllMocks()
})

/** Lets the begin/resize promise chain settle. */
const settle = () => act(async () => {})

describe('ResizeHandles', () => {
  it('puts the handles on the left and right of a horizontal strip', () => {
    render(<ResizeHandles vertical={false} />)
    expect(screen.getByTestId('resize-left')).toBeTruthy()
    expect(screen.getByTestId('resize-right')).toBeTruthy()
    expect(screen.queryByTestId('resize-top')).toBeNull()
  })

  it('puts the handles on the top and bottom of a vertical strip', () => {
    render(<ResizeHandles vertical />)
    expect(screen.getByTestId('resize-top')).toBeTruthy()
    expect(screen.getByTestId('resize-bottom')).toBeTruthy()
    expect(screen.queryByTestId('resize-left')).toBeNull()
  })

  it('sends the horizontal distance from the grab point for the grabbed end', async () => {
    render(<ResizeHandles vertical={false} />)
    fireEvent.pointerDown(screen.getByTestId('resize-right'), { button: 0, screenX: 1000, screenY: 50 })
    await settle()
    expect(api.pickerResizeBegin).toHaveBeenCalledOnce()
    fireEvent.pointerMove(window, { screenX: 700, screenY: 400 })
    await settle()
    expect(api.pickerResize).toHaveBeenLastCalledWith('end', -300)
  })

  it('uses the vertical distance on a vertical strip', async () => {
    render(<ResizeHandles vertical />)
    fireEvent.pointerDown(screen.getByTestId('resize-top'), { button: 0, screenX: 10, screenY: 200 })
    await settle()
    fireEvent.pointerMove(window, { screenX: 500, screenY: 260 })
    await settle()
    expect(api.pickerResize).toHaveBeenLastCalledWith('start', 60)
  })

  it('waits for the grab before resizing and sends only the newest distance', async () => {
    let grabbed!: () => void
    vi.mocked(api.pickerResizeBegin).mockReturnValue(new Promise<void>((r) => (grabbed = r)))
    render(<ResizeHandles vertical={false} />)
    fireEvent.pointerDown(screen.getByTestId('resize-left'), { button: 0, screenX: 100 })
    fireEvent.pointerMove(window, { screenX: 110 })
    fireEvent.pointerMove(window, { screenX: 150 })
    await settle()
    expect(api.pickerResize).not.toHaveBeenCalled()
    grabbed()
    await settle()
    expect(api.pickerResize).toHaveBeenCalledOnce()
    expect(api.pickerResize).toHaveBeenCalledWith('start', 50)
  })

  it('stops resizing when the pointer is released', async () => {
    render(<ResizeHandles vertical={false} />)
    fireEvent.pointerDown(screen.getByTestId('resize-right'), { button: 0, screenX: 100 })
    await settle()
    fireEvent.pointerUp(window)
    fireEvent.pointerMove(window, { screenX: 400 })
    await settle()
    expect(api.pickerResize).not.toHaveBeenCalled()
  })

  it('ignores the right mouse button', async () => {
    render(<ResizeHandles vertical={false} />)
    fireEvent.pointerDown(screen.getByTestId('resize-right'), { button: 2, screenX: 100 })
    await settle()
    expect(api.pickerResizeBegin).not.toHaveBeenCalled()
  })
})
