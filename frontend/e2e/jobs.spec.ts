import { expect, test, type Page } from '@playwright/test'

/**
 * The tasks page's history: a page of runs at a time, under the day each was
 * started on, with the failures one press away.
 *
 * Read-only, and the history answered by a stand-in: a catalogue of the
 * suite's size holds no more than a handful of runs, and a page of fifty, a
 * second one and three different days are what this is about. Nothing here
 * starts a run or changes what the server holds, so it runs beside the others.
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

interface Run {
  id: string
  kind: string
  status: string
  createdAt: string
  startedAt: string
  finishedAt: string
  detail?: string
  error?: string
  /** Whoever started it; none for the schedule. */
  triggeredBy?: string
}

/** A day of the reader's own calendar, at noon: no hour of it changes which day it is. */
function noonDaysAgo(days: number): string {
  const day = new Date()
  day.setDate(day.getDate() - days)
  day.setHours(12, 0, 0, 0)
  return day.toISOString()
}

function run(index: number, createdAt: string, status = 'succeeded'): Run {
  return {
    id: `run-${String(index).padStart(3, '0')}`,
    kind: 'refresh.sweep',
    status,
    createdAt,
    startedAt: createdAt,
    finishedAt: createdAt,
    ...(status === 'failed' ? { error: 'no provider answered' } : { detail: '0 refreshed, 0 failed' }),
  }
}

/** Sixty runs, newest first: today's, yesterday's, and a week and a half back; five of them failed. */
function history(): Run[] {
  const runs: Run[] = []
  const today = new Date().toISOString()
  for (let index = 0; index < 60; index += 1) {
    const created = index < 20 ? today : index < 30 ? noonDaysAgo(1) : noonDaysAgo(12)
    // Two of today's, one of yesterday's, two of the old ones.
    const failed = [3, 11, 24, 40, 55].includes(index)
    runs.push(run(index, created, failed ? 'failed' : 'succeeded'))
  }
  return runs
}

/** Answer the history from `runs`, as the server pages and filters it; every question is kept. */
async function standIn(page: Page, runs: Run[]) {
  const asked: URLSearchParams[] = []
  await page.route(/\/api\/v1\/jobs\?/, async (route) => {
    const params = new URL(route.request().url()).searchParams
    asked.push(params)
    const by = params.get('by')
    const wanted = runs
      .filter((r) => !params.get('status') || r.status === params.get('status'))
      .filter((r) => !by || (by === 'person' ? Boolean(r.triggeredBy) : !r.triggeredBy))
    const offset = Number(params.get('offset') ?? 0)
    const limit = Number(params.get('limit') ?? 50)
    await route.fulfill({ json: { jobs: wanted.slice(offset, offset + limit), total: wanted.length } })
  })
  return asked
}

const region = (page: Page) => page.getByRole('region', { name: /^(history|historique)$/i })
const shown = (page: Page) => page.locator('[data-job]:visible')

test.describe('the history of the runs', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
    await signIn(page)
  })

  test('is a page at a time, with the rest a press away', async ({ page }) => {
    const asked = await standIn(page, history())
    await page.goto('/admin/jobs')

    await expect(shown(page)).toHaveCount(50)
    // How far through it is, and the button that goes on.
    await expect(region(page).getByText(/^50 of 60$/)).toBeVisible()
    const more = region(page).getByRole('button', { name: /^(load more|en charger plus)$/i })
    await expect(more).toBeVisible()
    // The history's own first page — other parts of the screen may ask for
    // a few runs of their own before it.
    const first = asked.find((params) => params.get('offset') === '0')
    expect(first?.get('limit')).toBe('50')

    await more.click()
    await expect(shown(page)).toHaveCount(60)
    // Asked from where the last page ended, and nothing is left to ask for.
    expect(asked.some((params) => params.get('offset') === '50')).toBe(true)
    await expect(more).toHaveCount(0)
  })

  test('is read under the day each run was started on, today and yesterday said in words', async ({ page }) => {
    await standIn(page, history())
    await page.goto('/admin/jobs')
    await expect(shown(page)).not.toHaveCount(0)

    const days = region(page).getByRole('heading', { level: 3 })
    await expect(days.nth(0)).toContainText(/^today/i)
    await expect(days.nth(1)).toContainText(/^yesterday/i)
    // A fortnight back it is a date: a weekday and a day of the month, no longer "ago".
    await expect(days.nth(2)).toContainText(/\d/)
    await expect(days.nth(2)).not.toContainText(/today|yesterday|ago/i)
    // The failures of a day are counted beside its name.
    await expect(days.nth(0)).toContainText(/2 failed/i)
  })

  test('puts the failures first: a chip at the head asks for those and nothing else', async ({ page }) => {
    const asked = await standIn(page, history())
    await page.goto('/admin/jobs')
    await expect(shown(page)).not.toHaveCount(0)

    const chip = region(page).getByRole('button', { name: /^(failures only|échecs seulement)$/i })
    await expect(chip).toHaveAttribute('aria-pressed', 'false')
    await chip.click()
    await expect(chip).toHaveAttribute('aria-pressed', 'true')
    await expect(shown(page)).toHaveCount(5)
    expect(asked.at(-1)!.get('status')).toBe('failed')
    // The same filter as the outcome's own, which says so.
    await expect(region(page).getByLabel(/^(outcome|résultat)$/i)).toHaveValue('failed')
    // Every one of them said failed, in words.
    for (const row of await shown(page).all()) await expect(row).toContainText(/failed|échec/i)

    // Off again, the whole of it.
    await chip.click()
    await expect(chip).toHaveAttribute('aria-pressed', 'false')
    await expect(shown(page)).toHaveCount(50)
  })

  test('says that nothing failed as a good answer, and that a filter found nothing as another', async ({ page }) => {
    const good = history().map((r) => ({ ...r, status: 'succeeded', error: undefined }))
    const asked = await standIn(page, good)
    await page.goto('/admin/jobs')
    await expect(shown(page)).not.toHaveCount(0)

    // Nothing failed: said as the good answer, not as an empty history.
    const chip = region(page).getByRole('button', { name: /^(failures only|échecs seulement)$/i })
    await chip.click()
    await expect(region(page).getByText(/no run has failed/i)).toBeVisible()
    await expect(shown(page)).toHaveCount(0)

    // Nobody started any: a filter that finds nothing, which is not the same.
    await chip.click()
    await region(page).getByLabel(/^(started by|lancée par)$/i).selectOption('person')
    await expect.poll(() => asked.at(-1)!.get('by')).toBe('person')
    await expect(region(page).getByText(/no run matches these filters/i)).toBeVisible()
  })
})
