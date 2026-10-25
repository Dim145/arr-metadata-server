import { expect, test, type Page } from '@playwright/test'

/**
 * The palette in the administration: what can be done to the row the focus
 * is on, or to the work in the editor, each command with its keys beside it;
 * a choice to make — which list, which task — on a page of its own, and the
 * way back; a place reached by `g` and a letter, as the palette teaches.
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

const palette = (page: Page) => page.getByRole('combobox', { name: /command palette|palette de commandes|^(choose|choisir)/i })

/**
 * Open the palette with its keys — again while it is not open: a page still
 * being drawn may not hear the first press, and a second would close it.
 */
async function openPalette(page: Page) {
  await expect(async () => {
    if (!(await palette(page).isVisible())) await page.keyboard.press('ControlOrMeta+k')
    await expect(palette(page)).toBeVisible({ timeout: 1_000 })
  }).toPass()
}

test.describe('the administration’s palette', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('offers what can be done to the row the focus is on, with its keys, and a list to choose', async ({ page, isMobile }, info) => {
    test.skip(isMobile, 'a keyboard shortcut is a desktop thing')
    await signIn(page)
    const name = `Liste palette e2e ${info.project.name} ${Date.now()}`
    const created = await page.request.post('/api/v1/lists', { data: { name, mode: 'manual', isPublic: false } })
    expect(created.status()).toBe(201)
    const list = (await created.json()) as { id: string }

    try {
      await page.goto('/admin/catalogue')
      const row = page.locator('tbody tr[data-row]').first()
      await expect(row).toBeVisible()
      const title = (await row.locator('td[data-column="title"] a').textContent())?.trim() ?? ''
      await row.focus()
      await openPalette(page)
      await expect(palette(page)).toBeFocused()

      // The row's own commands first, its title said beside the first.
      const open = page.getByRole('option', { name: /open the editor|ouvrir l’éditeur/i })
      await expect(open).toContainText(title.slice(0, 12))
      // Each with the keys that do it without the palette.
      await expect(page.getByRole('option', { name: /^(this row|cette ligne)?\s*(refresh|rafraîchir)\b/i }).locator('kbd')).toHaveText(['r'])

      // A list to choose, on a page of its own; Backspace comes back.
      await palette(page).fill('list')
      await page.keyboard.press('Enter')
      await expect(page.getByRole('option', { name })).toBeVisible()
      await page.keyboard.press('Backspace')
      await expect(page.getByRole('option', { name })).toHaveCount(0)
      await expect(page.getByRole('option', { name: /open the editor|ouvrir l’éditeur/i })).toBeVisible()
      await page.keyboard.press('Escape')
      await expect(palette(page)).toBeHidden()
      // Nothing was added on the way.
      const members = (await (await page.request.get(`/api/v1/lists/${list.id}`)).json()) as { total: number }
      expect(members.total).toBe(0)
    } finally {
      await page.request.delete(`/api/v1/lists/${list.id}`)
    }
  })

  test('teaches the keys to each place, which then go there', async ({ page, isMobile }) => {
    test.skip(isMobile, 'a keyboard shortcut is a desktop thing')
    await signIn(page)
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    await openPalette(page)
    await palette(page).fill('catalogue')
    await expect(page.getByRole('option', { name: /^(go to|aller à)\s*catalogue/i })).toContainText(/g\s*(then|puis)\s*c/i)
    await page.keyboard.press('Escape')

    await page.keyboard.press('g')
    await page.keyboard.press('c')
    await expect(page).toHaveURL(/\/admin\/catalogue$/)
  })

  test('lists the tasks to run, and says which cannot be run now', async ({ page, isMobile }) => {
    test.skip(isMobile, 'a keyboard shortcut is a desktop thing')
    await signIn(page)
    const tasks = (await (await page.request.get('/api/v1/tasks')).json()) as { id: string; blocked?: string }[]
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    await openPalette(page)
    await palette(page).fill('task')
    await page.getByRole('option', { name: /run a task|lancer une tâche/i }).click()
    await expect(page.getByRole('option')).toHaveCount(tasks.length)
    const blocked = tasks.filter((task) => task.blocked).length
    await expect(page.locator('[role="option"][aria-disabled="true"]')).toHaveCount(blocked)
    await page.keyboard.press('Escape')
  })
})
