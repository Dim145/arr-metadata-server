import { expect, test, type Page } from '@playwright/test'

/**
 * The administration side, signed in.
 *
 * The credential comes from the environment and has no default. A password in
 * a repository is a password, whatever it was for; and a suite that silently
 * signed in as `admin/admin` would pass against somebody's real instance.
 *
 *   AMS_E2E_USER=admin AMS_E2E_PASSWORD=… npm run test:e2e
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

const SCREENS = [
  ['/admin', /dashboard|tableau de bord/i],
  ['/admin/catalogue', /catalogue/i],
  ['/admin/clients', /clients/i],
  ['/admin/jobs', /jobs|tâches/i],
  ['/admin/audit', /audit|journal/i],
  ['/admin/settings', /settings|réglages/i],
] as const

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

async function scrollsSideways(page: Page) {
  return page.evaluate(() => {
    window.scrollTo(600, 0)
    const moved = window.scrollX > 0
    window.scrollTo(0, 0)
    return moved
  })
}

test.describe('the administration side', () => {
  test.skip(
    !USERNAME || !PASSWORD,
    'set AMS_E2E_USER and AMS_E2E_PASSWORD to exercise the administration side',
  )

  test('turns an anonymous visitor away at the door', async ({ page }) => {
    await page.goto('/admin/settings')
    await expect(page).toHaveURL(/\/login/)
    await expect(page.getByRole('button', { name: /sign in|se connecter/i })).toBeVisible()
  })

  test('refuses a wrong password and says so beside the field', async ({ page }) => {
    await page.goto('/login')
    await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
    await page.getByLabel(/password|mot de passe/i).fill('not-the-password')
    await page.getByRole('button', { name: /sign in|se connecter/i }).click()

    await expect(page.getByRole('alert')).toBeVisible()
    await expect(page).toHaveURL(/\/login/)
  })

  test('opens every screen without scrolling sideways or logging an error', async ({ page }) => {
    const errors: string[] = []
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text())
    })

    await signIn(page)

    for (const [path, heading] of SCREENS) {
      await page.goto(path)
      await expect(page.getByRole('heading', { name: heading }).first()).toBeVisible()
      expect(await scrollsSideways(page), `${path} scrolls sideways`).toBe(false)
    }

    expect(errors.filter((t) => !/Failed to load resource|ERR_/.test(t))).toEqual([])
  })

  test('is unmistakably a different place from the catalogue', async ({ page }) => {
    await signIn(page)

    // Whatever the chrome looks like, the way back has to be obvious.
    await expect(page.getByRole('link', { name: /back to the catalogue|retour au catalogue/i })).toBeVisible()
    await expect(page.getByRole('button', { name: /sign out|déconnecter/i })).toBeVisible()
  })

  test('speaks French throughout when asked to', async ({ page }) => {
    await signIn(page)
    await page.getByRole('button', { name: 'fr', exact: true }).click()
    await expect(page.locator('html')).toHaveAttribute('lang', 'fr')

    for (const [path] of SCREENS) {
      await page.goto(path)
      await page.waitForLoadState('networkidle')

      // Headings and controls are the strings a port most often leaves behind.
      const stranded = await page.evaluate(() => {
        const english = /\b(Dashboard|Settings|Catalogue entries|Sign out|Back to the catalogue|Search titles|New entry|All kinds|Edited by hand)\b/
        return [...document.querySelectorAll('h1, h2, h3, button, a, label, th')]
          .map((el) => (el.textContent ?? '').trim())
          .filter((text) => text && english.test(text))
      })

      expect(stranded, `${path} still shows English`).toEqual([])
    }
  })
})

test.describe('a locked field', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs a credential; see above')

  test('survives a round trip and says who owns it', async ({ page }) => {
    test.slow()
    await signIn(page)

    await page.goto('/admin/catalogue')
    await page.locator('a[href^="/admin/catalogue/"]').first().click()
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()

    // Sort title is chosen because nothing else reads it: locking it cannot
    // change what any client is served.
    const row = page.locator('li[data-field="sortTitle"]')
    const locked = row.getByText(/^(locked|verrouillé)\b/i)
    const unlock = row.getByRole('button', { name: /^(unlock|déverrouiller)$/i })

    // A run that failed halfway would otherwise leave this field locked and
    // every later run would start from a state it did not create.
    if (await unlock.count()) {
      await unlock.click()
      await expect(locked).toHaveCount(0)
    }

    await row.getByRole('button', { name: /^(edit|modifier)$/i }).click()
    await page.getByLabel(/^sort title$/i).fill('Zzz e2e lock probe')
    await page.getByRole('button', { name: /save and lock|enregistrer et verrouiller/i }).click()

    await expect(locked).toBeVisible()

    // And it must come back after a reload, or it was never stored.
    await page.reload()
    await expect(locked).toBeVisible()
    await expect(row).toContainText('Zzz e2e lock probe')

    await row.getByRole('button', { name: /^(unlock|déverrouiller)$/i }).click()
    await expect(locked).toHaveCount(0)
  })
})
