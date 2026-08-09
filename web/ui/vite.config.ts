import { defineConfig } from 'vite'
import solid from 'vite-plugin-solid'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [solid(), tailwindcss()],
  build: {
    // The page is served from memory by a process that reads it once, so the
    // only thing that matters about the output is how much of it the browser
    // has to parse. No sourcemaps, and everything inlined that is smaller than
    // the request it would take to fetch it.
    sourcemap: false,
    assetsInlineLimit: 4096,
    target: 'es2022',
  },
  server: {
    // `npm run dev` for hot reload, against the same engine the built page
    // talks to. Nothing about the API changes between the two.
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:7777',
        changeOrigin: false,
      },
    },
  },
})
