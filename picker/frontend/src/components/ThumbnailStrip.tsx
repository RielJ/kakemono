import { useEffect, useRef, useState } from 'react'
import type { Thumbnail } from '../App'
import ContextMenu from './ContextMenu'

interface CtxMenu {
  x: number
  y: number
  thumb: Thumbnail
}

interface StripProps {
  vertical?: boolean
}

export default function ThumbnailStrip({ vertical = false }: StripProps) {
  const [thumbs, setThumbs] = useState<Thumbnail[]>([])
  const [ctxMenu, setCtxMenu] = useState<CtxMenu | null>(null)
  const [loading, setLoading] = useState(true)
  const stripRef = useRef<HTMLDivElement>(null)

  // Load thumbnails on mount
  useEffect(() => {
    window.go?.main?.PickerApp?.GetThumbnails()
      .then((t) => {
        setThumbs(t || [])
        setLoading(false)
      })
      .catch(() => setLoading(false))
  }, [])

  // Scroll with mouse wheel (maps vertical scroll to horizontal when horizontal)
  useEffect(() => {
    const el = stripRef.current
    if (!el) return
    const onWheel = (e: WheelEvent) => {
      e.preventDefault()
      if (vertical) {
        el.scrollTop += e.deltaY + e.deltaX
      } else {
        el.scrollLeft += e.deltaY + e.deltaX
      }
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
  }, [vertical])

  // Arrow keys to scroll
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = stripRef.current
      if (!el) return
      if (vertical) {
        if (e.key === 'ArrowDown') el.scrollTop += 200
        else if (e.key === 'ArrowUp') el.scrollTop -= 200
      } else {
        if (e.key === 'ArrowRight') el.scrollLeft += 200
        else if (e.key === 'ArrowLeft') el.scrollLeft -= 200
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [vertical])

  const handleClick = (thumb: Thumbnail) => {
    window.go?.main?.PickerApp?.SetWallpaper(thumb.path)
    // Optimistically update the current state
    setThumbs((prev) =>
      prev.map((t) => ({
        ...t,
        isCurrent: t.path === thumb.path,
      }))
    )
  }

  const handleContextMenu = (e: React.MouseEvent, thumb: Thumbnail) => {
    e.preventDefault()
    setCtxMenu({ x: e.clientX, y: e.clientY, thumb })
  }

  const handleFavorite = () => {
    if (ctxMenu) {
      window.go?.main?.PickerApp?.FavoriteWallpaper(ctxMenu.thumb.path)
      setCtxMenu(null)
    }
  }

  if (loading) {
    return (
      <div className="loading">
        <span className="loading-icon">󰸉</span>
        <span>Loading wallpapers…</span>
      </div>
    )
  }

  if (thumbs.length === 0) {
    return (
      <div className="empty">
        <span className="empty-icon">󰸉</span>
        <span>No wallpapers found — start the daemon first</span>
      </div>
    )
  }

  return (
    <>
      <div className={`strip ${vertical ? 'strip-vertical' : ''}`} ref={stripRef}>
        {thumbs.map((thumb) => (
          <div
            key={thumb.path}
            className={`thumb-item ${thumb.isCurrent ? 'current' : ''}`}
            onClick={() => handleClick(thumb)}
            onContextMenu={(e) => handleContextMenu(e, thumb)}
            title={`${thumb.title || thumb.source}\n${thumb.source}`}
          >
            <img
              src={thumb.imageUrl}
              alt={thumb.title}
              loading="lazy"
              draggable={false}
            />
            {thumb.isCurrent && <div className="current-badge">✓</div>}
            <div className="thumb-label">{thumb.source}</div>
          </div>
        ))}
      </div>
      {ctxMenu && (
        <ContextMenu
          x={ctxMenu.x}
          y={ctxMenu.y}
          thumb={ctxMenu.thumb}
          onFavorite={handleFavorite}
          onClose={() => setCtxMenu(null)}
        />
      )}
    </>
  )
}
