import { expect, test, type Page } from '@playwright/test'

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

/**
 * The catalogue in numbers, for everyone; the health, for the administrator
 * alone.
 */
test.describe('the numbers and the health', () => {
  test('the figures are counted and shown', async ({ request, page }) => {
    const response = await request.get('/api/v1/figures')
    expect(response.status()).toBe(200)
    const figures = await response.json()
    expect(figures.total).toBe(figures.series + figures.movies)
    for (const key of ['decades', 'genres', 'networks', 'languages', 'scores', 'statuses']) {
      expect(Array.isArray(figures[key]), key).toBe(true)
    }
    await page.goto('/stats')
    await expect(page.getByRole('heading', { level: 1, name: /in numbers|en chiffres/i })).toBeVisible()
    await expect(page.getByRole('heading', { name: /by decade|par décennie/i })).toBeVisible()

    // A tongue the providers spell two ways is one bar.
    const languages = page.getByRole('heading', { name: /by original language|par langue originale/i }).locator('xpath=ancestor::section[1]')
    const names = (await languages.getByRole('link').allTextContents()).map((text) => text.replace(/\s*[\d   ]+$/, '').trim())
    expect(new Set(names).size).toBe(names.length)

    // And every bar is a way into the catalogue it counts.
    const genres = page.getByRole('heading', { name: /by genre|par genre/i }).locator('xpath=ancestor::section[1]')
    const bar = genres.getByRole('link').first()
    test.skip((await bar.count()) === 0, 'no genre to count')
    await bar.click()
    await expect(page).toHaveURL(/\/browse\?genre=/)
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
  })

  test('the health is the administrator’s alone', async ({ request }) => {
    expect((await request.get('/api/v1/admin/health')).status()).toBe(401)
  })
})

test.describe('the health, read by the administrator', () => {
  test.skip(
    !USERNAME || !PASSWORD,
    'set AMS_E2E_USER and AMS_E2E_PASSWORD to read the health as an administrator',
  )

  test('the dashboard says what the server is and what it holds', async ({ page }) => {
    await signIn(page)

    // The same session, over the API: the figures the panel draws from.
    const response = await page.request.get('/api/v1/admin/health')
    expect(response.status()).toBe(200)
    const health = await response.json()
    expect(health.version).toMatch(/^\d+\.\d+\.\d+/)
    expect(['sqlite', 'postgres']).toContain(health.database)
    expect(health.works).toBeGreaterThan(0)
    expect(Array.isArray(health.sources)).toBe(true)

    // And on the dashboard, in words.
    const panel = page.locator('#health')
    await expect(panel).toBeVisible()
    await expect(panel.getByText(health.version, { exact: true })).toBeVisible()
    await expect(panel.getByText(health.database, { exact: true })).toBeVisible()
  })

  test('the server counts itself for a scrape, and the locks travel as a file', async ({ page }) => {
    await signIn(page)

    const metrics = await page.request.get('/api/v1/admin/metrics')
    expect(metrics.status()).toBe(200)
    expect(metrics.headers()['content-type']).toMatch(/^text\/plain/)
    const text = await metrics.text()
    expect(text).toContain('# TYPE ams_http_requests_total counter')
    expect(text).toMatch(/ams_http_requests_total\{surface="native",status="2xx"\} \d+/)
    expect(text).toMatch(/ams_works\{kind="series"\} \d+/)
    expect(text).toMatch(/ams_uptime_seconds \d/)

    // A lock to travel: the first series' sort title, set by hand.
    const { items } = await (await page.request.get('/api/v1/items?kind=series&limit=1')).json()
    const work = (items as { id: string }[])[0]
    expect(work).toBeTruthy()
    const locked = await page.request.put(`/api/v1/items/${work.id}/overrides`, {
      data: { scope: 'item', field: 'sortTitle', value: 'zz travelling lock' },
    })
    expect(locked.status()).toBe(200)

    const exported = await page.request.get('/api/v1/admin/locks')
    expect(exported.status()).toBe(200)
    const locks = await exported.json()
    expect(locks.version).toBe(1)
    expect(locks.locks.length).toBeGreaterThan(0)
    const travelling = locks.locks.find(
      (l: { work: { id: string }; field: string }) => l.work.id === work.id && l.field === 'sortTitle',
    )
    expect(travelling.value).toBe('zz travelling lock')
    expect(travelling.work.kind).toBe('series')

    // Imported back, every lock is already so; named by its ids alone, it
    // still finds its work.
    const imported = await page.request.post('/api/v1/admin/locks', { data: locks })
    expect(imported.status()).toBe(200)
    const done = await imported.json()
    expect(done.applied + done.unchanged).toBe(locks.locks.length)
    expect(done.unchanged).toBeGreaterThan(0)
    expect(done.unmatched).toEqual([])
    expect(done.refused).toEqual([])
    const bare = { ...travelling, work: { ...travelling.work, id: undefined }, value: 'zz travelled lock' }
    const again = await page.request.post('/api/v1/admin/locks', { data: { version: 1, locks: [bare] } })
    expect((await again.json()).applied).toBe(1)

    // A field a work cannot be read without is never cleared.
    const cleared = await page.request.post('/api/v1/admin/locks', {
      data: { version: 1, locks: [{ ...travelling, field: 'title', value: null }] },
    })
    expect((await cleared.json()).refused.length).toBe(1)

    // And the lock is taken off again, so the list keeps its order.
    expect((await page.request.delete(`/api/v1/items/${work.id}/overrides/item/sortTitle`)).status()).toBe(204)

    // A lock on a work nobody holds is named, not set.
    const stray = await page.request.post('/api/v1/admin/locks', {
      data: {
        version: 1,
        locks: [{ work: { kind: 'movie', title: 'Nobody Holds This', tmdb: 999999991 }, scope: 'item', field: 'title', value: 'x' }],
      },
    })
    expect(stray.status()).toBe(200)
    expect((await stray.json()).unmatched).toEqual(['Nobody Holds This'])

    // And the panel offers both.
    await page.goto('/admin')
    const panel = page.locator('#locks-file')
    await expect(panel.getByRole('button', { name: /download the locks|télécharger les verrous/i })).toBeVisible()
    await expect(panel.getByText(/import a locks file|importer un fichier de verrous/i)).toBeVisible()
  })
})

test.describe('what is the administrator’s alone', () => {
  test('the scrape and the locks turn a visitor away', async ({ request }) => {
    expect((await request.get('/api/v1/admin/metrics')).status()).toBe(401)
    expect((await request.get('/api/v1/admin/locks')).status()).toBe(401)
    expect((await request.post('/api/v1/admin/locks', { data: { version: 1, locks: [] } })).status()).toBe(401)
  })
})
