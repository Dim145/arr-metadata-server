import { expect, test } from '@playwright/test'

/**
 * The feeds: the schedule as a calendar a phone subscribes to, one work's
 * dates as another, and what arrives and what airs as Atom. Read under
 * public browsing as the pages they mirror are.
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

  test('what arrives and what airs are feeds', async ({ request }) => {
    for (const path of ['/api/v1/feed/added.atom', '/api/v1/feed/airing.atom']) {
      const response = await request.get(path)
      expect(response.status(), path).toBe(200)
      expect(response.headers()['content-type'], path).toContain('application/atom+xml')
      const body = await response.text()
      expect(body, path).toMatch(/<feed xmlns="http:\/\/www\.w3\.org\/2005\/Atom"( xml:lang="[A-Za-z-]+")?>/)
      expect(body, path).toMatch(new RegExp(`<link rel="self" href="https?://[^"]+${path}"/>`))
    }
    expect(await (await request.get('/api/v1/feed/added.atom')).text()).toContain('<entry>')
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
