import { defineConfig } from 'vite-plus'
import react from '@vitejs/plugin-react'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'
import type { Plugin } from 'vite'

function staticDirIndex(): Plugin {
  return {
    name: 'static-dir-index',
    configureServer(server) {
      server.middlewares.use((req, _res, next) => {
        const url = req.url?.split('?')[0] ?? ''
        if (url.endsWith('/') || url.includes('.')) return next()
        const indexPath = resolve('public', url.slice(1), 'index.html')
        if (existsSync(indexPath)) {
          req.url = url + '/index.html'
        }
        next()
      })
    },
  }
}

export default defineConfig({
  plugins: [staticDirIndex(), react()],
  lint: {
    ignorePatterns: ['dist/**'],
    options: {
      typeAware: true,
      typeCheck: true,
    },
  },
  fmt: {
    singleQuote: true,
    semi: false,
    sortPackageJson: true,
  },
})
