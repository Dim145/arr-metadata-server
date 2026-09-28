import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

/**
 * Where a work's values come from: the rules on the Sources page, the chip
 * beside each field, and a sync from one source alone.
 *
 * One sync, of one source, for one work — Fanart.tv when it answers, being a
 * single call — and a full refresh only when no work says yet where its values
 * came from. Server-wide, because a sync rewrites the work it touches.
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

/** The names the page writes providers under. */
const NAMES: Record<string, string> = {
  anilist: 'AniList',
  fanart: 'Fanart.tv',
  fankai: 'Fankai',
  fankaiwiki: 'Wiki Fankai',
  mal: 'MyAnimeList',
  radarr: 'Radarr',
  skyhook: 'Skyhook',
  tmdb: 'TMDB',
  tvdb: 'TheTVDB',
  tvmaze: 'TVmaze',
}

const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

interface Report {
  provenance?: { fields?: Record<string, { from: string; agreed?: string[] }>; episodes?: string }
  sources: { provider: string; unavailable?: string; fetchedAt?: string }[]
}

/** A series whose provenance is on record, refreshing one in full if none is. */
async function aTracedSeries(request: APIRequestContext): Promise<{ id: string; report: Report } | null> {
  const page = await (await request.get('/api/v1/items?kind=series&limit=40')).json()
  const works = (page as { items: { id: string; isManual?: boolean }[] }).items.filter((w) => !w.isManual)

  for (const work of works) {
    const report = (await (await request.get(`/api/v1/items/${work.id}/provenance`)).json()) as Report
    if (report.provenance) return { id: work.id, report }
  }

  const first = works[0]
  if (!first) return null
  const refreshed = await request.post(`/api/v1/items/${first.id}/refresh`)
  if (!refreshed.ok()) return null
  const report = (await (await request.get(`/api/v1/items/${first.id}/provenance`)).json()) as Report
  return report.provenance ? { id: first.id, report } : null
}

