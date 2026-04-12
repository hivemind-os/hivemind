import { defineConfig } from 'vite';
import solidPlugin from 'vite-plugin-solid';
import path from 'path';

export default defineConfig({
  plugins: [solidPlugin()],
  root: '.',
  resolve: {
    alias: {
      '~': path.resolve(__dirname, '../src'),
    },
  },
  server: {
    port: 3002,
    strictPort: true,
  },
  build: {
    target: 'esnext',
  },
  optimizeDeps: {
    include: ['js-yaml', 'solid-js', 'marked', 'dompurify', 'cytoscape', 'handlebars', '@tanstack/solid-table'],
  },
  appType: 'mpa',
});
