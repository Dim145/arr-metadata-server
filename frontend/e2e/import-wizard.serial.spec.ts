import { expect, test, type Page } from '@playwright/test'
import { scan } from './support/axe'
import { pattern } from './support/regex'

/**
 * Many works imported at once: lines pasted and a file dropped, set beside
 * the catalogue, then imported as a run that fills a selection.
 *
 * Only works the catalogue holds already are imported, so no provider is
 * asked for anything and nothing new comes in; the selection the run fills is
 * made for the test and deleted after it. Server-wide: a batch takes the
 * batch's lock, and a selection is written.
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

interface Work {
  id: string
  kind: 'series' | 'movie'
  title: string
  externalIds: { tmdb?: number; tvdb?: number; imdb?: string }
}

/** A line naming a work by an id it is held under. */
function lineOf(work: Work): string | undefined {
  const ids = work.externalIds
  if (work.kind === 'series' && ids.tvdb) return `tvdb:${ids.tvdb}`
  if (work.kind === 'movie' && ids.tmdb) return `https://www.themoviedb.org/movie/${ids.tmdb}`
  return ids.imdb
}

/** The same work as a row of a CSV, by the same id, as an export writes it. */
function csvOf(work: Work): string {
  const ids = work.externalIds
  const title = `"${work.title.replace(/"/g, '""')}"`
  if (work.kind === 'series' && ids.tvdb) return `title,tvdb_id,type\n${title},${ids.tvdb},series\n`
  if (work.kind === 'movie' && ids.tmdb) return `title,tmdb_id,type\n${title},${ids.tmdb},movie\n`
  return `title,imdb_id\n${title},${ids.imdb}\n`
}

test.describe('the import wizard', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('a batch is set beside the catalogue, then imported into a selection', async ({ page }) => {
    await signIn(page)

    // Three works the catalogue holds, each by an id it holds it under.
    const listed = (await (await page.request.get('/api/v1/items?limit=30')).json()) as { items: Work[] }
    const works = listed.items.filter((work) => lineOf(work)).slice(0, 3)
    test.skip(works.length < 3, 'needs three works with an id elsewhere')
    const lines = works.map((work) => lineOf(work)!)

    const name = `E2E batch ${Date.now()}`
    const made = await page.request.post('/api/v1/lists', { data: { name, kind: 'mixed', mode: 'manual', isPublic: false } })
    expect(made.status()).toBe(201)
    const selection = (await made.json()) as { id: string; slug: string }

    try {
      await page.goto('/admin/discover')
      await page.getByText(/^import many$/i).click()
      await expect(page).toHaveURL(/many=1/)

      // Pasted: the three works, one of them twice, and a line that is no id.
      await page.getByLabel(/^ids, a line each$/i).fill(['# picked for the test', ...lines, lines[0], 'not an id'].join('\n'))
      // And a file dropped: a CSV naming the first again.
      await page.locator('input[type="file"]').setInputFiles({
        name: 'picks.csv',
        mimeType: 'text/csv',
        buffer: Buffer.from(csvOf(works[0]!)),
      })
      await expect(page.getByText(/one line read from picks\.csv/i)).toBeVisible()
      await scan(page, 'the wizard’s source')

      await page.getByRole('button', { name: /^set it beside the catalogue$/i }).click()
      await expect(page.getByRole('heading', { name: /2 of 4 · where each stands/i })).toBeFocused()

      // Each line where it stands: the works held, the repeat, the stray.
      const lineList = page.getByRole('list', { name: /the lines, set beside the catalogue/i })
      for (const work of works) await expect(lineList.getByRole('link', { name: work.title }).first()).toBeVisible()
      await expect(lineList.getByText(/^not an id$/i).first()).toBeVisible()
      await expect(lineList.getByText(/^repeated$/i).first()).toBeVisible()
      await scan(page, 'the wizard’s preview')

      await page.getByRole('button', { name: /^next$/i }).click()
      await page.getByLabel(name).check()
      await scan(page, 'the wizard’s options')
      const started = page.waitForResponse((r) => r.url().endsWith('/api/v1/items/import/batch'))
      await page.getByRole('button', { name: /^import them$/i }).click()
      const answer = await started
      expect(answer.status()).toBe(202)
      const run = (await answer.json()) as { jobId: string; works: number }
      expect(run.works).toBe(works.length)

      // Followed here until it is done: every work already here, none fetched.
      await expect(page.getByText(/^succeeded$/i).first()).toBeVisible({ timeout: 30_000 })
      await expect(page.getByText(pattern(`${works.length} already here, 0 imported`, 'i')).first()).toBeVisible()
      await scan(page, 'the wizard’s run')
      await expect(page.getByRole('link', { name: /follow it on the tasks page/i })).toHaveAttribute(
        'href',
        '/admin/jobs#task-import.batch',
      )

      // And the selection holds them, in the batch's order.
      const page_ = (await (await page.request.get(`/api/v1/lists/${selection.slug}`)).json()) as { items: { id: string }[] }
      expect(page_.items.map((item) => item.id)).toEqual(works.map((work) => work.id))

      // The run, on the tasks page.
      const detail = (await (await page.request.get(`/api/v1/jobs/${run.jobId}`)).json()) as {
        job: { kind: string; status: string }
        entriesTotal: number
      }
      expect(detail.job.kind).toBe('import.batch')
      expect(detail.entriesTotal).toBe(works.length)
    } finally {
      await page.request.delete(`/api/v1/lists/${selection.id}`)
    }
  })

  test('is an administrator’s, and takes no more than two thousand lines', async ({ page }) => {
    await signIn(page)
    const lines = Array.from({ length: 2001 }, (_, at) => ({ text: `tmdb:${at + 1}` }))
    const refused = await page.request.post('/api/v1/items/import/preview', { data: { lines, kind: 'movie' } })
    expect(refused.status()).toBe(400)

    const stranger = await page.context().browser()!.newContext()
    expect((await stranger.request.post('/api/v1/items/import/batch', { data: { lines: [{ text: 'tt0133093' }] } })).status()).toBe(401)
    await stranger.close()
  })
})
