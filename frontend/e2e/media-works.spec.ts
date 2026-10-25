import { expect, test, type Page } from '@playwright/test'

/**
 * The media by work: the media page ranks the works by what their media
 * weigh — or keep, or failed — and the dashboard names the heaviest. Read
 * only; skipped without a store, as there is then nothing kept to rank.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

async function signIn(page: Page) {
  await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

interface Ranked {
  works: { id: string; title: string; stored: number; bytes: number; copiesBytes: number }[]
  total: number
}

test.describe('the media by work', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('the works are ranked by what their media weigh, and the heaviest are on the dashboard', async ({ page, isMobile }) => {
    await signIn(page)
    const status = (await (await page.request.get('/api/v1/media/status')).json()) as {
      backend: string
      counts: { failed: number }
    }
    test.skip(status.backend === 'off', 'no media store')

    const ranked = (await (await page.request.get('/api/v1/media/works?limit=5')).json()) as Ranked
    test.skip(ranked.works.length === 0, 'nothing kept yet')
    // The heaviest first.
    const weights = ranked.works.map((work) => work.bytes + work.copiesBytes)
    expect(weights).toEqual([...weights].sort((a, b) => b - a))

    await page.goto('/admin/media')
    const panel = page.getByRole('region', { name: /^by work$/i })
    await expect(panel.getByRole('link', { name: ranked.works[0]!.title }).first()).toBeVisible()

    // Another order is asked of the server: by the switch, on any screen.
    const asked = page.waitForRequest((request) => request.url().includes('/api/v1/media/works') && request.url().includes('sort=count'))
    await panel.getByText(/^the most kept$/i).click()
    await asked
    const table = panel.getByRole('table', { name: /^by work$/i })
    await expect(table.getByRole('columnheader', { name: /^kept/i })).toHaveAttribute('aria-sort', 'descending')

    if (!isMobile) {
      // And by a column's head where the table is wide enough to show them.
      const failed = page.waitForRequest((request) => request.url().includes('/api/v1/media/works') && request.url().includes('sort=failed'))
      await table.getByRole('button', { name: /^given up on/i }).click()
      await failed
      await expect(panel.getByRole('radio', { name: /^the most given up on$/i })).toBeChecked()

      // Enter on a row opens the work it is.
      const row = table.locator('tbody tr[data-work]').first()
      const id = await row.getAttribute('data-work')
      await row.focus()
      await page.keyboard.press('Enter')
      await page.waitForURL(`**/admin/catalogue/${id}`)
    }

    // The dashboard names the heaviest, each the way into its editor.
    await page.goto('/admin')
    const tile = page.locator('#media-tile')
    await expect(tile.getByText(/^the heaviest works$/i)).toBeVisible()
    await expect(tile.getByRole('link', { name: ranked.works[0]!.title }).first()).toHaveAttribute(
      'href',
      `/admin/catalogue/${ranked.works[0]!.id}`,
    )
    // The pictures given up on, when there are any, lead to the works they were for.
    const failedWorks = tile.getByRole('link', { name: /^the works they were for/i })
    if (status.counts.failed > 0) await expect(failedWorks).toHaveAttribute('href', '/admin/catalogue?missing=mediaFailed')
    else await expect(failedWorks).toHaveCount(0)

    // Not for a stranger.
    const stranger = await page.context().browser()!.newContext()
    expect((await stranger.request.get('/api/v1/media/works')).status()).toBe(401)
    await stranger.close()
  })
})
