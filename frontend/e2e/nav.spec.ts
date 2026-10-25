import AxeBuilder from '@axe-core/playwright'
import { expect, test } from '@playwright/test'
import { escaped, pattern } from './support/regex'

/**
 * The way into the catalogue: one tab, with the shortcuts hanging from it.
 * The tab is a link; the panel opens under a resting pointer or from the
 * button beside it, and closes on Escape, on leaving, and on going anywhere.
 */
test.describe('the browse tab and its panel', () => {
  test.beforeEach(({ isMobile }) => {
    test.skip(isMobile, 'the tabs are along the foot of the screen at this width')
  })

  test('the bar lists one way in, not three', async ({ page }) => {
    await page.goto('/')
    const bar = page.getByRole('navigation', { name: /^(browse|parcourir)$/i }).first()
    await expect(bar.getByRole('link', { name: /^(browse|parcourir)$/i })).toBeVisible()
    await expect(bar.getByRole('link', { name: /^(series|séries)$/i })).toHaveCount(0)
    await expect(bar.getByRole('link', { name: /^(films)$/i })).toHaveCount(0)
  })

  test('rests open under the pointer, and leads where it says', async ({ page }) => {
    await page.goto('/')
    const tab = page.getByRole('link', { name: /^(browse|parcourir)$/i }).first()
    await tab.hover()
    const panel = page.getByRole('group', { name: /^(browse|parcourir)$/i })
    await expect(panel).toBeVisible()

    // The commonest genres of each kind, each its page narrowed to that kind.
    const genre = panel.locator('a[href^="/genre/"][href$="?kind=series"]').first()
    await expect(genre).toBeVisible()
    const href = await genre.getAttribute('href')
    await genre.click()
    await expect(page).toHaveURL(pattern(escaped(href!) + '$'))
    // Gone with the page it opened.
    await expect(panel).toHaveCount(0)

    // The tab itself is still the catalogue.
    await page.goto('/')
    await tab.click()
    await expect(page).toHaveURL(/\/browse$/)
  })

  test('opens from the button for a finger or a keyboard, and Escape closes it', async ({ page }) => {
    await page.goto('/')
    const button = page.getByRole('button', { name: /browse: the menu|parcourir : le menu/i })
    await expect(button).toHaveAttribute('aria-expanded', 'false')
    await button.click()
    await expect(button).toHaveAttribute('aria-expanded', 'true')
    const panel = page.getByRole('group', { name: /^(browse|parcourir)$/i })
    await expect(panel).toBeVisible()
    await expect(panel.getByRole('link', { name: /^(all series|toutes les séries)$/i })).toBeVisible()

    await page.keyboard.press('Escape')
    await expect(panel).toHaveCount(0)
    await expect(button).toBeFocused()

    // From the keyboard alone: Enter opens, Tab walks in, Escape returns.
    await page.keyboard.press('Enter')
    await expect(panel).toBeVisible()
    await page.keyboard.press('Tab')
    await expect(panel.getByRole('link').first()).toBeFocused()
    await page.keyboard.press('Escape')
    await expect(panel).toHaveCount(0)
  })

  test('leads to the collections, which have a page of their own', async ({ page }) => {
    await page.goto('/')
    await page.getByRole('button', { name: /browse: the menu|parcourir : le menu/i }).click()
    const panel = page.getByRole('group', { name: /^(browse|parcourir)$/i })
    await panel.getByRole('link', { name: /^(collections)$/i }).click()
    await expect(page).toHaveURL(/\/collections$/)
    // The page itself, not the "nothing lives at this address" one.
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(/^(the sagas|les sagas)$/i)
  })

  test('leads to the index of people, which the bar has no place for', async ({ page }) => {
    await page.goto('/')
    await page.getByRole('button', { name: /browse: the menu|parcourir : le menu/i }).click()
    const panel = page.getByRole('group', { name: /^(browse|parcourir)$/i })
    await panel.getByRole('link', { name: /^(people|personnes)$/i }).click()
    await expect(page).toHaveURL(/\/people$/)
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(/^(people|personnes)$/i)
  })

  test('the panel meets WCAG 2.1 AA', async ({ page }) => {
    await page.goto('/')
    await page.getByRole('button', { name: /browse: the menu|parcourir : le menu/i }).click()
    const panel = page.getByRole('group', { name: /^(browse|parcourir)$/i })
    await expect(panel.locator('a[href^="/genre/"][href$="?kind=movie"]').first()).toBeVisible()
    // The front page's plate is driven by the scroll, and runs for as long as
    // the page is there: only what time drives is waited for.
    await page.waitForFunction(() =>
      document.getAnimations().every((a) => a.playState !== 'running' || a.timeline !== document.timeline),
    )
    const results = await new AxeBuilder({ page })
      .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
      .analyze()
    expect(results.violations.map((v) => `${v.id}: ${v.help}`)).toEqual([])
  })
})

test.describe('the sheet on a phone', () => {
  // The tabs are the bar along the foot of the screen (see tabbar.spec.ts);
  // what its four places leave out is in the sheet its last one opens.
  const more = /^(more|plus)$/i

  test('lists the collections and the figures, which the bar has no place for', async ({ page, isMobile }) => {
    test.skip(!isMobile, 'the bar has the tabs at this width')
    await page.goto('/')
    await page.getByRole('button', { name: more }).click()
    const sheet = page.getByRole('dialog', { name: more })
    await expect(sheet.getByRole('link', { name: /^(collections)$/i })).toBeVisible()
    await expect(sheet.getByRole('link', { name: /^(in numbers|en chiffres)$/i })).toBeVisible()
  })

  test('and the collections in it lead to a page of their own', async ({ page, isMobile }) => {
    test.skip(!isMobile, 'the bar has the tabs at this width')
    await page.goto('/')
    await page.getByRole('button', { name: more }).click()
    await page.getByRole('dialog', { name: more }).getByRole('link', { name: /^(collections)$/i }).click()
    await expect(page).toHaveURL(/\/collections$/)
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(/^(the sagas|les sagas)$/i)
  })
})
