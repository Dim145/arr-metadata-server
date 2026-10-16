import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

/**
 * What the catalogue writes for the programs around it: Kometa's metadata file
 * of what has been locked, and the `.nfo` documents of one selection. Each
 * test makes what it needs and takes it away again.
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

/** Waits for an export that is being written to be done: one at a time, and the next asks would be refused. */
async function exportIdle(request: APIRequestContext) {
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const { jobs } = (await (await request.get('/api/v1/jobs?kind=export.nfo&limit=1')).json()) as {
      jobs: { status: string }[]
    }
    if (jobs[0]?.status !== 'running') return
    await new Promise((resolve) => setTimeout(resolve, 500))
  }
}

test.describe('exports', () => {
  test.skip(
    !USERNAME || !PASSWORD,
    'set AMS_E2E_USER and AMS_E2E_PASSWORD to an administrator to run the export tests',
  )

  test('the Kometa file holds the locks of one kind, matched by the id its library reads, and is downloaded from the settings', async ({
    page,
    browser,
  }, info) => {
    await signIn(page)
    // One film's own: each project gives its own identifier.
    const tmdb = info.project.name === 'desktop' ? 7_700_001 : 7_700_002
    const title = `Kometa e2e ${info.project.name} ${Date.now()}: "quoted" & more`
    const summary = 'First line.\nSecond: "line", with a colon and a # sign.'

    const created = await page.request.post('/api/v1/items', { data: { kind: 'movie', title, year: 2036 } })
    expect(created.status()).toBe(201)
    const { id } = (await created.json()) as { id: string }

    try {
      for (const lock of [
        { field: 'externalIds', value: { tmdb } },
        { field: 'title', value: title },
        { field: 'overview', value: summary },
      ]) {
        const locked = await page.request.put(`/api/v1/items/${id}/overrides`, { data: lock })
        expect(locked.ok(), `${lock.field}: ${await locked.text()}`).toBe(true)
      }

      const films = await page.request.get('/api/v1/export/kometa.yml?kind=movie')
      expect(films.status()).toBe(200)
      expect(films.headers()['content-type']).toContain('yaml')
      expect(films.headers()['content-disposition']).toContain('kometa-movies.yml')
      expect(Number(films.headers()['x-ams-works'])).toBeGreaterThanOrEqual(1)
      const yaml = await films.text()
      expect(yaml).toMatch(/^# Kometa metadata file/)
      expect(yaml).toContain('\nmetadata:\n')
      // Matched by TMDB id, and carrying what was locked, every string quoted and escaped:
      // JSON's quoting of this text is YAML's, as far as it goes.
      expect(yaml).toContain(`mapping_id: ${tmdb}`)
      expect(yaml).toContain(`title: ${JSON.stringify(title)}`)
      expect(yaml).toContain(`summary: ${JSON.stringify(summary)}`)
      // Nothing of what was not locked: this film's runtime, and its genres, were never said.
      expect(yaml).not.toContain('genre.sync')

      // A library of series is told nothing of a film.
      const series = await page.request.get('/api/v1/export/kometa.yml?kind=series')
      expect(series.status()).toBe(200)
      expect(await series.text()).not.toContain(`mapping_id: ${tmdb}`)

      // A file belongs to one library: a kind is asked for, and is one.
      expect((await page.request.get('/api/v1/export/kometa.yml')).status()).toBe(400)
      expect((await page.request.get('/api/v1/export/kometa.yml?kind=anime')).status()).toBe(400)
      expect((await page.request.get('/api/v1/export/kometa.yml?kind=movie&language=not%20a%20language')).status()).toBe(400)

      // Made of the locks, so not a visitor's.
      const context = await browser.newContext()
      try {
        const refused = await context.request.get('/api/v1/export/kometa.yml?kind=movie')
        expect([401, 403]).toContain(refused.status())
      } finally {
        await context.close()
      }

      // The settings page gives it as a download, one button a kind.
      await page.goto('/admin/settings')
      const panel = page.locator('#kometa-export')
      await expect(panel).toBeVisible()
      await expect(panel.getByRole('button', { name: /series|séries/i })).toBeVisible()
      const downloading = page.waitForEvent('download')
      await panel.getByRole('button', { name: /films/i }).click()
      expect((await downloading).suggestedFilename()).toBe('kometa-movies.yml')
      await expect(panel.getByRole('status')).toContainText(/written|écrit/i)
    } finally {
      await page.request.delete(`/api/v1/items/${id}`)
    }
  })

  test('the documents of one selection are asked for by its key, and a selection that is not there is refused', async ({
    page,
    browser,
  }) => {
    await signIn(page)
    await exportIdle(page.request)

    const missing = await page.request.post('/api/v1/export/nfo?list=no-such-selection-e2e')
    expect(missing.status()).toBe(404)

    const created = await page.request.post('/api/v1/lists', {
      data: { name: `E2E documents ${Date.now()}`, kind: 'mixed', mode: 'manual', isPublic: true },
    })
    expect(created.status()).toBe(201)
    const list = (await created.json()) as { id: string; slug: string }
    try {
      // Written in the background where an export folder is set; refused, saying so, where none is.
      const asked = await page.request.post(`/api/v1/export/nfo?list=${list.slug}`)
      expect([202, 503]).toContain(asked.status())
      if (asked.status() === 503) {
        expect(((await asked.json()) as { error: string }).error).toBe('export_not_configured')
      }
      await exportIdle(page.request)

      // Not a visitor's to ask for.
      const context = await browser.newContext()
      try {
        const refused = await context.request.post(`/api/v1/export/nfo?list=${list.slug}`)
        expect([401, 403]).toContain(refused.status())
      } finally {
        await context.close()
      }
    } finally {
      await page.request.delete(`/api/v1/lists/${list.id}`)
    }
  })
})
