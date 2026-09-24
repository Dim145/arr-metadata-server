import { expect, test, type APIRequestContext, type Page } from '@playwright/test'

/**
 * The season chart: a calendar quarter of the catalogue, past or to come.
 *
 * Its seasons are found through the API — the quarter a series' first episode
 * aired in — so this runs against whatever catalogue the instance holds, and
 * skips what that catalogue cannot show.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

const SEASONS = ['winter', 'spring', 'summer', 'autumn'] as const

interface Entry {
  workId: string
  kind: 'newSeries' | 'newSeason' | 'continuing' | 'film'
  seasonNumber?: number
  starts: string
}

interface Chart {
  from: string
  to: string
  entries: Entry[]
  works: { id: string; title: string; trailerYoutubeId?: string }[]
}

interface Found {
  year: number
  season: (typeof SEASONS)[number]
  chart: Chart
  path: string
}

/**
 * Seasons the catalogue has something in: for each of its most popular
 * series, the quarter its first regular episode aired in.
 */
async function seasons(request: APIRequestContext, most = 10): Promise<Found[]> {
  const { items } = await (await request.get(`/api/v1/items?kind=series&limit=${most}`)).json()
  const found: Found[] = []
  for (const { id } of items as { id: string }[]) {
    const work = await (await request.get(`/api/v1/items/${id}`)).json()
    const first = ((work.episodes ?? []) as { seasonNumber: number; airDate?: string }[])
      .filter((e) => e.seasonNumber > 0 && e.airDate)
      .map((e) => e.airDate!.slice(0, 10))
      .sort()[0]
    if (!first) continue

    const year = Number(first.slice(0, 4))
    const season = SEASONS[Math.floor((Number(first.slice(5, 7)) - 1) / 3)]!
    if (found.some((f) => f.year === year && f.season === season)) continue

    const chart = (await (await request.get(`/api/v1/seasons/${year}/${season}`)).json()) as Chart
    if (chart.entries.length) found.push({ year, season, chart, path: `/seasons/${year}/${season}` })
  }
  return found
}

/** Where a card of the chart leads: a season to its own page, a premiere to its work. */
function cardHref(entry: Entry) {
  return entry.seasonNumber !== undefined && (entry.kind === 'newSeason' || entry.kind === 'continuing')
    ? `/work/${entry.workId}/season/${entry.seasonNumber}`
    : `/work/${entry.workId}`
}

async function scrollsSideways(page: Page) {
  return page.evaluate(() => {
    window.scrollTo(600, 0)
    const moved = window.scrollX > 0
    window.scrollTo(0, 0)
    return moved
  })
}

