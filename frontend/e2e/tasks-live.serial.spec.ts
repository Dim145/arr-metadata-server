import { expect, test, type Page } from '@playwright/test'
import { scan } from './support/axe'

/**
 * The tasks followed as they go, and when the periodic ones run.
 *
 * The page opens the server's stream and says it follows the tasks live; a
 * run started is in the history without the page being reloaded; a run
 * under way draws its bar; a stream that cannot be opened is asked for
 * instead. The sweep is given a window of the night and its own cadence
 * back. Server-wide: a schedule set, and a run started, change what the
 * other specs read — and each is put back.
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

interface Task {
  id: string
  mode: string
  schedulable: boolean
  schedule?: { kind: string; from?: string; to?: string; cron?: string }
  running?: { id: string }
  last?: { id: string }
}

const card = (page: Page, name: RegExp) => page.getByRole('region', { name })

test.describe('the tasks, followed', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test.afterEach(async ({ page }) => {
    // The sweep's own cadence, whatever a failed test left.
    await page.request.delete('/api/v1/tasks/refresh.sweep/schedule')
  })

  test('are followed live, and a run started is in the history at once', async ({ page }) => {
    await signIn(page)
    const opened = page.waitForResponse((r) => r.url().endsWith('/api/v1/admin/tasks/stream'))
    await page.goto('/admin/jobs')
    const stream = await opened
    expect(stream.status()).toBe(200)
    expect(stream.headers()['content-type']).toContain('text/event-stream')
    await expect(page.getByRole('status').filter({ hasText: /followed as it goes/i })).toBeVisible()

    // The three tasks this page gained, among the others.
    for (const name of [/^import a batch$/i, /^import rules$/i, /^keep the catalogue’s order current$/i]) {
      await expect(page.getByRole('heading', { level: 2, name })).toBeVisible()
    }

    // The listing, run now: quick, and filed under who asked.
    const listing = card(page, /keep the catalogue’s order current/i)
    await expect
      .poll(async () => {
        const tasks = (await (await page.request.get('/api/v1/tasks')).json()) as Task[]
        return tasks.find((task) => task.id === 'listing.relist')?.running
      })
      .toBeUndefined()
    const asked = page.waitForResponse((r) => r.url().endsWith('/api/v1/tasks/listing.relist/run'))
    await listing.getByRole('button', { name: /^(run now|run again): keep/i }).click()
    const answer = await asked
    expect(answer.status()).toBe(202)
    const { jobId } = (await answer.json()) as { jobId: string }

    // The history shows it without a reload: the stream said it moved.
    await expect(page.locator(`[data-job="${jobId}"]:visible`)).toBeVisible({ timeout: 15_000 })

    // Its log, from its card.
    await listing.getByRole('button', { name: /^log: keep/i }).click()
    const log = page.getByRole('dialog').filter({ hasText: /what “keep the catalogue’s order current” did/i })
    await expect(log).toBeVisible()
    await expect(log.getByText(/^started$/i)).toBeVisible()
    await scan(page, 'a run’s log')
    await log.getByRole('button', { name: /^close$/i }).click()
    await expect(log).toBeHidden()
  })

  test('a run under way draws how far it has got', async ({ page }) => {
    await signIn(page)
    // A board in which a refresh of everything is a third of the way: the
    // stream's event, and the same tasks when they are asked for.
    const tasks = (await (await page.request.get('/api/v1/tasks')).json()) as Task[]
    const running = tasks.map((task) =>
      task.id === 'refresh.all'
        ? {
            ...task,
            running: {
              id: 'stand-in-run',
              kind: 'refresh.all',
              target: null,
              status: 'running',
              startedAt: new Date().toISOString(),
              finishedAt: null,
              error: null,
              detail: '10 of 30: 9 refreshed, 1 failed',
              createdAt: new Date().toISOString(),
            },
            progress: { done: 10, total: 30 },
            cancelable: true,
            blocked: 'running',
          }
        : task,
    )
    await page.route('**/api/v1/admin/tasks/stream', (route) =>
      route.fulfill({
        status: 200,
        headers: { 'content-type': 'text/event-stream' },
        body: `retry: 60000\nevent: tasks\ndata: ${JSON.stringify({ tasks: running, newestRun: 'stand-in-run' })}\n\n`,
      }),
    )
    await page.route('**/api/v1/tasks', (route) => route.fulfill({ json: running }))
    await page.goto('/admin/jobs')

    const everything = card(page, /^refresh everything$/i)
    const bar = everything.getByRole('progressbar', { name: /how far “refresh everything” has got/i })
    await expect(bar).toBeVisible()
    await expect(bar).toHaveAttribute('value', '10')
    await expect(bar).toHaveAttribute('max', '30')
    await expect(everything.getByText('33%')).toBeVisible()
    await expect(everything.getByText(/10 of 30: 9 refreshed, 1 failed/i)).toBeVisible()
    await expect(everything.getByRole('button', { name: /^cancel: refresh everything$/i })).toBeVisible()
  })

  test('a stream that cannot be opened is asked for instead', async ({ page }) => {
    await signIn(page)
    await page.route('**/api/v1/admin/tasks/stream', (route) => route.abort())
    await page.goto('/admin/jobs')
    await expect(page.getByRole('status').filter({ hasText: /asked for every few seconds/i })).toBeVisible({
      timeout: 20_000,
    })
    await expect(page.getByRole('heading', { level: 2, name: /^refresh what is due$/i })).toBeVisible()
  })

  test('the sweep is kept to a window of the night, and given its own cadence back', async ({ page }) => {
    await signIn(page)
    await page.goto('/admin/jobs')
    const sweep = card(page, /^refresh what is due$/i)
    await sweep.getByRole('button', { name: /^when it runs: refresh what is due$/i }).click()

    const dialog = page.getByRole('dialog').filter({ hasText: /when “refresh what is due” runs/i })
    await expect(dialog).toBeVisible()
    await dialog.getByText(/^within a window$/i).click()
    await dialog.getByLabel(/^from$/i).fill('01:00')
    await dialog.getByLabel(/^to$/i).fill('06:00')
    // Its next runs, said before anything is kept.
    await expect(dialog.getByRole('listitem')).toHaveCount(3)
    await scan(page, 'a task’s schedule')
    await dialog.getByRole('button', { name: /^save$/i }).click()
    await expect(dialog).toBeHidden()

    // Said on its card, and kept.
    await expect(sweep.getByText('01:00–06:00 UTC')).toBeVisible()
    const kept = (await (await page.request.get('/api/v1/tasks')).json()) as (Task & { nextAt?: string })[]
    const state = kept.find((task) => task.id === 'refresh.sweep')!
    expect(state.schedule).toMatchObject({ kind: 'window', from: '01:00', to: '06:00' })
    const hour = new Date(state.nextAt!).getUTCHours()
    expect(hour >= 1 && hour < 6).toBe(true)

    // A cron expression that never comes is said so, and not saved.
    await sweep.getByRole('button', { name: /^when it runs: refresh what is due$/i }).click()
    await dialog.getByText(/^at set moments$/i).click()
    await dialog.getByLabel(/^cron expression$/i).fill('0 0 30 2 *')
    await expect(dialog.getByRole('alert')).toContainText(/never comes/i)
    await expect(dialog.getByRole('button', { name: /^save$/i })).toBeDisabled()

    // Its own cadence again.
    await dialog.getByText(/^its own cadence$/i).click()
    await dialog.getByRole('button', { name: /^back to its own cadence$/i }).click()
    await expect(dialog).toBeHidden()
    await expect(sweep.getByText('01:00–06:00 UTC')).toHaveCount(0)

    // A task that keeps its own cadence takes no schedule.
    const refused = await page.request.put('/api/v1/tasks/tls.renew/schedule', {
      data: { kind: 'cron', cron: '0 4 * * 0' },
    })
    expect(refused.status()).toBe(400)
  })
})
