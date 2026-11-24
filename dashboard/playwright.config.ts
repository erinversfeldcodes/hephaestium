import { defineConfig } from "@playwright/test";
import { origin } from "./browser/pages";

// The built dashboard, served as the control plane serves it, in Chromium. The API is answered
// from the fixtures inside each test, so nothing else needs to run.
export default defineConfig({
  testDir: "./browser",
  testMatch: "*.spec.ts",
  outputDir: "browser-results",
  // A test that fails once is run once more; the one that passes on the second try is reported as
  // flaky, and the github reporter shows it as a warning, so it is seen and not hidden.
  retries: 1,
  reporter: process.env["CI"] ? [["github"], ["list"]] : [["list"]],
  fullyParallel: true,
  use: { baseURL: origin, browserName: "chromium", trace: "retain-on-failure" },
  webServer: {
    command: "npm run build && npx vite preview --host 127.0.0.1 --port 4173 --strictPort",
    url: origin,
    reuseExistingServer: !process.env["CI"],
    // Measured locally: the build takes under 1s and the preview server answers within a second, so
    // 120s is over a hundred times that, for a slow runner. Past it Playwright stops with an error
    // that names the server, and no test runs.
    timeout: 120_000,
  },
});
