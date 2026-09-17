import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    // TauriCPP loads from embedded resources, so base should be ./
    base: './',
  },
  server: {
    port: 5173,
    strictPort: true,
  },
});
