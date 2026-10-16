import { expect, test, type Page } from '@playwright/test'

/**
 * The settings page: a setting is found by typing, the ones changed from what
 * they began as say so and offer the way back, and what was read from the
 * environment at startup is a section of its own.
 *
 * Read-only. The search changes nothing, and the marks and the way back are
 * tried against a stand-in for the one setting the page is told was changed —
 * what the server holds is not touched, so this runs beside the others.
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

const search = (page: Page) => page.getByRole('searchbox', { name: /^(search a setting|chercher un réglage)$/i })
const limit = (page: Page) => page.getByLabel(/^(results per search|résultats par recherche)$/i)
const batch = (page: Page) => page.getByLabel(/^(works per sweep|œuvres par passage)$/i)
const index = (page: Page) => page.getByRole('navigation', { name: /^(on this page|sur cette page)$/i })

test.describe('the settings page', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('finds a setting by its title, its key or what explains it, and says how many are left', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
    await signIn(page)
    await page.goto('/admin/settings')
    await expect(limit(page)).toBeVisible()
    await expect(batch(page)).toBeVisible()

    // By its title.
    await search(page).fill('works per sweep')
    await expect(batch(page)).toBeVisible()
    await expect(limit(page)).toHaveCount(0)

    // By its key.
    await search(page).fill('tmdb.searchLimit')
    await expect(limit(page)).toBeVisible()
    await expect(batch(page)).toHaveCount(0)

    // By a word of what explains it, and how much of the page is left said.
    await search(page).fill('hands back')
    await expect(limit(page)).toBeVisible()
    await expect(page.getByRole('status').filter({ hasText: /^1 of \d+ settings$/ })).toBeVisible()
    // Every word has to be found, wherever it is.
    await search(page).fill('refresh batch')
    await expect(batch(page)).toBeVisible()
    await expect(limit(page)).toHaveCount(0)

    // The index of the page leaves out the groups with nothing left in them.
    const refresh = index(page).getByRole('link', { name: /^automatic refresh$/i })
    await search(page).fill('hands back')
    await expect(refresh).toHaveCount(0)
    await search(page).fill('')
    await expect(refresh).toHaveCount(1)
  })

  test('folds accents and ligatures, and a search that finds nothing says so and clears', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'fr'))
    await signIn(page)
    await page.goto('/admin/settings')
    await expect(limit(page)).toBeVisible()

    // "œuvres", typed as a keyboard without the ligature types it.
    await search(page).fill('oeuvres par passage')
    await expect(batch(page)).toBeVisible()
    await expect(limit(page)).toHaveCount(0)
    await search(page).fill('  RESULTATS   par ')
    await expect(limit(page)).toBeVisible()

    await search(page).fill('zzzzqq')
    await expect(page.getByText(/aucun réglage ne répond à « zzzzqq »/i)).toBeVisible()
    await expect(limit(page)).toHaveCount(0)
    await page.getByRole('button', { name: /^effacer la recherche$/i }).click()
    await expect(search(page)).toHaveValue('')
    await expect(limit(page)).toBeVisible()
  })

  test('marks what was changed from what it began as, offers the way back, and filters to the changed', async ({
    page,
  }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
    await signIn(page)

    // The server's own settings, but for one: it began as 20 and holds 7. A
    // write is answered as the server answers one — with the settings as
    // they stand after it — and recorded here, not made.
    const target = 'tmdb.searchLimit'
    let held = '7'
    const writes: unknown[] = []
    await page.route(/\/api\/v1\/settings\/server\/-$/, async (route) => {
      if (route.request().method() === 'PUT') {
        writes.push(route.request().postDataJSON())
        held = '20'
      }
      const real = (await (await page.request.get('/api/v1/settings/server/-')).json()) as { key: string; value: string }[]
      await route.fulfill({
        json: real.map((setting) =>
          setting.key === target
            ? { ...setting, value: held, default: '20' }
            : { ...setting, default: setting.value },
        ),
      })
    })

    await page.goto('/admin/settings')
    const row = page.locator('li').filter({ has: limit(page) })
    await expect(limit(page)).toHaveValue('7')
    await expect(row.getByText(/^modified$/i)).toBeVisible()
    await expect(row.getByText(/^default: 20$/i)).toBeVisible()
    // Only that one: the others began as they are.
    await expect(page.getByText(/^modified$/i)).toHaveCount(1)

    // The changed ones alone, on request, and the whole page back.
    const only = page.getByRole('button', { name: /^changed only/i })
    await only.click()
    await expect(only).toHaveAttribute('aria-pressed', 'true')
    await expect(limit(page)).toBeVisible()
    await expect(batch(page)).toHaveCount(0)
    await expect(page.getByRole('status').filter({ hasText: /^1 of \d+ settings$/ })).toBeVisible()
    await only.click()
    await expect(batch(page)).toBeVisible()

    // The way back writes what it began as.
    await row.getByRole('button', { name: /^back to the default$/i }).click()
    await expect.poll(() => writes).toEqual([{ key: target, value: '20' }])
    await expect(limit(page)).toHaveValue('20')
    await expect(row.getByText(/^modified$/i)).toHaveCount(0)
    await expect(page.getByText(/^modified$/i)).toHaveCount(0)
  })

  test('keeps what was read at startup in a section of its own, after what can be changed', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
    await signIn(page)
    await page.goto('/admin/settings')
    await expect(limit(page)).toBeVisible()

    const startup = page.getByRole('region', { name: /^(read at startup|lus au démarrage)$/i })
    await expect(startup).toBeVisible()
    await expect(startup.getByText('Version', { exact: true })).toBeVisible()
    await expect(startup.locator('#settings-policy')).toBeVisible()
    // Nothing in it can be changed, and none of what can be is in it.
    await expect(startup.getByRole('textbox')).toHaveCount(0)
    await expect(startup.getByRole('switch')).toHaveCount(0)
    await expect(startup.getByLabel(/^results per search$/i)).toHaveCount(0)

    // Under the settings, not among them.
    const above = (await limit(page).boundingBox())!
    const below = (await startup.boundingBox())!
    expect(below.y).toBeGreaterThan(above.y)
    await expect(index(page).getByRole('link', { name: /^read at startup$/i })).toBeVisible()
  })
})
