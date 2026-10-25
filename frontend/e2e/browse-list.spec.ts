import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Page } from '@playwright/test'
import { escaped, pattern } from './support/regex'

/**
 * The catalogue as a list, beside the grid it is also offered as.
 *
 * `?view=list` in the address, and what the reader chose last remembered in
 * this browser (`ams.browse.view`) for an address that says nothing. A row is
 * a poster's thumbnail and the words a grid has no room for, with one link:
 * the work's.
 */

const KEY = 'ams.browse.view'

const grid = (page: Page) => page.getByRole('button', { name: /^(grid|grille)$/i })
const list = (page: Page) => page.getByRole('button', { name: /^(list|liste)$/i })
/** The rows, and nothing else that is a list item on the page. */
const rows = (page: Page) => page.locator('main ul > li > article')
const remembered = (page: Page) => page.evaluate((key) => localStorage.getItem(key), KEY)

test.describe('the catalogue as a list', () => {
  test.beforeEach(async ({ page }) => {
    // English, so that the words looked for are the ones that are drawn.
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
  })

  test('is a toggle away from the grid, and the address says so', async ({ page }) => {
    await page.goto('/browse')
    await expect(page.locator('a[href^="/work/"]').first()).toBeVisible()
    await expect(grid(page)).toHaveAttribute('aria-pressed', 'true')
    await expect(list(page)).toHaveAttribute('aria-pressed', 'false')
    const shown = await page.locator('a[href^="/work/"]').count()
    await expect(rows(page)).toHaveCount(0)

    await list(page).click()
    await expect(page).toHaveURL(/[?&]view=list/)
    await expect(list(page)).toHaveAttribute('aria-pressed', 'true')
    await expect(grid(page)).toHaveAttribute('aria-pressed', 'false')

    // The same works, a row each, and the work's link is the one link of a row.
    await expect(rows(page)).toHaveCount(shown)
    for (const row of await rows(page).all()) {
      await expect(row.locator('a')).toHaveCount(1)
      await expect(row.locator('a[href^="/work/"]')).toHaveCount(1)
    }

    await grid(page).click()
    await expect(page).not.toHaveURL(/view=/)
    await expect(rows(page)).toHaveCount(0)
    await expect(page.locator('a[href^="/work/"]').first()).toBeVisible()
  })

  test('says what a row has room for: the title, the year and kind, three genres at most, two lines of synopsis', async ({
    page,
    request,
  }) => {
    const { items } = (await (await request.get('/api/v1/items?sort=popularity&limit=1&language=en')).json()) as {
      items: { title: string; year?: number; kind: string }[]
    }
    const first = items[0]
    test.skip(!first, 'the catalogue is empty')

    await page.goto('/browse?view=list')
    const row = rows(page).first()
    await expect(row).toBeVisible()
    await expect(row.getByRole('heading', { level: 3 })).toContainText(first!.title)
    await expect(row).toContainText(first!.kind === 'series' ? /series/i : /film/i)
    if (first!.year) await expect(row).toContainText(String(first!.year))

    for (const each of await rows(page).all()) {
      // A genre is a chip in a list of its own; no row names more than three.
      expect(await each.locator('ul li').count()).toBeLessThanOrEqual(3)
    }

    // The synopsis, held to two lines whatever its length.
    const synopsis = rows(page).locator('p.line-clamp-2').first()
    if (await synopsis.count()) await expect(synopsis).toHaveCSS('-webkit-line-clamp', '2')

    // A long list is not laid out to be scrolled: the browser may skip what
    // is far from the screen.
    await expect(rows(page).first().locator('xpath=..')).toHaveCSS('content-visibility', 'auto')
  })

  test('is remembered for the next visit, and what the address says wins', async ({ page }) => {
    await page.goto('/browse')
    await list(page).click()
    expect(await remembered(page)).toBe('list')

    // A bare address is the reader's own view; the address stays bare.
    await page.goto('/browse')
    await expect(list(page)).toHaveAttribute('aria-pressed', 'true')
    await expect(page).not.toHaveURL(/view=/)
    await expect(rows(page).first()).toBeVisible()

    // A link sent as the grid is the grid, whatever is remembered.
    await page.goto('/browse?view=grid')
    await expect(grid(page)).toHaveAttribute('aria-pressed', 'true')
    await expect(rows(page)).toHaveCount(0)

    // And choosing the grid is remembered too.
    await page.goto('/browse')
    await grid(page).click()
    expect(await remembered(page)).toBe('grid')
    await page.goto('/browse')
    await expect(grid(page)).toHaveAttribute('aria-pressed', 'true')
  })

  test('works where the browser will not say what was remembered', async ({ page }) => {
    // A store that refuses the read — private browsing, disabled storage.
    await page.addInitScript((key) => {
      // eslint-disable-next-line @typescript-eslint/unbound-method -- called below, with the storage as `this`
      const original = Storage.prototype.getItem
      Storage.prototype.getItem = function (name: string) {
        if (name === key) throw new Error('denied')
        return original.call(this, name)
      }
    }, KEY)

    await page.goto('/browse')
    await expect(grid(page)).toHaveAttribute('aria-pressed', 'true')
    await expect(page.locator('a[href^="/work/"]').first()).toBeVisible()
    await list(page).click()
    await expect(rows(page).first()).toBeVisible()
  })

  test('keeps the view when the filters are cleared, and the row is the link', async ({ page }) => {
    await page.goto('/browse?view=list&kind=series')
    await expect(rows(page).first()).toBeVisible()

    // "Clear all", at the head of the filters (above the results on a phone).
    await page.getByRole('button', { name: /^(clear all|tout effacer)$/i }).click()
    await expect(page).toHaveURL(/view=list/)
    await expect(page).not.toHaveURL(/kind=/)

    // Anywhere on the row opens the work, not the title alone.
    const row = rows(page).first()
    const href = await row.locator('a[href^="/work/"]').getAttribute('href')
    await row.click({ position: { x: 24, y: 24 } })
    expect(href).toBeTruthy()
    await expect(page).toHaveURL(pattern(`${escaped(href!)}$`))
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
  })

  test('meets WCAG 2.1 AA', async ({ page }) => {
    await page.goto('/browse?view=list')
    await expect(rows(page).first()).toBeVisible()
    await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'))
    const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze()
    expect(results.violations.map((v) => `${v.id}: ${v.help}`)).toEqual([])
  })
})
