import { useEffect, useState } from 'react'
import ThumbnailStrip from './components/ThumbnailStrip'
import { getConfig, hidePicker } from '../shared/api'

export type PickerPosition = 'bottom' | 'top' | 'left' | 'right'

function toPosition(value: string): PickerPosition {
  return value === 'top' || value === 'left' || value === 'right' ? value : 'bottom'
}

export default function App() {
  const [position, setPosition] = useState<PickerPosition>('bottom')

  useEffect(() => {
    getConfig()
      .then((cfg) => setPosition(toPosition(cfg.picker.position)))
      .catch(() => {})
  }, [])

  // Hiding on blur is handled in Rust (`picker.rs`, honours `[picker] close_on_blur`).
  useEffect(() => {
    // Close on Escape key. `hide_picker` also saves a dragged position.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        void hidePicker()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const isVertical = position === 'left' || position === 'right'

  return (
    <div className={`app ${isVertical ? 'app-vertical' : 'app-horizontal'}`}>
      <ThumbnailStrip vertical={isVertical} />
    </div>
  )
}
