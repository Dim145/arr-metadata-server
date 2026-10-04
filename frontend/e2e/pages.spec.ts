import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { escaped, pattern } from './support/regex'

/**
 * The pages a work leads to, and the ways out of it.
 *
 * A season's contact sheet, an episode's card, somebody's filmography, the
 * week's schedule, the artwork and the trailer — and the identifiers, which
 * open the providers' own pages. The works are looked up through the API
 * rather than named, so this runs against whatever catalogue the instance
 * holds, and skips what that catalogue cannot show.
 */

interface Episode {
  seasonNumber: number
  episodeNumber: number
  title: string
  airDate?: string
  airDateUtc?: string
  tvdbId?: number
}

interface Work {
  id: string
  title: string
  episodes?: Episode[]
  credits?: { personName: string; tmdbPersonId?: number }[]
  images?: { coverType: string; seasonNumber?: number | null }[]
  trailerYoutubeId?: string
  externalIds: { tmdb?: number; tvdb?: number; imdb?: string }
}

/** The kinds the gallery offers, in its order, and how many of each the work has. */
const GALLERY = ['poster', 'fanart', 'landscape', 'banner', 'clearlogo', 'clearart']

function galleryKinds(work: Work): [string, number][] {
  // The work's own pictures, as the gallery shows them: not the seasons'.
  const own = (work.images ?? []).filter((i) => i.seasonNumber === undefined || i.seasonNumber === null)
  return GALLERY.map((kind) => [kind, own.filter((i) => i.coverType === kind).length] as [string, number]).filter(
    ([, n]) => n > 0,
  )
}

/** The most popular series whose record has episodes outside the specials. */
async function aSeries(request: APIRequestContext): Promise<Work | undefined> {
  const { items } = await (await request.get('/api/v1/items?kind=series&limit=10')).json()
  for (const { id } of items as { id: string }[]) {
    const work = (await (await request.get(`/api/v1/items/${id}`)).json()) as Work
    if (work.episodes?.some((e) => e.seasonNumber > 0)) return work
  }
  return undefined
}

/** The first regular season, its episodes in order. */
function firstSeason(work: Work) {
  const regular = work.episodes!.filter((e) => e.seasonNumber > 0)
  const number = Math.min(...regular.map((e) => e.seasonNumber))
  const episodes = regular
    .filter((e) => e.seasonNumber === number)
    .sort((a, b) => a.episodeNumber - b.episodeNumber)
  return { number, episodes }
}

/** The Monday of a date's week, as the schedule addresses weeks. */
function mondayOf(date: string): string {
  const [year, month, day] = date.split('-').map(Number)
  const at = new Date(Date.UTC(year!, month! - 1, day!))
  at.setUTCDate(at.getUTCDate() - ((at.getUTCDay() + 6) % 7))
  return at.toISOString().slice(0, 10)
}

async function scrollsSideways(page: Page) {
  return page.evaluate(() => {
    window.scrollTo(600, 0)
    const moved = window.scrollX > 0
    window.scrollTo(0, 0)
    return moved
  })
}

test.describe('a season', () => {
  test('is reached from its work, and lists every episode as a way to its page', async ({
    page,
    request,
  }) => {
    const work = await aSeries(request)
    test.skip(!work, 'the catalogue holds no series with episodes')
    const { number, episodes } = firstSeason(work!)

    await page.goto(`/work/${work!.id}`)
    await page.locator(`a[href="/work/${work!.id}/season/${number}"]`).first().click()
    await expect(page).toHaveURL(pattern(`/work/${work!.id}/season/${number}$`))
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()

    // Three levels down, so the way back up is spelled out.
    const trail = page.getByRole('navigation', { name: /where you are|vous êtes ici/i })
    await expect(trail.getByRole('link', { name: work!.title, exact: true })).toBeVisible()
    await expect(trail.locator('[aria-current="page"]')).toHaveCount(1)

    await expect(
      page.locator(`main a[href^="/work/${work!.id}/season/${number}/episode/"]`),
    ).toHaveCount(episodes.length)
    expect(await scrollsSideways(page)).toBe(false)
  })

  test('that does not exist says so', async ({ page, request }) => {
    const work = await aSeries(request)
    test.skip(!work, 'the catalogue holds no series with episodes')

    await page.goto(`/work/${work!.id}/season/999`)
    await expect(
      page.getByRole('heading', { name: /nothing lives at this address|rien à cette adresse/i }),
    ).toBeVisible()
  })
})

