import AxeBuilder from '@axe-core/playwright'
import { expect, test } from '@playwright/test'

/**
 * The way into the catalogue: one tab, with the shortcuts hanging from it.
 * The tab is a link; the panel opens under a resting pointer or from the
 * button beside it, and closes on Escape, on leaving, and on going anywhere.
 */
test.describe('the browse tab and its panel', () => {
  test.beforeEach(({ isMobile }) => {
    test.skip(isMobile, 'the tabs are behind the menu at this width')
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

    // The commonest genres of each kind, each a filter on that kind.
    const genre = panel.locator('a[href^="/browse?kind=series&genre="]').first()
    await expect(genre).toBeVisible()
    const href = await genre.getAttribute('href')
    await genre.click()
    await expect(page).toHaveURL(new RegExp(href!.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') + '$'))
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

  test('the panel meets WCAG 2.1 AA', async ({ page }) => {
    await page.goto('/')
    await page.getByRole('button', { name: /browse: the menu|parcourir : le menu/i }).click()
    const panel = page.getByRole('group', { name: /^(browse|parcourir)$/i })
    await expect(panel.locator('a[href^="/browse?kind=movie&genre="]').first()).toBeVisible()
    await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'))
    const results = await new AxeBuilder({ page })
      .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
      .analyze()
    expect(results.violations.map((v) => `${v.id}: ${v.help}`)).toEqual([])
  })
})

test.describe('the menu on a phone', () => {
  test('lists the series, the films and the collections beneath the catalogue', async ({ page, isMobile }) => {
    test.skip(!isMobile, 'the bar has the tab at this width')
    await page.goto('/')
    await page.getByRole('button', { name: /^(menu)$/i }).click()
    const drawer = page.getByRole('navigation', { name: /^(browse|parcourir)$/i }).last()
    await expect(drawer.getByRole('link', { name: /^(series|séries)$/i })).toBeVisible()
    await expect(drawer.getByRole('link', { name: /^(films)$/i })).toBeVisible()
    await expect(drawer.getByRole('link', { name: /^(collections)$/i })).toBeVisible()
  })
})
