import { expect, test, type Page } from '@playwright/test'

/**
 * Signing out of the catalogue's bar, and back in, with the page never loaded
 * again: the bar says who is here at once — a visitor's way in the moment the
 * session is over, the administrator's way to the administration once they
 * are back.
 *
 * The cache used to be emptied on the way out without a word to whoever read
 * it, and the bar, which is told who is looking from above every page, kept
 * "Admin" and the way out until a reload (`endSession`).
 *
 * Signing out ends this page's own session and no other: every spec signs in
 * on its own.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

/** Through the sign-in page the browser is on, to the administration. */
async function signIn(page: Page) {
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

/** Whether this is still the page that was marked: one loaded again has lost the mark. */
function sameVisit(page: Page) {
  return page.evaluate(() => (window as Window & { sameVisit?: boolean }).sameVisit === true)
}

test.describe('signing out', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('the bar offers a visitor’s way in at once, and the administration again once signed back in', async ({
    page,
    isMobile,
  }) => {
    test.skip(isMobile, 'the bar holds the way out from `md` up; on a phone it is in the sheet along the foot')
    await page.goto('/login')
    await signIn(page)

    await page.goto('/')
    const bar = page.getByRole('banner')
    const admin = bar.getByRole('link', { name: /^(admin|administration)$/i })
    const wayIn = bar.getByRole('link', { name: /^(sign in|se connecter)$/i })
    const wayOut = bar.getByRole('button', { name: /^(sign out|se déconnecter)$/i })
    await expect(admin).toBeVisible()
    await expect(wayOut).toBeVisible()
    await page.evaluate(() => {
      ;(window as Window & { sameVisit?: boolean }).sameVisit = true
    })

    // A page further on by a tab, so that there is one to come back to: the
    // sign-in page's own ways back to the catalogue load the page anew.
    await bar.getByRole('link', { name: /^(schedule|programme)$/i }).click()
    await page.waitForURL('**/calendar')

    await wayOut.click()
    await page.waitForURL('**/login')
    await expect(page.getByRole('heading', { level: 1, name: /^(sign in|connexion)$/i })).toBeVisible()
    await page.goBack()
    await page.waitForURL((url) => url.pathname === '/')
    await expect(wayIn).toBeVisible()
    await expect(admin).toHaveCount(0)
    await expect(wayOut).toHaveCount(0)
    expect(await sameVisit(page)).toBe(true)

    // In again from the bar, and back to the catalogue by the administration's
    // own way: the administrator's bar, not the visitor's it was just now.
    await wayIn.click()
    await page.waitForURL('**/login')
    await signIn(page)
    await page.getByRole('link', { name: /^(back to the catalogue|retour au catalogue)$/i }).click()
    await page.waitForURL((url) => url.pathname === '/')
    await expect(admin).toBeVisible()
    await expect(wayOut).toBeVisible()
    await expect(wayIn).toHaveCount(0)
    expect(await sameVisit(page)).toBe(true)
  })
})
