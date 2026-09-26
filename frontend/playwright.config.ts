import { defineConfig, devices } from '@playwright/test'

/**
 * End-to-end tests run against the Rust binary, not the dev server.
 *
 * That binary is what ships: it serves the built assets from the same origin
 * the API answers on, with the real guards in front of both. A suite that
 * passed against Vite's proxy would say nothing about whether a visitor can
 * actually read the catalogue.
 *
 * Start one first, with a database that has some works in it:
 *
 *   AMS_PUBLIC_BROWSE=true AMS_BIND_ADDRESS=127.0.0.1:8479 AMS_RATE_LIMIT_PER_MINUTE=6000 \
 *   AMS_DATABASE_URL='sqlite://data/e2e.db?mode=rwc' ./target/debug/arr-metadata-server
 *
 * The limit is raised because the whole suite comes from one address; see
 * e2e/README.md.
 */
const baseURL = process.env.AMS_E2E_URL ?? 'http://127.0.0.1:8479'

/** The specs that change server-wide settings: see the last project. */
const SERVER_WIDE = /.*\.serial\.spec\.ts/

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? 'line' : [['list']],

  use: {
    // Dark unless a test asks for daylight: the page's own baseline.
    colorScheme: 'dark',
    baseURL,
    // Screenshots and traces only for what failed: a green run should leave
    // nothing behind to clean up.
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
  },

  projects: [
    {
      name: 'desktop',
      testIgnore: SERVER_WIDE,
      use: { ...devices['Desktop Chrome'], channel: 'chromium', viewport: { width: 1440, height: 900 } },
    },
    {
      name: 'mobile',
      // A phone described rather than borrowed: the device presets for real
      // handsets carry a WebKit engine with them, and only Chromium is
      // installed here. 375 is the narrowest width still worth supporting — if
      // the catalogue works there it works everywhere above it.
      use: {
        ...devices['Desktop Chrome'],
        channel: 'chromium',
        viewport: { width: 375, height: 812 },
        deviceScaleFactor: 2,
        isMobile: true,
        hasTouch: true,
      },
      testIgnore: SERVER_WIDE,
    },
    {
      // What changes how the whole server answers — a private site, an API
      // switched off — runs after everything else, alone: both projects run
      // at once against one server, and a site closed for three seconds is a
      // dozen unrelated failures in the other.
      name: 'server-wide',
      testMatch: SERVER_WIDE,
      dependencies: ['desktop', 'mobile'],
      fullyParallel: false,
      use: { ...devices['Desktop Chrome'], channel: 'chromium', viewport: { width: 1440, height: 900 } },
    },
  ],
})
