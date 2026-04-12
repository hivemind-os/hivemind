import { defineConfig } from 'vitest/config';
import { resolve } from 'path';

export default defineConfig({
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
  },
  resolve: {
    alias: {
      '~': resolve(__dirname, 'src'),
      '@tauri-apps/api/core': resolve(__dirname, 'src/__mocks__/tauri.ts'),
    },
  },
});
