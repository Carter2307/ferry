/// <reference types="vitest/config" />
import { fileURLToPath, URL } from 'node:url'

import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig, type ProxyOptions } from 'vite'

// Where `npm run dev` forwards API calls. ferryd's default listen address is
// 127.0.0.1:7878; point it elsewhere with FERRY_API_URL=http://host:port.
const apiTarget = process.env.FERRY_API_URL ?? 'http://127.0.0.1:7878'

const proxy: ProxyOptions = {
  target: apiTarget,
  changeOrigin: true,
  // Tell ferryd which host the browser used: it compares it with the Origin
  // of requests that carry the session cookie.
  xfwd: true,
  // Server-Sent Events (log streams, change feed) must reach the browser
  // unbuffered and uncompressed.
  configure: (p) => {
    p.on('proxyReq', (proxyReq, req) => {
      if (req.headers.accept?.includes('text/event-stream')) {
        proxyReq.setHeader('accept-encoding', 'identity')
      }
    })
    p.on('proxyRes', (proxyRes) => {
      if (proxyRes.headers['content-type']?.includes('text/event-stream')) {
        proxyRes.headers['cache-control'] = 'no-cache, no-transform'
        proxyRes.headers['x-accel-buffering'] = 'no'
      }
    })
  },
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  server: {
    port: 5173,
    proxy: { '/api': proxy, '/hooks': proxy, '/healthz': proxy },
  },
  preview: {
    port: 4173,
    proxy: { '/api': proxy, '/hooks': proxy, '/healthz': proxy },
  },
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    sourcemap: false,
    chunkSizeWarningLimit: 900,
    rolldownOptions: {
      output: {
        // Long-lived vendor chunks: app deploys don't invalidate the framework cache.
        codeSplitting: {
          groups: [
            { name: 'react', test: /node_modules[\\/](react|react-dom|scheduler|react-router)[\\/]/, priority: 30 },
            { name: 'radix', test: /node_modules[\\/](@radix-ui|radix-ui|cmdk|@floating-ui)[\\/]/, priority: 20 },
            { name: 'vendor', test: /node_modules[\\/]/, priority: 10 },
          ],
        },
      },
    },
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.{ts,tsx}'],
  },
})
