import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// https://vite.dev/config/
export default defineConfig({
  plugins: [svelte()],

  // Ensure Svelte resolves to the client (not server) entry.
  // Svelte 5's exports map defaults to index-server.js; the "browser"
  // condition selects index-client.js for client-side builds.
  resolve: {
    conditions: ["browser"],
  },

  // Dev server proxies API calls to the Rust navi-server backend.
  server: {
    host: "0.0.0.0",
    port: 5173,
    proxy: {
      // API routes (HTTP)
      "/health": "http://127.0.0.1:9800",
      "/models": "http://127.0.0.1:9800",
      "/config": "http://127.0.0.1:9800",
      "/model": "http://127.0.0.1:9800",
      "/providers": "http://127.0.0.1:9800",
      "/usage": "http://127.0.0.1:9800",
      "/skills": "http://127.0.0.1:9800",
      "/memory": "http://127.0.0.1:9800",
      "/credentials": "http://127.0.0.1:9800",
      "/oauth": "http://127.0.0.1:9800",
      "/permission-mode": "http://127.0.0.1:9800",
      "/plugins": "http://127.0.0.1:9800",
      "/mcp": "http://127.0.0.1:9800",
      "/routing": "http://127.0.0.1:9800",
      "/registry": "http://127.0.0.1:9800",
      "/voice": "http://127.0.0.1:9800",
      // Sessions (HTTP + WebSocket) — must handle both protocols.
      "/sessions": {
        target: "http://127.0.0.1:9800",
        ws: true,
      },
    },
  },

  build: {
    outDir: "dist",
    // Use base: './' so assets work regardless of the mount path.
    base: "./",
  },
});
