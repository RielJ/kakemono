import { describe, expect, it } from 'vitest'

import type { Config } from '../shared/api'
import { diffConfig, isEmptyPatch } from './configPatch'

function config(): Config {
  return {
    daemon: {
      rotation_enabled: true,
      interval: '30m',
      download_dir: '/home/u/Pictures/Wallpapers/cache',
      cache_size: 50,
      trash_dir: '/home/u/Pictures/Wallpapers/trash',
      socket_path: null,
      rotate_on_theme_change: true,
      reapply_on_space_change: false,
    },
    wallhaven: { enabled: false, api_key: '', purity: '100', sorting: 'random', categories: '111' },
    unsplash: { enabled: false, access_key: '' },
    reddit: { enabled: false, subreddits: ['EarthPorn'] },
    bing: { enabled: true, market: 'en-US' },
    apod: { enabled: false, api_key: 'DEMO_KEY' },
    local: { enabled: false, dirs: [], recursive: false },
    light: { sources: ['bing'], queries: [] },
    dark: { sources: ['bing'], queries: [] },
    favorites: { dir: '/home/u/Pictures/Wallpapers/favorites' },
    picker: { position: 'bottom', force_x11: false, close_on_blur: true, placement: 'edge', custom: null },
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
    ui: {},
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
    next.reddit.subreddits = ['EarthPorn', 'SpacePorn']
    expect(diffConfig(before, next)).toEqual({
      daemon: { socket_path: null },
      reddit: { subreddits: ['EarthPorn', 'SpacePorn'] },
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
