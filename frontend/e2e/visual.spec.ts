import { readFileSync } from 'node:fs'

import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

/**
 * The interface as pictures: every gallery of the styleguide, the dialogs it
 * opens, and a few pages as a visitor and an administrator first meet them —
 * held to the pictures in e2e/__screenshots__/visual.spec.ts/.
 *
 * Read by the `visual` project alone (playwright.config.ts says what it holds
 * still), against a build with the styleguide (`vite build --mode
 * styleguide`) served by a server on an empty database. The pictures are made
 * in the official Playwright image and nowhere else (CONTRIBUTING.md): a
 * change that means to change one makes it again there — with Docker, or with
 * the workflow that opens a pull request (.github/workflows/visual-update.yml).
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

/** The styleguide's galleries, in its order. One added there and not here fails the first test below. */
const GALLERIES = [
  'colours',
  'type',
  'glyphs',
  'buttons',
  'glyph-buttons',
  'chips',
  'lamps',
  'choices',
  'fields',
  'switches',
  'labels',
  'panels',
  'page-index',
  'empty',
  'waiting',
  'dialogs',
  'menus',
  'shelves',
  'tables',
]

/** The moment every picture is taken at: nothing drawn from the date moves between two runs. */
const NOW = new Date('2026-10-21T12:00:00Z')

test.skip(process.platform !== 'linux', 'The pictures are drawn on Linux, in the Playwright image: see CONTRIBUTING.md.')

test.beforeEach(async ({ page }) => {
  await page.clock.setFixedTime(NOW)
})

/** Whether the server's build has the styleguide: its first bundle names the route, a production one does not. */
async function hasStyleguide(http: APIRequestContext): Promise<boolean> {
  const page = await (await http.get('/')).text()
  const entry = /<script[^>]+src="(\/assets\/index-[^"]+\.js)"/.exec(page)?.[1]
  return Boolean(entry && (await (await http.get(entry)).text()).includes('__styleguide'))
}

/** Every cell drawn in its language (one waits for its words), the faces loaded, and what a drawing opened laid out. */
async function settled(page: Page) {
  await expect(page.locator('td[data-column]:empty')).toHaveCount(0)
  await page.evaluate(() => document.fonts.ready.then(() => undefined))
  await page.evaluate(
    () => new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))),
  )
}

/** What is painted out of the pages' pictures, and why: see the file. */
const QUIET = readFileSync(new URL('./support/visual.css', import.meta.url), 'utf8')

/**
 * A page whose questions to the server have all been answered, in its own
 * faces, the grain and the washes painted out. Through a stylesheet of the
 * page's own: the content security policy refuses a `<style>` element, which
 * is how the screenshot's `stylePath` would add one — and should go on
 * refusing it here, where a page broken by it has to look broken.
 */
async function settledPage(page: Page) {
  await page.evaluate((css) => {
    const sheet = new CSSStyleSheet()
    sheet.replaceSync(css)
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet]
  }, QUIET)
  await page.waitForLoadState('networkidle')
  await page.evaluate(() => document.fonts.ready.then(() => undefined))
  await page.evaluate(
    () => new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))),
  )
}

