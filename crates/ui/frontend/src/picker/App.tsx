import { useEffect, useState } from 'react'
import ThumbnailStrip from './components/ThumbnailStrip'
import { DEFAULT_LAYOUT, isVertical, toLayout } from './orientation'
import { getAppInfo, getConfig } from '../shared/api'

export default function App() {
  const [layout, setLayout] = useState(DEFAULT_LAYOUT)

  useEffect(() => {
    // The window is hidden, not closed, and Rust re-sizes it from the config on every show:
    // re-read `[picker] position` / `orientation` each time it is shown so the layout matches.
    let current = true
    const loadLayout = () => {
      getConfig()
        .then((cfg) => {
          if (current) setLayout(toLayout(cfg.picker))
        })
        .catch(() => {})
    }
    loadLayout()
    window.addEventListener('focus', loadLayout)
    // 13 px type on macOS (tokens.css keys off data-os).
    getAppInfo()
      .then((info) => {
        document.documentElement.dataset.os = info.os
      })
      .catch(() => {})
    return () => {
      current = false
      window.removeEventListener('focus', loadLayout)
    }
  }, [])

  // Esc is handled in ThumbnailStrip (it also sends a pending trash first). Hiding on blur is
  // handled in Rust (`picker.rs`, honours `[picker] close_on_blur`).
  const vertical = isVertical(layout)

  return (
    <main className={`app ${vertical ? 'app-vertical' : 'app-horizontal'}`}>
      <ThumbnailStrip vertical={vertical} />
    </main>
  )
}
