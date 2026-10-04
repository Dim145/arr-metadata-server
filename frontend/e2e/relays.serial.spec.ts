import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

/**
 * The TheTVDB and AniList relays, as the interface shows them and as their
 * doors answer without a service behind them: the switches on the access
 * page, the sign-in a TheTVDB client makes with a key issued here, and the
 * refusals — a surface switched off, a member kept out, a key that is not
 * one. What the relays hand on, and what they write into the answers, is
 * scripts/e2e-relays.sh's, against a stand-in for both services.
 *
 * Serial: switching a relay off changes how the whole server answers.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

test.describe.configure({ mode: 'serial' })

async function signIn(page: Page, username: string, password: string) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(username)
  await page.getByLabel(/password|mot de passe/i).fill(password)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
}

async function setting(request: APIRequestContext, key: string, value: string) {
  const answer = await request.put('/api/v1/settings/server/-', { data: { key, value } })
  expect(answer.ok(), `${key}=${value}`).toBe(true)
}

async function forget(request: APIRequestContext, username: string) {
  const found = await request.get(`/api/v1/users?term=${encodeURIComponent(username)}`)
  for (const user of ((await found.json()) as { users: { id: string; username: string }[] }).users) {
    if (user.username === username) await request.delete(`/api/v1/users/${user.id}`)
  }
}

const unique = (what: string) => `e2e_${what}_${Date.now().toString(36)}`

test.describe('the relays', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test.beforeEach(async ({ page }) => {
    await signIn(page, USERNAME!, PASSWORD!)
    await page.waitForURL('**/admin')
  })

  test('the access page lists both relays, each with its address and a switch', async ({ page }) => {
    await page.goto('/admin/access')
    const apis = page.getByRole('list', { name: /who calls the apis|qui appelle les api/i })
    await expect(apis).toContainText('/v4/*')
    await expect(apis).toContainText('graphql.anilist.co')
    await expect(page.getByRole('switch', { name: /thetvdb/i })).toHaveAttribute('aria-checked', 'true')
    await expect(page.getByRole('switch', { name: /anilist/i })).toHaveAttribute('aria-checked', 'true')

    // Read back by the API with their policies, the deployment's own.
    const access = (await (await page.request.get('/api/v1/admin/access')).json()) as {
      apis: { api: string; enabled: boolean; policy: string }[]
    }
    const named = Object.fromEntries(access.apis.map((x) => [x.api, x]))
    expect(named.tvdb?.enabled).toBe(true)
    expect(named.anilist?.enabled).toBe(true)
    expect(['apikey', 'allowlist', 'open']).toContain(named.tvdb?.policy)
    expect(['apikey', 'allowlist', 'open']).toContain(named.anilist?.policy)
  })

  test('the configuration page shows both relays’ policies beside the others', async ({ page }) => {
    await page.goto('/admin/settings')
    const policy = page.locator('#settings-policy')
    await expect(policy).toContainText('/v4/*')
    await expect(policy).toContainText('graphql.anilist.co')
  })

  test('the TheTVDB relay switched off answers 503 and asks nothing; back on, it answers again', async ({ page }) => {
    await page.goto('/admin/access')
    const tvdb = page.getByRole('switch', { name: /thetvdb/i })
    await expect(tvdb).toHaveAttribute('aria-checked', 'true')
    try {
      await tvdb.click()
      await expect(tvdb).toHaveAttribute('aria-checked', 'false')

      const off = await page.request.get('/v4/series/81189')
      expect(off.status()).toBe(503)
      expect(((await off.json()) as { error: string }).error).toBe('surface_disabled')
      // The other relay keeps answering: switched off is one switch.
      const sonarr = await page.request.get('/v1/tvdb/shows/en/81189')
      expect(sonarr.status()).not.toBe(503)
    } finally {
      await setting(page.request, 'api.tvdb', 'true')
    }
    // Back on: refused for another reason at most — no key to stand in with,
    // or TheTVDB's own answer — never as switched off.
    const on = await page.request.get('/v4/series/81189')
    expect(on.status() === 503 ? ((await on.json()) as { error: string }).error : 'answered').not.toBe('surface_disabled')
    await page.reload()
    await expect(page.getByRole('switch', { name: /thetvdb/i })).toHaveAttribute('aria-checked', 'true')
  })

  test('a TheTVDB client signs in with a key issued here, and is answered that key as its token', async ({
    page,
    browser,
  }) => {
    // A name of its own: a key's name is unique, and a run that failed
    // before its `finally` would otherwise leave this one taken.
    const issued = await page.request.post('/api/v1/clients', { data: { name: unique('tvdb'), scopes: ['read'] } })
    expect(issued.status(), await issued.text()).toBe(201)
    // The key's record, flattened, with the key itself beside it — shown once.
    const { key, id } = (await issued.json()) as { key: string; id: string }
    const bare = await browser.newContext()
    try {
      // A sign-in that is not one.
      const nothing = await bare.request.post('/v4/login', { data: { pin: '1234' } })
      expect(nothing.status()).toBe(400)

      const answer = await bare.request.post('/v4/login', { data: { apikey: key } })
      if (answer.status() === 503) {
        // The suite's server has no TheTVDB key of its own to stand in with.
        expect(((await answer.json()) as { error: string }).error).toBe('provider_not_configured')
      } else {
        expect(answer.status()).toBe(200)
        const body = (await answer.json()) as { status: string; data: { token: string } }
        expect(body.status).toBe('success')
        expect(body.data.token).toBe(key)
      }

      const config = (await (await page.request.get('/api/v1/settings')).json()) as { tvdbPolicy: string }
      if (config.tvdbPolicy === 'apikey') {
        // Somebody else's key is nobody's here.
        const refused = await bare.request.post('/v4/login', { data: { apikey: 'not-a-key-of-this-server' } })
        expect(refused.status()).toBe(401)
        // Nor is a request with no credential at all.
        expect((await bare.request.get('/v4/series/81189')).status()).toBe(401)
      }
      // A write is never relayed, whoever asks.
      const write = await bare.request.post('/v4/user/favorites', {
        headers: { authorization: `Bearer ${key}` },
        data: { series: 81189 },
      })
      expect(write.status()).toBe(403)
    } finally {
      await bare.close()
      await page.request.delete(`/api/v1/clients/${id}`)
    }
  })

  test('a member’s key is kept off the relays until members are let in', async ({ page, browser }) => {
    const username = unique('relays')
    const opened = await page.request.post('/api/v1/users', { data: { username, role: 'member' } })
    expect(opened.status()).toBe(201)
    const { password } = (await opened.json()) as { password: string }
    const guest = await browser.newContext()
    try {
      const them = await guest.newPage()
      await signIn(them, username, password)
      await them.waitForURL((url) => url.pathname === '/')
      const issued = await them.request.post('/api/v1/account/keys', { data: { name: 'e2e relays' } })
      expect(issued.status()).toBe(201)
      const { secret } = (await issued.json()) as { secret: string }

      const bare = await browser.newContext()
      try {
        const refused = await bare.request.post('/v4/login', { data: { apikey: secret } })
        expect(refused.status()).toBe(403)
        expect(((await refused.json()) as { error: string }).error).toBe('relay_not_for_members')
        // The account page says so too.
        const keptOut = (await (await them.request.get('/api/v1/account/keys')).json()) as { relay: boolean }
        expect(keptOut.relay).toBe(false)

        await setting(page.request, 'api.tmdbMembers', 'true')
        const let_in = await bare.request.post('/v4/login', { data: { apikey: secret } })
        expect(let_in.status()).not.toBe(403)
        const letIn = (await (await them.request.get('/api/v1/account/keys')).json()) as { relay: boolean }
        expect(letIn.relay).toBe(true)
      } finally {
        await bare.close()
      }
    } finally {
      await setting(page.request, 'api.tmdbMembers', 'false')
      await guest.close()
      await forget(page.request, username)
    }
  })

  test('the interface’s door keeps its page at the root: AniList is reached by its name alone', async ({ page }) => {
    const root = await page.request.post('/', { data: { query: '{ Media(id: 1) { id } }' } })
    expect(root.headers()['content-type'] ?? '').not.toContain('application/json')
    // Whereas TheTVDB's paths are the relay's on this door too: answered as
    // the relay answers — a document, or a refusal — never as the page.
    const relayed = await page.request.get('/v4/series/81189')
    expect(relayed.headers()['content-type'] ?? '').toContain('application/json')
  })
})
