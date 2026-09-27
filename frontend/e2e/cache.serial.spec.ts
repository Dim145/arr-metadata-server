import { expect, test, type Page } from '@playwright/test'

/**
 * The cache, read back and acted on: the page says which tiers answer, a
 * space is switched off and on and emptied, everything is emptied after a
 * word of warning, a public page carries the headers a proxy keeps it by,
 * and a stranger reads none of it.
 *
 * The second tier needs a Valkey or Redis the suite's server was started
 * with (AMS_REDIS_URL); without one the server-side assertions are skipped
 * and the memory-only page is checked instead. Server-wide, because a
 * flush changes what every other test reads.
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

interface Report {
  server: { configured: boolean; attached: boolean; up: boolean; keys?: number; prefix: string }
  spaces: { id: string; enabled: boolean; memory: { entries: number; hits: number; misses: number }; server?: { keys?: number } }[]
  epoch: number
}

test.describe('the cache', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('the page reads the tiers back, and a space is switched and emptied', async ({ page }) => {
    await signIn(page)

    // Something to hold: a list read twice is served from memory the second time.
    await page.request.get('/api/v1/items?kind=series&limit=2')
    await page.request.get('/api/v1/items?kind=series&limit=2')
    const before = (await (await page.request.get('/api/v1/admin/cache')).json()) as Report
    const searches = before.spaces.find((s) => s.id === 'searches')!
    expect(searches.enabled).toBe(true)
    expect(searches.memory.entries).toBeGreaterThan(0)

    await page.goto('/admin/cache')
    await expect(page.getByRole('heading', { level: 1, name: /^cache$/i })).toBeVisible()
    const region = page.getByRole('region', { name: /^(spaces|espaces)$/i })
    await expect(region).toBeVisible()
    if (before.server.attached) {
      await expect(page.getByText(/both tiers answer|les deux niveaux répondent/i)).toBeVisible()
    } else {
      await expect(page.getByText(/memory alone|mémoire seule/i).first()).toBeVisible()
    }

    // Emptied from the page: the space's entries go, and the journal says who.
    const row = region.getByRole('listitem').filter({ hasText: /searches and pages|recherches et pages/i })
    await row.getByRole('button', { name: /^(flush|vider)\b/i }).click()
    await expect(page.getByRole('status').filter({ hasText: /flushed|vidé/i })).toBeVisible()
    const emptied = (await (await page.request.get('/api/v1/admin/cache')).json()) as Report
    expect(emptied.spaces.find((s) => s.id === 'searches')!.memory.entries).toBe(0)
    const journal = (await (await page.request.get('/api/v1/audit?limit=3')).json()) as {
      entries: { action: string; target: string | null }[]
    }
    expect(journal.entries.some((e) => e.action === 'cache.flushed' && e.target === 'searches')).toBe(true)

    // Switched off from the page: a read is not kept; switched on again: it
    // is. The switch is put back whatever happens, or every later run would
    // find the searches uncached.
    const searchesOn = async () =>
      ((await (await page.request.get('/api/v1/admin/cache')).json()) as Report).spaces.find((s) => s.id === 'searches')!
    try {
      await row.getByRole('switch').click()
      await expect.poll(async () => (await searchesOn()).enabled).toBe(false)
      await page.request.get('/api/v1/items?kind=movie&limit=2')
      expect((await searchesOn()).memory.entries).toBe(0)
      // The other spaces were not emptied by the switch: a setting of its own.
      const items = (await (await page.request.get('/api/v1/admin/cache')).json()) as Report
      expect(items.spaces.find((s) => s.id === 'sessions')!.memory.entries).toBeGreaterThan(0)
    } finally {
      await page.request.put('/api/v1/settings/server/-', { data: { key: 'cache.searches', value: 'true' } })
    }
    await expect.poll(async () => (await searchesOn()).enabled).toBe(true)
    await page.request.get('/api/v1/items?kind=movie&limit=2')
    expect((await searchesOn()).memory.entries).toBeGreaterThan(0)
  })

  test('everything is emptied after a word of warning, on the server too', async ({ page }) => {
    await signIn(page)
    await page.request.get('/api/v1/items?kind=series&limit=2')
    const before = (await (await page.request.get('/api/v1/admin/cache')).json()) as Report
    expect(before.spaces.find((s) => s.id === 'searches')!.memory.entries).toBeGreaterThan(0)
    if (before.server.attached) {
      // The server holds the page too, under the searches' prefix.
      expect(before.server.up).toBe(true)
      await expect.poll(async () => ((await (await page.request.get('/api/v1/admin/cache')).json()) as Report).spaces.find((s) => s.id === 'searches')!.server?.keys ?? 0).toBeGreaterThan(0)
    }

    await page.goto('/admin/cache')
    await page.getByRole('button', { name: /flush everything|tout vider/i }).click()
    const dialog = page.getByRole('dialog')
    await expect(dialog).toBeVisible()
    await dialog.getByRole('button', { name: /flush the cache|vider le cache/i }).click()
    await expect(dialog).toBeHidden()
    await expect(page.getByRole('status').filter({ hasText: /everything forgotten|tout est oublié/i })).toBeVisible()
    // Every space's keys are gone; the prefix keeps the generation and the epoch.
    const after = (await (await page.request.get('/api/v1/admin/cache')).json()) as Report
    expect(after.spaces.every((s) => (s.server?.keys ?? 0) === 0)).toBe(true)
    if (before.server.attached) expect(after.server.keys ?? 0).toBeLessThanOrEqual(2)
    // The request that reads the report signs in, and that session is remembered again.
    expect(after.spaces.filter((s) => s.id !== 'sessions').every((s) => s.memory.entries === 0)).toBe(true)
  })

  test('a public page is kept by a proxy, a private one is not, and a stranger reads nothing of the cache', async ({ page }) => {
    await signIn(page)
    const stranger = await page.context().browser()!.newContext()
    const open = await stranger.request.get('/api/v1/items?kind=series&limit=1')
    if (open.status() === 200) {
      // Public browsing on: a visitor's page carries the headers, with Vary on the credentials.
      expect(open.headers()['cache-control']).toMatch(/public, max-age=\d+, stale-while-revalidate=\d+/)
      expect(open.headers()['vary']).toContain('Cookie')
    }
    const mine = await page.request.get('/api/v1/items?kind=series&limit=1')
    expect(mine.headers()['cache-control']).toBe('private, no-cache')

    expect((await stranger.request.get('/api/v1/admin/cache')).status()).toBe(401)
    expect((await stranger.request.post('/api/v1/admin/cache/flush')).status()).toBe(401)
    await stranger.close()
  })
})
