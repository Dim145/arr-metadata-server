import { expect, test, type Page } from '@playwright/test'
import { escaped, pattern } from './support/regex'

/**
 * The tasks page: every task on a card, one run started from it, and the
 * history saying who started what.
 *
 * Only the sweep is run — the one the schedule starts every quarter of an
 * hour anyway. A refresh of everything would ask the providers for every work
 * in the suite's catalogue, which is not a test's to spend. Server-wide, for
 * what a run changes.
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

test.describe('the tasks', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('each task says when it runs, and one run from its card is filed under who started it', async ({
    page,
  }) => {
    await signIn(page)
    await page.goto('/admin/jobs')

    for (const name of [
      /refresh what is due|actualiser ce qui est dû/i,
      /refresh everything|tout actualiser/i,
      /anime identifier list|liste des identifiants d’anime/i,
      /imdb ratings|notes imdb/i,
      /nfo export|export nfo/i,
    ]) {
      await expect(page.getByRole('heading', { level: 2, name })).toBeVisible()
    }

    // The schedule's own sweep, or one left running, would take the lock:
    // wait until neither refresh is running before asking for one.
    await expect
      .poll(
        async () => {
          const tasks = (await (await page.request.get('/api/v1/tasks')).json()) as {
            id: string
            running?: unknown
          }[]
          return tasks.some((task) => task.id.startsWith('refresh.') && task.running)
        },
        { timeout: 60_000 },
      )
      .toBe(false)
    await page.reload()

    const sweep = page.getByRole('region', { name: /refresh what is due|actualiser ce qui est dû/i })
    const asked = page.waitForResponse((r) => r.url().endsWith('/api/v1/tasks/refresh.sweep/run'))
    await sweep.getByRole('button', { name: /^(run now|lancer)$/i }).click()
    const answer = await asked
    expect(answer.status()).toBe(202)
    const { jobId } = (await answer.json()) as { jobId: string }

    // Filed under the person, whoever the schedule's runs are.
    const run = await (await page.request.get(`/api/v1/jobs?by=person&kind=refresh.sweep&limit=5`)).json()
    const mine = (run as { jobs: { id: string; triggeredBy?: string }[] }).jobs.find((job) => job.id === jobId)
    const name = USERNAME!.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
    expect(mine?.triggeredBy).toMatch(pattern(`:${escaped(name)}$`))
    await page.getByLabel(/^(started by|lancée par)$/i).selectOption('person')
    await expect(page.getByRole('table').getByRole('row').nth(1)).toContainText(
      /refresh what is due|actualiser ce qui est dû/i,
    )

    // Only runs somebody started, then.
    const listed = await page.request.get('/api/v1/jobs?by=person&limit=50')
    const { jobs } = (await listed.json()) as { jobs: { triggeredBy?: string }[] }
    expect(jobs.length).toBeGreaterThan(0)
    expect(jobs.every((job) => job.triggeredBy)).toBe(true)

    // Nothing that is not a task, and nothing for a key or a stranger.
    expect((await page.request.post('/api/v1/tasks/refresh.nothing/run')).status()).toBe(404)
    const stranger = await page.context().browser()!.newContext()
    expect((await stranger.request.get('/api/v1/tasks')).status()).toBe(401)
    await stranger.close()
  })

  test('a run opens onto what it did, work by work', async ({ page }) => {
    await signIn(page)

    // The newest run, whatever it is: the sweep the test above started, or
    // the schedule's. Its detail by the API first, then as the page shows it.
    const listed = await page.request.get('/api/v1/jobs?limit=1')
    const { jobs } = (await listed.json()) as { jobs: { id: string; kind: string }[] }
    test.skip(jobs.length === 0, 'nothing has run yet')
    const run = jobs[0]!
    const answer = await page.request.get(`/api/v1/jobs/${run.id}`)
    expect(answer.status()).toBe(200)
    const detail = (await answer.json()) as {
      job: { id: string; kind: string }
      entries: { outcome: string; title?: string }[]
      entriesTotal: number
    }
    expect(detail.job.id).toBe(run.id)
    expect(detail.entries.length).toBe(Math.min(detail.entriesTotal, 500))
    for (const entry of detail.entries) expect(['ok', 'failed', 'skipped']).toContain(entry.outcome)

    await page.goto('/admin/jobs')
    // The run's name is the button that opens it — and says "hide" once it
    // has, so the button is found by its row, which carries the run's id:
    // the schedule may have started another run since the list was read,
    // and the first row would then be that one's.
    const opener = page
      .locator(`[data-job="${run.id}"]:visible`)
      .getByRole('button', { name: /^(show|hide) what|^(afficher|masquer) ce qu/i })
    await expect(opener).toBeVisible()
    await opener.click()
    await expect(opener).toHaveAttribute('aria-expanded', 'true')
    const panel = page.locator(`#${await opener.getAttribute('aria-controls')}`)
    await expect(panel).toBeVisible()
    await expect(panel.getByText(/^(started|début)$/i)).toBeVisible()
    if (detail.entriesTotal === 0) {
      // Nothing to list, said one way or the other: a run that kept no
      // detail, or a sweep that found nothing due.
      await expect(
        panel.getByText(/nothing is written down|rien n’est consigné|no work was due|aucune fiche n’était due/i),
      ).toBeVisible()
    } else {
      await expect(panel.getByRole('listitem')).toHaveCount(detail.entries.length)
    }
    // Opened and closed again, from the same button.
    await opener.click()
    await expect(opener).toHaveAttribute('aria-expanded', 'false')
    await expect(panel).toHaveCount(0)

    // A stranger reads nothing of it.
    const stranger = await page.context().browser()!.newContext()
    expect((await stranger.request.get(`/api/v1/jobs/${run.id}`)).status()).toBe(401)
    await stranger.close()
  })
})
