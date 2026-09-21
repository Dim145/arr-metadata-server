import { expect, test } from '@playwright/test'

/**
 * What public browsing actually opens, over real HTTP.
 *
 * The allowlist is unit-tested in Rust, but that tests a function. This tests
 * the server: that turning `AMS_PUBLIC_BROWSE` on publishes the catalogue and
 * nothing about how the machine is run. It is the check worth having, because
 * the cost of getting it wrong is someone's configuration on the open internet.
 */

const OPEN = ['/api/v1/items', '/api/v1/stats', '/api/v1/auth/me']

const CLOSED = [
  '/api/v1/settings',
  '/api/v1/jobs',
  '/api/v1/audit',
  '/api/v1/clients',
  '/api/v1/fields',
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
    ]

    for (const attempt of attempts) {
      const response = await attempt
      expect([401, 403]).toContain(response.status())
    }
  })

  test('is offered a way in rather than an admin link', async ({ page }) => {
    await page.goto('/')

    await expect(page.getByRole('link', { name: /sign in|se connecter/i })).toBeVisible()
    await expect(page.getByRole('link', { name: /^(admin|administration)$/i })).toHaveCount(0)
  })
})
