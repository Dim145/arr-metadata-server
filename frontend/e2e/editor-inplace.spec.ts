import { expect, test, type Page } from '@playwright/test'
import { escaped, pattern } from './support/regex'

/**
 * The work editor, edited in place: a field is changed where it is shown,
 * kept as a change not saved yet — the field and its tab marked — and saved
 * from the bar along the foot of the page, or the next work opened with it.
 * Leaving with a change unsaved asks first. The public page can be kept in
 * view beside the editor, and shows what was saved.
 *
 * Every work here is made by hand for the test, one per project — the two
 * projects run at once against one server — and removed at the end.
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

/** A film made by hand, named after the test and the project, and its id. */
async function aFilm(page: Page, title: string): Promise<string> {
  const created = await page.request.post('/api/v1/items', { data: { kind: 'movie', title, year: 2036 } })
  expect(created.status()).toBe(201)
  return ((await created.json()) as { id: string }).id
}

const titleOf = async (page: Page, id: string) =>
  ((await (await page.request.get(`/api/v1/items/${id}`)).json()) as { title: string }).title

const saveButton = (page: Page) => page.getByRole('button', { name: /^(save and stay|enregistrer et rester)$/i })
const unsavedBar = (page: Page) => page.getByRole('region', { name: /not saved yet|pas encore enregistrée/i })

