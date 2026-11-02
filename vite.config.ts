import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import path from 'path';
import {defineConfig} from 'vite';

export default defineConfig(() => {
  return {
    plugins: [react(), tailwindcss()],
    resolve: {
      alias: {
        '@': path.resolve(__dirname, '.'),
      },
    },
    // Tauri expects a fixed dev port and must see Rust errors, so don't clear the screen.
    clearScreen: false,
    server: {
      port: 1420,
      strictPort: true,
      watch: {ignored: ['**/src-tauri/**', '**/crates/**', '**/target/**']},
    },
  };
});
