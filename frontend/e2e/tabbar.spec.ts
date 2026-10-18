import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Page } from '@playwright/test'

/**
 * The bar along the foot of a phone: four places and a sheet for the rest.
 *
 * Only a phone has it. From a tablet's width up the bar at the top holds the
 * tabs, and this is not drawn at all — and the menu button that used to stand
 * for it is gone.
 */

const PLACES = [
  /^(home|accueil)$/i,
  /^(browse|parcourir)$/i,
  /^(schedule|programme)$/i,
  /^(seasons|saisons)$/i,
]
const MORE = /^(more|plus)$/i

const bar = (page: Page) => page.getByRole('navigation', { name: /^(main navigation|navigation principale)$/i })
const sheet = (page: Page) => page.getByRole('dialog', { name: MORE })

test.describe('the bar along the foot of a phone', () => {
  test.skip(({ isMobile }) => !isMobile, 'only a phone has it; the tabs are at the top from a tablet up')

  test('is fixed to the foot of the screen, with its four places and the sheet’s', async ({ page }) => {
    await page.goto('/')
    await expect(bar(page)).toBeVisible()

    for (const name of PLACES) await expect(bar(page).getByRole('link', { name })).toBeVisible()
    await expect(bar(page).getByRole('button', { name: MORE })).toBeVisible()

    // At the foot, whatever the page's length — and every place a target a
    // thumb finds.
    const box = (await bar(page).boundingBox())!
    expect(Math.round(box.y + box.height)).toBe(page.viewportSize()!.height)
    for (const control of [...(await bar(page).getByRole('link').all()), bar(page).getByRole('button', { name: MORE })]) {
      expect((await control.boundingBox())!.height).toBeGreaterThanOrEqual(44)
    }

    // The top keeps the title and the search; the menu button is gone.
    await expect(page.getByRole('button', { name: /^menu$/i })).toHaveCount(0)
    await expect(page.getByRole('button', { name: /^(search|rechercher)$/i })).toBeVisible()
  })

  test('says which place the reader is in, one at a time', async ({ page }) => {
    await page.goto('/')
    await expect(bar(page).locator('[aria-current="page"]')).toHaveCount(1)
    await expect(bar(page).getByRole('link', { name: PLACES[0]! })).toHaveAttribute('aria-current', 'page')

    await bar(page).getByRole('link', { name: PLACES[1]! }).click()
    await expect(page).toHaveURL(/\/browse$/)
    await expect(bar(page).locator('[aria-current="page"]')).toHaveCount(1)
    await expect(bar(page).getByRole('link', { name: PLACES[1]! })).toHaveAttribute('aria-current', 'page')

    await bar(page).getByRole('link', { name: PLACES[2]! }).click()
    await expect(page).toHaveURL(/\/calendar$/)
    await expect(bar(page).getByRole('link', { name: PLACES[2]! })).toHaveAttribute('aria-current', 'page')

    // Anywhere beneath a place is the place: a season chart is Seasons.
    await bar(page).getByRole('link', { name: PLACES[3]! }).click()
    await expect(page).toHaveURL(/\/seasons/)
    await expect(bar(page).getByRole('link', { name: PLACES[3]! })).toHaveAttribute('aria-current', 'page')
    await expect(bar(page).locator('[aria-current="page"]')).toHaveCount(1)
  })

  test('the last place opens a sheet with the rest, and Escape puts it away', async ({ page }) => {
    await page.goto('/')
    const more = bar(page).getByRole('button', { name: MORE })
    await expect(more).toHaveAttribute('aria-expanded', 'false')

    await more.click()
    await expect(more).toHaveAttribute('aria-expanded', 'true')
    await expect(sheet(page)).toBeVisible()
    await expect(sheet(page).getByRole('link', { name: /^collections$/i })).toBeVisible()
    await expect(sheet(page).getByRole('link', { name: /^(in numbers|en chiffres)$/i })).toBeVisible()
    await expect(sheet(page).getByRole('button', { name: /light theme|thème clair/i })).toBeVisible()
    await expect(sheet(page).getByRole('button', { name: 'fr', exact: true })).toBeVisible()
    await expect(
      sheet(page).getByRole('link', { name: /^(sign in|se connecter|admin|administration|my account|mon compte)$/i }),
    ).toBeVisible()

    await page.keyboard.press('Escape')
    await expect(sheet(page)).toBeHidden()
    await expect(more).toHaveAttribute('aria-expanded', 'false')
    // Back where it came from.
    await expect(more).toBeFocused()
  })

  test('a place in the sheet is followed, the sheet is gone, and its tab says it is the one', async ({ page }) => {
    await page.goto('/')
    const more = bar(page).getByRole('button', { name: MORE })

    await more.click()
    await sheet(page).getByRole('link', { name: /^collections$/i }).click()
    await expect(page).toHaveURL(/\/collections$/)
    await expect(sheet(page)).toBeHidden()
    await expect(more).toHaveAttribute('aria-current', 'true')
    // And no place of the four claims it.
    await expect(bar(page).locator('a[aria-current="page"]')).toHaveCount(0)
  })

  test('the lamp and the language are in the sheet, and work from there', async ({ page }) => {
    await page.goto('/')
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')

    await bar(page).getByRole('button', { name: MORE }).click()
    const lamp = sheet(page).getByRole('button', { name: /light theme|thème clair/i })
    await expect(lamp).toHaveAttribute('aria-pressed', 'false')
    await lamp.click()
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light')
    await lamp.click()
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')

    await sheet(page).getByRole('button', { name: 'fr', exact: true }).click()
    await expect(page.locator('html')).toHaveAttribute('lang', 'fr')
    // The bar speaks it too, and the sheet is still open over it.
    await expect(bar(page).getByRole('link', { name: 'Accueil' })).toBeAttached()
    await sheet(page).getByRole('button', { name: 'en', exact: true }).click()
    await expect(page.locator('html')).toHaveAttribute('lang', 'en')
  })

  test('leaves nothing under it, the footer included', async ({ page }) => {
    await page.goto('/')
    await expect(page.locator('a[href^="/work/"]').first()).toBeVisible()

    // The page grows as its rows arrive, each by a request of its own: it is
    // scrolled to its foot again until, grown to its length, the last pixel
    // of the page is above the first of the bar.
    await expect(async () => {
      await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight))
      const footer = (await page.getByRole('contentinfo').boundingBox())!
      const strip = (await bar(page).boundingBox())!
      expect(footer.y + footer.height).toBeLessThanOrEqual(strip.y + 1)
    }).toPass({ timeout: 10_000 })
  })

  test('does not scroll sideways, and meets WCAG 2.1 AA with the sheet open', async ({ page }) => {
    await page.goto('/')
    await expect(page.locator('a[href^="/work/"]').first()).toBeVisible()
    expect(
      await page.evaluate(() => {
        window.scrollTo(600, 0)
        const moved = window.scrollX > 0
        window.scrollTo(0, 0)
        return moved
      }),
    ).toBe(false)

    await bar(page).getByRole('button', { name: MORE }).click()
    await expect(sheet(page)).toBeVisible()
    await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'))
    const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze()
    expect(results.violations.map((v) => `${v.id}: ${v.help}`)).toEqual([])
  })
})

