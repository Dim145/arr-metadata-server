import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

/**
 * The way in: a private site, sign-ups in each of their modes, an API
 * switched off.
 *
 * Each of these changes how the whole server answers, so they run in a
 * project of their own, after every other has finished (see the Playwright
 * configuration), one at a time, and each puts back what it changed.
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

/** What the server says now, to put it back afterwards. */
async function current(request: APIRequestContext) {
  const answer = await request.get('/api/v1/admin/access')
  expect(answer.ok()).toBe(true)
  return (await answer.json()) as { site: string; registration: string; registrationRole: string }
}

async function forget(request: APIRequestContext, username: string) {
  const found = await request.get(`/api/v1/users?term=${encodeURIComponent(username)}`)
  for (const user of ((await found.json()) as { users: { id: string; username: string }[] }).users) {
    if (user.username === username) await request.delete(`/api/v1/users/${user.id}`)
  }
}

const unique = (what: string) => `e2e_${what}_${Date.now().toString(36)}`

test.describe('the way in', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test.beforeEach(async ({ page }) => {
    await signIn(page, USERNAME!, PASSWORD!)
    await page.waitForURL('**/admin')
  })

  test('a private site sends a stranger to the door, and back where they were after it', async ({
    page,
    browser,
  }) => {
    const before = await current(page.request)
    const stranger = await browser.newContext()
    try {
      await setting(page.request, 'site.access', 'private')
      const them = await stranger.newPage()
      await them.goto('/browse')
      await them.waitForURL(/\/login\?next=%2Fbrowse$/)
      await expect(them.getByText(/private: sign in|privé : connectez-vous/i)).toBeVisible()
      // Nothing to go back to on a private site.
      await expect(them.getByRole('link', { name: /back|retour/i })).toHaveCount(0)

      // The API refuses the catalogue to them too, not only the pages.
      expect((await them.request.get('/api/v1/items?limit=1')).status()).toBe(401)

      await them.getByLabel(/username|identifiant/i).fill(USERNAME!)
      await them.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
      await them.getByRole('button', { name: /sign in|se connecter/i }).click()
      await them.waitForURL((url) => url.pathname === '/browse')
    } finally {
      await stranger.close()
      await setting(page.request, 'site.access', before.site)
    }
  })

  test('by invitation, a link opens an account once, with the role it gives', async ({ page, browser }) => {
    const before = await current(page.request)
    const username = unique('invited')
    const guest = await browser.newContext()
    let made: string | undefined
    try {
      await setting(page.request, 'registration.mode', 'invite')
      const answer = await page.request.post('/api/v1/invitations', {
        data: { role: 'member', maxUses: 1, days: 1, note: 'e2e' },
      })
      expect(answer.status()).toBe(201)
      const { code, invitation } = (await answer.json()) as { code: string; invitation: { id: string } }
      made = invitation.id

      const them = await guest.newPage()
      // Without the code, the page asks for it and the server agrees.
      await them.goto('/register')
      await expect(them.getByLabel(/^(invitation code|code d’invitation)$/i)).toBeVisible()

      await them.goto(`/register#invite=${code}`)
      await expect(them.getByText(/valid invitation|invitation valide/i)).toBeVisible()
      // Read, then out of the address bar and the history.
      await expect(them).toHaveURL(/\/register$/)
      await them.getByLabel(/^(username|identifiant)$/i).fill(username)
      await them.getByLabel(/^(password|mot de passe)$/i).fill('an-e2e-password-long-enough')
      await them.getByRole('button', { name: /create my account|créer mon compte/i }).click()
      await them.waitForURL((url) => url.pathname === '/')

      const me = (await (await them.request.get('/api/v1/auth/me')).json()) as { user?: { role: string } }
      expect(me.user?.role).toBe('member')

      // Used once, used up.
      const again = await them.request.post('/api/v1/auth/register', {
        data: { username: `${username}_2`, password: 'an-e2e-password-long-enough', code },
      })
      expect(again.status()).toBe(403)
      expect(((await again.json()) as { error: string }).error).toBe('invitation_invalid')
    } finally {
      await guest.close()
      if (made) await page.request.delete(`/api/v1/invitations/${made}`)
      await forget(page.request, username)
      await setting(page.request, 'registration.mode', before.registration)
    }
  })

  test('with approval, a sign-up waits until an administrator lets it in', async ({ page, browser }) => {
    const before = await current(page.request)
    const username = unique('waiting')
    const guest = await browser.newContext()
    try {
      await setting(page.request, 'registration.mode', 'approval')
      const them = await guest.newPage()
      await them.goto('/login')
      await them.getByRole('link', { name: /ask for one|en demander un/i }).click()
      await them.waitForURL('**/register')
      await them.getByLabel(/^(username|identifiant)$/i).fill(username)
      await them.getByLabel(/^(password|mot de passe)$/i).fill('an-e2e-password-long-enough')
      await them.getByRole('button', { name: /create my account|créer mon compte/i }).click()
      await expect(them.getByRole('heading', { name: /waiting|attend/i })).toBeVisible()

      // Not in yet.
      await signIn(them, username, 'an-e2e-password-long-enough')
      await expect(them.getByText(/waiting for an administrator|attend qu’un administrateur/i)).toBeVisible()

      // The sidebar says someone waits; the members page lets them in.
      await page.goto('/admin/users')
      await expect(page.getByRole('link', { name: /members|membres/i }).first()).toContainText(/\d/)
      const found = await page.request.get(`/api/v1/users?term=${username}`)
      const { users } = (await found.json()) as { users: { id: string; status: string }[] }
      expect(users[0]?.status).toBe('pending')
      const approved = await page.request.post('/api/v1/users/bulk', {
        data: { ids: [users[0].id], action: 'status', value: 'active' },
      })
      expect(approved.ok()).toBe(true)

      await signIn(them, username, 'an-e2e-password-long-enough')
      await them.waitForURL((url) => url.pathname === '/')
    } finally {
      await guest.close()
      await forget(page.request, username)
      await setting(page.request, 'registration.mode', before.registration)
    }
  })

  test('closed, the sign-up page says so and the sign-in page offers nothing', async ({ page, browser }) => {
    const before = await current(page.request)
    const guest = await browser.newContext()
    try {
      await setting(page.request, 'registration.mode', 'closed')
      const them = await guest.newPage()
      await them.goto('/login')
      await expect(them.getByRole('link', { name: /create one|en créer un|ask for one|en demander un/i })).toHaveCount(0)
      await them.goto('/register')
      await expect(them.getByRole('heading', { name: /closed|fermées/i })).toBeVisible()
      const refused = await them.request.post('/api/v1/auth/register', {
        data: { username: unique('nobody'), password: 'an-e2e-password-long-enough' },
      })
      expect(refused.status()).toBe(403)
    } finally {
      await guest.close()
      await setting(page.request, 'registration.mode', before.registration)
    }
  })

  test('the clients’ door is read back, and its authority and script are served', async ({ page }) => {
    await page.goto('/admin/access')
    // By its own name, whole: the APIs' panel speaks of the clients' door too,
    // since the AniList relay answers there, and a section that merely
    // mentions it would be found first.
    const panel = page
      .getByRole('region', { name: /^(the clients’ door|la porte des clients)$/i })
      .or(page.locator('section', { has: page.getByRole('heading', { name: /^(the clients’ door|la porte des clients)$/i }) }))
    await expect(panel.first()).toBeVisible()

    const status = (await (await page.request.get('/api/v1/admin/tls')).json()) as {
      web: { bind: string; tls: boolean }
      clients: { names: string[]; authority?: { fingerprint: string } } | null
    }
    expect(status.web.bind).toBeTruthy()
    const served = await page.request.get('/ca.crt')
    // Without a door the page says so and nothing is served; the rest of
    // the test needs the suite's server started with AMS_CLIENTS_BIND.
    test.skip(!status.clients?.authority, 'no clients’ door with an authority on this server: set AMS_CLIENTS_BIND')
    if (status.clients?.authority) {
      // A door with an authority of the server's own: both files, public.
      expect(served.status()).toBe(200)
      expect(await served.text()).toContain('BEGIN CERTIFICATE')
      expect(status.clients.names).toContain('skyhook.sonarr.tv')
      await expect(panel.first().getByText('skyhook.sonarr.tv', { exact: true }).first()).toBeVisible()
      const script = await page.request.get('/trust-ca.sh')
      expect(script.status()).toBe(200)
      expect(await script.text()).toContain('update-ca-certificates')
      // And a stranger reads them too: they hold no secret.
      const stranger = await page.context().browser()!.newContext()
      expect((await stranger.request.get('/ca.crt')).status()).toBe(200)
      expect((await stranger.request.get('/api/v1/admin/tls')).status()).toBe(401)
      await stranger.close()
    } else {
      expect(served.status()).toBe(404)
    }
  })

  test('an API switched off from the page answers 503, and the others keep answering', async ({ page }) => {
    await page.goto('/admin/access')
    const sonarr = page.getByRole('switch', { name: /sonarr/i })
    await expect(sonarr).toHaveAttribute('aria-checked', 'true')

    try {
      await sonarr.click()
      await expect(sonarr).toHaveAttribute('aria-checked', 'false')
      await expect(page.getByText(/only radarr|seuls radarr/i)).toBeVisible()

      const off = await page.request.get('/v1/tvdb/shows/en/81189')
      expect(off.status()).toBe(503)
      expect(((await off.json()) as { error: string }).error).toBe('surface_disabled')
      // Fight Club is in the suite's catalogue already: asked for, it is read
      // from there rather than fetched and stored.
      expect((await page.request.get('/v1/movie/550')).status()).not.toBe(503)
    } finally {
      await setting(page.request, 'api.sonarr', 'true')
    }

    await page.reload()
    await expect(page.getByRole('switch', { name: /sonarr/i })).toHaveAttribute('aria-checked', 'true')
  })

  test('a member’s key reads the catalogue, and the TMDB relay only when members are let in', async ({
    page,
    browser,
  }) => {
    const username = unique('relay')
    const opened = await page.request.post('/api/v1/users', { data: { username, role: 'member' } })
    expect(opened.status()).toBe(201)
    const { password } = (await opened.json()) as { password: string }

    const guest = await browser.newContext()
    try {
      const them = await guest.newPage()
      await signIn(them, username, password)
      await them.waitForURL((url) => url.pathname === '/')
      const issued = await them.request.post('/api/v1/account/keys', { data: { name: 'e2e relay' } })
      expect(issued.status()).toBe(201)
      const { secret } = (await issued.json()) as { secret: string }

      // Kept apart: a request of its own, with nothing but the key.
      const bare = await browser.newContext()
      try {
        const catalogue = await bare.request.get('/api/v1/items?limit=1', { headers: { 'x-api-key': secret } })
        expect(catalogue.status()).toBe(200)

        const refused = await bare.request.get(`/3/configuration?api_key=${secret}`)
        expect(refused.status()).toBe(403)
        expect(((await refused.json()) as { error: string }).error).toBe('relay_not_for_members')

        await setting(page.request, 'api.tmdbMembers', 'true')
        const let_in = await bare.request.get(`/3/configuration?api_key=${secret}`)
        expect(let_in.status()).not.toBe(403)
      } finally {
        await bare.close()
      }
    } finally {
      await setting(page.request, 'api.tmdbMembers', 'false')
      await guest.close()
      await forget(page.request, username)
    }
  })
})
