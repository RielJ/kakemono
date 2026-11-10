import { describe, expect, it } from 'vitest'

import type { Config } from '../shared/api'
import { diffConfig, isEmptyPatch } from './configPatch'

function config(): Config {
  return {
    daemon: {
      rotation_enabled: true,
      interval: '30m',
      download_dir: '/home/u/Pictures/Wallpapers/cache',
      trash_dir: '/home/u/Pictures/Wallpapers/trash',
      socket_path: null,
      rotate_on_theme_change: true,
      reapply_on_space_change: false,
      change_on_start: false,
      internet: true,
      download_enabled: true,
      download_interval: '10m',
      download_limit_mb_enabled: true,
      download_limit_mb: 500,
      manual_dir: null,
    },
    wallhaven: { api_key: '', purity: '100', sorting: 'random', categories: '111' },
    unsplash: { access_key: '' },
    bing: { market: 'en-US' },
    apod: { api_key: 'DEMO_KEY' },
    sources: [
      { id: 'bing', enabled: true, type: 'bing', location: '' },
      { id: 'wh-nature', enabled: true, type: 'wallhaven', location: 'nature' },
      { id: 'favorites', enabled: true, type: 'folder', location: '/home/u/Pictures/Wallpapers/favorites' },
    ],
    filters: {
      landscape: false,
      min_size_enabled: false,
      min_size_percent: 80,
      brightness: 'off',
      color_enabled: false,
      color: '#3b82f6',
      rating_enabled: false,
      min_rating: 4,
      name_regex_enabled: false,
      name_regex: '',
    },
    favorites: { dir: '/home/u/Pictures/Wallpapers/favorites' },
    picker: { position: 'bottom', orientation: 'auto', force_x11: false, close_on_blur: true, blur_nsfw: true, placement: 'edge', custom: null },
    desktop: {
      backend: 'auto',
      set_command: null,
      persistent: false,
      fit: 'fill',
      theme: 'auto',
      managed_by: 'detect',
      detected_session: null,
      detected_at: null,
    },
    hooks: { on_change: null },
    ui: { appearance: 'light' },
  }
}

describe('diffConfig', () => {
  it('is empty when nothing changed', () => {
    expect(isEmptyPatch(diffConfig(config(), config()))).toBe(true)
  })

  it('sends only the changed keys', () => {
    const next = config()
    next.daemon.interval = '1h'
    next.bing.market = 'ja-JP'
    expect(diffConfig(config(), next)).toEqual({
      daemon: { interval: '1h' },
      bing: { market: 'ja-JP' },
    })
  })

  it('replaces arrays whole and sends null for removed values', () => {
    const before = config()
    before.daemon.socket_path = '/tmp/k.sock'
    const next = config()
    next.sources = [...next.sources, { id: 'rd-1337', enabled: true, type: 'reddit', location: 'r/SpacePorn' }]
    expect(diffConfig(before, next)).toEqual({
      daemon: { socket_path: null },
      sources: next.sources,
    })
  })

  it('sends a whole object that replaces null', () => {
    const next = config()
    next.picker.custom = { x: 10, y: 20, monitor: 'DP-1' }
    expect(diffConfig(config(), next)).toEqual({
      picker: { custom: { x: 10, y: 20, monitor: 'DP-1' } },
    })
  })
})
