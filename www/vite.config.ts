import { defineConfig } from 'vite-plus'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
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
