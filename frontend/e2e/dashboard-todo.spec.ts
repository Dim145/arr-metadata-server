import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Page } from '@playwright/test'

/**
 * The dashboard: what there is to do, first; the figures after it, the locked
 * fields among them and quiet; and the explanation of locking, which folds.
 *
 * The list of errands is read from `/admin/todo`, which is answered by a
 * stand-in where a test wants figures of its own — what a catalogue of the
 * suite's size holds is nothing to build a test on — and nothing here changes
 * what the server holds.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

const NOTHING = {
  refreshFailed: 0,
  noPoster: 0,
  noOverview: 0,
  noImdb: 0,
  neverRefreshed: 0,
  expiringKeys: 0,
  keyHorizonDays: 14,
  failedMedia: 0,
}

/** Answer the list of errands with these figures. */
async function told(page: Page, errands: typeof NOTHING) {
  await page.route(/\/api\/v1\/admin\/todo$/, (route) => route.fulfill({ json: errands }))
}

const todo = (page: Page) => page.getByRole('region', { name: /^(to do|à faire)$/i })

test.describe('the dashboard', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
  })

  test('lists what is to do, each line the way to the catalogue filtered to it or to the page that settles it', async ({
    page,
  }) => {
    await told(page, { ...NOTHING, refreshFailed: 2, noPoster: 3, noImdb: 5, neverRefreshed: 1, expiringKeys: 1, failedMedia: 4 })
    await signIn(page)
    await page.goto('/admin')

    const card = todo(page)
    const line = (name: RegExp) => card.getByRole('link', { name })
    await expect(line(/^3 works have no poster$/)).toHaveAttribute('href', '/admin/catalogue?missing=poster')
    await expect(line(/^5 works have no IMDb identifier$/)).toHaveAttribute('href', '/admin/catalogue?missing=imdb')
    await expect(line(/^1 work was never refreshed$/)).toHaveAttribute('href', '/admin/catalogue?missing=refresh')
    await expect(line(/^2 works failed their last refresh$/)).toHaveAttribute('href', '/admin/catalogue?refreshFailed=1')
    await expect(line(/^1 API key expires within 14 days$/)).toHaveAttribute('href', '/admin/clients')
    await expect(line(/^4 pictures or themes could not be fetched$/)).toHaveAttribute('href', '/admin/media')

    // A figure of nought is no line: the synopsis count was nought.
    await expect(card.getByRole('link')).toHaveCount(6)
    await expect(card.getByText(/synopsis/i)).toHaveCount(0)

    // A line is followed to where it says.
    await line(/^3 works have no poster$/).click()
    await expect(page).toHaveURL(/\/admin\/catalogue\?missing=poster$/)
  })

  test('says once that there is nothing to do, rather than showing a row of noughts', async ({ page }) => {
    await told(page, NOTHING)
    await signIn(page)
    await page.goto('/admin')

    await expect(todo(page).getByText(/nothing to do/i)).toBeVisible()
    await expect(todo(page).getByRole('link')).toHaveCount(0)
  })

  test('is counted by the server, for the administrator alone', async ({ page, browser }) => {
    await signIn(page)
    const answer = await page.request.get('/api/v1/admin/todo')
    expect(answer.status()).toBe(200)
    const errands = (await answer.json()) as Record<string, unknown>
    for (const key of Object.keys(NOTHING)) expect(typeof errands[key], key).toBe('number')
    expect(errands.keyHorizonDays).toBe(14)

    const stranger = await browser.newContext()
    expect((await stranger.request.get('/api/v1/admin/todo')).status()).toBe(401)
    await stranger.close()
  })

  test('folds the explanation of locking, and keeps it folded', async ({ page }) => {
    await signIn(page)
    await page.goto('/admin')

    const toggle = page.getByRole('button', { name: /^(how locking works|comment fonctionne le verrouillage)$/i })
    // The panel it opens, by the id the button names — read afresh each
    // time: React's ids are made per render, and a reload makes new ones.
    const body = async () => page.locator(`[id="${await toggle.getAttribute('aria-controls')}"]`)
    // Open the first time, which is what it is for.
    await expect(toggle).toHaveAttribute('aria-expanded', 'true')
    const shown = await body()
    await expect(shown).toBeVisible()

    await toggle.click()
    await expect(toggle).toHaveAttribute('aria-expanded', 'false')
    await expect(shown).toHaveCount(0)

    // Where the reader left it, on the next visit.
    await page.reload()
    await expect(toggle).toHaveAttribute('aria-expanded', 'false')
    expect(await page.evaluate(() => localStorage.getItem('ams.dashboard.locking'))).toBe('folded')

    await toggle.click()
    await expect(toggle).toHaveAttribute('aria-expanded', 'true')
    await expect(await body()).toBeVisible()
  })

  test('counts the locked fields small, among the other figures and no longer above them', async ({ page }) => {
    await told(page, NOTHING)
    await signIn(page)
    await page.goto('/admin')

    const size = (label: RegExp) =>
      page
        .getByText(label)
        .first()
        .locator('xpath=ancestor::div[1]')
        .locator('p.font-display')
        .first()
        .evaluate((figure) => parseFloat(getComputedStyle(figure).fontSize))
    const series = await size(/^series$/i)
    const locked = await size(/^locked fields$/i)
    expect(locked).toBeLessThan(series)

    // After the list of errands, and no bigger than a figure among figures.
    const card = (await todo(page).boundingBox())!
    const lockedAt = (await page.getByText(/^locked fields$/i).first().boundingBox())!
    expect(lockedAt.y).toBeGreaterThan(card.y)
  })

  test('meets WCAG 2.1 AA with a list of errands in it', async ({ page }) => {
    await told(page, { ...NOTHING, refreshFailed: 2, noPoster: 3, expiringKeys: 1, failedMedia: 4 })
    await signIn(page)
    await page.goto('/admin')
    await expect(todo(page).getByRole('link').first()).toBeVisible()
    await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'))
    const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze()
    expect(results.violations.map((v) => `${v.id}: ${v.help}`)).toEqual([])
  })
})