test.describe('the styleguide', () => {
  let drawn = false

  test.beforeAll(async ({ playwright }) => {
    const http = await playwright.request.newContext({ baseURL: test.info().project.use.baseURL })
    drawn = await hasStyleguide(http)
    await http.dispose()
  })

  test.beforeEach(() => {
    test.skip(!drawn, 'This server’s build has no styleguide: build it with `vite build --mode styleguide`.')
  })

  test('lists the galleries this spec draws', async ({ page }) => {
    await page.goto('/__styleguide')
    const links = page.getByRole('navigation', { name: 'Galleries' }).getByRole('link')
    await expect(links.first()).toBeVisible()
    const listed = await links.evaluateAll((all) =>
      all.map((link) => (link as HTMLAnchorElement).pathname.split('/')[2]).filter(Boolean),
    )
    expect(listed).toEqual(GALLERIES)
  })

  for (const gallery of GALLERIES) {
    test(`draws the ${gallery} gallery`, async ({ page }) => {
      await page.goto(`/__styleguide/${gallery}`)
      const section = page.locator(`[data-gallery="${gallery}"]`)
      await expect(section).toBeVisible()
      await settled(page)
      await expect(section).toHaveScreenshot(`${gallery}.png`)
    })
  }

  test('draws a failure as the want of a connection, offline', async ({ page, context }) => {
    await page.goto('/__styleguide/empty')
    await settled(page)
    await context.setOffline(true)
    await expect(page.getByText('No connection').first()).toBeVisible()
    await expect(page.locator('[data-gallery="empty"]')).toHaveScreenshot('empty-offline.png')
    await context.setOffline(false)
  })

  // Each kind of dialog in both themes and both languages, between them.
  for (const [column, which] of [
    ['dark-en', 'question'],
    ['light-fr', 'question'],
    ['dark-fr', 'wide'],
    ['light-en', 'wide'],
  ] as const) {
    test(`opens the ${which} dialog from the ${column} column`, async ({ page }) => {
      await page.goto('/__styleguide/dialogs')
      await settled(page)
      await page
        .locator(`[data-gallery="dialogs"] td[data-column="${column}"]`)
        .getByRole('button', { name: which === 'question' ? 'Open the question' : 'Open the wide one' })
        .click()
      await expect(page.getByRole('dialog')).toBeVisible()
      await settledPage(page)
      await expect(page).toHaveScreenshot(`dialog-${which}-${column}.png`)
    })
  }
})

test.describe('the pages, on an empty catalogue', () => {
  let empty = false

  test.beforeAll(async ({ playwright }) => {
    const http = await playwright.request.newContext({ baseURL: test.info().project.use.baseURL })
    const answer = await http.get('/api/v1/items?limit=1')
    empty = answer.ok() && ((await answer.json()) as { total: number }).total === 0
    await http.dispose()
  })

  test.beforeEach(() => {
    test.skip(!empty, 'The pages are drawn on an empty catalogue: start the server on a new database.')
  })

  test('the front page', async ({ page }) => {
    await page.goto('/')
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    await settledPage(page)
    await expect(page).toHaveScreenshot('home.png', { fullPage: true })
  })

  test('the catalogue', async ({ page }) => {
    await page.goto('/browse')
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    await settledPage(page)
    await expect(page).toHaveScreenshot('browse.png', { fullPage: true })
  })

  test('a page that is not there', async ({ page }) => {
    await page.goto('/no-such-page')
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    await settledPage(page)
    await expect(page).toHaveScreenshot('not-found.png', { fullPage: true })
  })

  test('the sign-in page', async ({ page }) => {
    await page.goto('/login')
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    await settledPage(page)
    await expect(page).toHaveScreenshot('sign-in.png')
  })

  // As a server started for the run shows it: the one sign-in this test makes
  // in its audit trail, nothing run yet, and an archive asked for — which
  // the backup schedule first looks at ten minutes after the start
  // (jobs/backup.rs, `FIRST_AFTER`), by when this run is long over.
  test('the dashboard', async ({ page }) => {
    test.skip(!USERNAME || !PASSWORD, 'AMS_E2E_USER and AMS_E2E_PASSWORD are the administrator the server was started with.')
    await page.goto('/login')
    await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
    await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
    await page.getByRole('button', { name: /sign in|se connecter/i }).click()
    await page.waitForURL('**/admin')
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    await settledPage(page)
    // The window alone: the sidebar stays put as the page scrolls, and the
    // health panel further down is the server's running figures. What moves
    // in it — the version, beside the sidebar's title — is painted over in
    // the sidebar's own ink, so that a longer number moves nothing.
    await expect(page).toHaveScreenshot('dashboard.png', {
      mask: [page.locator('aside').getByText(/^v\d+\.\d+\.\d+/)],
      maskColor: '#111113',
    })
  })

  test.describe('in daylight', () => {
    test.use({ colorScheme: 'light' })

    test('the front page', async ({ page }) => {
      await page.goto('/')
      await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
      await settledPage(page)
      await expect(page).toHaveScreenshot('home-light.png', { fullPage: true })
    })
  })

  test.describe('on a phone', () => {
    test.use({ viewport: { width: 375, height: 812 }, isMobile: true, hasTouch: true })

    test('the front page', async ({ page }) => {
      await page.goto('/')
      await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
      await settledPage(page)
      await expect(page).toHaveScreenshot('home-phone.png', { fullPage: true })
    })
  })
})
