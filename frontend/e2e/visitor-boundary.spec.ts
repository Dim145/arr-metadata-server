import { expect, test } from '@playwright/test'

/**
 * What public browsing actually opens, over real HTTP.
 *
 * The allowlist is unit-tested in Rust, but that tests a function. This tests
 * the server: that turning `AMS_PUBLIC_BROWSE` on publishes the catalogue and
 * nothing about how the machine is run. It is the check worth having, because
 * the cost of getting it wrong is someone's configuration on the open internet.
 */

/** A week from now, as the schedule asks for one. */
function week() {
  const from = new Date()
  const to = new Date(from.getTime() + 7 * 86_400_000)
  return `from=${encodeURIComponent(from.toISOString())}&to=${encodeURIComponent(to.toISOString())}`
}

const OPEN = [
  '/api/v1/items',
  '/api/v1/stats',
  '/api/v1/auth/me',
  '/api/v1/facets',
  `/api/v1/calendar?${week()}`,
  '/api/v1/seasons/2019/spring',
]

const CLOSED = [
  '/api/v1/settings',
  '/api/v1/jobs',
  '/api/v1/audit',
  '/api/v1/clients',
  '/api/v1/fields',
  // What TMDB lists for a season is asked of TMDB, on this server's key: for
  // whoever maintains the catalogue, not for anyone passing.
  '/api/v1/seasons/2019/spring/candidates',
]

test.describe('a visitor with no credential', () => {
  test('reads the catalogue', async ({ request }) => {
    for (const path of OPEN) {
      const response = await request.get(path)
      expect(response.status(), `${path} should be readable`).toBe(200)
    }
  })

  test('is told it may not write', async ({ request }) => {
    const me = await (await request.get('/api/v1/auth/me')).json()

    expect(me.canWrite).toBe(false)
    expect(me.isAdmin).toBe(false)
  })

  test('cannot read how the server is run', async ({ request }) => {
    for (const path of CLOSED) {
      const response = await request.get(path)
      expect([401, 403], `${path} answered ${response.status()}`).toContain(response.status())
    }
  })

  test('cannot read a work’s snapshots or overrides', async ({ request }) => {
    const { items } = await (await request.get('/api/v1/items?limit=1')).json()
    const id = items[0].id

    for (const path of [`/api/v1/items/${id}/snapshots`, `/api/v1/items/${id}/overrides`]) {
      const response = await request.get(path)
      expect([401, 403], `${path} answered ${response.status()}`).toContain(response.status())
    }
  })

  test('cannot change anything', async ({ request }) => {
    const attempts = [
      request.post('/api/v1/items', { data: { kind: 'movie', title: 'Trespass' } }),
      request.post('/api/v1/cache/clear'),
      request.post('/api/v1/export/nfo'),
      request.post('/api/v1/datasets/imdb/import'),
    ]

    for (const attempt of attempts) {
      const response = await attempt
      expect([401, 403]).toContain(response.status())
    }
  })

  test('is not told which works failed to refresh', async ({ request }) => {
    // How the server is doing is its administrators' business: to a visitor
    // the filter is ignored, and the list is the whole catalogue.
    const all = await (await request.get('/api/v1/items?limit=1')).json()
    const failed = await (await request.get('/api/v1/items?limit=1&refreshFailed=true')).json()

    expect(failed.total).toBe(all.total)
  })

  test('is not told how a work’s refresh went, when the next is due, or what is locked', async ({
    request,
  }) => {
    // An error can name a provider's address or the host this server reaches
    // it on; the rest is how the catalogue is maintained, not what it holds.
    const { items } = await (await request.get('/api/v1/items?limit=50')).json()
    const told = (items as Record<string, unknown>[]).filter(
      (item) => 'refreshError' in item || 'refreshAfter' in item || 'lockedFields' in item,
    )
    expect(told.map((item) => item.title)).toEqual([])
  })

  test('reads a season chart, and not what TMDB lists for the season', async ({ request }) => {
    expect((await request.get('/api/v1/seasons/2019/spring')).status()).toBe(200)

    // Refused at the door, before any handler: the allowlist opens the chart
    // and not one segment more. A 403 would be the handler turning away a
    // caller the door had let in.
    expect((await request.get('/api/v1/seasons/2019/spring/candidates')).status()).toBe(401)
  })

  test('reads a person by their id, and nothing beneath it', async ({ request }) => {
    // The allowlist opens `/people/{id}` and not one segment more.
    for (const path of ['/api/v1/people/1/credits', '/api/v1/people/1%2F..%2Fsettings']) {
      const response = await request.get(path)
      expect([400, 401, 403, 404], `${path} answered ${response.status()}`).toContain(response.status())
    }
  })

  test('is answered with a policy that says where a page may be loaded from', async ({
    request,
  }) => {
    const response = await request.get('/')
    const csp = response.headers()['content-security-policy'] ?? ''

    // `frame-ancestors` is the one that earns its place on a server sitting on
    // a home network: it is what stops a page elsewhere framing this one and
    // borrowing an administrator's clicks.
    expect(csp).toContain("frame-ancestors 'none'")
    expect(csp).toContain("default-src 'self'")
    expect(csp).toContain("object-src 'none'")
    // One exception, for a trailer someone asked to play.
    expect(csp).toContain('frame-src https://www.youtube-nocookie.com')

    expect(response.headers()['referrer-policy']).toBe('no-referrer')
    expect(response.headers()['x-content-type-options']).toBe('nosniff')
  })

  test('cannot make the TMDB relay write with this server’s key', async ({ request }) => {
    // The relay exists for Jellyseerr and Plex, which only read. Forwarding a
    // write would let any key issued here rate a film as the operator.
    for (const attempt of [
      request.post('/3/movie/550/rating', { data: { value: 1 } }),
      request.delete('/3/list/1/clear'),
    ]) {
      const response = await attempt
      expect([401, 403, 405]).toContain(response.status())
    }
  })

  test('reaches TMDB’s v4 lists only with a credential, and IMDb’s lists only by name', async ({
    request,
  }) => {
    // The v4 relay is a surface like the v3 one: closed to a visitor.
    expect((await request.get('/4/list/8136')).status()).toBe(401)
    // Radarr's IMDb lists: the route exists, and takes only what Radarr can
    // ask for, so it is never a way to reach the metadata service by any path.
    // Knocking as Radarr does: the arr surface remembers its last caller's
    // agent per address, and another test reads it back as Sonarr's.
    const asRadarr = { headers: { 'user-agent': 'Sonarr/4.0.20.3014 (e2e)' } }
    expect((await request.get('/v1/list/imdb/ls012345678', asRadarr)).status()).toBe(400)
    expect((await request.get('/v1/list/imdb/..%2Fmovie%2F238', asRadarr)).status()).toBe(400)
  })

  test('is offered a way in rather than an admin link', async ({ page }) => {
    await page.goto('/')

    // On a phone the way in is inside the menu; see catalogue.spec.ts.
    const menu = page.getByRole('button', { name: /^(menu)$/i })
    if (await menu.isVisible()) {
      await menu.click()
    }

    await expect(page.getByRole('link', { name: /sign in|se connecter/i })).toBeVisible()
    await expect(page.getByRole('link', { name: /^(admin|administration)$/i })).toHaveCount(0)
  })
})
