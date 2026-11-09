import { fileURLToPath } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

const page = (name: string) => fileURLToPath(new URL(`./${name}.html`, import.meta.url))

// Tauri loads `settings.html` and `picker.html` from `dist/` (release) or from the dev
// server on a fixed port (`tauri.conf.json` `build.devUrl`).
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: process.env.TAURI_DEV_HOST || false,
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    // Tauri webviews: WKWebView (macOS 11+), WebView2, WebKitGTK 2.40+.
    target: 'safari15',
    rollupOptions: {
      input: {
        settings: page('settings'),
        picker: page('picker'),
      },
    },
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.{ts,tsx}'],
  },
})
