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
  status?: string
  network?: string
  episodes?: Episode[]
  credits?: { personName: string; tmdbPersonId?: number }[]
  images?: { coverType: string; seasonNumber?: number | null }[]
  trailerYoutubeId?: string
  externalIds: { tmdb?: number; tvdb?: number; imdb?: string }
}

/** The reader's own day, `YYYY-MM-DD`: the page counts what has aired from it. */
function localDay(date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

/** When an episode airs, for sorting: the moment, or midnight UTC on its day. */
function airsAt(episode: Episode): number {
  const raw = episode.airDateUtc ?? (episode.airDate ? `${episode.airDate}T00:00:00Z` : undefined)
  const time = raw ? Date.parse(raw) : Number.NaN
  return Number.isNaN(time) ? Number.POSITIVE_INFINITY : time
}

/** Whether it has aired, as the page decides it: a moment once past, a day once over. */
function hasAired(episode: Episode): boolean {
  const utc = episode.airDateUtc?.trim()
  if (utc && /^\d{4}-\d{2}-\d{2}T/.test(utc) && !Number.isNaN(Date.parse(utc))) return Date.parse(utc) <= Date.now()
  const day = [utc, episode.airDate?.trim()].find((v) => v && /^\d{4}-\d{2}-\d{2}$/.test(v))
  return day !== undefined && day < localDay()
}

/** `S01E02`, as the page prints it. */
function codeOf(episode: Episode): string {
  const two = (n: number) => String(n).padStart(2, '0')
  return `S${two(episode.seasonNumber)}E${two(episode.episodeNumber)}`
}

/**
 * The most popular running series with an episode still to air, and that
 * episode: the first regular one not yet aired, as the page picks it.
 */
async function aRunningSeries(request: APIRequestContext): Promise<{ work: Work; next: Episode } | undefined> {
  const { items } = await (await request.get('/api/v1/items?kind=series&limit=30')).json()
  for (const { id } of items as { id: string }[]) {
    const work = (await (await request.get(`/api/v1/items/${id}`)).json()) as Work
    if (work.status === 'ended') continue
    const next = (work.episodes ?? [])
      .filter((e) => e.seasonNumber > 0 && Number.isFinite(airsAt(e)))
      .sort((a, b) => airsAt(a) - airsAt(b))
      .find((e) => !hasAired(e))
    if (next) return { work, next }
  }
  return undefined
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
      await expect(page.getByText(/^(born |née? le |né·e le )/i)).toBeVisible()
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

test.describe('a person, read in French', () => {
  test.use({ locale: 'fr-FR' })

  test('is written of in the gender TMDB gives, and otherwise in both', async ({ page, request }) => {
    const work = await aSeries(request)
    const credit = work?.credits?.find((c) => c.tmdbPersonId)
    test.skip(!credit, 'no credit carries a TMDB person id')

    const person = await (await request.get(`/api/v1/people/${credit!.tmdbPersonId}?language=fr`)).json()
    test.skip(!person.details?.birthday, 'TMDB is not configured, or has no birthday for them')
    const gender: number | undefined = person.details.gender
    if (gender !== undefined) expect([1, 2, 3]).toContain(gender)

    await page.goto(`/person/${credit!.tmdbPersonId}`)
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(credit!.personName)
    // The participle agreed: "Née" for a woman, "Né" for a man, the inclusive
    // "Né·e" where TMDB says non-binary or nothing.
    const born = gender === 1 ? /^Née le / : gender === 2 ? /^Né le / : /^Né·e le /
    await expect(page.getByText(born)).toBeVisible()
    if (person.details.knownFor) {
      const known = gender === 1 ? /^Connue pour / : gender === 2 ? /^Connu pour / : /^Connu·e pour /
      await expect(page.getByText(known)).toBeVisible()
    }
    if (person.details.biography) {
      // A paragraph to a line, not one run of text; a title TMDB wrote *so*
      // is set as emphasis, and an asterisk with a space inside stays one.
      const text: string = person.details.biography
      const biography = page.getByText(/^(biographie)$/i).locator('xpath=following-sibling::div[1]')
      await expect(biography.locator('p')).toHaveCount(text.split(/\r\n?|\n/).filter((line) => line.trim()).length)
      await expect(biography.locator('p').first()).toBeVisible()
      if (/\*[^\s*](?:[^*\n]*[^\s*])?\*/.test(text)) await expect(biography.locator('em').first()).toBeAttached()
    }
  })
})

test.describe('a running series', () => {
  test('says on its plate what airs next, and how far the season has got', async ({ page, request }) => {
    const found = await aRunningSeries(request)
    test.skip(!found, 'no series here has an episode still to air')
    const { work, next } = found!
    // Counted as the season's card below counts it.
    const season = (work.episodes ?? []).filter((e) => e.seasonNumber === next.seasonNumber)
    const aired = season.filter(hasAired).length

    await page.goto(`/work/${work.id}`)
    const plate = page.locator('article > header').first()
    await expect(plate.getByText(/^(next episode:|prochain épisode :)$/i)).toBeVisible()

    // The episode, as the way to its page and named for what it is; the
    // season's count beside it, as the way to the season's.
    const episode = plate.locator(
      `a[href="/work/${work.id}/season/${next.seasonNumber}/episode/${next.episodeNumber}"]`,
    )
    await expect(episode).toHaveAccessibleName(pattern(`^(Next episode:|Prochain épisode :)\\s+${codeOf(next)} · `))
    if (next.title) await expect(episode).toContainText(next.title)
    // "2 of 8 aired" while the season is under way, "8 episodes" before it
    // is — the season chart's words.
    const count = plate.locator(`a[href="/work/${work.id}/season/${next.seasonNumber}"]`)
    await expect(count).toContainText(
      aired > 0 && aired < season.length
        ? pattern(`(${aired} of ${season.length} aired|${aired} sur ${season.length} diffusés?)$`)
        : pattern(`(${season.length} episodes?|${season.length} épisodes?)$`),
    )
    expect(await scrollsSideways(page)).toBe(false)
  })

  test('that has ended says nothing of a next episode on its plate', async ({ page, request }) => {
    const { items } = await (await request.get('/api/v1/items?kind=series&limit=30')).json()
    let ended: Work | undefined
    for (const { id } of items as { id: string }[]) {
      const work = (await (await request.get(`/api/v1/items/${id}`)).json()) as Work
      if (work.status === 'ended') {
        ended = work
        break
      }
    }
    test.skip(!ended, 'no series here has ended')

    await page.goto(`/work/${ended!.id}`)
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    await expect(page.locator('article > header').getByText(/next episode:|prochain épisode :/i)).toHaveCount(0)
  })
})

test.describe('the shelf of seasons', () => {
  test('says where it goes on, and walks along at the arrows', async ({ page, request }) => {
    // Enough seasons to overflow a shelf at any width worth supporting: nine
    // cards are fourteen hundred pixels, and the column beside the facts is
    // narrower than that on the widest screen the suite uses.
    const { items } = await (await request.get('/api/v1/items?kind=series&limit=30')).json()
    let work: Work | undefined
    for (const { id } of items as { id: string }[]) {
      const candidate = (await (await request.get(`/api/v1/items/${id}`)).json()) as Work
      if (new Set((candidate.episodes ?? []).map((e) => e.seasonNumber)).size >= 9) {
        work = candidate
        break
      }
    }
    test.skip(!work, 'no series here has enough seasons to overflow the shelf')

    await page.goto(`/work/${work!.id}`)
    const shelf = page.getByRole('list', { name: /^(seasons|saisons)$/i })
    await shelf.scrollIntoViewIfNeeded()
    // From the start, whatever season a running series opened on.
    await shelf.evaluate((list) => {
      list.scrollLeft = 0
    })

    // Where the row is once it has stopped moving: an arrow scrolls smoothly.
    const resting = () =>
      shelf.evaluate(async (list) => {
        let last = -1
        while (list.scrollLeft !== last) {
          last = list.scrollLeft
          await new Promise((settle) => setTimeout(settle, 120))
        }
        return last
      })

    // The arrows of this shelf: on a phone, the shelves of related works have their own.
    const arrows = page.locator(`button[aria-controls="${await shelf.getAttribute('id')}"]`)
    const back = arrows.first()
    const on = arrows.last()
    await expect(back).toHaveAccessibleName(/^(scroll back|revenir en arrière)$/i)
    await expect(on).toHaveAccessibleName(/^(scroll forward|voir la suite)$/i)
    await expect(on).toHaveAttribute('aria-disabled', 'false')
    await expect(back).toHaveAttribute('aria-disabled', 'true')
    // Where the row goes on, its edge fades.
    await expect(shelf.locator('xpath=following-sibling::div[2]')).toHaveCSS('opacity', '1')

    // A keyboard reaches the arrows, and they show themselves to it.
    await on.focus()
    await expect(on).toHaveCSS('opacity', '1')
    await on.press('Enter')
    await expect.poll(resting).toBeGreaterThan(0)
    await expect(back).toHaveAttribute('aria-disabled', 'false')

    // And a pointer: the way back, pressed.
    const moved = await resting()
    await back.click()
    await expect.poll(resting).toBeLessThan(moved)

    // Walked to the end, the way on is said to be none, and stays to be pressed.
    await shelf.evaluate((list) => {
      list.scrollLeft = list.scrollWidth
    })
    await expect(on).toHaveAttribute('aria-disabled', 'true')
    await expect(on).toBeVisible()
    expect(await scrollsSideways(page)).toBe(false)
  })
})

test.describe('where to watch', () => {
  test('folds a long list of services to four, and opens it out on request', async ({ page, request }) => {
    const { items } = await (await request.get('/api/v1/items?limit=40')).json()
    const groups = ['flatrate', 'free', 'ads', 'rent', 'buy']
    let found: { id: string; count: number } | undefined
    for (const { id } of items as { id: string }[]) {
      const response = await request.get(`/api/v1/items/${id}/watch`)
      if (response.status() === 503) break
      if (response.status() !== 200) continue
      const data = (await response.json()) as Record<string, unknown[]>
      const group = groups.find((g) => (data[g]?.length ?? 0) > 4)
      if (group) {
        found = { id, count: data[group]!.length }
        break
      }
    }
    test.skip(!found, 'no work here has more than four services in a group, or TMDB is not configured')

    await page.goto(`/work/${found!.id}`)
    const more = page.getByRole('button', { name: /^\+\d+ (more|autres?)$/ }).first()
    await expect(more).toBeVisible()
    await expect(more).toHaveAttribute('aria-expanded', 'false')
    await expect(more).toHaveText(new RegExp(`^\\+${found!.count - 4} `))

    // Four services and the count, then every one and the way back. The
    // list is pinned by its place among the lists that fold: once the
    // button reads "show fewer", a locator by the button's name would walk
    // on to the next group's "+n more" and its list.
    const lists = page.locator('ul:has(> li > button[aria-expanded])')
    const index = await lists.evaluateAll(
      (uls, button) => uls.findIndex((ul) => ul.contains(button)),
      await more.elementHandle(),
    )
    const list = lists.nth(index)
    await expect(list.locator(':scope > li')).toHaveCount(5)
    await more.click()
    const fewer = list.getByRole('button', { name: /^(show fewer|en afficher moins)$/i })
    await expect(fewer).toHaveAttribute('aria-expanded', 'true')
    await expect(list.locator(':scope > li')).toHaveCount(found!.count + 1)
    await fewer.click()
    await expect(list.locator(':scope > li')).toHaveCount(5)
  })
})

test.describe('a platform’s episode', () => {
  test('is available on a day rather than aired at midnight', async ({ page, request }) => {
    // A series on a streaming service whose episodes are timed at midnight
    // UTC: the platform's release day, which no clock should be made of.
    // Stranger Things, in the suite's catalogue: filed on 15 July 2016 and
    // timed at midnight UTC on the 16th, the day the page names.
    const platforms = /netflix|prime video|amazon|disney\+|apple tv|\bmax\b|paramount\+|crunchyroll|hulu|peacock/i
    const { items } = await (await request.get('/api/v1/items?kind=series&limit=40')).json()
    let found: { work: Work; episode: Episode } | undefined
    for (const { id } of items as { id: string }[]) {
      const work = (await (await request.get(`/api/v1/items/${id}`)).json()) as Work
      if (!work.network || !platforms.test(work.network)) continue
      const episode = work.episodes?.find((e) => e.seasonNumber > 0 && /T00:00(:00(\.0+)?)?Z$/.test(e.airDateUtc ?? ''))
      if (episode) {
        found = { work, episode }
        break
      }
    }
    test.skip(!found, 'no series here is on a platform with an episode timed at midnight UTC')
    const { work, episode } = found!

    await page.goto(`/work/${work.id}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`)
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    // The day that midnight opens, in the page's own words and its engine's
    // own calendar — not the date filed beside it, and no clock at all.
    const day = await page.evaluate(
      (moment) =>
        new Intl.DateTimeFormat(document.documentElement.lang === 'fr' ? 'fr-FR' : 'en-GB', {
          weekday: 'long',
          day: 'numeric',
          month: 'long',
          year: 'numeric',
          timeZone: 'UTC',
        }).format(new Date(moment)),
      episode.airDateUtc!,
    )
    await expect(page.getByText(pattern(`^(Available on|Disponible le) ${escaped(day)}$`))).toBeVisible()
    await expect(page.getByText(/^(available|disponible)$/i)).toBeVisible()
    await expect(page.getByText(/UTC$/)).toHaveCount(0)
    await expect(page.getByText(/\(your time\)|\(heure locale\)/)).toHaveCount(0)
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

test.describe('an address that is not a work’s', () => {
  test('never reaches the API as a path of its own', async ({ page }) => {
    const asked: string[] = []
    page.on('request', (request) => asked.push(new URL(request.url()).pathname))

    // `..%2Fstats` reaches the router as `../stats`, which, put into
    // `/api/v1/items/…` as it was, asked for `/api/v1/stats`.
    await page.goto('/work/..%2Fstats')
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(/no longer in the catalogue|plus au catalogue/i)
    expect(asked.filter((path) => path === '/api/v1/stats')).toEqual([])

    // Spelled out as text, which the browser leaves be, `..` is a work's id
    // like any other: nothing there, and the API's root is not asked instead.
    asked.length = 0
    await page.goto('/work/%252e%252e')
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(/no longer in the catalogue|plus au catalogue/i)
    expect(asked.filter((path) => /^\/api\/v1\/?$/.test(path))).toEqual([])
  })

  test('a page that cannot be shown says so, with the bar still on screen', async ({ page, request }) => {
    const { items } = await (await request.get('/api/v1/items?limit=1')).json()
    const work = items[0] as { id: string } | undefined
    test.skip(!work, 'no work in the catalogue')

    // An answer that is JSON and is not a work, as a proxy in front of the
    // server may give: the page used to throw, and the whole app went blank.
    await page.route(pattern(`${escaped(`/api/v1/items/${work!.id}`)}(\\?.*)?$`), (route) =>
      route.fulfill({ status: 200, contentType: 'application/json', body: '{"series": 3, "movies": 2}' }),
    )
    await page.goto(`/work/${work!.id}`)

    await expect(page.getByText(/something went wrong|quelque chose s’est mal passé/i)).toBeVisible()
    await expect(page.getByRole('button', { name: /try again|réessayer/i })).toBeVisible()
    await expect(page.getByRole('link', { name: /cinémathèque/i }).first()).toBeVisible()
  })
})
