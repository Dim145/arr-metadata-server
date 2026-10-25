import { expect, test, type Page } from '@playwright/test'

/**
 * The server's own log, read in the administration: the lines it wrote, the
 * graver ones by themselves, the lines of one request a press away, and the
 * whole of it as a file.
 *
 * The lines are answered by a stand-in, so that what is on the page is known
 * — the server's own log holds whatever the suite beside this one made it
 * write. One test reads the real log too, for what only the server can say:
 * that a request it answered is there, under the id it answered with.
 * Nothing here changes what the server holds.
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

const TRACE = '4bf92f3577b34da6a3ce929d0e0e4736'

interface Entry {
  seq: number
  at: string
  level: 'error' | 'warn' | 'info' | 'debug' | 'trace'
  target: string
  message: string
  fields: { name: string; value: string }[]
  traceId?: string
  requestId?: string
  request?: string
}

/** Five lines, newest first: two of one request, an error among them. */
function lines(): Entry[] {
  const at = (seconds: number) => new Date(Date.now() - seconds * 1000).toISOString()
  return [
    {
      seq: 5,
      at: at(1),
      level: 'error',
      target: 'arr_metadata_server::error',
      message: 'request failed',
      fields: [{ name: 'error', value: 'upstream provider unavailable: TMDB request failed' }],
      traceId: TRACE,
      requestId: TRACE,
      request: 'GET /v1/tvdb/shows/en/81189',
    },
    {
      seq: 4,
      at: at(2),
      level: 'warn',
      target: 'arr_metadata_server::providers',
      message: 'the provider asked to be left alone',
      fields: [{ name: 'provider', value: 'tmdb' }],
      traceId: TRACE,
      requestId: TRACE,
      request: 'GET /v1/tvdb/shows/en/81189',
    },
    { seq: 3, at: at(30), level: 'info', target: 'arr_metadata_server::jobs::refresh', message: 'sweep done', fields: [] },
    { seq: 2, at: at(60), level: 'info', target: 'arr_metadata_server::web', message: 'listening', fields: [] },
    { seq: 1, at: at(90), level: 'debug', target: 'arr_metadata_server::cache', message: 'cache warmed', fields: [] },
  ]
}

const GRAVITY = ['error', 'warn', 'info', 'debug', 'trace']

/** Answer the log as the server filters it; every question asked is kept. */
async function standIn(page: Page, entries: Entry[]) {
  const asked: URLSearchParams[] = []
  await page.route(/\/api\/v1\/admin\/logs(\?|$)/, async (route) => {
    const params = new URL(route.request().url()).searchParams
    asked.push(params)
    const level = GRAVITY.indexOf(params.get('level') ?? 'trace')
    const words = (params.get('q') ?? '').toLowerCase().split(/\s+/).filter(Boolean)
    const wanted = entries.filter(
      (entry) =>
        GRAVITY.indexOf(entry.level) <= level &&
        words.every((word) => JSON.stringify(entry).toLowerCase().includes(word)),
    )
    await route.fulfill({
      json: {
        instance: 'metadata-1',
        capacity: 2000,
        held: entries.length,
        traces: true,
        reports: false,
        entries: wanted,
      },
    })
  })
  return asked
}

const list = (page: Page) => page.getByRole('list', { name: /^(lines of the log|lignes du journal)$/i })
const items = (page: Page) => list(page).getByRole('listitem')

