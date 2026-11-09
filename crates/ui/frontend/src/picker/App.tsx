import { useEffect, useState } from 'react'
import ThumbnailStrip from './components/ThumbnailStrip'

// Wails runtime bindings will be generated at build time
declare global {
  interface Window {
    go: {
      main: {
        PickerApp: {
          GetThumbnails(): Promise<Thumbnail[]>
          GetPosition(): Promise<string>
          SetWallpaper(path: string): Promise<void>
          FavoriteWallpaper(path: string): Promise<void>
          Close(): Promise<void>
        }
      }
    }
    runtime: {
      Quit(): void
    }
  }
}

export interface Thumbnail {
  path: string
  source: string
  title: string
  isCurrent: boolean
  imageUrl: string
}

export type PickerPosition = 'bottom' | 'top' | 'left' | 'right'

export default function App() {
  const [position, setPosition] = useState<PickerPosition>('bottom')

  useEffect(() => {
    window.go?.main?.PickerApp?.GetPosition()
      .then((p) => setPosition((p || 'bottom') as PickerPosition))
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
        window.go?.main?.PickerApp?.Close()
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
        window.go?.main?.PickerApp?.Close()
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
