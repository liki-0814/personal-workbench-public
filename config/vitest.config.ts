import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'
import * as path from 'path'

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./tests/setup.ts'],
    exclude: ['**/e2e/**', '**/node_modules/**', '**/.claude/**', '**/.worktrees/**', '**/integration-live/**', '**/temp_dir/**'],
  },
  resolve: {
    alias: {
      '@': path.resolve(__dirname, '../src'),
    },
  },
})