test.describe('the season chart', () => {
  test('opens on the season the reader is in', async ({ page }) => {
    await page.goto('/seasons')
    await expect(page).toHaveURL(/\/seasons\/\d{4}\/(winter|spring|summer|autumn)$/)

    // The reader's own calendar, not the server's.
    const [year, quarter] = await page.evaluate(() => {
      const now = new Date()
      return [now.getFullYear(), Math.floor(now.getMonth() / 3)]
    })
    expect(page.url()).toMatch(new RegExp(`/seasons/${year}/${SEASONS[quarter!]}$`))
    await expect(page.getByRole('heading', { level: 1 })).toContainText(String(year))

    // Already there: nothing offers to go back to it.
    await expect(page.getByRole('link', { name: /^(this season|cette saison)$/i })).toHaveCount(0)
  })

  test('steps a season, a year, to any season of it, and back to this one', async ({ page }) => {
    await page.goto('/seasons/2019/spring')
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(/(spring|printemps)\s*2019/i)

    await page.getByRole('link', { name: /next season|saison suivante/i }).click()
    await expect(page).toHaveURL(/\/seasons\/2019\/summer$/)
    await page.getByRole('link', { name: /previous season|saison précédente/i }).click()
    await expect(page).toHaveURL(/\/seasons\/2019\/spring$/)

    // Autumn is followed by the next year's winter.
    await page.goto('/seasons/2019/autumn')
    await page.getByRole('link', { name: /next season|saison suivante/i }).click()
    await expect(page).toHaveURL(/\/seasons\/2020\/winter$/)

    const picker = page.getByRole('navigation', { name: /^(seasons|saisons)$/i })
    await picker.getByRole('link', { name: /year after|année suivante/i }).click()
    await expect(page).toHaveURL(/\/seasons\/2021\/winter$/)

    const autumn = picker.getByRole('link', { name: /^(autumn|automne)$/i })
    await autumn.click()
    await expect(page).toHaveURL(/\/seasons\/2021\/autumn$/)
    await expect(autumn).toHaveAttribute('aria-current', 'page')

    await page.getByRole('link', { name: /^(this season|cette saison)$/i }).click()
    const [year, quarter] = await page.evaluate(() => {
      const now = new Date()
      return [now.getFullYear(), Math.floor(now.getMonth() / 3)]
    })
    await expect(page).toHaveURL(new RegExp(`/seasons/${year}/${SEASONS[quarter!]}$`))
  })

  test('lists a series in the quarter it began, and leads to it', async ({ page, request }) => {
    const [found] = await seasons(request)
    test.skip(!found, 'no series has a dated episode')

    await page.goto(found!.path)
    for (const entry of found!.chart.entries.slice(0, 5)) {
      await expect(page.locator(`article a[href="${cardHref(entry)}"]`)).toBeVisible()
    }

    const entry = found!.chart.entries[0]!
    await page.locator(`article a[href="${cardHref(entry)}"]`).click()
    await expect(page).toHaveURL(new RegExp(`${cardHref(entry)}$`))
  })

  test('keeps its filters in the address, and counts what each would leave', async ({ page, request }) => {
    // Two kinds at least, or narrowing to one would leave the season as it was.
    const found = (await seasons(request)).find((f) => new Set(f.chart.entries.map((e) => e.kind)).size > 1)
    test.skip(!found, 'no season holds two kinds of entry')
    const total = found!.chart.entries.length

    await page.goto(found!.path)
    const kinds = page.getByRole('group', { name: /^(kind|type)$/i })
    const everything = kinds.getByRole('radio', { name: /^(all|tout)\b/i })
    await expect(everything).toBeChecked()
    await expect(page.locator('main article')).toHaveCount(total)

    // The first kind the season has, by the count it shows.
    const kind = kinds.getByRole('radio').nth(1)
    const label = (await kind.evaluate((input) => input.closest('label')!.textContent))!
    const shown = Number(label.match(/(\d+)\s*$/)![1])
    expect(shown).toBeLessThan(total)
    // By its label, as a pointer chooses it: the radio itself is drawn as
    // the pill round it.
    await kind.locator('xpath=..').click()
    await expect(page).toHaveURL(/[?&]type=\w+/)
    await expect(kind).toBeChecked()
    await expect(page.locator('main article')).toHaveCount(shown)

    // Linked or reloaded, the view is the same — and so is the next season's.
    await page.reload()
    await expect(kinds.getByRole('radio').nth(1)).toBeChecked()
    await expect(page.locator('main article')).toHaveCount(shown)
    await page.getByRole('link', { name: /next season|saison suivante/i }).click()
    await expect(page).toHaveURL(/[?&]type=\w+/)

    await page.goBack()
    await everything.locator('xpath=..').click()
    await expect(page).not.toHaveURL(/type=/)
    await expect(page.locator('main article')).toHaveCount(total)
  })

  test('leads from a frame of its strip to that week, set out by date', async ({ page, request }) => {
    const found = (await seasons(request)).find((f) => f.chart.entries.some((e) => e.kind !== 'continuing'))
    test.skip(!found, 'no season opens anything')

    await page.goto(found!.path)
    const strip = page.getByRole('navigation', { name: /week by week|semaine par semaine/i })
    await strip.getByRole('link').first().click()

    await expect(page).toHaveURL(/[?&]sort=date\b.*#week-\d{4}-\d{2}-\d{2}$/)
    const week = new URL(page.url()).hash.slice(1)
    const section = page.locator(`section[id="${week}"]`)
    await expect(section).toBeFocused()
    await expect(section.locator('article').first()).toBeInViewport()
  })

  test('plays a trailer from its card, and gives focus back to it', async ({ page, request }) => {
    const found = (await seasons(request, 20)).find((f) => f.chart.works.some((w) => w.trailerYoutubeId))
    test.skip(!found, 'no season holds a work with a trailer')

    // Played for real, the frame would be YouTube's; what is tested is ours.
    await page.route(/youtube(-nocookie)?\.com/, (route) => route.abort())

    await page.goto(found!.path)
    const play = page.getByRole('button', { name: /^(trailer|bande-annonce)\s?: /i }).first()
    await play.click()
    await expect(page.getByRole('dialog')).toBeVisible()
    await expect(page.getByRole('dialog').locator('iframe')).toHaveCount(1)

    // By its own button as much as by Escape.
    await page.getByRole('dialog').getByRole('button', { name: /^(close|fermer)$/i }).click()
    await expect(page.locator('iframe')).toHaveCount(0)
    await expect(play).toBeFocused()

    await play.click()
    await page.keyboard.press('Escape')
    await expect(page.locator('iframe')).toHaveCount(0)
    await expect(play).toBeFocused()
  })

  test('says so when there is no such season', async ({ page }) => {
    for (const path of ['/seasons/2019/monsoon', '/seasons/1700/spring', '/seasons/twenty/spring']) {
      await page.goto(path)
      await expect(
        page.getByRole('heading', { name: /nothing lives at this address|rien à cette adresse/i }),
        path,
      ).toBeVisible()
    }
  })

  test('says a season it holds nothing of is empty, and offers no filters for it', async ({ page }) => {
    await page.goto('/seasons/1890/winter')
    await expect(page.getByText(/^(nothing from this season|rien de cette saison)/i)).toBeVisible()
    await expect(page.getByRole('group', { name: /^(kind|type)$/i })).toHaveCount(0)
    await expect(page.getByRole('navigation', { name: /week by week|semaine par semaine/i })).toHaveCount(0)
  })

  test('does not scroll sideways', async ({ page, request }) => {
    const [found] = await seasons(request)
    await page.goto(found?.path ?? '/seasons')
    await page.waitForLoadState('networkidle')
    expect(await scrollsSideways(page)).toBe(false)
  })

  test('is a step away from the schedule', async ({ page }) => {
    await page.goto('/calendar')
    await page.getByRole('link', { name: /season chart|calendrier saisonnier/i }).click()
    await expect(page).toHaveURL(/\/seasons\/\d{4}\/(winter|spring|summer|autumn)$/)
  })
})

test.describe('the season chart, to whoever maintains the catalogue', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs a credential; see admin.spec.ts')

  test('offers what TMDB lists that the catalogue lacks, each with its page there', async ({ page }) => {
    await page.goto('/login')
    await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
    await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
    await page.getByRole('button', { name: /sign in|se connecter/i }).click()
    await page.waitForURL('**/admin')

    // The season after this one: what is to come is what there is to import.
    const next = await page.evaluate(() => {
      const now = new Date()
      const index = now.getFullYear() * 4 + Math.floor(now.getMonth() / 3) + 1
      return { year: Math.floor(index / 4), quarter: index % 4 }
    })
    const path = `/seasons/${next.year}/${SEASONS[next.quarter]}`

    await page.goto(path)
    // Asked in the page's own language, as the page asks: the same list.
    const lang = await page.evaluate(() => document.documentElement.lang)
    const response = await page.request.get(`/api/v1${path}/candidates?language=${lang}`)
    test.skip(response.status() === 503, 'the instance has no TMDB key')
    expect(response.status()).toBe(200)
    const candidates = (await response.json()) as { kind: string; tmdbId: number }[]
    test.skip(!candidates.length, 'TMDB lists nothing the catalogue lacks for the season')

    const panel = page.getByRole('region', { name: /not in the catalogue|pas encore au catalogue/i })
    await expect(panel).toBeVisible()
    await expect(panel.getByRole('button', { name: /^(import|importer): /i }).first()).toBeVisible()

    // Each offered with its page on TMDB, to judge it by.
    const first = candidates[0]!
    const tmdb = panel.locator(
      `a[href="https://www.themoviedb.org/${first.kind === 'series' ? 'tv' : 'movie'}/${first.tmdbId}"]`,
    )
    await expect(tmdb).toHaveAttribute('target', '_blank')
  })
})
