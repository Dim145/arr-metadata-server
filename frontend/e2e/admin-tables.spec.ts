import { expect, test, type Page } from '@playwright/test'
import { escaped, pattern } from './support/regex'

/**
 * The administration's dense tables, as the catalogue has them: ordered by a
 * column's head and said so (`aria-sort`), columns hidden and shown again
 * and kept with the account across a reload, the rows walked with the arrow
 * keys and opened with Enter, cards under 600 pixels of table — and the
 * catalogue's own views, kept with the account, and its filter of the works
 * whose media failed, with how many there are.
 *
 * What is kept with the account is one admin's for both projects, which run
 * at once: what writes it runs on the desk alone, and puts it back.
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

const columnsMenu = (page: Page) => page.getByRole('button', { name: /^(columns and density|colonnes et densité)$/i })
const columnsPanel = (page: Page) => page.getByRole('dialog', { name: /^(columns and density|colonnes et densité)$/i })

test.describe('the catalogue’s table', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('is ordered by a column’s head, which says how', async ({ page, isMobile }) => {
    test.skip(isMobile, 'as cards, the heads are read rather than pressed; the order is the select’s')
    await signIn(page)
    await page.goto('/admin/catalogue')
    const head = page.locator('th[data-column="title"]')
    await expect(page.locator('tr[data-work]').first()).toBeVisible()
    await expect(head).not.toHaveAttribute('aria-sort', /.+/)

    await head.getByRole('button').click()
    await expect(page).toHaveURL(/sort=title/)
    await expect(head).toHaveAttribute('aria-sort', 'ascending')
    await expect(page.locator('#works-sort')).toHaveValue('title')
    // The focus stays on the head pressed: the rows are not drawn anew.
    await expect(head.getByRole('button')).toBeFocused()

    await head.getByRole('button').click()
    await expect(page).toHaveURL(/sort=title&order=desc/)
    await expect(head).toHaveAttribute('aria-sort', 'descending')
    await expect(page.locator('#works-sort')).toHaveValue('title-desc')
  })

  test('hides a column, and keeps it hidden after a reload, with the account', async ({ page, isMobile }) => {
    test.skip(isMobile, 'the layout is the account’s: the desk alone writes it')
    await signIn(page)
    await page.request.delete('/api/v1/account/preferences/table.catalogue')

    try {
      await page.goto('/admin/catalogue')
      await expect(page.locator('th[data-column="state"]')).toBeVisible()
      await columnsMenu(page).click()
      await columnsPanel(page).getByRole('checkbox', { name: /^(state|état)$/i }).uncheck()
      await columnsPanel(page).getByRole('checkbox', { name: /^(added|ajoutée)$/i }).check()
      await page.keyboard.press('Escape')
      await expect(page.locator('th[data-column="state"]')).toHaveCount(0)
      await expect(page.locator('tr[data-work] td[data-column="state"]')).toHaveCount(0)
      await expect(page.locator('th[data-column="added"]')).toBeVisible()

      // Kept with the account, a moment after the change.
      await expect
        .poll(async () => ((await (await page.request.get('/api/v1/account/preferences')).json()) as { values: Record<string, unknown> }).values['table.catalogue'])
        .toEqual({ hidden: ['state'], shown: ['added'] })
      await page.reload()
      await expect(page.locator('tr[data-work]').first()).toBeVisible()
      await expect(page.locator('th[data-column="state"]')).toHaveCount(0)
      await expect(page.locator('th[data-column="added"]')).toBeVisible()

      // The table's own way again, from the same menu.
      await columnsMenu(page).click()
      await columnsPanel(page).getByRole('button', { name: /back to how the table was|revenir à la table/i }).click()
      await expect(page.locator('th[data-column="state"]')).toBeVisible()
      await expect(page.locator('th[data-column="added"]')).toHaveCount(0)
    } finally {
      await page.request.delete('/api/v1/account/preferences/table.catalogue')
    }
  })

  test('is walked a row at a time with the arrow keys, and Enter opens the row', async ({ page, isMobile }) => {
    test.skip(isMobile, 'the arrow keys are a keyboard’s')
    await signIn(page)
    await page.goto('/admin/catalogue')
    const rows = page.locator('tbody tr[data-row]')
    await expect(rows.nth(2)).toBeVisible()

    // One row takes the Tab key, the first; the arrows move it.
    await expect(page.locator('tbody tr[data-row][tabindex="0"]')).toHaveCount(1)
    await rows.first().focus()
    await page.keyboard.press('ArrowDown')
    await page.keyboard.press('ArrowDown')
    await expect(rows.nth(2)).toBeFocused()
    await expect(rows.nth(2)).toHaveAttribute('tabindex', '0')
    await page.keyboard.press('ArrowUp')
    await expect(rows.nth(1)).toBeFocused()

    const id = await rows.nth(1).getAttribute('data-row')
    await page.keyboard.press('Enter')
    await expect(page).toHaveURL(pattern(`/admin/catalogue/${escaped(id!)}$`))
  })

  test('is a column of cards on a narrow screen, the box that chooses every row still there', async ({ page, isMobile }) => {
    test.skip(!isMobile, 'cards are for a narrow table')
    await signIn(page)
    await page.goto('/admin/catalogue')
    const row = page.locator('tbody tr[data-row]').first()
    await expect(row).toBeVisible()
    expect(await row.evaluate((element) => getComputedStyle(element).display)).toBe('grid')
    // Each value after its column's name; the heads out of sight, not out of reach.
    await expect(row.locator('td[data-column="marks"]')).toBeVisible()
    await expect(page.getByRole('checkbox', { name: /select every|sélectionner toutes/i })).toBeVisible()
    expect(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth + 1)).toBe(false)
  })
})

test.describe('the catalogue’s views', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('are offered, and one is saved under a name, kept across a reload, and forgotten', async ({ page, isMobile }) => {
    test.skip(isMobile, 'the views are the account’s: the desk alone writes them')
    await signIn(page)
    const views = page.getByRole('group', { name: /^(views|vues)$/i })
    const name = `Vue e2e ${Date.now()}`

    try {
      await page.goto('/admin/catalogue')
      await expect(views.getByRole('button', { name: /^(everything|tout)$/i })).toHaveAttribute('aria-pressed', 'true')
      // A view offered is its address.
      await views.getByRole('button', { name: /^(films)$/i }).click()
      await expect(page).toHaveURL(/kind=movie/)
      await expect(views.getByRole('button', { name: /^(films)$/i })).toHaveAttribute('aria-pressed', 'true')

      // A view of one's own: the films, by title.
      await page.locator('#works-sort').selectOption('title')
      await expect(page).toHaveURL(/kind=movie&sort=title|sort=title&kind=movie/)
      await views.getByRole('button', { name: /^(save this view|enregistrer cette vue)/i }).click()
      const form = page.getByRole('dialog', { name: /^(save this view|enregistrer cette vue)$/i })
      await form.getByLabel(/^(its name|son nom)$/i).fill(name)
      await form.getByRole('button', { name: /^(save the view|enregistrer la vue)$/i }).click()
      const chip = views.getByRole('button', { name: pattern(`^${escaped(name)}$`) })
      await expect(chip).toHaveAttribute('aria-pressed', 'true')

      // Kept with the account: there after a reload, and it leads back.
      await page.goto('/admin/catalogue')
      await expect(chip).toHaveAttribute('aria-pressed', 'false')
      await chip.click()
      await expect(page).toHaveURL(/kind=movie/)
      await expect(page).toHaveURL(/sort=title/)
      await expect(chip).toHaveAttribute('aria-pressed', 'true')

      // Saved once: the same view again is said to be saved already.
      await views.getByRole('button', { name: /^(save this view|enregistrer cette vue)/i }).click()
      await expect(form.getByText(pattern(escaped(name)))).toBeVisible()
      await page.keyboard.press('Escape')

      await views.getByRole('button', { name: pattern(`^(forget the view|oublier la vue) [“«]\\s?${escaped(name)}`, 'i') }).click()
      await expect(chip).toHaveCount(0)
    } finally {
      const kept = ((await (await page.request.get('/api/v1/account/preferences')).json()) as { values: Record<string, unknown> })
        .values['catalogue.views'] as { name: string; query: string }[] | undefined
      const others = (kept ?? []).filter((view) => view.name !== name)
      if (others.length) await page.request.put('/api/v1/account/preferences/catalogue.views', { data: { value: others } })
      else await page.request.delete('/api/v1/account/preferences/catalogue.views')
    }
  })
})

test.describe('the catalogue’s failed media', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('are a filter of their own, with how many works it leaves', async ({ page }) => {
    await signIn(page)
    const gaps = (await (await page.request.get('/api/v1/admin/gaps')).json()) as Record<string, number>
    expect(Object.keys(gaps).sort()).toEqual(['divergingLock', 'duplicates', 'imdb', 'mediaFailed', 'overview', 'poster', 'refresh'])
    const listed = (await (await page.request.get('/api/v1/items?missing=mediaFailed&includeDisabled=true&limit=1')).json()) as {
      total: number
    }
    // The figure beside the filter is what the filter lists.
    expect(listed.total).toBe(gaps.mediaFailed)

    await page.goto('/admin/catalogue')
    const filter = page
      .getByRole('group', { name: /what is missing|ce qui manque/i })
      .getByRole('button', { name: /media failed|médias en échec/i })
    await expect(filter).toContainText(String(gaps.mediaFailed))
    await filter.click()
    await expect(page).toHaveURL(/missing=mediaFailed/)
    await expect(filter).toHaveAttribute('aria-pressed', 'true')
    if (gaps.mediaFailed) await expect(page.locator('tr[data-work]')).toHaveCount(Math.min(gaps.mediaFailed, 60))
    else await expect(page.getByText(/nothing lacks it|rien ne manque/i)).toBeVisible()
  })
})
