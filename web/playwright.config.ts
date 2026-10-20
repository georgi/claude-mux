import { defineConfig, devices } from "@playwright/test";

// `npm run e2e` drives a real cmuxd whose agents are the fake claude. Build
// both first: `cargo build -p cmuxd --bin cmuxd --example fake-claude` (a
// debug cmuxd serves web/dist/ from disk, so `npm run build` comes first).
export default defineConfig({
  testDir: "e2e",
  timeout: 60_000,
  expect: { timeout: 15_000 },
  workers: 1,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    ...devices["Desktop Chrome"],
    viewport: { width: 1280, height: 800 },
    trace: "retain-on-failure",
  },
});
