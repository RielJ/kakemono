import { useEffect, useRef } from 'react'
import type { Thumbnail } from '../App'

interface Props {
  x: number
  y: number
  thumb: Thumbnail
  onFavorite: () => void
  onClose: () => void
}

export default function ContextMenu({ x, y, thumb, onFavorite, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const onClick = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) {
        onClose()
      }
    }
    document.addEventListener('mousedown', onClick)
    return () => document.removeEventListener('mousedown', onClick)
  }, [onClose])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])

  // Position menu above click point (strip is at the bottom)
  const menuStyle: React.CSSProperties = {
    position: 'fixed',
    left: Math.min(x, window.innerWidth - 180),
    bottom: window.innerHeight - y + 8,
  }

  return (
    <div className="context-menu" style={menuStyle} ref={ref}>
      <div className="ctx-title">{thumb.title || thumb.source}</div>
      <button className="ctx-item" onClick={onFavorite}>
        <span className="ctx-icon">󰋑</span> Add to favorites
      </button>
      <button className="ctx-item" onClick={onClose}>
        <span className="ctx-icon">✕</span> Cancel
      </button>
    </div>
  )
}
