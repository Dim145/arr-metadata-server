import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// In development the UI runs on its own port and forwards API calls to the
// Rust server. In production the built assets are embedded in that same binary,
// so every path below is same-origin and no proxy exists.
const backend = process.env.AMS_DEV_BACKEND ?? 'http://127.0.0.1:8080'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: {
    outDir: 'dist',
    // The binary embeds these; a manifest and source maps would only bloat it.
    sourcemap: false,
    reportCompressedSize: false,
  },
  server: {
    port: 5173,
    proxy: Object.fromEntries(
      ['/api', '/v1', '/3', '/health', '/ready'].map((path) => [
        path,
        { target: backend, changeOrigin: false },
      ]),
    ),
  },
})
