import { expect, test, type Page } from '@playwright/test'

/**
 * Bulk actions in the catalogue: a selection across the rows, and one thing
 * done to all of it — here, added to a list made for the purpose and taken
 * away again, so nothing another test reads is touched meanwhile.
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

test.describe('bulk actions', () => {
  test.skip(!USERNAME || !PASSWORD, 'set AMS_E2E_USER and AMS_E2E_PASSWORD to an administrator to run the bulk tests')

  test('a selection is added to a list all at once, and a deletion is asked about first', async ({ page }) => {
    await signIn(page)
    const name = `E2E bulk ${Date.now()}`
    const created = await page.request.post('/api/v1/lists', { data: { name, mode: 'manual', isPublic: false } })
    expect(created.status()).toBe(201)
    const list = (await created.json()) as { id: string; slug: string }
    try {
      await page.goto('/admin/catalogue')
      const boxes = page.getByRole('checkbox', { name: /^(select|sélectionner) (?!every|toutes)/i })
      await expect(boxes.first()).toBeVisible()
      test.skip((await boxes.count()) < 2, 'needs two works')

      await boxes.nth(0).check()
      await boxes.nth(1).check()
      const bar = page.getByRole('region', { name: /2 (selected|sélectionnées)/i })
      await expect(bar).toBeVisible()

      // Deleting is asked about, and the asking can be walked away from.
      await bar.getByRole('button', { name: /^(delete|supprimer)$/i }).click()
      const dialog = page.getByRole('dialog')
      await expect(dialog).toContainText(/2/)
      await dialog.getByRole('button', { name: /^(cancel|annuler)$/i }).click()
      await expect(dialog).toBeHidden()
      await expect(bar).toBeVisible()

      // Added to the list, all at once.
      await bar.getByRole('combobox', { name: /add to a list|ajouter à une liste/i }).selectOption(list.id)
      await expect(page.getByRole('status')).toHaveText(/2 added|2 ajoutées/i)
      const members = (await (await page.request.get(`/api/v1/lists/${list.id}`)).json()) as { total: number }
      expect(members.total).toBe(2)

      // Select all, then clear.
      await page.getByRole('checkbox', { name: /select every|sélectionner toutes/i }).check()
      await expect(page.getByRole('region', { name: /(selected|sélectionnées)/i })).toBeVisible()
      await page.getByRole('button', { name: /clear the selection|vider la sélection/i }).click()
      await expect(boxes.first()).not.toBeChecked()
    } finally {
      await page.request.delete(`/api/v1/lists/${list.id}`)
    }
  })
})