test.describe('an episode', () => {
  test('has its own page, a way to TheTVDB’s, and the episodes either side', async ({
    page,
    request,
  }) => {
    const work = await aSeries(request)
    test.skip(!work, 'the catalogue holds no series with episodes')
    const { number, episodes } = firstSeason(work!)
    test.skip(episodes.length < 2, 'the first season has a single episode')
    const [first, second] = episodes as [Episode, Episode]

    await page.goto(`/work/${work!.id}/season/${number}/episode/${first.episodeNumber}`)
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(
      first.title || pattern(`^(Episode|Épisode) ${first.episodeNumber}$`),
    )

    if (first.tvdbId) {
      // Offered in the header and again in the record: every one opens beside
      // the catalogue, and tells TheTVDB nothing about where it came from.
      const tvdb = page.locator(`a[href="https://thetvdb.com/dereferrer/episode/${first.tvdbId}"]`)
      await expect(tvdb.first()).toBeVisible()
      for (const link of await tvdb.all()) {
        await expect(link).toHaveAttribute('target', '_blank')
        await expect(link).toHaveAttribute('rel', /noreferrer/)
      }
    }

    await page.locator('a[rel="next"]').click()
    await expect(page).toHaveURL(
      pattern(`/work/${work!.id}/season/${number}/episode/${second.episodeNumber}$`),
    )
    expect(await scrollsSideways(page)).toBe(false)
  })
})

test.describe('identifiers', () => {
  test('open the providers’ own pages in a new tab, and say so', async ({ page, request }) => {
    const work = await aSeries(request)
    test.skip(!work, 'the catalogue holds no series with episodes')

    const { tvdb, tmdb, imdb } = work!.externalIds
    const expected = [
      tvdb && `https://thetvdb.com/dereferrer/series/${tvdb}`,
      tmdb && `https://www.themoviedb.org/tv/${tmdb}`,
      imdb && `https://www.imdb.com/title/${imdb}/`,
    ].filter((href): href is string => Boolean(href))
    test.skip(!expected.length, 'the series carries no identifier with a page to open')

    await page.goto(`/work/${work!.id}`)

    for (const href of expected) {
      const link = page.locator(`a[href="${href}"]`).first()
      await expect(link, href).toHaveAttribute('target', '_blank')
      // No referrer: the provider learns nothing about who reads this catalogue.
      await expect(link, href).toHaveAttribute('rel', /noreferrer/)
      await expect(link, href).toHaveAccessibleName(/new tab|nouvel onglet/i)
    }
  })
})

