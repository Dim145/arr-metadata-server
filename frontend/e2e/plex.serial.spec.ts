import { expect, test, type Page } from '@playwright/test'

/**
 * Plex's provider switched off and on again from its panel: off, Plex is
 * answered 503 — what it takes for "try again later" — and on, the manifest
 * again. Serial: the switch changes how the whole server answers. It is put
 * back on whatever happens.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

test.describe.configure({ mode: 'serial' })

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

test.describe('the Plex switch', () => {
  test.skip(!USERNAME || !PASSWORD, 'set AMS_E2E_USER and AMS_E2E_PASSWORD to exercise the administration side')

  test.afterEach(async ({ page }) => {
    await page.request.put('/api/v1/settings/server/-', { data: { key: 'api.plex', value: 'true' } })
  })

  test('off answers Plex 503, on answers it again', async ({ page, request }) => {
    await signIn(page)
    await page.goto('/admin/clients')
    const toggle = page.getByRole('region', { name: /^plex$/i }).getByRole('switch', { name: /answer plex|répondre à plex/i })
    await expect(toggle).toHaveAttribute('aria-checked', 'true')

    await toggle.click()
    await expect(toggle).toHaveAttribute('aria-checked', 'false')
    expect((await request.get('/plex')).status()).toBe(503)

    await toggle.click()
    await expect(toggle).toHaveAttribute('aria-checked', 'true')
    expect((await request.get('/plex')).status()).toBe(200)
  })
})
