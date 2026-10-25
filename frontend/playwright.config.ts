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

/** The specs that change server-wide settings: see the `server-wide` project. */
const SERVER_WIDE = /.*\.serial\.spec\.ts/

/** The pictures the interface is held to, and the styleguide they are drawn from: see the `visual` project. */
const VISUAL = /(visual|styleguide)\.spec\.ts/

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
      testIgnore: [SERVER_WIDE, VISUAL],
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
      testIgnore: [SERVER_WIDE, VISUAL],
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
      // One file at a time too: a private site from one spec and passwords
      // switched off by another would otherwise meet.
      workers: 1,
      use: { ...devices['Desktop Chrome'], channel: 'chromium', viewport: { width: 1440, height: 900 } },
    },
    {
      // The interface as pictures, held to the ones in e2e/__screenshots__ —
      // which are drawn in one place only: the official Playwright image of
      // the version in package-lock.json, on x86-64 (CONTRIBUTING.md). Type is
      // drawn differently by every system's fonts and rasteriser, so a
      // picture taken on a Mac would differ from CI's in every glyph; on
      // anything but Linux the spec stands aside.
      //
      // Everything that could move between two runs is held still: one
      // browser and one worker, the window's size and density fixed, the
      // page's clock frozen (in the spec), motion off and animations
      // finished, the caret hidden, text rasterised without hinting or
      // subpixel colour, and what still varies masked by the spec. What
      // remains is a rendering difference too small to be a change: a fifth
      // of a percent of the picture's pixels.
      //
      // Run apart, and only by name (`--project visual`): it wants a build
      // with the styleguide (`vite build --mode styleguide`) and a server on
      // an empty database, which none of the other projects do. The
      // styleguide's own spec (styleguide.spec.ts) runs here too, on any
      // system: it scans the galleries rather than drawing them.
      name: 'visual',
      testMatch: VISUAL,
      fullyParallel: false,
      workers: 1,
      retries: 0,
      // No `{platform}` in the path: there is one set of pictures, Linux's.
      snapshotPathTemplate: '{testDir}/__screenshots__/{testFilePath}/{arg}{ext}',
      expect: {
        toHaveScreenshot: { maxDiffPixelRatio: 0.002, animations: 'disabled', caret: 'hide', scale: 'css' },
      },
      use: {
        ...devices['Desktop Chrome'],
        channel: 'chromium',
        viewport: { width: 1600, height: 1000 },
        deviceScaleFactor: 1,
        colorScheme: 'dark',
        reducedMotion: 'reduce',
        locale: 'en-GB',
        timezoneId: 'UTC',
        launchOptions: { args: ['--disable-lcd-text', '--font-render-hinting=none'] },
      },
    },
  ],
})