test.describe('the server log', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
    await signIn(page)
  })

  test('is reached from the navigation, and says whose log it is and what leaves the server', async ({
    page,
    isMobile,
  }) => {
    await standIn(page, lines())
    if (isMobile) {
      // The tabs along the foot have no place for it: its address does.
      await page.goto('/admin/logs')
    } else {
      await page.goto('/admin')
      await page.getByRole('link', { name: /^server log$/i }).first().click()
    }
    await expect(page).toHaveURL(/\/admin\/logs$/)
    await expect(page.getByRole('heading', { level: 1, name: /^server log$/i })).toBeVisible()
    await expect(page.getByText('metadata-1', { exact: true })).toBeVisible()
    await expect(page.getByText(/^5 of 2,000 lines kept/)).toBeVisible()
    await expect(page.getByText(/sent to the collector/i)).toBeVisible()
    // Error reports are off: the page says where they are set.
    await expect(page.getByRole('link', { name: /settings › error reports/i })).toBeVisible()
  })

  test('shows the information and what is graver, then the warnings and the errors alone', async ({ page }) => {
    const asked = await standIn(page, lines())
    await page.goto('/admin/logs')

    // The debug line is under the default level.
    await expect(items(page)).toHaveCount(4)
    expect(asked.at(-1)!.get('level')).toBe('info')
    await expect(items(page).first()).toContainText('request failed')
    await expect(items(page).first()).toContainText(/error=upstream provider unavailable/)
    await expect(items(page).first()).toContainText('GET /v1/tvdb/shows/en/81189')

    await page.getByLabel(/^level$/i).selectOption('warn')
    await expect(items(page)).toHaveCount(2)
    expect(asked.at(-1)!.get('level')).toBe('warn')

    await page.getByLabel(/^level$/i).selectOption('trace')
    await expect(items(page)).toHaveCount(5)
  })

  test('keeps the lines of one request at the press of its trace, and lets them go again', async ({ page }) => {
    const asked = await standIn(page, lines())
    await page.goto('/admin/logs')
    await expect(items(page)).toHaveCount(4)

    await items(page)
      .first()
      .getByRole('button', { name: new RegExp(`only the lines of trace ${TRACE}`, 'i') })
      .click()
    await expect(page.getByLabel(/^find$/i)).toHaveValue(TRACE)
    await expect(items(page)).toHaveCount(2)
    expect(asked.at(-1)!.get('q')).toBe(TRACE)

    await page.getByRole('button', { name: /^clear the filter$/i }).click()
    await expect(items(page)).toHaveCount(4)
  })

  test('finds lines by their words, and says when none answers', async ({ page }) => {
    await standIn(page, lines())
    await page.goto('/admin/logs')
    await page.getByLabel(/^find$/i).fill('sweep')
    await expect(items(page)).toHaveCount(1)
    await expect(items(page).first()).toContainText('sweep done')

    await page.getByLabel(/^find$/i).fill('nothing says this')
    await expect(page.getByText(/no line answers that filter/i)).toBeVisible()
  })

  test('downloads what the filters keep, as text or as JSON lines', async ({ page }) => {
    await standIn(page, lines())
    await page.goto('/admin/logs')
    await page.getByLabel(/^level$/i).selectOption('error')
    await page.getByLabel(/^find$/i).fill('failed')
    await expect(items(page)).toHaveCount(1)

    const text = page.getByRole('link', { name: /^text$/i })
    await expect(text).toHaveAttribute('href', '/api/v1/admin/logs?level=error&q=failed&format=text')
    await expect(text).toHaveAttribute('download')
    await expect(page.getByRole('link', { name: /^json lines$/i })).toHaveAttribute(
      'href',
      '/api/v1/admin/logs?level=error&q=failed&format=json',
    )
  })

  test('stops reading new lines when it is not followed', async ({ page }) => {
    const asked = await standIn(page, lines())
    await page.goto('/admin/logs')
    await expect(items(page)).toHaveCount(4)

    // Followed: read again within the five seconds.
    const before = asked.length
    await expect.poll(() => asked.length, { timeout: 8_000 }).toBeGreaterThan(before)

    const follow = page.getByRole('switch', { name: /^follow$/i })
    await expect(follow).toHaveAttribute('aria-checked', 'true')
    await follow.click()
    await expect(follow).toHaveAttribute('aria-checked', 'false')
    const paused = asked.length
    await page.waitForTimeout(6_000)
    expect(asked.length).toBe(paused)
  })

  test('reads the server’s own lines, newest first, each with its level, its module and its message', async ({
    page,
  }) => {
    // The real log, not the stand-in: whatever the server wrote since it
    // started — at the least that it was listening.
    const read = await page.request.get('/api/v1/admin/logs?limit=20&level=trace')
    expect(read.ok()).toBe(true)
    const log = await read.json()
    expect(log.capacity).toBeGreaterThan(0)
    expect(typeof log.instance).toBe('string')
    expect(log.entries.length).toBeGreaterThan(0)
    const seqs = log.entries.map((entry: Entry) => entry.seq)
    expect(seqs).toEqual([...seqs].sort((a, b) => b - a))
    for (const entry of log.entries as Entry[]) {
      expect(GRAVITY).toContain(entry.level)
      expect(entry.target.length).toBeGreaterThan(0)
    }

    await page.goto('/admin/logs')
    await page.getByLabel(/^level$/i).selectOption('trace')
    await expect(items(page).first()).toBeVisible()
  })
})
