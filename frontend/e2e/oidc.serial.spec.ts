import { expect, test, type APIRequestContext, type Browser, type Page } from '@playwright/test'

import { startProvider, type Provider } from './support/oidc-provider'

/**
 * Signing in through an identity provider — a real flow, against the small
 * provider in `support/`: the browser goes there and back, the server
 * exchanges the code, checks the ID token's signature, nonce and `at_hash`,
 * and finds or opens the account.
 *
 * The server needs AMS_PUBLIC_URL for the provider to have somewhere to send
 * people back; without it these stand aside. They change server-wide
 * settings, so they run in the `server-wide` project, and put back what they
 * changed.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

test.describe.configure({ mode: 'serial' })

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

interface Configuration {
  enabled: boolean
  issuer: string
  clientId: string
  secretSet: boolean
  secretFromEnv: boolean
  scopes: string
  buttonLabel: string
  autoRegister: boolean
  roleClaim: string
  adminValues: string
  editorValues: string
  passwordLogin: boolean
  redirectUri?: string
  ready: boolean
}

async function configure(request: APIRequestContext, change: Record<string, unknown>) {
  const answer = await request.put('/api/v1/admin/oidc', { data: change })
  expect(answer.ok(), JSON.stringify(change)).toBe(true)
  return (await answer.json()) as Configuration
}

/**
 * The configuration as it was, when the tests may borrow the provider: a
 * server with a real one set up is left alone — its secret could not be put
 * back.
 */
async function borrowable(request: APIRequestContext) {
  const before = (await (await request.get('/api/v1/admin/oidc')).json()) as Configuration
  test.skip(!before.redirectUri, 'needs AMS_PUBLIC_URL on the server')
  test.skip(before.enabled || before.secretSet || before.secretFromEnv, 'a real provider is set up here')
  return before
}

function restore(before: Configuration) {
  return {
    enabled: before.enabled,
    issuer: before.issuer,
    clientId: before.clientId,
    clientSecret: '',
    scopes: before.scopes,
    buttonLabel: before.buttonLabel,
    autoRegister: before.autoRegister,
    roleClaim: before.roleClaim,
    adminValues: before.adminValues,
    editorValues: before.editorValues,
    passwordLogin: before.passwordLogin,
  }
}

async function forget(request: APIRequestContext, username: string) {
  const found = await request.get(`/api/v1/users?term=${encodeURIComponent(username)}`)
  for (const user of ((await found.json()) as { users: { id: string; username: string }[] }).users) {
    if (user.username === username) await request.delete(`/api/v1/users/${user.id}`)
  }
}

/** A browser of its own, signing in through the provider from the sign-in page. */
async function throughTheProvider(browser: Browser) {
  const context = await browser.newContext()
  const page = await context.newPage()
  await page.goto('/login')
  await page.getByRole('link', { name: /the e2e provider/i }).click()
  return { context, page }
}

