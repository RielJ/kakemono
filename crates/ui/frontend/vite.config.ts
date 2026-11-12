import { fileURLToPath } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { searchForWorkspaceRoot } from 'vite'
import { defineConfig } from 'vitest/config'

const page = (name: string) => fileURLToPath(new URL(`./${name}.html`, import.meta.url))

// Tauri loads `settings.html`, `picker.html` and `welcome.html` from `dist/` (release) or from the dev
// server on a fixed port (`tauri.conf.json` `build.devUrl`).
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    host: process.env.TAURI_DEV_HOST || false,
    // The Welcome window shows the app icon from `src-tauri/icons`, outside this root.
    fs: { allow: [searchForWorkspaceRoot(process.cwd()), '../src-tauri/icons'] },
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
        welcome: page('welcome'),
      },
    },
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.{ts,tsx}'],
  },
})
