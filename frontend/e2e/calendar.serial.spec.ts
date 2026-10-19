import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Page } from '@playwright/test'

/**
 * The films in the schedule: one coming out this week is listed on its day —
 * in the week, in the month, on the front page's strip — and in the feeds.
 *
 * Made here rather than looked for: the catalogue's films came out long ago.
 * A film is put in by hand, out in cinemas and digitally today and on disc in
 * months, and taken out again at the end. Server-wide, because the schedule,
 * its calendar and its feed are everyone's: a film that came and went while
 * another spec polled the calendar would change what that spec read.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

/** What a film made here is called, so one a failed run left is found again. */
const PREFIX = 'Sortie e2e'

const WCAG = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

async function scan(page: Page, label: string) {
  // Mid-fade, text is measured at part of its opacity.
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'))
  const results = await new AxeBuilder({ page }).withTags(WCAG).analyze()
  const violations = results.violations.map((v) => ({
    rule: v.id,
    help: v.help,
    where: v.nodes.slice(0, 3).map((n) => n.target.join(' ')),
  }))
  expect(violations, `${label} has accessibility violations`).toEqual([])
}

/** `2026-10-11`: a day as this machine has it, which is the browser's too. */
function localDay(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

type Episode = { airDate?: string; airDateUtc?: string }
type Entry =
  | { kind: 'episode'; workId: string; episode: Episode }
  | { kind: 'release'; workId: string; date: string; releaseTypes: string[] }

/** The day the interface files an episode under: its moment's, here, or its date. */
function episodeDay(episode: Episode): string | undefined {
  return episode.airDateUtc && /^\d{4}-\d{2}-\d{2}T/.test(episode.airDateUtc)
    ? localDay(new Date(episode.airDateUtc))
    : episode.airDate
}

test.describe('the films in the schedule', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('a film coming out this week is listed on its day, and in the feeds', async ({ page, request }) => {
    test.slow()
    await signIn(page)

    // A run that failed before taking its film out left it: out first.
    const left = (await (
      await page.request.get(`/api/v1/items?kind=movie&term=${encodeURIComponent(PREFIX)}&limit=50`)
    ).json()) as { items: { id: string; title: string }[] }
    for (const work of left.items) {
      if (work.title.startsWith(PREFIX)) await page.request.delete(`/api/v1/items/${work.id}`)
    }

    const now = new Date()
    const today = localDay(now)
    const months = localDay(new Date(now.getTime() + 200 * 86_400_000))
    const title = `${PREFIX} ${Date.now()}`
    const created = await page.request.post('/api/v1/items', {
      data: { kind: 'movie', title, year: now.getFullYear(), inCinemas: today },
    })
    expect(created.status()).toBe(201)
    const { id } = (await created.json()) as { id: string }

    // Today, from yesterday to the day after tomorrow: wide enough for a
    // release placed at midnight UTC, wherever this machine is.
    const from = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1).toISOString()
    const to = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 2).toISOString()
    const schedule = async () =>
      (await (
        await request.get(`/api/v1/calendar?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`)
      ).json()) as { entries: Entry[]; works: { id: string }[] }

    try {
      for (const [field, value] of [
        ['digitalRelease', today],
        ['physicalRelease', months],
      ]) {
        const set = await page.request.put(`/api/v1/items/${id}/overrides`, { data: { field, value } })
        expect(set.ok(), field).toBe(true)
      }

      // One release today with both its kinds, among the episodes and none
      // of them; the disc, months away, nowhere.
      const listing = await schedule()
      expect(listing.entries.filter((e) => e.workId === id)).toEqual([
        { kind: 'release', workId: id, date: today, releaseTypes: ['cinema', 'digital'] },
      ])
      expect(listing.works.some((w) => w.id === id)).toBe(true)
      const before = listing.entries.filter((e) => e.kind === 'episode' && episodeDay(e.episode) === today).length

      // The week: a row under today, with what comes out said.
      await page.goto('/calendar')
      const row = page.locator(`#day-${today} li`).filter({ has: page.locator(`a[href="/work/${id}"]`) })
      await expect(row).toBeVisible()
      await expect(row.getByText(/^(cinema release|sortie en salles)$/i)).toBeVisible()
      await expect(row.getByText(/^(digital release|sortie numérique)$/i)).toBeVisible()
      // Counted apart from the episodes, in the heading and on the day's mark.
      await expect(page.getByText(/ · \d+ (releases?|sorties?)$/)).toBeVisible()
      const days = page.getByRole('list', { name: /days of the week|jours de la semaine/i })
      await expect(days.locator(`a[href="#day-${today}"]`)).toHaveAttribute('aria-label', /\d+ (releases?|sorties?)$/)
      await scan(page, 'the week, with a film in it')

      // The month: in today's cell after its episodes — three at most before
      // the rest is a step into the week.
      await page.goto('/calendar?view=month')
      const cell = page.getByRole('list', { name: /days of the month|jours du mois/i }).locator(`li[data-day="${today}"]`)
      if (before < 3) await expect(cell.locator(`a[href="/work/${id}"]`)).toBeVisible()
      else await expect(cell.getByRole('link', { name: /^\+ \d+ (more|autres?)$/ })).toBeVisible()
      await scan(page, 'the month, with a film in it')

      // On a phone, the day's mark counts it.
      await page.setViewportSize({ width: 375, height: 812 })
      const mark = page.getByRole('list', { name: /days of the month|jours du mois/i }).locator(`li[data-day="${today}"] a`)
      await expect(mark).toHaveAttribute('aria-label', /\d+ (releases?|sorties?)$/)
      await page.setViewportSize({ width: 1440, height: 900 })

      // The front page's week: a frame of its own, after today's episodes.
      await page.goto('/')
      const strip = page.getByRole('region', { name: /^(airing this week|à l’antenne cette semaine)$/i })
      if (before < 16) {
        const frame = strip.locator(`a[href="/work/${id}"]`)
        await expect(frame).toBeVisible()
        await expect(frame).toContainText(/cinema release|sortie en salles/i)
      }

      // The schedule as a calendar, in the language asked: an all-day event
      // named for what comes out, under a UID that is the film's and the kind's.
      const unfold = (text: string) => text.replace(/\r\n /g, '')
      const french = unfold(await (await request.get('/api/v1/calendar.ics?language=fr')).text())
      expect(french).toContain(`\r\nUID:release-${id}-cinema@arr-metadata-server\r\n`)
      expect(french).toContain(`\r\nSUMMARY:${title} — sortie en salles et numérique\r\n`)
      expect(french).toContain(`\r\nDTSTART;VALUE=DATE:${today.replaceAll('-', '')}\r\n`)
      expect(french).not.toContain(`release-${id}-physical@`)
      const english = unfold(await (await request.get('/api/v1/calendar.ics?language=en')).text())
      expect(english).toContain(`\r\nSUMMARY:${title} — cinema and digital release\r\n`)

      // The film's own calendar: every release, the disc too.
      const own = unfold(await (await request.get(`/api/v1/items/${id}/calendar.ics?language=en`)).text())
      expect(own.match(/\r\nBEGIN:VEVENT\r\n/g)).toHaveLength(2)
      expect(own).toContain(`\r\nUID:release-${id}-physical@arr-metadata-server\r\n`)
      expect(own).toContain(`\r\nSUMMARY:${title} — physical release\r\n`)

      // What airs, as a feed: an entry for the day it comes out.
      const feed = await (await request.get('/api/v1/feed/airing.atom?language=fr')).text()
      expect(feed).toContain(`<id>urn:ams:release:${id}:cinema</id>`)
      expect(feed).toContain(`<title>${title} — sortie en salles et numérique</title>`)
    } finally {
      await page.request.delete(`/api/v1/items/${id}`)
    }

    // Taken out, it is out of the schedule too.
    expect((await schedule()).entries.some((e) => e.workId === id)).toBe(false)
  })
})
