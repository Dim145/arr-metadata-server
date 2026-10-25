import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Page } from '@playwright/test'
import { escaped, pattern } from './support/regex'

/**
 * The catalogue's own pages — a genre's, a network's, a year's, a decade's —
 * and two works side by side.
 *
 * A page is reached from a work's chips and answers to any name its subject
 * goes by; it leads with its name, its count and the five rated highest, and
 * lists every one of its works below with the catalogue's own filters. The
 * works are looked up rather than named, so the tests read whatever the
 * catalogue holds; it needs a few dramas, which the suite's catalogue has.
 */

const WCAG = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']

interface Work {
  id: string
  title: string
  kind: 'series' | 'movie'
  year?: number
  network?: string
  studio?: string
  genres: string[]
}

async function works(page: Page, query: string): Promise<Work[]> {
  const response = await page.request.get(`/api/v1/items?${query}`)
  expect(response.ok(), await response.text()).toBeTruthy()
  return ((await response.json()) as { items: Work[] }).items
}

async function scan(page: Page, label: string) {
  test.slow()
  await page.waitForFunction(() =>
    document.getAnimations().every((a) => a.playState !== 'running' || a.timeline !== document.timeline),
  )
  const results = await new AxeBuilder({ page }).withTags(WCAG).analyze()
  const violations = results.violations.map((v) => ({
    rule: v.id,
    help: v.help,
    where: v.nodes.slice(0, 3).map((n) => n.target.join(' ')),
  }))
  expect(violations, `${label} has accessibility violations`).toEqual([])
}

