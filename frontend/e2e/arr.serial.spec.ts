import { expect, test, type Page } from '@playwright/test'

import { startSonarr, type ArrStub } from './support/arr-stub'

/**
 * The Sonarr & Radarr page, against the small Sonarr in `support/`: a
 * connection tested and saved, the library read by the seed, and a lock put
 * on a series told to it as one command. It changes server-wide settings, so
 * it runs in the `server-wide` project, and puts back what it changed — and
 * stands aside on a server where a real Sonarr is connected, whose key could
 * not be put back.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

test.describe.configure({ mode: 'serial' })

interface Configuration {
  sonarr: { url: string; keySet: boolean; ready: boolean }
  radarr: { url: string; keySet: boolean; ready: boolean }
  pushRefresh: boolean
  pushes: { title?: string; outcome: string; arrId?: number }[]
}

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

test.describe('the operator’s own Sonarr', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  let stub: ArrStub | undefined
  test.afterAll(async () => {
    await stub?.close()
  })

  test('is connected, seeded from and told of a lock', async ({ page }) => {
    test.setTimeout(120_000)
    await signIn(page)

    const before = (await (await page.request.get('/api/v1/admin/arr')).json()) as Configuration
    test.skip(before.sonarr.keySet || before.radarr.keySet, 'a real Sonarr or Radarr is connected here')

    // A series the catalogue holds by its TheTVDB id, which the stand-in holds too.
    const { items } = (await (await page.request.get('/api/v1/items?kind=series&limit=50')).json()) as {
      items: { id: string; title: string; externalIds?: { tvdb?: number } }[]
    }
    const series = items.find((item) => item.externalIds?.tvdb)
    test.skip(!series, 'no series with a TheTVDB id in the catalogue')
    const tvdbId = series!.externalIds!.tvdb!
    stub = await startSonarr([{ id: 7, title: series!.title, tvdbId }])

    try {
      await page.goto('/admin/arr')
      await expect(page.getByRole('heading', { level: 1, name: 'Sonarr & Radarr' })).toBeVisible()

      // The address and the key, tested before they are kept.
      await page.locator('#arr-sonarr-url').fill(stub.url)
      await page.locator('#arr-sonarr-key').fill(stub.key)
      await page.getByRole('button', { name: /^(test|tester)$/i }).first().click()
      await expect(page.getByRole('status').filter({ hasText: '4.0.15.2941' })).toBeVisible()
      await page.getByRole('button', { name: /save the connections|enregistrer les connexions/i }).click()
      await expect(page.getByRole('status').filter({ hasText: /saved|enregistré/i })).toBeVisible()

      // Kept, and never shown again.
      const saved = (await (await page.request.get('/api/v1/admin/arr')).json()) as Configuration
      expect(saved.sonarr).toMatchObject({ url: stub.url, keySet: true, ready: true })
      expect(JSON.stringify(saved)).not.toContain(stub.key)

      // Moved to another address, the key kept stays behind: the field asks
      // for it again, saying why, and nothing is tried without it — nor
      // saved, the server says as much.
      const elsewhere = new URL(stub.url)
      elsewhere.port = String(Number(elsewhere.port) + 1)
      await page.locator('#arr-sonarr-url').fill(elsewhere.origin)
      await expect(page.locator('#arr-sonarr-key')).toHaveAttribute('required', '')
      await expect(page.getByText(/not the address the key kept|pas l’adresse pour laquelle la clé gardée/i)).toBeVisible()
      await expect(page.getByRole('button', { name: /^(test|tester)$/i }).first()).toBeDisabled()
      const moved = await page.request.put('/api/v1/admin/arr', { data: { sonarrUrl: elsewhere.origin } })
      expect(moved.status()).toBe(400)
      // Back at its own address, the key kept is the one used.
      await page.locator('#arr-sonarr-url').fill(stub.url)
      await expect(page.locator('#arr-sonarr-key')).toHaveCount(0)
      await expect(page.getByRole('button', { name: /^(test|tester)$/i }).first()).toBeEnabled()

      // The library read: the one series is held here already.
      await page.getByRole('button', { name: /run now|lancer maintenant/i }).click()
      await expect(page.getByText(/1 already here|1 déjà là/)).toBeVisible({ timeout: 30_000 })

      // Telling on, then a lock: one RefreshSeries for Sonarr's own id.
      await page.getByRole('switch', { name: /tell them of a change|les prévenir d’un changement/i }).click()
      await expect
        .poll(async () => ((await (await page.request.get('/api/v1/admin/arr')).json()) as Configuration).pushRefresh)
        .toBe(true)
      const locked = await page.request.put(`/api/v1/items/${series!.id}/overrides`, {
        data: { field: 'title', value: `${series!.title} (e2e)` },
      })
      expect(locked.ok()).toBe(true)
      await expect
        .poll(() => stub!.commands.map((command) => [command.name, command.seriesIds]), { timeout: 45_000 })
        .toContainEqual(['RefreshSeries', [7]])

      await page.reload()
      await expect(page.getByRole('cell', { name: /sent|envoyé/i }).first()).toBeVisible({ timeout: 15_000 })
    } finally {
      await page.request.delete(`/api/v1/items/${series!.id}/overrides/item/title`)
      const put = await page.request.put('/api/v1/admin/arr', {
        data: { sonarrUrl: before.sonarr.url, sonarrKey: '', pushRefresh: before.pushRefresh },
      })
      expect(put.ok()).toBe(true)
    }
  })
})