test.describe('a person', () => {
  test('is reached from the cast, and lists the work they were reached from', async ({
    page,
    request,
  }) => {
    const work = await aSeries(request)
    const credit = work?.credits?.find((c) => c.tmdbPersonId)
    test.skip(!credit, 'no credit carries a TMDB person id')

    await page.goto(`/work/${work!.id}`)
    await page.locator(`a[href="/person/${credit!.tmdbPersonId}"]`).first().click()

    await expect(page).toHaveURL(pattern(`/person/${credit!.tmdbPersonId}$`))
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(credit!.personName)
    await expect(page.locator(`main a[href="/work/${work!.id}"]`)).toBeVisible()

    // Only what this catalogue holds is listed; the rest of the career is on TMDB.
    await expect(
      page.locator(`a[href="https://www.themoviedb.org/person/${credit!.tmdbPersonId}"]`),
    ).toHaveAttribute('target', '_blank')
  })

  test('says who they are, as TMDB has it, when TMDB answers', async ({ page, request }) => {
    const work = await aSeries(request)
    const credit = work?.credits?.find((c) => c.tmdbPersonId)
    test.skip(!credit, 'no credit carries a TMDB person id')

    const response = await request.get(`/api/v1/people/${credit!.tmdbPersonId}?language=en`)
    expect(response.status()).toBe(200)
    const person = await response.json()
    test.skip(!person.details, 'TMDB is not configured, so nobody has a biography here')

    // What TMDB says, read down to the page: a date, a department, portraits.
    if (person.details.birthday) expect(person.details.birthday).toMatch(/^\d{4}-\d{2}-\d{2}$/)
    expect(Array.isArray(person.details.photos ?? [])).toBe(true)

    await page.goto(`/person/${credit!.tmdbPersonId}`)
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(credit!.personName)
    if (person.details.biography) {
      await expect(page.getByText(/^(biography|biographie)$/i)).toBeVisible()
    }
    if (person.details.birthday) {
      await expect(page.getByText(/^(born |né·e le )/i)).toBeVisible()
    }
    if (person.details.imdbId) {
      await expect(
        page.locator(`a[href="https://www.imdb.com/name/${person.details.imdbId}/"]`),
      ).toHaveAttribute('target', '_blank')
    }
  })

  test('nobody by that id says so', async ({ page }) => {
    await page.goto('/person/999999999')
    await expect(
      page.getByRole('heading', { name: /nothing lives at this address|rien à cette adresse/i }),
    ).toBeVisible()
  })
})

test.describe('related works', () => {
  test('lead to the work here when it is held, and to AniList when it is not', async ({
    page,
    request,
  }) => {
    // A work AniList filed others beside: only an anime refreshed with
    // AniList switched on carries any, so this may have nothing to look at.
    const { items } = await (await request.get('/api/v1/items?limit=60')).json()
    let related:
      | { id: string; relations: { title: string; workId?: string; externalId: number; medium: string }[] }
      | undefined
    for (const { id } of items as { id: string }[]) {
      const work = await (await request.get(`/api/v1/items/${id}`)).json()
      if (work.relations?.length) {
        related = work
        break
      }
    }
    test.skip(!related, 'no work carries relations; switch AniList on and refresh an anime')

    await page.goto(`/work/${related!.id}`)
    await expect(page.getByText(/^(related works|œuvres liées)$/i)).toBeVisible()

    const first = related!.relations[0]
    const card = page.getByRole('link', { name: first.title.slice(0, 24) }).first()
    await expect(card).toBeVisible()
    if (first.workId) {
      await expect(card).toHaveAttribute('href', `/work/${first.workId}`)
    } else {
      await expect(card).toHaveAttribute(
        'href',
        `https://anilist.co/${first.medium === 'manga' ? 'manga' : 'anime'}/${first.externalId}`,
      )
      await expect(card).toHaveAttribute('rel', /noreferrer/)
    }
  })
})

test.describe('the other orders', () => {
  test('are offered where TheTVDB keeps them, and the aired order stays the way in', async ({
    page,
    request,
  }) => {
    const { items } = await (await request.get('/api/v1/items?kind=series&limit=30')).json()
    let found: { id: string; orders: { kind: string; episodes: { seasonNumber: number }[] }[] } | undefined
    for (const { id } of items as { id: string }[]) {
      const response = await request.get(`/api/v1/items/${id}/orders`)
      expect(response.status()).toBe(200)
      const { orders } = await response.json()
      if (orders.length) {
        found = { id, orders }
        break
      }
    }
    test.skip(!found, 'no series here is numbered another way; refresh one with TheTVDB on')

    const order = found!.orders[0]
    const season = order.episodes[0].seasonNumber
    await page.goto(`/work/${found!.id}/season/${season}?order=${order.kind}`)
    const tabs = page.getByRole('group', { name: /numbering|numérotation/i })
    await expect(tabs).toBeVisible()
    await expect(tabs.locator('a[aria-current="page"]')).toHaveCount(1)
    await expect(page.getByText(/as thetvdb numbers them|telle que thetvdb/i)).toBeVisible()

    // Back to the aired order, which is the page's own.
    await tabs.getByRole('link', { name: /^(as aired|diffusion)$/i }).click()
    await expect(page).toHaveURL(/\/season\/\d+$/)
    await expect(page.getByText(/as thetvdb numbers them|telle que thetvdb/i)).toHaveCount(0)
  })
})

