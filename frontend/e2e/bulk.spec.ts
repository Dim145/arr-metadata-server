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

  /**
   * Two works made by hand for the purpose, one per project, found by their
   * name and selected whole; removed at the end.
   */
  async function twoWorks(page: Page, project: string) {
    const stamp = `E2E masse ${project} ${Date.now()}`
    const ids: string[] = []
    for (const n of [1, 2]) {
      const created = await page.request.post('/api/v1/items', {
        data: { kind: 'series', title: `${stamp} ${n}`, genres: ['Drama'] },
      })
      expect(created.status()).toBe(201)
      ids.push(((await created.json()) as { id: string }).id)
    }
    await page.goto('/admin/catalogue')
    await page.getByLabel(/search titles|chercher un titre/i).fill(stamp)
    await expect(page.locator('tr[data-work]')).toHaveCount(2)
    return ids
  }

  const bar = (page: Page) => page.getByRole('region', { name: /(selected|sélectionnée)/i })
  const selectBoth = (page: Page) => page.getByRole('checkbox', { name: /select every|sélectionner toutes/i }).check()

  test('a field is given to every work of the selection, each its own lock, and the lock is lifted again', async ({
    page,
  }, info) => {
    test.slow()
    await signIn(page)
    const ids = await twoWorks(page, info.project.name)
    const work = async (id: string) =>
      (await (await page.request.get(`/api/v1/items/${id}`)).json()) as { status?: string; genres: string[] }
    const locked = async (id: string, field: string) =>
      ((await (await page.request.get(`/api/v1/items/${id}/overrides`)).json()) as { scope: string; field: string }[]).some(
        (lock) => lock.scope === 'item' && lock.field === field,
      )

    // The field, what to do with it, and a value: said before it is done.
    const change = async (field: string, action: RegExp, value: string | undefined, said: RegExp) => {
      await selectBoth(page)
      await bar(page).getByRole('button', { name: /change a field|modifier un champ/i }).click()
      const dialog = page.getByRole('dialog')
      await dialog.getByLabel(/^(field|champ)$/i).selectOption(field)
      // The radio sits under its label, which is what a finger presses.
      await dialog.getByText(action).click()
      if (value !== undefined) await dialog.getByLabel(/^(value|valeur|genre)$/i).fill(value)
      await dialog.getByRole('button', { name: /^(apply to|appliquer à) 2 /i }).click()
      await expect(dialog).toBeHidden()
      await expect(bar(page).getByRole('status')).toHaveText(said)
    }

    try {
      await change('status', /^(set to|fixer à)$/i, 'ended', /^(done for 2|fait pour 2)\.$/i)
      for (const id of ids) {
        expect((await work(id)).status).toBe('ended')
        expect(await locked(id, 'status')).toBe(true)
      }

      // A genre added is added once: asked again, both already have it.
      await change('genres', /^(add a genre|ajouter un genre)$/i, 'Mystery', /^(done for 2|fait pour 2)\.$/i)
      await change('genres', /^(add a genre|ajouter un genre)$/i, 'mystery', /(2 already so|2 déjà à jour)/i)
      await change('genres', /^(take a genre off|retirer un genre)$/i, 'Drama', /^(done for 2|fait pour 2)\.$/i)
      for (const id of ids) expect((await work(id)).genres).toEqual(['Mystery'])

      // Handed back: the lock is gone from each.
      await change('status', /^(lift the lock|lever le verrou)$/i, undefined, /^(done for 2|fait pour 2)\.$/i)
      for (const id of ids) expect(await locked(id, 'status')).toBe(false)
    } finally {
      for (const id of ids) await page.request.delete(`/api/v1/items/${id}`)
    }
  })

  test('asking the sources again passes over a work that has none', async ({ page }, info) => {
    await signIn(page)
    const ids = await twoWorks(page, info.project.name)

    try {
      await selectBoth(page)
      await bar(page).getByRole('button', { name: /ask the sources again|réinterroger les sources/i }).click()
      const dialog = page.getByRole('dialog')
      const source = dialog.getByRole('checkbox').first()
      await expect(source.or(dialog.getByText(/no source is switched on|aucune source n’est activée/i))).toBeVisible()
      test.skip((await dialog.getByRole('checkbox').count()) === 0, 'no source is switched on')

      await source.check()
      // Entered by hand, neither has a source to be asked: nothing is sent.
      let asked = false
      page.on('request', (request) => {
        if (/\/api\/v1\/items\/[^/]+\/(sync|provenance)$/.test(request.url())) asked = true
      })
      await dialog.getByRole('button', { name: /^(ask|interroger) 1 source/i }).click()
      await expect(bar(page).getByRole('status')).toHaveText(/(2 passed over|2 laissées de côté)/i)
      expect(asked).toBe(false)
    } finally {
      for (const id of ids) await page.request.delete(`/api/v1/items/${id}`)
    }
  })
})