test.describe('where values come from', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('the Sources page ranks every provider for every field, in the configured order', async ({ page }) => {
    await signIn(page)
    await page.goto('/admin/sources')
    await expect(page.getByRole('heading', { level: 1, name: 'Sources' })).toBeVisible()

    const rules = (await (await page.request.get('/api/v1/sources/rules')).json()) as {
      providers: { id: string; rank: number }[]
      rows: { field: string; suppliers: string[]; authorities?: string[] }[]
    }
    expect(rules.providers.map((p) => p.rank)).toEqual(rules.providers.map((_, i) => i + 1))
    const studio = rules.rows.find((row) => row.field === 'studio')
    expect(studio?.authorities).toEqual(['anilist', 'mal'])
    const list = rules.rows.find((row) => row.field === 'episodeList')
    expect(list?.suppliers[0]).toBe('tvdb')

    // Said in words as well as marks: the first to give the title is named.
    const first = rules.rows.find((row) => row.field === 'title')!.suppliers[0]!
    const table = page.getByRole('table')
    if (await table.isVisible()) {
      const title = table
        .getByRole('row')
        .filter({ has: page.getByRole('rowheader', { name: /^(title|titre)$/i }) })
      // Under the column of the first to give it, as its header names it.
      const column = rules.providers.findIndex((provider) => provider.id === first)
      await expect(table.getByRole('columnheader').nth(column + 1)).toContainText(NAMES[first] ?? first)
      await expect(title.getByRole('cell').nth(column)).toContainText(/gives it first|la donne d’abord/i)
    } else {
      await expect(page.getByRole('region', { name: /identity|identité/i })).toBeVisible()
    }

    const stranger = await page.context().browser()!.newContext()
    expect((await stranger.request.get('/api/v1/sources/rules')).status()).toBe(401)
    await stranger.close()
  })

  test('each field names its source, and one source can be asked again alone', async ({ page }) => {
    test.slow()
    await signIn(page)

    const traced = await aTracedSeries(page.request)
    test.skip(!traced, 'no series can say where its values came from; the providers are not configured')
    const { id, report } = traced!

    await page.goto(`/admin/catalogue/${id}`)
    await expect(page.getByText(/every field says where its value came from|chaque champ dit d’où vient sa valeur/i)).toBeVisible()

    // The title's chip names who gave it, or says several did.
    const title = report.provenance!.fields!.title!
    const row = page.locator('li[data-field="title"]')
    await expect(
      row
        .getByText(/^(multi-sources · \d|tmdb|thetvdb|skyhook|tvmaze|anilist|myanimelist|fankai)$/i)
        .filter({ visible: true }),
    ).toHaveCount(1)
    expect(title.from).toBeTruthy()

    // One source that can answer, Fanart.tv first: a single call.
    const askable = report.sources.filter((source) => !source.unavailable && source.fetchedAt)
    const source = askable.find((s) => s.provider === 'fanart') ?? askable[0]
    test.skip(!source, 'no source of this work can be asked now')

    const before = (await (await page.request.get(`/api/v1/items/${id}`)).json()) as { refreshAfter?: string }

    const sources = page.locator('#sources')
    await sources.getByRole('checkbox', { name: new RegExp(escape(NAMES[source!.provider] ?? source!.provider), 'i') }).check()
    const asked = page.waitForResponse((r) => r.url().endsWith(`/api/v1/items/${id}/sync`))
    await sources.getByRole('button', { name: /sync from 1 source|synchroniser depuis 1 source/i }).click()
    const answer = await asked

    if (answer.status() === 502) {
      // The provider did not answer: nothing was written, and it is said.
      await expect(sources.getByRole('alert')).toBeVisible()
      return
    }
    expect(answer.status()).toBe(200)
    await expect(sources.getByRole('status')).toContainText(/synced from|synchronisée depuis/i)

    // A sync is not a refresh: the schedule stands.
    const after = (await (await page.request.get(`/api/v1/items/${id}`)).json()) as { refreshAfter?: string }
    expect(after.refreshAfter).toBe(before.refreshAfter)

    // Filed under the person, and saying what it asked.
    const runs = (await (
      await page.request.get(`/api/v1/jobs?kind=refresh.item&by=person&limit=5`)
    ).json()) as { jobs: { target?: string; detail?: string }[] }
    expect(runs.jobs.find((job) => job.target === id)?.detail).toMatch(/^synced from /)
  })

  test('the import page takes a pasted page for the lookup it stands for, and one source alone', async ({ page }) => {
    await signIn(page)
    // A work already held, so the lookup is answered from the store.
    const works = (await (await page.request.get('/api/v1/items?kind=series&limit=40')).json()) as {
      items: { id: string; externalIds: { tmdb?: number } }[]
    }
    const held = works.items.find((work) => work.externalIds.tmdb)
    test.skip(!held, 'no series here carries a TMDB id')

    const link = `https://www.themoviedb.org/tv/${held!.externalIds.tmdb}-anything?language=fr`
    const found = (await (await page.request.get(`/api/v1/discover?term=${encodeURIComponent(link)}`)).json()) as {
      tmdbId?: number
      kind: string
      stored: boolean
    }[]
    expect(found).toHaveLength(1)
    expect(found[0]).toMatchObject({ tmdbId: held!.externalIds.tmdb, kind: 'series', stored: true })

    // Only the sources that can be searched alone, and a source for the other kind finds nothing.
    expect((await page.request.get('/api/v1/discover?term=x&source=mal')).status()).toBe(400)
    const films = await page.request.get('/api/v1/discover?term=x&kind=series&source=radarr')
    expect(films.status()).toBe(200)
    expect(await films.json()).toEqual([])
  })

  test('a work entered by hand says so, and asks no source it has no id to be asked by', async ({ page }) => {
    await signIn(page)
    // Invented, and removed again: nothing any other test reads.
    const created = await page.request.post('/api/v1/items', {
      data: {
        kind: 'movie',
        title: `L’Atelier des Ombres ${Date.now()}`,
        year: 2031,
        overview: 'Une restauratrice de films muets découvre des plans que personne n’a tournés.',
      },
    })
    expect(created.status()).toBe(201)
    const { id } = (await created.json()) as { id: string }

    try {
      await page.goto(`/admin/catalogue/${id}`)
      const panel = page.locator('#sources')
      await expect(panel).toContainText(/entered by hand|saisie à la main/i)
      await expect(panel.getByRole('checkbox')).toHaveCount(0)

      // A manual entry has no source: refused as such, before the sources named are looked at.
      const refused = await page.request.post(`/api/v1/items/${id}/sync`, { data: { sources: ['tmdb'] } })
      expect(refused.status()).toBe(409)
      const noRefresh = await page.request.post(`/api/v1/items/${id}/refresh`)
      expect(noRefresh.status()).toBe(409)
    } finally {
      await page.request.delete(`/api/v1/items/${id}`)
    }
  })

  test('a Fan-Kai never leads to another production on Fankai’s site', async ({ page }) => {
    const works = (await (await page.request.get('/api/v1/items?kind=series&limit=100')).json()) as {
      items: { id: string; externalIds: { fankai?: number } }[]
    }
    const fankai = works.items.find((work) => work.externalIds.fankai)
    test.skip(!fankai, 'no Fan-Kai in the catalogue')

    await page.goto(`/work/${fankai!.id}`)
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    // Its id is the metadata service's, which the site numbers otherwise.
    await expect(page.locator('a[href*="fankai.fr/productions"]')).toHaveCount(0)

    const detail = (await (await page.request.get(`/api/v1/items/${fankai!.id}`)).json()) as { homepage?: string }
    if (detail.homepage) expect(detail.homepage).toMatch(/^https:\/\/fan-kai\.fandom\.com\//)
  })

  test('the import page says why a page of Fankai’s site cannot be taken', async ({ page }) => {
    await signIn(page)
    await page.goto('/admin/discover')
    await page.getByLabel(/^(title|titre)$/i).fill('https://fankai.fr/productions/101')
    await expect(page.getByRole('alert')).toContainText(/metadata service|service de métadonnées/i)
    await expect(page.getByRole('button', { name: /search the providers|chercher chez les sources/i })).toBeDisabled()
  })

  test('a sync is refused what it cannot do, and to anyone who may not write', async ({ page }) => {
    await signIn(page)
    const works = (await (await page.request.get('/api/v1/items?kind=series&limit=1')).json()) as {
      items: { id: string }[]
    }
    const id = works.items[0]?.id
    test.skip(!id, 'the catalogue is empty')

    const nothing = await page.request.post(`/api/v1/items/${id}/sync`, { data: { sources: [] } })
    expect(nothing.status()).toBe(400)
    const stranger = await page.request.post(`/api/v1/items/${id}/sync`, { data: { sources: ['radarr'] } })
    // Radarr describes films; an untraced series is refused before that.
    expect([400, 409]).toContain(stranger.status())

    const visitor = await page.context().browser()!.newContext()
    expect((await visitor.request.post(`/api/v1/items/${id}/sync`, { data: { sources: ['tmdb'] } })).status()).toBe(401)
    expect((await visitor.request.get(`/api/v1/items/${id}/provenance`)).status()).toBe(401)
    await visitor.close()
  })
})