test.describe('the pages of the catalogue', () => {
  test.beforeEach(async ({ page }) => {
    // English, so that the words looked for are the ones that are drawn.
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
  })

  test('a genre has a page: its name, its count, its five best and every one of its works', async ({ page }) => {
    const dramas = await works(page, 'genre=drama&limit=1')
    test.skip(!dramas.length, 'the catalogue holds no drama')

    await page.goto('/genre/drama')
    await expect(page.getByRole('heading', { level: 1, name: 'Drama' })).toBeVisible()
    const counted = page.getByText(/^(1 work|\d+ works) in the catalogue$/)
    await expect(counted).toBeVisible()
    const total = Number((await counted.textContent())?.match(/\d+/)?.[0] ?? '1')

    // The five rated highest, at most, each a work's page.
    const best = page.getByRole('region', { name: 'Rated highest' })
    await expect(best.locator('a[href^="/work/"]').first()).toBeVisible()
    const shelved = await best.locator('li').count()
    expect(shelved).toBeGreaterThan(0)
    expect(shelved).toBeLessThanOrEqual(5)

    // Then every one of them, in the catalogue's own list: the same count.
    await expect(page.getByRole('heading', { level: 2, name: 'Everything filed here' })).toBeVisible()
    await expect(page.getByText(pattern(`^${total} works?$`))).toBeVisible()
    await expect(page).toHaveTitle(/^Drama · /)
  })

  test('a work’s genre chip leads to its genre’s page, whatever name it goes by', async ({ page }) => {
    const [drama] = await works(page, 'genre=drama&limit=1')
    test.skip(!drama, 'the catalogue holds no drama')

    await page.goto(`/work/${drama!.id}`)
    await page.getByRole('link', { name: 'Drama', exact: true }).first().click()
    await expect(page).toHaveURL(/\/genre\/drama$/)
    await expect(page.getByRole('heading', { level: 1, name: 'Drama' })).toBeVisible()

    // By its French name, the same page, which takes the address it is called by.
    await page.goto('/genre/drame')
    await expect(page).toHaveURL(/\/genre\/drama$/)
    await expect(page.getByRole('heading', { level: 1, name: 'Drama' })).toBeVisible()
  })

  test('a genre’s page is named in the reader’s language', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'fr'))
    await page.goto('/genre/drama')
    await expect(page.getByRole('heading', { level: 1, name: 'Drame' })).toBeVisible()
    await expect(page.getByText(/œuvres? au catalogue$/)).toBeVisible()
  })

  test('a year and its decade have pages, each leading to the next', async ({ page }) => {
    const [dated] = await works(page, 'genre=drama&limit=1')
    test.skip(!dated?.year, 'no dated work to start from')
    const year = dated!.year!

    await page.goto(`/work/${dated!.id}`)
    await page.locator(`h1 a[href="/year/${year}"]`).click()
    await expect(page).toHaveURL(pattern(`/year/${year}$`))
    await expect(page.getByRole('heading', { level: 1, name: String(year) })).toBeVisible()

    const around = page.getByRole('navigation', { name: 'The years around it' })
    await around.getByRole('link', { name: String(year + 1) }).click()
    await expect(page).toHaveURL(pattern(`/year/${year + 1}$`))

    // A decade by any of its years, under its first.
    const first = year - (year % 10)
    await page.goto(`/decade/${first + 4}`)
    await expect(page).toHaveURL(pattern(`/decade/${first}$`))
    await expect(page.getByRole('heading', { level: 1, name: `The ${first}s` })).toBeVisible()
  })

  test('a network has a page, reached from the work it made', async ({ page }) => {
    const shown = (await works(page, 'kind=series&limit=20')).find((w) => w.network)
    test.skip(!shown, 'no series with a network')

    await page.goto(`/work/${shown!.id}`)
    await page.getByRole('link', { name: shown!.network!, exact: true }).first().click()
    await expect(page).toHaveURL(/\/network\/[a-z0-9-]+$/)
    await expect(page.getByRole('heading', { level: 1, name: shown!.network! })).toBeVisible()
    await expect(page.getByRole('heading', { level: 2, name: 'Everything filed here' })).toBeVisible()
  })

  test('a name nothing goes by is said to be one', async ({ page }) => {
    await page.goto('/genre/nothing-goes-by-this-name')
    await expect(page.getByText('Nothing is filed under that name')).toBeVisible()
  })

  test('a link to a genre’s page carries a preview of it', async ({ request }) => {
    const page = await (await request.get('/genre/drama')).text()
    expect(page).toContain('<meta property="og:title" content="Drama" />')
    expect(page).toContain('<meta property="og:type" content="website" />')
    expect(page).toMatch(/<meta property="og:description" content="[^"]+" \/>/)
  })

  test('the genre’s page meets WCAG 2.1 AA', async ({ page }) => {
    await page.goto('/genre/drama')
    await expect(page.getByRole('heading', { level: 1, name: 'Drama' })).toBeVisible()
    await expect(page.locator('a[href^="/work/"]').first()).toBeVisible()
    await scan(page, 'the genre’s page')
  })
})

test.describe('two works side by side', () => {
  test.beforeEach(async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
  })

  test('are compared by their facts and the people they share', async ({ page }) => {
    const [first, second] = await works(page, 'genre=drama&limit=2&sort=rating')
    test.skip(!first || !second, 'two works are needed')

    await page.goto(`/compare?a=${first!.id}&b=${second!.id}`)
    await expect(page.getByRole('heading', { level: 1, name: 'Two works side by side' })).toBeVisible()
    const table = page.getByRole('table')
    await expect(table.getByRole('columnheader', { name: first!.title })).toBeVisible()
    await expect(table.getByRole('columnheader', { name: second!.title })).toBeVisible()
    await expect(table.getByRole('rowheader', { name: 'Running time' })).toBeVisible()
    await expect(page.getByRole('region', { name: 'The people they share' })).toBeVisible()
    await scan(page, 'the comparison')

    // A side is changed by searching the catalogue in its column.
    await page.getByRole('region', { name: 'The second' }).getByRole('button', { name: 'Choose another' }).click()
    await expect(page).not.toHaveURL(/[?&]b=/)
    const search = page.getByRole('searchbox', { name: 'The second' })
    await search.fill(second!.title.slice(0, 6))
    await page.getByRole('button', { name: pattern(escaped(second!.title)) }).first().click()
    await expect(page).toHaveURL(pattern(`[?&]b=${escaped(second!.id)}`))
  })
})
