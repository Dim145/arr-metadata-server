import { expect, test, type Page } from '@playwright/test'

/**
 * Plex's provider, as Plex reads its manifest and as the Keys & network
 * page shows it: the address to paste into Plex, the switch, and the way to
 * the list of allowed addresses that lets Plex in. The suite calls from
 * 127.0.0.1, which the default list allows. Switching the provider off
 * changes how the whole server answers, so that is plex.serial.spec.ts's.
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

test('the provider tells Plex what it serves, and a path it lacks is no page', async ({ request }) => {
  const manifest = await request.get('/plex')
  expect(manifest.status()).toBe(200)
  const provider = (await manifest.json()).MediaProvider
  expect(provider.identifier).toBe('tv.plex.agents.custom.arrmetadataserver')
  expect(provider.Types.map((t: { type: number }) => t.type)).toEqual([1, 2, 3, 4])
  expect(provider.Feature.map((f: { key: string }) => f.key)).toEqual([
    '/library/metadata',
    '/library/metadata/matches',
  ])

  // A key that names nothing is a 404 in JSON, not the interface's page.
  const missing = await request.get('/plex/library/metadata/movie-tmdb-0')
  expect(missing.status()).toBe(404)
  expect(missing.headers()['content-type']).toContain('application/json')

  // A match is read from JSON said so, and from nothing a web page may send
  // another site without asking first: text is refused before it is read.
  const asText = await request.post('/plex/library/metadata/matches', {
    headers: { 'content-type': 'text/plain' },
    data: JSON.stringify({ type: 1, guid: 'tmdb://603' }),
  })
  expect(asText.status()).toBe(415)
  expect(((await asText.json()) as { error: string }).error).toBe('unsupported_media_type')
})

test.describe('the Plex panel', () => {
  test.skip(!USERNAME || !PASSWORD, 'set AMS_E2E_USER and AMS_E2E_PASSWORD to exercise the administration side')

  test('gives the address to paste, its switch, and the way to the allowed addresses', async ({ page }) => {
    await signIn(page)
    await page.goto('/admin/clients')

    const panel = page.getByRole('region', { name: /^plex$/i })
    await expect(panel).toBeVisible()
    await expect(panel.getByLabel(/address to give plex|adresse à donner à plex/i)).toHaveValue(/\/plex$/)
    await expect(panel.getByRole('switch', { name: /answer plex|répondre à plex/i })).toBeVisible()
    await expect(panel).toContainText(/no credential|aucun identifiant/i)

    // The link leads to the field an address is typed in, below.
    await panel.getByRole('link', { name: /add it to the list|l’ajouter à la liste/i }).click()
    await expect(page.getByLabel(/address or block|adresse ou plage/i)).toBeFocused()
  })
})
