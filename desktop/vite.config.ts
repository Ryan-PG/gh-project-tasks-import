import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Tauri sets this when running `tauri dev --host` for device testing.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  plugins: [react(), tailwindcss()],

  // Tauri owns the terminal, so Vite must not clear it.
  clearScreen: false,

  server: {
    // Must match `build.devUrl` in tauri.conf.json.
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    // Rust sources are watched by cargo, not Vite; watching them here causes
    // a full reload on every Rust edit.
    watch: { ignored: ["**/src-tauri/**"] },
  },

  envPrefix: ["VITE_", "TAURI_ENV_*"],

  build: {
    // WebView2 on Windows tracks Chromium; WKWebView on macOS and WebKitGTK on
    // Linux lag, so the older floor is the one that matters.
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
