import { expect, test, type Page } from '@playwright/test'
import { escaped, pattern } from './support/regex'

/**
 * Two records of one work: proposed as a pair, set side by side, merged, the
 * merged one's address answered with the kept one, and the merge undone from
 * the kept one's editor.
 *
 * The two are entered by hand for the test, under a title no work of the
 * suite's catalogue carries, and deleted at the end whatever became of them:
 * the catalogue is the suite's, not this test's to fold. Server-wide, since
 * the register and the dashboard count them while they are there.
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

type Entry = { id: string; slug: string; title: string }

/** A film entered by hand, as the editor's form enters one. */
async function entered(page: Page, title: string, overview: string): Promise<Entry> {
  const answer = await page.request.post('/api/v1/items', {
    data: { kind: 'movie', title, year: 1959, overview },
  })
  expect(answer.status()).toBe(201)
  return (await answer.json()) as Entry
}

test.describe('duplicates', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
  })

  test('a pair proposed is merged, answered under the merged one’s address, and taken apart again', async ({
    page,
  }) => {
    await signIn(page)
    // A title of its own, so that only these two look alike to it.
    const title = `Pickpocket ${Date.now().toString(36)}`
    const first = await entered(page, title, 'Entered first, by hand.')
    const second = await entered(page, title, 'Entered second, by hand.')

    try {
      // The dashboard leads to the register narrowed to the pairs.
      await page.goto('/admin')
      const errand = page
        .getByRole('region', { name: /^to do$/i })
        .getByRole('link', { name: /look like another one$/ })
      await expect(errand).toHaveAttribute('href', '/admin/catalogue?missing=duplicates')
      await errand.click()
      await expect(page).toHaveURL(/\/admin\/catalogue\?missing=duplicates$/)

      // Each of the two carries the other's title, and opens the two side by side.
      const marks = page.getByRole('button', { name: `Compare with “${title}”` })
      await expect(marks).toHaveCount(2)
      await marks.first().click()

      const dialog = page.getByRole('dialog')
      await expect(dialog.getByRole('heading', { name: 'Merge two records of one work' })).toBeVisible()
      await expect(dialog.getByText(/^Kept by the rule: /)).toBeVisible()
      await dialog.getByRole('button', { name: 'Keep this one instead' }).click()
      await expect(dialog.getByText('Kept by your choice: the rule would keep the other.')).toBeVisible()

      // The kept record takes the other's synopsis, and is locked on it.
      const synopsis = dialog.getByRole('group', { name: 'Synopsis' })
      await synopsis.getByRole('radio').nth(1).check()

      const asked = page.waitForResponse(
        (r) => r.url().endsWith('/merge') && r.request().method() === 'POST',
      )
      await dialog.getByRole('button', { name: 'Merge', exact: true }).click()
      const answer = await asked
      expect(answer.status()).toBe(200)
      const outcome = (await answer.json()) as { survivor: string; merged: string; undoUntil?: string }
      expect([first.id, second.id].sort()).toEqual([outcome.survivor, outcome.merged].sort())
      expect(outcome.undoUntil).toBeTruthy()
      await expect(dialog).toBeHidden()
      await expect(page.getByText(`“${title}” is merged into “${title}”.`, { exact: false })).toBeVisible()
      await expect(page.getByRole('button', { name: `Compare with “${title}”` })).toHaveCount(0)

      // The merged record's id and slug both answer with the kept one, whose
      // address takes the place of the old one.
      const merged = outcome.merged === first.id ? first : second
      for (const address of [merged.id, merged.slug]) {
        await page.goto(`/work/${address}`)
        await expect(page).toHaveURL(pattern(`/work/${escaped(outcome.survivor)}$`))
        await expect(page.getByRole('heading', { level: 1, name: pattern(escaped(title)) })).toBeVisible()
      }
      const read = await page.request.get(`/api/v1/items/${merged.id}`)
      expect(((await read.json()) as Entry).id).toBe(outcome.survivor)

      // The kept one's editor says what was merged into it, and undoes it.
      await page.goto(`/admin/catalogue/${merged.id}`)
      await expect(page).toHaveURL(pattern(`/admin/catalogue/${escaped(outcome.survivor)}$`))
      await expect(page.getByText(`“${title}” was merged into this work`, { exact: false })).toBeVisible()
      await page.getByRole('button', { name: 'Undo the merge' }).click()
      await expect(page.getByRole('status').getByText(`“${title}” is a work of its own again.`)).toBeVisible()

      const back = await page.request.get(`/api/v1/items/${merged.id}`)
      expect(((await back.json()) as Entry).id).toBe(merged.id)

      // Taken apart, the two are not proposed as one work again.
      await page.goto('/admin/catalogue?missing=duplicates')
      await expect(page.getByRole('heading', { level: 1, name: 'Catalogue' })).toBeVisible()
      await expect(page.getByRole('button', { name: `Compare with “${title}”` })).toHaveCount(0)
    } finally {
      for (const entry of [first, second]) {
        await page.request.delete(`/api/v1/items/${entry.id}`)
      }
    }
  })
})