test.describe('the editor, edited in place', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('a typo in a title is put right where it is shown, without a dialog', async ({ page }, info) => {
    await signIn(page)
    const stamp = `${info.project.name} ${Date.now()}`
    const id = await aFilm(page, `Typo e2e ${stamp} Smiht`)
    let dialogs = 0
    page.on('dialog', () => (dialogs += 1))

    try {
      await page.goto(`/admin/catalogue/${id}`)
      const row = page.locator('li[data-field="title"]')
      // The value is the way in, named for what pressing it does.
      await row.getByRole('button', { name: /^(edit|modifier)\b/i }).click()
      const box = row.getByRole('textbox')
      await expect(box).toBeFocused()
      await expect(box).toHaveValue(`Typo e2e ${stamp} Smiht`)
      await box.fill(`Typo e2e ${stamp} Smith`)
      await box.press('Enter')

      // Kept for saving: the box closed, the field and its tab marked, the bar up.
      await expect(box).toHaveCount(0)
      await expect(row).toHaveAttribute('data-changed', '')
      await expect(row).toContainText(`Typo e2e ${stamp} Smith`)
      await expect(row.getByText(/^(changed, not saved yet|modifié, pas encore enregistré)$/i)).toBeAttached()
      await expect(page.getByRole('tab', { name: /changes not saved yet|modifications pas encore enregistrées/i })).toHaveAttribute(
        'aria-selected',
        'true',
      )
      await expect(unsavedBar(page)).toBeVisible()
      // Nothing is sent until it is saved.
      expect(await titleOf(page, id)).toBe(`Typo e2e ${stamp} Smiht`)

      await saveButton(page).click()
      await expect(page.getByText(/^(the change is saved\.|la modification est enregistrée\.)$/i)).toBeVisible()
      await expect.poll(() => titleOf(page, id)).toBe(`Typo e2e ${stamp} Smith`)
      await expect(row).not.toHaveAttribute('data-changed', '')
      await expect(page.getByRole('dialog')).toHaveCount(0)
      expect(dialogs).toBe(0)

      // Escape puts a field back as it was, and nothing is left to save.
      await row.getByRole('button', { name: /^(edit|modifier)\b/i }).click()
      await row.getByRole('textbox').fill('Not this')
      await row.getByRole('textbox').press('Escape')
      await expect(row.getByRole('textbox')).toHaveCount(0)
      await expect(row).toContainText(`Typo e2e ${stamp} Smith`)
      await expect(saveButton(page)).toHaveCount(0)
    } finally {
      await page.request.delete(`/api/v1/items/${id}`)
    }
  })

  test('leaving with a change unsaved asks first, and the browser asks before the tab closes', async ({ page }, info) => {
    await signIn(page)
    const stamp = `${info.project.name} ${Date.now()}`
    const id = await aFilm(page, `Partir e2e ${stamp}`)

    try {
      await page.goto(`/admin/catalogue/${id}`)
      const row = page.locator('li[data-field="sortTitle"]')
      await row.getByRole('button', { name: /^(edit|modifier)\b/i }).click()
      await row.getByRole('textbox').fill(`Zzz partir ${stamp}`)
      await row.getByRole('textbox').press('Enter')
      await expect(unsavedBar(page)).toBeVisible()

      // The way back to the catalogue: the sidebar's on a desk, the bar's on
      // a phone — one of the two is shown.
      const away = page.getByRole('navigation', { name: /^sections$/i }).getByRole('link', { name: /^catalogue$/i })
      await away.click()
      const asking = page.getByRole('dialog').filter({ hasText: /leave without saving|partir sans enregistrer/i })
      await expect(asking).toBeVisible()
      await asking.getByRole('button', { name: /^(stay here|rester ici)$/i }).click()
      await expect(asking).toBeHidden()
      await expect(page).toHaveURL(pattern(`/admin/catalogue/${escaped(id)}`))
      await expect(row).toHaveAttribute('data-changed', '')

      // Leaving without saving: the change is gone, and nothing was sent.
      await away.click()
      await asking.getByRole('button', { name: /^(leave without saving|partir sans enregistrer)$/i }).click()
      await expect(page).toHaveURL(/\/admin\/catalogue$/)
      const sortTitle = ((await (await page.request.get(`/api/v1/items/${id}`)).json()) as { sortTitle?: string }).sortTitle
      expect(sortTitle ?? '').not.toContain('Zzz partir')

      // Closing the tab with a change unsaved is asked about by the browser.
      await page.goto(`/admin/catalogue/${id}`)
      await row.getByRole('button', { name: /^(edit|modifier)\b/i }).click()
      await row.getByRole('textbox').fill(`Zzz encore ${stamp}`)
      await row.getByRole('textbox').press('Enter')
      await expect(unsavedBar(page)).toBeVisible()
      const asked = page.waitForEvent('dialog')
      await page.close({ runBeforeUnload: true })
      const dialog = await asked
      expect(dialog.type()).toBe('beforeunload')
      await dialog.accept()
    } finally {
      await page.request.delete(`/api/v1/items/${id}`)
    }
  })

  test('saves, then opens the next work of the catalogue’s list', async ({ page }, info) => {
    await signIn(page)
    const stamp = `Suite e2e ${info.project.name} ${Date.now()}`
    const first = await aFilm(page, `${stamp} A`)
    const second = await aFilm(page, `${stamp} B`)

    try {
      // The list the editor walks is the one the catalogue showed.
      await page.goto('/admin/catalogue?sort=title')
      await page.getByLabel(/search titles|chercher un titre/i).fill(stamp)
      await expect(page.locator('tr[data-work]')).toHaveCount(2)
      await page.getByRole('link', { name: pattern(`^(open|ouvrir) ${escaped(`${stamp} A`)}\\b`, 'i') }).click()
      await expect(page.getByRole('heading', { level: 1 })).toContainText(`${stamp} A`)

      const row = page.locator('li[data-field="overview"]')
      await row.getByRole('button', { name: /^(edit|modifier)\b/i }).click()
      await row.getByRole('textbox').fill('Écrit en place.')
      await page.getByRole('button', { name: /^(more ways to save|d’autres façons d’enregistrer)$/i }).click()
      await page.getByRole('button', { name: pattern(`(save and open|enregistrer et ouvrir) [“«]\\s?${escaped(`${stamp} B`)}`, 'i') }).click()

      await expect(page).toHaveURL(pattern(`/admin/catalogue/${escaped(second)}$`))
      await expect(page.getByRole('heading', { level: 1 })).toContainText(`${stamp} B`)
      const overview = ((await (await page.request.get(`/api/v1/items/${first}`)).json()) as { overview?: string }).overview
      expect(overview).toBe('Écrit en place.')
    } finally {
      await page.request.delete(`/api/v1/items/${first}`)
      await page.request.delete(`/api/v1/items/${second}`)
    }
  })

  test('the preview shows the public page as saved, and again after the next save', async ({ page, isMobile }, info) => {
    await signIn(page)
    const stamp = `Aperçu e2e ${info.project.name} ${Date.now()}`
    const id = await aFilm(page, stamp)

    try {
      await page.goto(`/admin/catalogue/${id}`)
      await page.getByRole('button', { name: /^(preview|aperçu)$/i }).click()
      await expect(page.getByRole('button', { name: /^(preview|aperçu)$/i })).toHaveAttribute('aria-pressed', 'true')
      // Beside the editor on a wide screen, over it on a phone: the same page.
      const frame = page.frameLocator(`iframe[title*="${stamp}"]`)
      await expect(frame.getByRole('heading', { level: 1 })).toContainText(stamp)
      if (isMobile) await expect(page.getByRole('dialog', { name: /the public page|la page publique/i })).toBeVisible()

      // Saved, and the preview shows it: on a phone, from the sheet closed and opened again.
      if (isMobile) await page.getByRole('button', { name: /^(close the preview|fermer l’aperçu)$/i }).click()
      const row = page.locator('li[data-field="title"]')
      await row.getByRole('button', { name: /^(edit|modifier)\b/i }).click()
      await row.getByRole('textbox').fill(`${stamp} vu`)
      await row.getByRole('textbox').press('Enter')
      await saveButton(page).click()
      await expect.poll(() => titleOf(page, id)).toBe(`${stamp} vu`)
      if (isMobile) await page.getByRole('button', { name: /^(preview|aperçu)$/i }).click()
      await expect(page.frameLocator(`iframe[title*="${stamp}"]`).getByRole('heading', { level: 1 })).toContainText(`${stamp} vu`)
    } finally {
      await page.request.delete(`/api/v1/items/${id}`)
      // The preview beside the editor is remembered with the account: the
      // other tests open the editor without it.
      await page.request.delete('/api/v1/account/preferences/editor.preview')
    }
  })
})
