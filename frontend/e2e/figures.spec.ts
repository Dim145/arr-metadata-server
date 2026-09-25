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
})