test.describe('signing in through an identity provider', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  let provider: Provider
  test.beforeAll(async () => {
    provider = await startProvider()
  })
  test.afterAll(async () => {
    await provider?.close()
  })

  test('a first visit opens an account with the role the groups give; the next one finds it', async ({
    page,
    browser,
  }) => {
    test.slow()
    await signIn(page)
    const before = await borrowable(page.request)

    const username = `e2e_sso_${Date.now().toString(36)}`
    const opened: { close(): Promise<void> }[] = []
    try {
      const saved = await configure(page.request, {
        enabled: true,
        issuer: provider.issuer,
        clientId: provider.clientId,
        clientSecret: provider.clientSecret,
        scopes: 'openid profile email',
        buttonLabel: 'Continue with the e2e provider',
        autoRegister: true,
        roleClaim: 'groups',
        adminValues: 'e2e-admins',
        editorValues: 'e2e-editors',
      })
      expect(saved.ready).toBe(true)
      expect(saved.secretSet).toBe(true)
      // The secret went one way.
      expect(JSON.stringify(saved)).not.toContain(provider.clientSecret)

      // Discovery, as the test button reads it — and the reason, when it fails.
      const tried = await page.request.post('/api/v1/admin/oidc/test', { data: { issuer: provider.issuer } })
      expect(tried.ok()).toBe(true)
      expect(((await tried.json()) as { discovery: { keys: number } }).discovery.keys).toBe(1)
      const wrong = await page.request.post('/api/v1/admin/oidc/test', {
        data: { issuer: `${provider.issuer}/nowhere` },
      })
      expect(((await wrong.json()) as { ok: boolean; error: string }).ok).toBe(false)

      // Not through the generic settings: only together, where it is checked.
      const sideways = await page.request.put('/api/v1/settings/server/-', {
        data: { key: 'auth.passwordLogin', value: 'false' },
      })
      expect(sideways.status()).toBe(400)

      // First visit: an editor's account, and the administration.
      provider.signInAs({
        sub: `sub-${username}`,
        preferred_username: username,
        email: `${username}@example.org`,
        email_verified: true,
        name: 'Somebody Through SSO',
        groups: ['family', 'e2e-editors'],
      })
      const first = await throughTheProvider(browser)
      opened.push(first.context)
      await first.page.waitForURL('**/admin')
      const me = (await (await first.page.request.get('/api/v1/auth/me')).json()) as {
        identity: string
        user: { id: string; role: string; hasPassword: boolean }
      }
      expect(me.identity).toBe(`editor:${username}`)
      expect(me.user.hasPassword).toBe(false)

      // Again, out of the editors' group: the same account, a member's now.
      provider.signInAs({ sub: `sub-${username}`, preferred_username: username, groups: ['family'] })
      const second = await throughTheProvider(browser)
      opened.push(second.context)
      await second.page.waitForURL((url) => url.pathname === '/')
      const again = (await (await second.page.request.get('/api/v1/auth/me')).json()) as {
        user: { id: string; role: string }
      }
      expect(again.user.id).toBe(me.user.id)
      expect(again.user.role).toBe('member')

      // Refused at the provider: back at the door, and told so.
      provider.signInAs('denied')
      const refused = await throughTheProvider(browser)
      opened.push(refused.context)
      await refused.page.waitForURL(/\/login\?sso=denied/)
      await expect(refused.page.getByRole('alert')).toContainText(/did not sign you in|ne vous a pas connecté/i)

      // Nobody here yet, and no account opened for strangers.
      await configure(page.request, { autoRegister: false })
      provider.signInAs({ sub: 'sub-a-stranger', preferred_username: 'a-stranger' })
      const stranger = await throughTheProvider(browser)
      opened.push(stranger.context)
      await stranger.page.waitForURL(/\/login\?sso=no_account/)

      // A callback nobody started is refused, whatever it carries.
      const forged = await browser.newContext()
      opened.push(forged)
      const answer = await forged.request.get('/api/v1/auth/oidc/callback?code=x&state=y', { maxRedirects: 0 })
      expect(answer.status()).toBe(303)
      expect(answer.headers().location).toBe('/login?sso=expired')
    } finally {
      for (const context of opened) await context.close()
      await configure(page.request, restore(before))
      await forget(page.request, username)
    }
  })

  test('with passwords switched off, only the provider and the environment’s administrator get in', async ({
    page,
    browser,
  }) => {
    await signIn(page)
    const before = await borrowable(page.request)

    // Not while the provider is not ready: there would be no way in.
    const early = await page.request.put('/api/v1/admin/oidc', { data: { enabled: false, passwordLogin: false } })
    expect(early.status()).toBe(409)

    const guest = await browser.newContext()
    try {
      await configure(page.request, {
        enabled: true,
        issuer: provider.issuer,
        clientId: provider.clientId,
        clientSecret: provider.clientSecret,
        buttonLabel: 'Continue with the e2e provider',
        passwordLogin: false,
      })

      const them = await guest.newPage()
      await them.goto('/login')
      // The provider first; the password behind a door of its own.
      await expect(them.getByRole('link', { name: /the e2e provider/i })).toBeVisible()
      await expect(them.getByLabel(/^(username|identifiant)$/i)).toBeHidden()

      const refused = await them.request.post('/api/v1/auth/login', {
        data: { username: 'somebody-else', password: 'whatever-it-is-long' },
      })
      expect(refused.status()).toBe(403)
      expect(((await refused.json()) as { error: string }).error).toBe('password_login_off')

      // The door's own name, with a wrong password, answers the same: nobody
      // learns which name it is.
      const guessed = await them.request.post('/api/v1/auth/login', {
        data: { username: USERNAME, password: 'not-the-password-at-all' },
      })
      expect(guessed.status()).toBe(403)
      expect(((await guessed.json()) as { error: string }).error).toBe('password_login_off')

      // And nobody signs up with a password meanwhile.
      const options = (await (await them.request.get('/api/v1/auth/options')).json()) as { registration: string }
      expect(options.registration).toBe('closed')

      // The environment's administrator keeps the door.
      const admin = await them.request.post('/api/v1/auth/login', {
        data: { username: USERNAME, password: PASSWORD },
      })
      expect(admin.ok()).toBe(true)
    } finally {
      await guest.close()
      await configure(page.request, restore(before))
    }
  })

  test('an account with a password is tied to the provider from its own page, by its owner', async ({
    page,
    browser,
  }) => {
    test.slow()
    await signIn(page)
    const before = await borrowable(page.request)

    const username = `e2e_tie_${Date.now().toString(36)}`
    const opened = await page.request.post('/api/v1/users', { data: { username, role: 'member' } })
    expect(opened.status()).toBe(201)
    const { password, user } = (await opened.json()) as { password: string; user: { id: string } }

    const guest = await browser.newContext()
    const later = await browser.newContext()
    try {
      await configure(page.request, {
        enabled: true,
        issuer: provider.issuer,
        clientId: provider.clientId,
        clientSecret: provider.clientSecret,
        buttonLabel: 'Continue with the e2e provider',
        autoRegister: false,
      })

      // Not to a stranger: tying needs its owner signed in.
      const stranger = await browser.newContext()
      const cold = await stranger.request.get('/api/v1/auth/oidc/start?link=1', { maxRedirects: 0 })
      expect(cold.headers().location).toBe('/login?sso=sign_in_first')
      await stranger.close()

      const them = await guest.newPage()
      await them.goto('/login')
      await them.getByLabel(/^(username|identifiant)$/i).fill(username)
      await them.getByLabel(/^(password|mot de passe)$/i).fill(password)
      await them.getByRole('button', { name: /^(sign in|se connecter)$/i }).click()
      await them.waitForURL((url) => url.pathname === '/')

      provider.signInAs({ sub: `sub-${username}`, preferred_username: `${username}-there`, groups: [] })
      await them.goto('/account')
      await them.getByRole('link', { name: /tie my account|lier mon compte/i }).click()
      await them.waitForURL('**/account')
      await expect(them.getByText(/tied to the identity provider|lié au fournisseur d’identité/i)).toBeVisible()

      // From now on the provider alone lets them in, to the same account.
      const next = await later.newPage()
      await next.goto('/login')
      await next.getByRole('link', { name: /the e2e provider/i }).click()
      await next.waitForURL((url) => url.pathname === '/')
      const me = (await (await next.request.get('/api/v1/auth/me')).json()) as { user: { id: string } }
      expect(me.user.id).toBe(user.id)

      // Untied by an administrator: its sessions go with the tie.
      const untied = await page.request.delete(`/api/v1/users/${user.id}/oidc`)
      expect(untied.status()).toBe(204)
      expect((await next.request.get('/api/v1/account')).status()).toBe(401)
    } finally {
      await guest.close()
      await later.close()
      await configure(page.request, restore(before))
      await forget(page.request, username)
    }
  })
})
