import { defineConfig, devices } from "@playwright/test";

// BASE_URL is required. CI sets it to the live Fly URL or a localhost
// dev server. The per-run namespace prefix (per converged plan K4) is
// derived from PLAYWRIGHT_RUN_ID — github.run_id in CI, a short random
// value for local runs. All test handles get this suffix so multiple
// runs against shared production state don't collide.

const BASE_URL = process.env.BASE_URL ?? "http://127.0.0.1:8080";
const RUN_ID =
  process.env.PLAYWRIGHT_RUN_ID ??
  `local${Date.now().toString(36).slice(-6)}`;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false, // sequential — the verified backend is shared state
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: process.env.CI ? [["github"], ["list"]] : "list",
  use: {
    baseURL: BASE_URL,
    extraHTTPHeaders: {
      "X-Test-Run-Id": RUN_ID,
    },
    screenshot: "only-on-failure",
    trace: "on-first-retry",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
