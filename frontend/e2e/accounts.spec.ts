import { expect, test, type Browser, type Page } from '@playwright/test'

/**
 * Accounts: an administrator opens one, its member signs in, finds their own
 * page and makes a key that does no more than they may.
 *
 * Needs the administrator's credential, like the rest of the admin suite;
 * without it these say so and stand aside. Each project makes its own member,
 * named after it, and removes it afterwards: both run at once.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

async function signIn(page: Page, username: string, password: string) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(username)
  await page.getByLabel(/password|mot de passe/i).fill(password)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
}

async function member(browser: Browser, project: string) {
  const context = await browser.newContext()
  const page = await context.newPage()
  return { context, page, username: `e2e-${project}-${Date.now().toString(36)}` }
}

test.describe('accounts', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('an administrator opens an account, and its member lands on their own page', async ({
    page,
    browser,
  }, info) => {
    test.slow()
    await signIn(page, USERNAME!, PASSWORD!)
    await page.waitForURL('**/admin')

    await page.goto('/admin/users')
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(/^(members|membres)$/i)

    const them = await member(browser, info.project.name)
    const shown = `Zoé ${info.project.name}`
    await page.getByRole('button', { name: /open an account|ouvrir un compte/i }).click()
    const dialog = page.getByRole('dialog')
    await dialog.getByLabel(/^(username|identifiant)$/i).fill(them.username)
    await dialog.getByLabel(/display name|nom affiché/i).fill(shown)
    await dialog.getByRole('button', { name: /open an account|ouvrir un compte/i }).click()

    // The password was generated: shown once, in a box of its own.
    const secret = dialog.getByLabel(/their password|son mot de passe/i)
    await expect(secret).toHaveValue(/^.{20}$/)
    const password = await secret.inputValue()
    await dialog.getByRole('button', { name: /^(done|terminé)$/i }).click()

    await expect(page.getByRole('link', { name: shown, exact: true })).toBeVisible()

    const id = await page.evaluate(async (username) => {
      const answer = await fetch(`/api/v1/users?term=${username}`)
      return (await answer.json()).users[0].id as string
    }, them.username)

    try {

    // The member, signed in elsewhere: to the catalogue, not the administration.
    await signIn(them.page, them.username, password)
    await them.page.waitForURL((url) => url.pathname === '/')
    await them.page.goto('/admin')
    await them.page.waitForURL('**/account')
    await expect(them.page.getByRole('heading', { level: 1 })).toHaveText(shown)
    await expect(them.page.getByText(/^(member|membre)$/i).first()).toBeVisible()

    // A key, as the page offers it: one of theirs, or a list.
    const keys = await them.page.request.get('/api/v1/account/keys')
    const { limit } = (await keys.json()) as { limit?: number }
    if (limit === 0) {
      await expect(them.page.getByText(/made by an administrator|créées par un administrateur/i)).toBeVisible()
    } else {
      if (limit === 1) {
        await them.page.getByRole('button', { name: /make your key|créer votre clé/i }).click()
      } else {
        await them.page.getByLabel(/^(name|nom)$/i).fill('Sonarr')
        await them.page.getByRole('button', { name: /^(create|créer)$/i }).click()
      }
      const issued = them.page.getByLabel(/new key|nouvelle clé/i)
      await expect(issued).toHaveValue(/^ams_/)
      const key = await issued.inputValue()

      // It reads; it does not write, because its owner may not.
      const read = await them.page.request.get('/api/v1/items?limit=1', { headers: { 'X-Api-Key': key } })
      expect(read.status()).toBe(200)
      const write = await them.page.request.post('/api/v1/cache/clear', { headers: { 'X-Api-Key': key } })
      expect(write.status()).toBe(403)
    }

    // Disabled, the member's session ends at once.
    const disabled = await page.request.patch(`/api/v1/users/${id}`, { data: { status: 'disabled' } })
    expect(disabled.ok()).toBe(true)
    expect((await them.page.request.get('/api/v1/account')).status()).toBe(401)

    } finally {
      // Whatever happened above, the catalogue is left as it was found.
      expect((await page.request.delete(`/api/v1/users/${id}`)).status()).toBe(204)
      await them.context.close()
    }
  })

  test('nobody takes their own rights away, not even an administrator', async ({ page }) => {
    await signIn(page, USERNAME!, PASSWORD!)
    await page.waitForURL('**/admin')

    const me = (await (await page.request.get('/api/v1/auth/me')).json()) as { user: { id: string } }
    const demoted = await page.request.patch(`/api/v1/users/${me.user.id}`, { data: { role: 'member' } })
    expect(demoted.status()).toBe(409)
    const deleted = await page.request.delete(`/api/v1/users/${me.user.id}`)
    expect(deleted.status()).toBe(409)

    // And the page says so beside the choice, which it does not offer.
    await page.goto(`/admin/users/${me.user.id}`)
    await expect(page.getByText(/another administrator|un autre administrateur/i)).toBeVisible()
    await expect(page.getByRole('radio', { name: /^(member|membre)$/i })).toBeDisabled()
  })
})
