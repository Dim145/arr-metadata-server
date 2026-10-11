import { expect, test } from '@playwright/test'
import { escaped, pattern } from './support/regex'

/**
 * The feeds: the schedule as a calendar a phone subscribes to, one work's
 * dates as another, and what arrives and what airs as Atom. Read under
 * public browsing as the pages they mirror are. A film coming out, made for
 * the purpose, is followed through them in `calendar.serial.spec.ts`.
 */
test.describe('feeds', () => {
  test('the schedule is a calendar', async ({ request }) => {
    const response = await request.get('/api/v1/calendar.ics')
    expect(response.status()).toBe(200)
    expect(response.headers()['content-type']).toContain('text/calendar')
    const body = await response.text()
    expect(body.startsWith('BEGIN:VCALENDAR\r\nVERSION:2.0\r\n')).toBe(true)
    expect(body).toContain('X-WR-CALNAME:')
    expect(body.endsWith('END:VCALENDAR\r\n')).toBe(true)
    // No content line longer than the format allows, whatever the titles.
    for (const line of body.split('\r\n')) {
      expect(Buffer.byteLength(line), line).toBeLessThanOrEqual(75)
    }
    // Every event an episode or a film's release, each once.
    const uids = body.replace(/\r\n /g, '').match(/\r\nUID:[^\r\n]+/g) ?? []
    for (const uid of uids) {
      expect(uid).toMatch(/^\r\nUID:(episode-.+|release-.+-(cinema|digital|physical))@arr-metadata-server$/)
    }
    expect(new Set(uids).size).toBe(uids.length)
    // The window is bounded as the schedule's is.
    expect((await request.get('/api/v1/calendar.ics?pastDays=40&futureDays=40')).status()).toBe(400)
    expect((await request.get('/api/v1/calendar.ics?pastDays=-1')).status()).toBe(400)
  })

  test('a work has a calendar of its own', async ({ request }) => {
    // A series with a dated episode, among the first few: one with none
    // makes an empty calendar, which is right but proves nothing.
    const { items } = await (await request.get('/api/v1/items?kind=series&limit=10')).json()
    let body = ''
    for (const item of items as { id: string }[]) {
      const response = await request.get(`/api/v1/items/${item.id}/calendar.ics`)
      expect(response.status()).toBe(200)
      expect(response.headers()['content-type']).toContain('text/calendar')
      body = await response.text()
      if (body.includes('BEGIN:VEVENT')) break
    }
    test.skip(!body.includes('BEGIN:VEVENT'), 'holds no series with a dated episode')
    expect(body).toMatch(/\r\nUID:episode-[^\r\n]+@arr-metadata-server\r\n/)
    expect(body).toMatch(/\r\nDTSTART(;VALUE=DATE)?:\d{8}/)
    // A work that is not there is not a calendar either.
    expect((await request.get('/api/v1/items/no-such-work/calendar.ics')).status()).toBe(404)
  })

  test('a film’s calendar is its releases, a day each', async ({ request }) => {
    // A film with a release date among the first few: the catalogue's came
    // out long ago, which a calendar of its own keeps all the same.
    const { items } = await (await request.get('/api/v1/items?kind=movie&limit=10')).json()
    let body = ''
    for (const item of items as { id: string }[]) {
      const response = await request.get(`/api/v1/items/${item.id}/calendar.ics?language=fr`)
      expect(response.status()).toBe(200)
      body = (await response.text()).replace(/\r\n /g, '')
      if (body.includes('BEGIN:VEVENT')) break
    }
    test.skip(!body.includes('BEGIN:VEVENT'), 'holds no film with a release date')
    expect(body).toMatch(/\r\nUID:release-[^\r\n]+-(cinema|digital|physical)@arr-metadata-server\r\n/)
    expect(body).toMatch(/\r\nDTSTART;VALUE=DATE:\d{8}\r\n/)
    // Named for what comes out, in the language asked for.
    expect(body).toMatch(/\r\nSUMMARY:[^\r\n]+ — sortie (en salles|numérique|physique)/)
    // One event a day: two kinds on the same day are one release.
    const days = body.match(/\r\nDTSTART;VALUE=DATE:\d{8}/g) ?? []
    expect(new Set(days).size).toBe(days.length)
  })

  test('what arrives and what airs are feeds', async ({ request }) => {
    for (const path of ['/api/v1/feed/added.atom', '/api/v1/feed/airing.atom']) {
      const response = await request.get(path)
      expect(response.status(), path).toBe(200)
      expect(response.headers()['content-type'], path).toContain('application/atom+xml')
      const body = await response.text()
      expect(body, path).toMatch(/<feed xmlns="http:\/\/www\.w3\.org\/2005\/Atom"( xml:lang="[A-Za-z-]+")?>/)
      expect(body, path).toMatch(pattern(`<link rel="self" href="https?://[^"]+${escaped(path)}"/>`))
    }
    expect(await (await request.get('/api/v1/feed/added.atom')).text()).toContain('<entry>')
    // What airs: an entry an episode, or one a day a film comes out.
    const airing = await (await request.get('/api/v1/feed/airing.atom')).text()
    for (const [, id] of airing.matchAll(/<entry>\s*<id>([^<]+)<\/id>/g)) {
      expect(id).toMatch(/^urn:ams:(episode:.+|release:.+:(cinema|digital|physical))$/)
    }
  })

  test('the footer offers them, and the schedule a subscription', async ({ page }) => {
    await page.goto('/calendar')
    const subscribe = page.getByRole('link', { name: /^(subscribe|s’abonner)$/i })
    await expect(subscribe).toBeVisible()
    // `webcal:` for a page served over http, `webcals:` over https.
    await expect(subscribe).toHaveAttribute('href', /^webcals?:\/\/[^/]+\/api\/v1\/calendar\.ics/)
    const feeds = page.getByRole('navigation', { name: /^(feeds|flux)$/i })
    await expect(feeds.getByRole('link')).toHaveCount(3)
    await expect(feeds.getByRole('link', { name: /recently added|ajouts récents/i })).toHaveAttribute(
      'href',
      '/api/v1/feed/added.atom',
    )
  })
})
