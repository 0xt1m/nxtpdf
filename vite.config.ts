import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { fileURLToPath, URL } from 'node:url';

// Tauri expects a fixed port and fails if it is not available.
const DEV_PORT = 5183;

export default defineConfig(() => ({
  plugins: [react()],

  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },

  // Vite options tailored for Tauri development.
  clearScreen: false,
  server: {
    port: DEV_PORT,
    strictPort: true,
    host: process.env.TAURI_DEV_HOST || false,
    hmr: process.env.TAURI_DEV_HOST
      ? { protocol: 'ws', host: process.env.TAURI_DEV_HOST, port: DEV_PORT + 1 }
      : undefined,
    watch: {
      // Rust sources are watched by the Tauri CLI, not Vite.
      ignored: ['**/src-tauri/**'],
    },
  },

  // Produce output the Tauri bundler can consume.
  build: {
    // Match the webview baseline: WebView2 (Windows), WKWebView (macOS), WebKitGTK (Linux).
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    // `as const` keeps TS from widening this to `string`, which Vite rejects.
    minify: process.env.TAURI_ENV_DEBUG ? false : ('esbuild' as const),
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    outDir: 'dist',
    emptyOutDir: true,
  },
}));
