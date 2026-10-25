import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Page } from '@playwright/test'

/**
 * The styleguide as a screen: every gallery drawn, each column in its own
 * theme and language, and every component in every state it shows — both
 * themes, both languages — scanned against WCAG 2.1 AA, which no single page
 * of the site ever puts together. Run with the `visual` project, against a
 * build with the styleguide (`vite build --mode styleguide`); not bound to
 * Linux, as its pictures are (visual.spec.ts).
 */

const WCAG = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']

/** The galleries, as the styleguide lists them. */
async function galleries(page: Page): Promise<string[]> {
  await page.goto('/__styleguide')
  const links = page.getByRole('navigation', { name: 'Galleries' }).getByRole('link')
  await expect(links.first()).toBeVisible()
  return links.evaluateAll((all) =>
    all.map((link) => (link as HTMLAnchorElement).pathname.split('/')[2]).filter((name): name is string => Boolean(name)),
  )
}

test.beforeEach(async ({ page }) => {
  const entry = /<script[^>]+src="(\/assets\/index-[^"]+\.js)"/.exec(await (await page.request.get('/')).text())?.[1]
  const drawn = Boolean(entry && (await (await page.request.get(entry)).text()).includes('__styleguide'))
  test.skip(!drawn, 'This server’s build has no styleguide: build it with `vite build --mode styleguide`.')
})

test('wears each column’s theme and speaks its language', async ({ page }) => {
  await page.goto('/__styleguide/buttons')
  const dark = page.locator('[data-gallery="buttons"] td[data-column="dark-en"]').first()
  const light = page.locator('[data-gallery="buttons"] td[data-column="light-fr"]').first()
  await expect(light.getByRole('button', { name: 'Enregistrer' })).toBeVisible()
  await expect(dark.getByRole('button', { name: 'Save' })).toBeVisible()
  // The page itself stays dark whatever the reader chose; each column wears its own.
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')
  const ink = (cell: typeof dark) => cell.evaluate((element) => getComputedStyle(element).backgroundColor)
  expect(await ink(dark)).toBe('rgb(11, 11, 12)')
  expect(await ink(light)).toBe('rgb(246, 243, 238)')
  await expect(light.locator('[lang="fr"]').first()).toBeAttached()
})

test('forces a state with the rule that draws the real one', async ({ page }) => {
  await page.goto('/__styleguide/buttons')
  const rest = page.locator('[data-gallery="buttons"] tbody tr').nth(0).locator('td[data-column="dark-en"] button').first()
  const hovered = page.locator('[data-gallery="buttons"] tbody tr').nth(1).locator('td[data-column="dark-en"] button').nth(1)
  const ghostAtRest = page.locator('[data-gallery="buttons"] tbody tr').nth(0).locator('td[data-column="dark-en"] button').nth(1)
  await expect(rest).toBeVisible()
  await expect(hovered).toHaveAttribute('data-sg-hover', '')
  // The ghost button under a pointer takes the high ink, as `hover:bg-ink-high` says.
  const fill = (button: typeof rest) => button.evaluate((element) => getComputedStyle(element).backgroundColor)
  expect(await fill(hovered)).toBe('rgb(23, 23, 26)')
  expect(await fill(ghostAtRest)).not.toBe('rgb(23, 23, 26)')
})

test('puts every state of every component within WCAG 2.1 AA', async ({ page }) => {
  test.slow()
  for (const gallery of await galleries(page)) {
    await page.goto(`/__styleguide/${gallery}`)
    await expect(page.locator(`[data-gallery="${gallery}"]`)).toBeVisible()
    await expect(page.locator('td[data-column]:empty')).toHaveCount(0)
    await page.waitForFunction(() =>
      document.getAnimations().every((a) => a.playState !== 'running' || a.timeline !== document.timeline),
    )
    const results = await new AxeBuilder({ page }).include(`[data-gallery="${gallery}"]`).withTags(WCAG).analyze()
    const violations = results.violations.map((v) => ({
      rule: v.id,
      help: v.help,
      where: v.nodes.slice(0, 3).map((n) => n.target.join(' ')),
      count: v.nodes.length,
    }))
    expect(violations, `the ${gallery} gallery`).toEqual([])
  }
})