test.describe('the schedule', () => {
  test('lists an episode in the week it aired, and leads to it', async ({ page, request }) => {
    const work = await aSeries(request)
    const dated = work?.episodes?.find((e) => e.seasonNumber > 0 && e.airDate)
    test.skip(!dated, 'no episode has an air date')

    // The day it is listed on, as the page files it: where a provider knew the
    // moment, the browser's own date of it — a US evening is the next morning
    // in Paris — and otherwise the date as given.
    await page.goto('/')
    const day = dated!.airDateUtc?.includes('T')
      ? await page.evaluate((utc) => {
          const d = new Date(utc)
          const pad = (n: number) => String(n).padStart(2, '0')
          return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
        }, dated!.airDateUtc)
      : dated!.airDate!

    await page.goto(`/calendar?week=${mondayOf(day)}`)

    const entry = page.locator(
      `a[href="/work/${work!.id}/season/${dated!.seasonNumber}/episode/${dated!.episodeNumber}"]`,
    )
    await expect(entry).toBeVisible()

    // The week at a glance leads to the day.
    const strip = page.getByRole('list', { name: /days of the week|jours de la semaine/i })
    await expect(strip.locator(`a[href="#day-${day}"]`)).toBeVisible()
    await strip.locator(`a[href="#day-${day}"]`).click()
    await expect(page).toHaveURL(pattern(`#day-${day}$`))

    await entry.click()
    await expect(page).toHaveURL(/\/episode\/\d+$/)
  })

  test('moves a week at a time, and back to this one', async ({ page }) => {
    await page.goto('/calendar')

    const thisWeek = page.getByRole('button', { name: /^(this week|cette semaine)$/i })
    await expect(thisWeek).toBeDisabled()

    await page.getByRole('button', { name: /next week|semaine suivante/i }).click()
    await expect(page).toHaveURL(/[?&]week=\d{4}-\d{2}-\d{2}/)
    await expect(thisWeek).toBeEnabled()

    await thisWeek.click()
    await expect(page).not.toHaveURL(/week=/)
    expect(await scrollsSideways(page)).toBe(false)
  })

  test('shows the month at a glance, and leads back to the week', async ({ page, request, isMobile }) => {
    await page.goto('/calendar?view=month')

    const views = page.getByRole('group', { name: /^(view|affichage)$/i })
    await expect(views.getByRole('link', { name: /^(month|mois)$/i })).toHaveAttribute('aria-current', 'true')

    // Whole weeks, Monday to Sunday, around the month — once the listing has
    // arrived and the grid with it.
    const grid = page.getByRole('list', { name: /days of the month|jours du mois/i })
    await expect(grid.locator('li').first()).toBeVisible()
    const days = await grid.locator('li').count()
    expect(days % 7).toBe(0)
    expect(days).toBeGreaterThanOrEqual(28)
    expect(days).toBeLessThanOrEqual(42)
    await expect(grid.locator('[aria-current="date"]')).toHaveCount(1)

    // What the server lists for the month is in the grid, on a screen wide
    // enough to list it; a phone marks the days and leads to the week.
    const now = new Date()
    const from = new Date(now.getFullYear(), now.getMonth(), 1).toISOString()
    const to = new Date(now.getFullYear(), now.getMonth() + 1, 1).toISOString()
    const month = await (await request.get(`/api/v1/calendar?from=${from}&to=${to}`)).json()
    if (!isMobile && month.episodes.length) {
      expect(await grid.locator('a[href*="/episode/"]').count()).toBeGreaterThan(0)
    }

    const thisMonth = page.getByRole('button', { name: /^(this month|ce mois-ci)$/i })
    await expect(thisMonth).toBeDisabled()
    await page.getByRole('button', { name: /next month|mois suivant/i }).click()
    await expect(page).toHaveURL(/[?&]month=\d{4}-\d{2}/)
    await expect(thisMonth).toBeEnabled()
    await thisMonth.click()
    await expect(page).not.toHaveURL(/month=/)

    await views.getByRole('link', { name: /^(week|semaine)$/i }).click()
    await expect(page).toHaveURL(/\/calendar$/)
    await expect(page.getByRole('button', { name: /^(this week|cette semaine)$/i })).toBeVisible()
    expect(await scrollsSideways(page)).toBe(false)
  })
})