test.describe('the bar on the narrowest phone', () => {
  test.skip(({ isMobile }) => !isMobile, 'only a phone has it')
  test.use({ viewport: { width: 320, height: 640 } })

  test('keeps its five places on the screen, in the longest words', async ({ page }) => {
    // French, where "Programme" is the widest word the bar carries.
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'fr'))
    await page.goto('/')
    // Drawn once its language is in: the bar's places are counted after that.
    await expect(bar(page).getByRole('link')).toHaveCount(4)

    const controls = [...(await bar(page).getByRole('link').all()), bar(page).getByRole('button', { name: MORE })]
    expect(controls).toHaveLength(5)
    for (const control of controls) {
      const box = (await control.boundingBox())!
      expect(box.x).toBeGreaterThanOrEqual(0)
      expect(box.x + box.width).toBeLessThanOrEqual(320)
      // The word is whole, and inside its own cell.
      const word = control.locator('span').last()
      const edge = (await word.boundingBox())!
      expect(edge.x).toBeGreaterThanOrEqual(box.x - 2)
      expect(edge.x + edge.width).toBeLessThanOrEqual(box.x + box.width + 2)
    }
  })
})

test.describe('the bar at a tablet’s width', () => {
  test.skip(({ isMobile }) => isMobile, 'the phone project is the other half of this')

  test('is not drawn: the tabs are at the top', async ({ page }) => {
    await page.goto('/')
    await expect(bar(page)).toBeHidden()
    await expect(page.getByRole('navigation', { name: /^(browse|parcourir)$/i })).toBeVisible()
  })
})
