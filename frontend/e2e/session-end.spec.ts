import { expect, test as base, type APIRequestContext, type Locator, type Page } from '@playwright/test'

/**
 * A session that ends other than by the way out, with the page told at once
 * and never loaded again: an administrator closing every session of their
 * own from their own page, the password they cannot give themselves there,
 * and a session closed from somewhere else, heard of at the next thing the
 * server refuses (`sessionCaches`).
 *
 * Each test ends the sessions of an administrator of its own, opened for it
 * and deleted after it whatever happens — in the teardown of the fixture
 * that opened it, which runs even when the test runs out of time. Never the
 * shared administrator's: the other specs are signed in with it meanwhile.
 * That one signs in here only in a request context of its own, to open,
 * delete and close from outside, and signs out of that alone.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

interface Account {
  id: string
  username: string
  password: string
}

const test = base.extend<{ administrator: APIRequestContext; account: Account }>({
  administrator: async ({ request }, use) => {
    const signedIn = await request.post('/api/v1/auth/login', { data: { username: USERNAME, password: PASSWORD } })
    expect(signedIn.ok()).toBe(true)
    await use(request)
    await request.post('/api/v1/auth/logout')
  },
  account: async ({ administrator }, use, info) => {
    const username = `e2e-end-${info.project.name}-${Date.now().toString(36)}`
    const created = await administrator.post('/api/v1/users', { data: { username, role: 'admin' } })
    expect(created.status()).toBe(201)
    const { user, password } = (await created.json()) as { user: { id: string }; password: string }
    await use({ id: user.id, username, password })
    expect((await administrator.delete(`/api/v1/users/${user.id}`)).status()).toBe(204)
  },
})

/** The account, signed in in this page's browser, which shares its cookies with `page.request`. */
async function signIn(page: Page, account: Account) {
  const signedIn = await page.request.post('/api/v1/auth/login', {
    data: { username: account.username, password: account.password },
  })
  expect(signedIn.ok()).toBe(true)
}

/** Mark the page as it is now: one loaded again has lost the mark. */
async function mark(page: Page) {
  await page.evaluate(() => {
    ;(window as Window & { sameVisit?: boolean }).sameVisit = true
  })
}

function sameVisit(page: Page) {
  return page.evaluate(() => (window as Window & { sameVisit?: boolean }).sameVisit === true)
}

/**
 * Where the catalogue keeps the way in and the way out: its bar, from `md`
 * up; on a phone, the sheet along the foot, opened for the purpose.
 */
async function ways(page: Page, isMobile: boolean): Promise<Locator> {
  if (!isMobile) return page.getByRole('banner')
  await page.getByRole('button', { name: /^(more|plus)$/i }).click()
  return page.getByRole('dialog', { name: /^(more|plus)$/i })
}

const ADMIN = /^(admin|administration)$/i
const WAY_IN = /^(sign in|se connecter)$/i
const WAY_OUT = /^(sign out|se déconnecter)$/i

test.describe('a session ended other than by the way out', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')
  // Two sign-ins and an account opened before each, and several pages after.
  test.slow()

  test('one’s own page gives no new password, and leads to the account page that changes it', async ({
    page,
    account,
  }) => {
    await signIn(page, account)
    // The server refuses it: a session left open must not be enough to lock
    // its owner out. The page offered it all the same.
    const reset = await page.request.post(`/api/v1/users/${account.id}/password`, { data: {} })
    expect(reset.status()).toBe(409)

    await page.goto(`/admin/users/${account.id}`)
    await expect(page.getByRole('heading', { level: 1, name: account.username })).toBeVisible()
    await expect(page.getByRole('button', { name: /^(set the password|définir le mot de passe)$/i })).toHaveCount(0)
    const password = page.getByRole('region', { name: /^(password|mot de passe)$/i })
    await expect(password).toContainText(/account page|page de compte/i)

    await password.getByRole('link', { name: /^(my account|mon compte)$/i }).click()
    await page.waitForURL('**/admin/account')
    await expect(page.getByLabel(/^(current password|mot de passe actuel)$/i)).toBeVisible()
  })

  test('closing every session of one’s own, from one’s own page, signs out here at once', async ({
    page,
    account,
    isMobile,
  }) => {
    await signIn(page, account)
    await page.goto(`/admin/users/${account.id}`)
    await expect(page.getByRole('heading', { level: 1, name: account.username })).toBeVisible()
    await mark(page)

    // The catalogue a page further on, to come forward to once signed out —
    // by the administration's own way back, which its side bar holds from
    // `lg` up.
    const bar = page.getByRole('banner')
    if (!isMobile) {
      await page.getByRole('link', { name: /^(back to the catalogue|retour au catalogue)$/i }).click()
      await page.waitForURL((url) => url.pathname === '/')
      await expect(bar.getByRole('link', { name: ADMIN })).toBeVisible()
      await page.goBack()
      await page.waitForURL(`**/admin/users/${account.id}`)
    }

    // Asked first, saying this one goes with the rest.
    await page.getByRole('button', { name: /^(close every session|fermer toutes les sessions)$/i }).click()
    const asked = page.getByRole('dialog', { name: /^(close every session of yours\?|fermer toutes vos sessions \?)$/i })
    await expect(asked).toContainText(/this one included|celle-ci comprise/i)
    await asked.getByRole('button', { name: /^(close them all|toutes les fermer)$/i }).click()

    await page.waitForURL('**/login')
    expect(await sameVisit(page)).toBe(true)
    // Closed on the server too, this one with the rest.
    expect((await page.request.get('/api/v1/account')).status()).toBe(401)

    if (!isMobile) {
      await page.goForward()
      await page.waitForURL((url) => url.pathname === '/')
      await expect(bar.getByRole('link', { name: WAY_IN })).toBeVisible()
      await expect(bar.getByRole('link', { name: ADMIN })).toHaveCount(0)
      await expect(bar.getByRole('button', { name: WAY_OUT })).toHaveCount(0)
      expect(await sameVisit(page)).toBe(true)
    }
  })

  test('a session closed from somewhere else is heard of at the next refusal, and the bar shows a visitor', async ({
    page,
    administrator,
    account,
    isMobile,
  }) => {
    await signIn(page, account)
    await page.goto('/')
    const before = await ways(page, isMobile)
    const admin = before.getByRole('link', { name: ADMIN })
    await expect(admin).toBeVisible()
    await expect(before.getByRole('button', { name: WAY_OUT })).toBeVisible()
    await mark(page)

    // Closed by an administrator from another browser: nothing tells this page.
    expect((await administrator.delete(`/api/v1/users/${account.id}/sessions`)).status()).toBe(204)

    // The administration, from the catalogue: what it asks first is refused,
    // who is looking is asked again, and the answer is nobody. The bar's
    // answer, five minutes fresh, used to keep the page in the administration
    // with every panel refused.
    await admin.click()
    await page.waitForURL('**/login')
    expect(await sameVisit(page)).toBe(true)

    await page.goBack()
    await page.waitForURL((url) => url.pathname === '/')
    const after = await ways(page, isMobile)
    await expect(after.getByRole('link', { name: WAY_IN })).toBeVisible()
    await expect(after.getByRole('link', { name: ADMIN })).toHaveCount(0)
    await expect(after.getByRole('button', { name: WAY_OUT })).toHaveCount(0)
    expect(await sameVisit(page)).toBe(true)
  })
})
