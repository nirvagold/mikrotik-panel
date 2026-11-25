import { defineConfig } from 'vite';
import preact from '@preact/preset-vite';

export default defineConfig({
  plugins: [preact()],
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2022',
    // CSP tanpa inline: jangan sisipkan aset kecil sebagai data: di script.
    assetsInlineLimit: 0,
    modulePreload: { polyfill: false },
    sourcemap: false,
  },
  server: {
    proxy: { '/api': { target: 'http://127.0.0.1:8765', changeOrigin: false } },
  },
});
