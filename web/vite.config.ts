import { defineConfig } from "vitest/config";
import preact from "@preact/preset-vite";

// `npm run dev` proxies the API to a local cmuxd (CMUXD_URL, default
// http://127.0.0.1:7878), so the cookie and the WebSocket stay same-origin.
const target = process.env.CMUXD_URL ?? "http://127.0.0.1:7878";

export default defineConfig({
  plugins: [preact()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // The CSP allows no inline scripts and no data: scripts.
    assetsInlineLimit: 0,
    modulePreload: { polyfill: false },
  },
  server: {
    proxy: { "/v1": { target, ws: true, changeOrigin: false } },
  },
  test: {
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
  },
});
