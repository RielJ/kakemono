import { useEffect, useState } from 'react'
import ThumbnailStrip from './components/ThumbnailStrip'
import { closeWindow, getConfig } from '../shared/api'

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

  useEffect(() => {
    // Close when window loses focus (click outside)
    // Delay enabling blur detection to avoid closing immediately on launch
    let active = false
    const timer = setTimeout(() => {
      active = true
    }, 800)

    const onBlur = () => {
      if (active) {
        void closeWindow()
      }
    }
    window.addEventListener('blur', onBlur)
    return () => {
      clearTimeout(timer)
      window.removeEventListener('blur', onBlur)
    }
  }, [])

  useEffect(() => {
    // Close on Escape key
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        void closeWindow()
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