test.describe('the artwork', () => {
  test('opens one picture at a time, moves with the arrow keys, and gives focus back', async ({
    page,
    request,
  }) => {
    const work = await aSeries(request)
    // The viewer moves within the first kind the gallery opens on.
    const first = work ? galleryKinds(work)[0] : undefined
    test.skip(!first || first[1] < 2, 'the gallery opens on a kind with a single picture')

    await page.goto(`/work/${work!.id}`)
    const opener = page.getByRole('button', { name: /^(open artwork|ouvrir l’image) 1$/i })
    await expect(opener).toBeVisible()
    await opener.click()

    const viewer = page.getByRole('dialog')
    await expect(viewer).toBeVisible()
    await expect(viewer.getByText(/^1 (of|sur) \d+$/)).toBeVisible()

    await page.keyboard.press('ArrowRight')
    await expect(viewer.getByText(/^2 (of|sur) \d+$/)).toBeVisible()

    await page.keyboard.press('Escape')
    await expect(viewer).toBeHidden()
    await expect(opener).toBeFocused()
  })
})

test.describe('the kinds of artwork', () => {
  test('are one stop for the Tab key, with the arrow keys between them', async ({ page, request }) => {
    const work = await aSeries(request)
    test.skip(!work || galleryKinds(work).length < 2, 'the series has a single kind of artwork')

    await page.goto(`/work/${work!.id}`)
    const tabs = page.getByRole('tablist').getByRole('tab')
    const first = tabs.first()
    await expect(first).toHaveAttribute('aria-selected', 'true')
    await expect(tabs.nth(1)).toHaveAttribute('tabindex', '-1')

    await first.focus()
    await page.keyboard.press('ArrowRight')

    const second = tabs.nth(1)
    await expect(second).toBeFocused()
    await expect(second).toHaveAttribute('aria-selected', 'true')
    await expect(page.getByRole('tabpanel')).toHaveAttribute('aria-labelledby', (await second.getAttribute('id'))!)

    await page.keyboard.press('Home')
    await expect(first).toBeFocused()
  })
})

test.describe('a trailer', () => {
  test('asks nothing of YouTube until it is played', async ({ page, request }) => {
    const { items } = await (await request.get('/api/v1/items?limit=50')).json()
    const work = (items as Work[]).find((item) => item.trailerYoutubeId)
    test.skip(!work, 'no work has a trailer')

    // Played for real, the frame would be YouTube's; what is tested is ours.
    const asked: string[] = []
    await page.route(/youtube(-nocookie)?\.com/, (route) => {
      asked.push(route.request().url())
      return route.abort()
    })

    await page.goto(`/work/${work!.id}`)
    await page.waitForLoadState('networkidle')
    expect(asked, 'the page reached YouTube before anyone pressed play').toEqual([])
    await expect(page.locator('iframe')).toHaveCount(0)

    await page.getByRole('button', { name: /^(trailer|bande-annonce)$/i }).click()
    const frame = page.getByRole('dialog').locator('iframe')
    await expect(frame).toHaveAttribute(
      'src',
      pattern(`^https://www\\.youtube-nocookie\\.com/embed/${escaped(String(work!.trailerYoutubeId))}\\?`),
    )

    // Closed, the player goes with it rather than playing on behind the page.
    await page.keyboard.press('Escape')
    await expect(page.locator('iframe')).toHaveCount(0)
  })
})
