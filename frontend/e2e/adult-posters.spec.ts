import { expect, test, type Page } from '@playwright/test'
import { scan } from './support/axe'

/**
 * The posters of works for adults, as an account chooses them on its own
 * page: veiled until it asks, then shown — in every browser it signs in on,
 * not only the one it asked in — and veiled again from the bar's reading
 * menu, which holds the same choice.
 *
 * The administrator's own choice, which the desk alone turns: both projects
 * run at once, and would turn the one account's choice under each other.
 * What the account kept before is put back afterwards, whatever happened.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

/** Where the account keeps the choice (`adultPosters.ts`). */
const PREFERENCE = '/api/v1/account/preferences/reader.adultPosters'

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

interface Work {
  id: string
  isAdult?: boolean
  contentRating?: string | null
  images?: { coverType: string }[]
}

/**
 * Whether the interface veils a work's poster for a reader who has not asked
 * to see it, as `forAdults` decides: marked for adults, or rated eighteen or
 * more, or in one of the few ratings said in letters.
 */
function forAdults(work: Work): boolean {
  const rating = work.contentRating?.trim().toUpperCase() ?? ''
  const age = Number(/\d+/.exec(rating)?.[0] ?? Number.NaN)
  return work.isAdult === true || ['NC-17', 'X', 'XXX', 'AO', 'ADULTS ONLY'].includes(rating) || (age >= 18 && age <= 21)
}

/** What the account keeps of the choice, if anything. */
async function kept(page: Page): Promise<unknown> {
  const answer = await page.request.get('/api/v1/account/preferences')
  return ((await answer.json()) as { values: Record<string, unknown> }).values['reader.adultPosters']
}

/** The choice as the page sends it, a moment after it is made. */
function sent(page: Page) {
  return page.waitForResponse(
    (response) => response.url().endsWith(PREFERENCE) && response.request().method() === 'PUT',
  )
}

/** A work's plate: its poster, and the veil over it, which is a button. */
function plate(page: Page) {
  const header = page.locator('main article > header')
  return {
    poster: header.getByRole('img', { name: /^(poster for|affiche de) /i }),
    veil: header.getByRole('button', { name: /for adults|pour adultes/i }),
  }
}

test.describe('the posters of works for adults', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('are shown once the account asks on its own page, in every browser, and veiled again from the menu', async ({
    page,
    browser,
    isMobile,
  }) => {
    test.skip(isMobile, 'the choice is the account’s: the desk alone turns it')
    test.slow()
    await signIn(page)

    const listed = await page.request.get('/api/v1/items?includeAdult=true&limit=100')
    const { items } = (await listed.json()) as { items: Work[] }
    const work = items.find((item) => forAdults(item) && item.images?.some((image) => image.coverType === 'poster'))
    test.skip(!work, 'the catalogue holds no work for adults with a poster')

    const before = await kept(page)
    try {
      // Whatever the account chose before is set aside, and this browser has
      // chosen nothing: the poster is veiled.
      expect((await page.request.delete(PREFERENCE)).ok()).toBe(true)
      await page.goto(`/work/${work!.id}`)
      await expect(plate(page).poster).toBeVisible()
      await expect(plate(page).veil).toBeVisible()

      // Asked for on the account's page, by the switch its words name.
      await page.goto('/account')
      const choice = page.getByRole('switch', {
        name: /^(show the posters of works for adults|montrer les affiches des œuvres pour adultes)$/i,
      })
      await expect(choice).toHaveAttribute('aria-checked', 'false')
      await page.waitForLoadState('networkidle')
      await scan(page, 'the account page')
      const on = sent(page)
      await choice.click()
      await expect(choice).toHaveAttribute('aria-checked', 'true')
      expect((await on).ok()).toBe(true)
      expect(await kept(page)).toBe(true)

      await page.goto(`/work/${work!.id}`)
      await expect(plate(page).poster).toBeVisible()
      await expect(plate(page).veil).toHaveCount(0)

      // Another browser, which kept nothing of it: the account brings it.
      const elsewhere = await browser.newContext()
      try {
        const other = await elsewhere.newPage()
        await signIn(other)
        await other.goto(`/work/${work!.id}`)
        await expect(plate(other).poster).toBeVisible()
        await expect(plate(other).veil).toHaveCount(0)
      } finally {
        await elsewhere.close()
      }

      // The bar's reading menu holds the same choice: turned off there, the
      // poster is veiled at once, and the account keeps that too.
      await page.getByRole('button', { name: /^(language and reading|langue et lecture)/i }).click()
      const menu = page.getByRole('switch', { name: /^(show adult posters|montrer les affiches pour adultes)$/i })
      await expect(menu).toHaveAttribute('aria-checked', 'true')
      const off = sent(page)
      await menu.click()
      await expect(menu).toHaveAttribute('aria-checked', 'false')
      await expect(plate(page).veil).toBeVisible()
      expect((await off).ok()).toBe(true)
      expect(await kept(page)).toBe(false)
    } finally {
      // The account's choice as it was found.
      if (before === undefined) await page.request.delete(PREFERENCE)
      else await page.request.put(PREFERENCE, { data: { value: before } })
    }
  })
})
