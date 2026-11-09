import { useEffect, useState } from 'react'
import ThumbnailStrip from './components/ThumbnailStrip'
import { getAppInfo, getConfig } from '../shared/api'

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
    // 13 px type on macOS (tokens.css keys off data-os).
    getAppInfo()
      .then((info) => {
        document.documentElement.dataset.os = info.os
      })
      .catch(() => {})
  }, [])

  // Esc is handled in ThumbnailStrip (it also sends a pending trash first). Hiding on blur is
  // handled in Rust (`picker.rs`, honours `[picker] close_on_blur`).
  const isVertical = position === 'left' || position === 'right'

  return (
    <main className={`app ${isVertical ? 'app-vertical' : 'app-horizontal'}`}>
      <ThumbnailStrip vertical={isVertical} />
    </main>
  )
}
