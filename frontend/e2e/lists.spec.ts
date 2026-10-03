import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { pattern } from './support/regex'

/**
 * Curated lists: composed by whoever maintains the catalogue, shown on the
 * site, and served to Sonarr and Radarr in the shapes their import lists
 * read. Each test makes its own list and takes it away again.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

interface Work {
  id: string
  title: string
  kind: 'series' | 'movie'
  externalIds: { tvdb?: number; tmdb?: number; imdb?: string }
}

interface List {
  id: string
  slug: string
  name: string
  mode: 'manual' | 'filter'
  isPublic: boolean
  itemCount: number
  filter?: { descending?: boolean; sort?: string }
}

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

/** A series with a TheTVDB id and a film with a TMDB id, for a list to hold. */
async function picks(request: APIRequestContext) {
  const series = ((await (await request.get('/api/v1/items?kind=series&limit=10')).json()).items as Work[]).find(
    (w) => w.externalIds.tvdb,
  )
  const movie = ((await (await request.get('/api/v1/items?kind=movie&limit=10')).json()).items as Work[]).find(
    (w) => w.externalIds.tmdb,
  )
  return { series, movie }
}

async function createList(request: APIRequestContext, body: Record<string, unknown>): Promise<List> {
  const response = await request.post('/api/v1/lists', { data: body })
  expect(response.status(), await response.text()).toBe(201)
  return (await response.json()) as List
}

/** Taken away without an assertion, so a failure keeps its own message. */
async function deleteList(request: APIRequestContext, id: string) {
  await request.delete(`/api/v1/lists/${id}`)
}

const escapeRegExp = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

