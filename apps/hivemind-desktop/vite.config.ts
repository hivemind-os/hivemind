import { defineConfig } from 'vite';
import solidPlugin from 'vite-plugin-solid';
import devtools from 'solid-devtools/vite';
import path from 'path';

export default defineConfig({
  plugins: [devtools(), solidPlugin()],
  cacheDir: path.resolve(__dirname, '.vite'),
  resolve: {
    alias: {
      '~': path.resolve(__dirname, './src'),
    },
  },
  server: {
    port: 3000,
    strictPort: true,
  },
  build: {
    target: 'esnext',
  },
  worker: {
    format: 'es',
  },
});