test.describe('curated lists', () => {
  test.skip(
    !USERNAME || !PASSWORD,
    'set AMS_E2E_USER and AMS_E2E_PASSWORD to an administrator to run the list tests',
  )

  test('a list composed by hand is served in the shapes the clients import', async ({ page }) => {
    await signIn(page)
    const { series, movie } = await picks(page.request)
    test.skip(!series || !movie, 'needs a series with a TheTVDB id and a film with a TMDB id')
    const list = await createList(page.request, {
      name: `E2E picks ${Date.now()}`,
      kind: 'mixed',
      mode: 'manual',
      isPublic: true,
      items: [series!.id, movie!.id],
    })
    try {
      expect(list.itemCount).toBe(2)

      const sonarr = await (await page.request.get(`/api/v1/lists/${list.slug}/sonarr.json`)).json()
      expect(sonarr).toEqual([{ title: series!.title, tvdbId: series!.externalIds.tvdb }])
      const radarr = await (await page.request.get(`/api/v1/lists/${list.slug}/radarr.json`)).json()
      expect(radarr).toEqual([{ id: movie!.externalIds.tmdb, title: movie!.title }])
      const stevenlu = await (await page.request.get(`/api/v1/lists/${list.slug}/stevenlu.json`)).json()
      expect(stevenlu).toEqual(movie!.externalIds.imdb ? [{ title: movie!.title, imdb_id: movie!.externalIds.imdb }] : [])

      // The page holds them in the list's own order, and takes a new one.
      const detail = await (await page.request.get(`/api/v1/lists/${list.slug}`)).json()
      expect(detail.items.map((item: Work) => item.id)).toEqual([series!.id, movie!.id])
      const reordered = await page.request.put(`/api/v1/lists/${list.id}/items`, {
        data: { items: [movie!.id, series!.id] },
      })
      expect(reordered.status()).toBe(200)
      const again = await (await page.request.get(`/api/v1/lists/${list.id}`)).json()
      expect(again.items.map((item: Work) => item.id)).toEqual([movie!.id, series!.id])

      // A work that is not in the catalogue is refused, and the list kept.
      const refused = await page.request.put(`/api/v1/lists/${list.id}/items`, { data: { items: ['no-such-work'] } })
      expect(refused.status()).toBe(400)
      const kept = await (await page.request.get(`/api/v1/lists/${list.id}`)).json()
      expect(kept.total).toBe(2)

      // The work knows which selections hold it.
      const holding = await (await page.request.get(`/api/v1/items/${series!.id}/lists`)).json()
      expect(holding.lists.map((l: List) => l.id)).toContain(list.id)

      // A new name keeps the address the clients hold.
      const renamed = await page.request.put(`/api/v1/lists/${list.id}`, { data: { name: `${list.name} renamed` } })
      expect(renamed.status()).toBe(200)
      expect(((await renamed.json()) as List).slug).toBe(list.slug)
      expect(((await renamed.json()) as List).isPublic).toBe(true)

      // A change naming a work that is not there writes nothing at all.
      const refusedChange = await page.request.put(`/api/v1/lists/${list.id}`, {
        data: { name: 'not this', items: ['no-such-work'] },
      })
      expect(refusedChange.status()).toBe(400)
      const unchanged = (await (await page.request.get(`/api/v1/lists/${list.id}`)).json()).list as List
      expect(unchanged.name).toBe(`${list.name} renamed`)
      expect(unchanged.itemCount).toBe(2)
    } finally {
      await deleteList(page.request, list.id)
    }
  })

  test('a list composed by a filter is what matches when it is read', async ({ page }) => {
    await signIn(page)
    const list = await createList(page.request, {
      name: `E2E filter ${Date.now()}`,
      kind: 'series',
      mode: 'filter',
      isPublic: true,
      filter: { sort: 'title', limit: 3 },
    })
    try {
      const detail = await (await page.request.get(`/api/v1/lists/${list.slug}`)).json()
      expect(detail.items.length).toBeLessThanOrEqual(3)
      expect(detail.total).toBe(detail.items.length)
      for (const item of detail.items as Work[]) expect(item.kind).toBe('series')
      expect((await (await page.request.get(`/api/v1/lists/${list.slug}/radarr.json`)).json()).length).toBe(0)

      // A filter that is not one is refused.
      const refused = await page.request.put(`/api/v1/lists/${list.id}`, {
        data: { name: list.name, kind: 'series', mode: 'filter', isPublic: true, filter: { sort: 'sideways' } },
      })
      expect(refused.status()).toBe(400)
      // A filter list keeps its filter through a change that says nothing
      // of it, and its direction stays the sort's own — absent, not null.
      const kept = await page.request.put(`/api/v1/lists/${list.id}`, { data: { description: 'Still a filter.' } })
      expect(kept.status()).toBe(200)
      const after = (await kept.json()) as List
      expect(after.mode).toBe('filter')
      expect(after.filter?.sort).toBe('title')
      expect('descending' in (after.filter ?? {})).toBe(false)
      // Members are not a filter list's to have.
      expect((await page.request.put(`/api/v1/lists/${list.id}/items`, { data: { items: [] } })).status()).toBe(400)
      // And no work's page names it.
      const { items } = await (await page.request.get('/api/v1/items?kind=series&limit=1')).json()
      if (items[0]) {
        const holding = await (await page.request.get(`/api/v1/items/${items[0].id}/lists`)).json()
        expect(holding.lists.some((l: List) => l.mode === 'filter')).toBe(false)
      }
    } finally {
      await deleteList(page.request, list.id)
    }
  })

  test('a list that names a stranger is not created at all', async ({ page }) => {
    await signIn(page)
    const name = `E2E stranger ${Date.now()}`
    const refused = await page.request.post('/api/v1/lists', { data: { name, items: ['no-such-work'] } })
    expect(refused.status()).toBe(400)
    const listed = (await (await page.request.get('/api/v1/lists')).json()).lists as List[]
    expect(listed.some((l) => l.name === name)).toBe(false)
  })

  test('a private list is the maintainer’s alone', async ({ page, browser }) => {
    await signIn(page)
    const list = await createList(page.request, { name: `E2E private ${Date.now()}`, mode: 'manual', isPublic: false })
    try {
      // A visitor: a fresh context, with no cookie.
      const context = await browser.newContext()
      try {
        const anon = context.request
        expect((await anon.get(`/api/v1/lists/${list.slug}`)).status()).toBe(404)
        expect((await anon.get(`/api/v1/lists/${list.slug}/sonarr.json`)).status()).toBe(404)
        expect((await anon.get(`/api/v1/lists/${list.id}`)).status()).toBe(404)
        const listed = await (await anon.get('/api/v1/lists')).json()
        expect(listed.lists.some((l: List) => l.id === list.id)).toBe(false)
        // Composing is not a visitor's to do.
        expect((await anon.post('/api/v1/lists', { data: { name: 'x' } })).status()).toBe(401)
        expect((await anon.delete(`/api/v1/lists/${list.id}`)).status()).toBe(401)
      } finally {
        await context.close()
      }
      // The maintainer sees it, marked as private.
      const mine = await (await page.request.get('/api/v1/lists')).json()
      expect(mine.lists.some((l: List) => l.id === list.id && l.isPublic === false)).toBe(true)
    } finally {
      await deleteList(page.request, list.id)
    }
  })

  test('the selections are on the site, and in the editor', async ({ page }) => {
    await signIn(page)
    const { series } = await picks(page.request)
    test.skip(!series, 'needs a series with a TheTVDB id')
    const name = `E2E page ${Date.now()}`
    const list = await createList(page.request, {
      name,
      description: 'Made by the tests, taken away by them.',
      kind: 'series',
      mode: 'manual',
      isPublic: true,
      items: [series!.id],
    })
    try {
      await page.goto('/lists')
      await page.getByRole('link', { name: pattern(escapeRegExp(name)) }).click()
      await expect(page).toHaveURL(pattern(`/lists/${escapeRegExp(list.slug)}$`))
      await expect(page.getByRole('heading', { level: 1, name })).toBeVisible()
      await expect(page.getByText(series!.title).first()).toBeVisible()
      // The addresses a client imports from: the series' shape, not the films'.
      await expect(page.getByText(`/api/v1/lists/${list.slug}/sonarr.json`)).toBeVisible()
      await expect(page.getByText(`/api/v1/lists/${list.slug}/radarr.json`)).toHaveCount(0)

      // The work's page says which selection holds it.
      await page.goto(`/work/${series!.id}`)
      await expect(page.getByRole('link', { name })).toBeVisible()

      // The editor: the list opens with its name and its member.
      await page.goto('/admin/lists')
      await page.getByRole('button', { name }).first().click()
      await expect(page.locator('#edit-name')).toHaveValue(name)
      await expect(page.locator('#list-editor')).toContainText(series!.title)
      await expect(page.locator('#list-editor')).toContainText(`/api/v1/lists/${list.slug}/sonarr.json`)
    } finally {
      await deleteList(page.request, list.id)
    }
  })
})
