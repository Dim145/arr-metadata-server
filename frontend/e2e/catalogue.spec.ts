import { expect, test, type Page } from '@playwright/test'
import { scrollsSideways } from './support/layout'
import { escaped, pattern } from './support/regex'

/**
 * The public catalogue, as a visitor with no credential sees it.
 *
 * These run against the real binary with `AMS_PUBLIC_BROWSE=true`, so they
 * exercise the guard as well as the interface: if the browse allowlist ever
 * stops letting an anonymous reader through, half of this file fails.
 */

/** Wait for the grid to have something in it rather than for a fixed delay. */
async function catalogueLoaded(page: Page) {
  await expect(page.locator('a[href^="/work/"]').first()).toBeVisible()
}

/**
 * The filters, wherever this width keeps them: in a rail beside the results on
 * a wide screen, in a sheet behind a button on a phone.
 */
async function filters(page: Page) {
  const open = page.getByRole('button', { name: /^(filters|filtres)/i })
  if (await open.isVisible()) {
    await open.click()
    return page.getByRole('dialog')
  }
  return page.getByRole('complementary', { name: /^(filters|filtres)$/i })
}

/** Back to the results from the phone's sheet; nothing to do beside a rail. */
async function backToResults(page: Page) {
  const sheet = page.getByRole('dialog')
  if (await sheet.isVisible()) {
    await sheet.getByRole('button', { name: /^(show|voir) /i }).click()
    await expect(sheet).toBeHidden()
  }
}

test.describe('the catalogue', () => {
  test('opens on a featured work and rows of posters', async ({ page }) => {
    await page.goto('/')
    await catalogueLoaded(page)

    // The work's own logo where it has one, its title read out either way.
    const title = page.getByRole('heading', { level: 1 })
    await expect(title).toBeAttached()
    expect((await title.textContent())?.trim()).not.toBe('')
    await expect(page.getByRole('link', { name: /details|fiche/i }).first()).toBeVisible()

    // The arrivals are offered — those with a trailer where there are enough
    // of them — and the row says how many it is a window onto.
    await expect(
      page.getByRole('heading', {
        name: /^(Recently added|Ajoutés récemment|New, with a trailer|Nouveautés avec bande-annonce)$/,
      }),
    ).toBeVisible()
    await expect(page.locator('a[href="/browse?order=added"]')).toContainText(/\d/)
  })

  test('every poster carries a described image or a placeholder, never a bare box', async ({
    page,
  }) => {
    await page.goto('/')
    await catalogueLoaded(page)

    // A work's poster, that is: an episode's card names its episode in text,
    // and the picture beside it is rightly left undescribed.
    const described = await page.evaluate(() =>
      [...document.querySelectorAll('a[href^="/work/"]:not([href*="/episode/"]) img')].every(
        (img) => (img as HTMLImageElement).alt.trim().length > 0,
      ),
    )

    expect(described).toBe(true)
  })

  test('does not scroll sideways', async ({ page }) => {
    for (const path of ['/', '/browse']) {
      await page.goto(path)
      await catalogueLoaded(page)
      expect(await scrollsSideways(page), `${path} scrolls sideways`).toBe(false)
    }
  })

  test('a poster whose picture fails gives way to a drawn stand-in, never a broken image', async ({ page }) => {
    // Every picture refused, as a dead provider address or a moved media
    // store would refuse it: the card has to say what it is all the same.
    await page.route('**/*', (route) => (route.request().resourceType() === 'image' ? route.abort() : route.continue()))
    await page.goto('/browse')
    await catalogueLoaded(page)

    // The first card, which is on screen: the ones below the fold are lazy,
    // and a picture never asked for never fails.
    const card = page.locator('a[href^="/work/"]').first()
    const standIn = card.locator('[data-placeholder]')
    await expect(standIn).toBeVisible()
    await expect(card.locator('img')).toHaveCount(0)
    // The initial of the title, in the stand-in.
    const title = (await card.locator('h3').textContent())?.trim() ?? ''
    expect(title).not.toBe('')
    expect((await standIn.textContent()) ?? '').toContain([...title][0]!.toLocaleUpperCase())
  })

  test('draws a work at random under the filters, and lands on it', async ({ page, request }) => {
    await page.goto('/browse?kind=series')
    await catalogueLoaded(page)

    await page.getByRole('button', { name: /^(at random|au hasard)$/i }).click()
    await page.waitForURL(/\/work\/[^/]+$/)
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()

    // Under the filter it was drawn from: a series.
    const id = new URL(page.url()).pathname.split('/').pop()!
    const work = await (await request.get(`/api/v1/items/${id}`)).json()
    expect(work.kind).toBe('series')
  })

  test('names each original language once, however many ways it is spelt', async ({ page }) => {
    await page.goto('/browse')
    await catalogueLoaded(page)
    const panel = await filters(page)
    const select = panel.getByLabel(/original language|langue originale/i)
    test.skip((await select.count()) === 0, 'no language to choose from')

    const names = (await select.locator('option').allTextContents())
      .slice(1) // "any language"
      .map((text) => text.replace(/\s*\(\d+\)\s*$/, '').trim())
    expect(new Set(names).size).toBe(names.length)
  })

  test('remembers on the front page what was opened here, and forgets on request', async ({ page, request }) => {
    const { items } = await (await request.get('/api/v1/items?limit=1')).json()
    const work = items[0] as { id: string; title: string } | undefined
    test.skip(!work, 'no work in the catalogue')

    // Nothing opened yet in this browser: no shelf.
    await page.goto('/')
    await catalogueLoaded(page)
    const heading = page.getByRole('heading', { name: /^(recently viewed|vus récemment)$/i })
    await expect(heading).toHaveCount(0)

    await page.goto(`/work/${work!.id}`)
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    await page.goto('/')
    await expect(heading).toBeVisible()
    const shelf = page.getByRole('list', { name: /^(recently viewed|vus récemment)$/i })
    await expect(shelf.locator(`a[href="/work/${work!.id}"]`)).toBeVisible()

    await page.getByRole('button', { name: /clear the recent|effacer les récents/i }).click()
    await expect(heading).toHaveCount(0)
  })

  test('offers what it finds as the search is typed, and opens one with the keys', async ({
    page,
    request,
  }) => {
    const { items } = await (await request.get('/api/v1/items?kind=series&limit=1')).json()
    const work = items[0] as { id: string; title: string } | undefined
    test.skip(!work, 'the catalogue holds no series')

    await page.goto('/')
    // Drawn once its language is in: below a laptop's width the search is a
    // button that opens the field, and the button is there only then.
    await expect(page.locator('a[href^="/work/"]').first()).toBeVisible()
    const reveal = page.getByRole('button', { name: /^(search|rechercher)/i })
    if (await reveal.isVisible()) await reveal.click()

    const box = page.getByRole('combobox').first()
    await box.fill(work!.title.slice(0, 4))
    // Escaped: a title such as "[Oshi no Ko]" is not a pattern.
    const option = page.getByRole('option', { name: pattern(escaped(work!.title.slice(0, 4)), 'i') }).first()
    await expect(option).toBeVisible()
    await expect(page.getByRole('option', { name: /see all|see the|voir le/i })).toBeVisible()

    // The first option, by the keys alone.
    await box.press('ArrowDown')
    await expect(box).toHaveAttribute('aria-activedescendant', /option-0$/)
    await box.press('Enter')
    await expect(page).toHaveURL(/\/work\/[0-9a-f-]{36}$/)
  })

  test('puts the cursor in the search when / is pressed', async ({ page }) => {
    await page.goto('/')
    await catalogueLoaded(page)
    await page.keyboard.press('/')
    const box = page.getByRole('combobox').first()
    await expect(box).toBeFocused()
    // The key itself is not typed in; typed into a field, it is left alone.
    await expect(box).toHaveValue('')
    await box.pressSequentially('a/b')
    await expect(box).toHaveValue('a/b')
  })

  test('a search from the bar lands on a filtered list', async ({ page }) => {
    await page.goto('/')
    await catalogueLoaded(page)

    const reveal = page.getByRole('button', { name: /^(search|rechercher)$/i })
    if (await reveal.isVisible()) {
      await reveal.click()
    }

    const box = page.getByRole('combobox').first()
    await box.fill('breaking')
    await box.press('Enter')

    await expect(page).toHaveURL(/\/browse\?q=breaking/)
    await expect(page.getByRole('heading', { level: 1 })).toContainText('breaking')
    await expect(page.locator('a[href^="/work/"]')).toHaveCount(1)
  })

  test('a filter is carried in the URL, so the view can be linked', async ({ page }) => {
    await page.goto('/browse?kind=movie')
    await catalogueLoaded(page)

    // The control reflects the URL rather than its own default.
    const panel = await filters(page)
    await expect(panel.getByRole('radio', { name: /^(films)$/i })).toBeChecked()
  })

  test('a genre narrows the list, and its chip takes it off again', async ({ page }) => {
    await page.goto('/browse')
    await catalogueLoaded(page)

    const panel = await filters(page)
    const genre = panel.getByRole('group', { name: /^(genres)$/i }).getByRole('button').first()
    await genre.click()

    await expect(page).toHaveURL(/[?&]genre=/)
    await expect(genre).toHaveAttribute('aria-pressed', 'true')

    await backToResults(page)
    await catalogueLoaded(page)
    await page.getByRole('button', { name: /remove the filter|retirer le filtre/i }).first().click()
    await expect(page).not.toHaveURL(/genre=/)
  })

  test('the genres beyond the commonest fold away, and a name finds one', async ({ page }) => {
    await page.goto('/browse')
    await catalogueLoaded(page)

    const panel = await filters(page)
    const group = panel.getByRole('group', { name: /^(genres)$/i })
    const all = await group.locator('button[aria-pressed]').count()
    test.skip(all <= 10, 'the catalogue has ten genres or fewer, so nothing folds away')

    // Ten in view, the rest behind one line that says how many.
    await expect(group.locator('button[aria-pressed]:visible')).toHaveCount(10)
    const others = group.locator('summary')
    await expect(others).toHaveText(pattern(`${all - 10}`))
    await others.click()
    await expect(group.locator('button[aria-pressed]:visible')).toHaveCount(all)

    // A name finds one, accents or not.
    const field = group.getByRole('searchbox', { name: /find a genre|chercher un genre/i })
    await field.fill('dram')
    await expect(group.locator('button[aria-pressed]:visible')).toHaveCount(1)
    await expect(group.locator('button[aria-pressed]:visible')).toHaveText(/drama|drame/i)
    await field.fill('zzzz')
    await expect(group.getByText(/no genre by that name|aucun genre de ce nom/i)).toBeVisible()
  })

  test('two genres combine as all of them, or as any of them', async ({ page, request }) => {
    const every = (await (await request.get('/api/v1/items?genre=Drama,Crime&limit=1')).json()).total as number
    const either = (await (await request.get('/api/v1/items?genre=Drama,Crime&genreMode=any&limit=1')).json())
      .total as number
    expect(either).toBeGreaterThanOrEqual(every)
    test.skip(either === every, 'no work here tells the two apart')

    await page.goto('/browse?genre=Drama,Crime')
    await catalogueLoaded(page)
    await expect(page.getByText(pattern(`^${every} (works?|œuvres?)$`))).toBeVisible()

    const panel = await filters(page)
    // The radio is off screen; its label is what a finger presses.
    await panel.getByText(/^(any of them|l’un d’eux)$/i).click()
    await expect(panel.getByRole('radio', { name: /^(any of them|l’un d’eux)$/i })).toBeChecked()
    await expect(page).toHaveURL(/genreMode=any/)
    await backToResults(page)
    await catalogueLoaded(page)
    await expect(page.getByText(pattern(`^${either} (works?|œuvres?)$`))).toBeVisible()
  })

  test('an order is asked of the server, and kept in the URL', async ({ page }) => {
    await page.goto('/browse')
    await catalogueLoaded(page)

    const asked = page.waitForRequest((r) => r.url().includes('/api/v1/items') && r.url().includes('sort=title'))
    await page.getByLabel(/^(sort|tri)$/i).selectOption('title')

    await asked
    await expect(page).toHaveURL(/[?&]order=title/)
  })

  test('a link made for a single year still lands on that year', async ({ page }) => {
    const asked = page.waitForRequest(
      (r) => r.url().includes('/api/v1/items') && r.url().includes('yearFrom=2008') && r.url().includes('yearTo=2008'),
    )
    await page.goto('/browse?year=2008')

    await asked
    await expect(page.getByRole('button', { name: /(remove the filter|retirer le filtre).*2008/i })).toBeVisible()
  })

  test('says so plainly when a search matches nothing, and offers to clear it', async ({ page }) => {
    await page.goto('/browse?q=zzzzznothinghere')

    await expect(page.getByText(/no work matches|aucune œuvre/i)).toBeVisible()
    await page.getByRole('button', { name: /^(clear filters|effacer les filtres)$/i }).first().click()
    await expect(page).not.toHaveURL(/q=/)
    await catalogueLoaded(page)
  })

  test('offers to take off the filter that emptied the list', async ({ page }) => {
    await page.goto('/browse?kind=movie&genre=zzz-no-such-genre')

    await expect(page.getByText(/these filters leave nothing|ces filtres ne laissent rien/i)).toBeVisible()
    await page.getByRole('button', { name: /^(without|sans).*zzz-no-such-genre/i }).click()

    await expect(page).not.toHaveURL(/genre=/)
    await expect(page).toHaveURL(/kind=movie/)
    await catalogueLoaded(page)
  })
})

test.describe('a work', () => {
  test('shows its record, its cast and every season', async ({ page }) => {
    await page.goto('/browse?kind=series')
    await catalogueLoaded(page)
    await page.locator('a[href^="/work/"]').first().click()

    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    await expect(page.getByRole('heading', { name: /^(Cast|Distribution)$/ })).toBeVisible()
    await expect(page.getByRole('heading', { name: /^(Seasons|Saisons)$/ })).toBeVisible()

    // The record panel is what makes this a catalogue and not a poster wall.
    await expect(page.getByText(/^(Details|Fiche)$/i).first()).toBeVisible()
    await expect(page.getByText(/^(Identifiers|Identifiants)$/i).first()).toBeVisible()

    expect(await scrollsSideways(page)).toBe(false)
  })

  test('hands its address on, and says when it has been copied', async ({ page, context, request }) => {
    const { items } = await (await request.get('/api/v1/items?limit=1')).json()
    const work = items[0] as { id: string } | undefined
    test.skip(!work, 'no work in the catalogue')

    await context.grantPermissions(['clipboard-read', 'clipboard-write'])
    // No share sheet on this machine: the address is copied instead.
    await page.addInitScript(() => {
      Object.defineProperty(navigator, 'share', { value: undefined, configurable: true })
    })

    await page.goto(`/work/${work!.id}`)
    await page.getByRole('button', { name: /^(share|partager)$/i }).click()
    await expect(page.getByRole('status').filter({ hasText: /link copied|lien copié/i })).toBeVisible()
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(page.url())
  })

  test('scrolls its seasons inside their own shelf, not the page', async ({ page, request }) => {
    // The series with the most seasons, the one whose shelf is likeliest to
    // be wider than the screen.
    const { items } = await (await request.get('/api/v1/items?kind=series&limit=20')).json()
    let most: { id: string; seasons: number } | undefined
    for (const { id } of items as { id: string }[]) {
      const work = await (await request.get(`/api/v1/items/${id}`)).json()
      const seasons = new Set((work.episodes ?? []).map((e: { seasonNumber: number }) => e.seasonNumber)).size
      if (seasons > (most?.seasons ?? 0)) most = { id, seasons }
    }
    test.skip(!most, 'the catalogue holds no series with episodes')

    await page.goto(`/work/${most!.id}`)
    await expect(page.getByRole('heading', { name: /^(Seasons|Saisons)$/ })).toBeVisible()

    const shelf = page.getByRole('list', { name: /^(Seasons|Saisons)$/ })
    const fits = await shelf.evaluate((list) => list.scrollWidth <= list.clientWidth)
    test.skip(fits, 'every season fits on the screen')

    const moved = await shelf.evaluate(async (list) => {
      const before = list.scrollLeft
      list.scrollBy(list.scrollLeft > 0 ? -400 : 400, 0)
      await new Promise((r) => setTimeout(r, 150))
      return list.scrollLeft !== before
    })
    expect(moved).toBe(true)
    expect(await scrollsSideways(page)).toBe(false)
  })

  test('leads to each season, and leaves its episodes to the season', async ({ page, request }) => {
    // The longest series held: a page listing its every episode was the one
    // that grew to ten thousand nodes.
    const { items } = await (await request.get('/api/v1/items?kind=series&limit=20')).json()
    let longest: { id: string; episodes: { seasonNumber: number }[] } | undefined
    for (const { id } of items as { id: string }[]) {
      const work = await (await request.get(`/api/v1/items/${id}`)).json()
      if ((work.episodes?.length ?? 0) > (longest?.episodes.length ?? 0)) longest = work
    }
    test.skip(!longest?.episodes.length, 'the catalogue holds no series with episodes')
    const seasons = new Set(longest!.episodes.map((e) => e.seasonNumber))

    await page.goto(`/work/${longest!.id}`)
    await expect(page.getByRole('heading', { name: /^(Seasons|Saisons)$/ })).toBeVisible()
    // Below the plate, whose one line on a running series leads to the next
    // episode and its season too.
    const record = page.locator('main article > :not(header)')
    await expect(record.locator(`a[href^="/work/${longest!.id}/season/"]:not([href*="/episode/"])`)).toHaveCount(
      seasons.size,
    )
    // The episode that aired last and the one airing next, and no more.
    expect(await record.locator(`a[href*="/episode/"]`).count()).toBeLessThanOrEqual(2)
  })
})

/**
 * Where the header's secondary controls are.
 *
 * On a phone the language toggle and the way in live in the sheet the bar along
 * the foot of the screen opens — together with the logo, search and a menu
 * button at the top they were wider than the screen, and at 320px the menu
 * button itself was pushed off the edge. Opening the sheet first, when there
 * is one, is what a person on a phone does. Wider, the language is in the
 * small menu beside the search, which is opened in its place.
 */
async function openMenuIfNarrow(page: Page) {
  const more = page.getByRole('button', { name: /^(more|plus)$/i })
  if (await more.isVisible()) {
    await more.click()
  } else {
    await page.getByRole('button', { name: /^(language and reading|langue et lecture)/i }).click()
  }
}

test.describe('language', () => {
  test('switches the whole interface and survives a reload', async ({ page }) => {
    await page.goto('/')
    await catalogueLoaded(page)

    await openMenuIfNarrow(page)
    await page.getByRole('button', { name: 'fr', exact: true }).click()
    await expect(page.locator('html')).toHaveAttribute('lang', 'fr')
    await expect(page.getByRole('link', { name: /se connecter/i })).toBeVisible()

    await page.reload()
    await expect(page.locator('html')).toHaveAttribute('lang', 'fr')

    await openMenuIfNarrow(page)
    await page.getByRole('button', { name: 'en', exact: true }).click()
    await expect(page.locator('html')).toHaveAttribute('lang', 'en')
  })

  test('asks the server for the same language it is showing', async ({ page }) => {
    await page.goto('/')
    await catalogueLoaded(page)

    const asked = page.waitForRequest((request) => request.url().includes('language=fr'))
    await openMenuIfNarrow(page)
    await page.getByRole('button', { name: 'fr', exact: true }).click()

    expect((await asked).url()).toContain('language=fr')
  })
})

/**
 * Who the data comes from, in the footer of every page.
 *
 * Owed to the sources rather than decorative — TMDB's terms ask for a notice,
 * TVmaze's licence for a link, IMDb's for a line — so a visitor must see it,
 * and it must name exactly what is switched on: never a source that
 * contributed nothing, never one left out.
 */
test.describe('credits', () => {
  const NAMES: Record<string, string> = {
    tmdb: 'TMDB',
    tvdb: 'TheTVDB',
    fanart: 'Fanart.tv',
    tvmaze: 'TVmaze',
    anilist: 'AniList',
    mal: 'MyAnimeList',
    imdb: 'IMDb',
    fankai: 'Fankai',
    fankaiwiki: 'Wiki Fankai',
  }

  test('name every source that is switched on, and only those', async ({ page, request }) => {
    const answer = await request.get('/api/v1/sources')
    expect(answer.ok()).toBe(true)
    const { sources } = (await answer.json()) as { sources: string[] }

    await page.goto('/')
    const footer = page.getByRole('contentinfo')

    for (const [key, name] of Object.entries(NAMES)) {
      await expect(footer.getByRole('link', { name, exact: true })).toHaveCount(
        sources.includes(key) ? 1 : 0,
      )
    }

    await expect(footer.getByText(/not endorsed or certified by TMDB/)).toHaveCount(
      sources.includes('tmdb') ? 1 : 0,
    )
    await expect(footer.getByText(/Information courtesy of IMDb/)).toHaveCount(
      sources.includes('imdb') ? 1 : 0,
    )
  })
})

test.describe('an address with nothing behind it', () => {
  test('says so, rather than quietly landing on the front page', async ({ page }) => {
    await page.goto('/there-is-no-such-page')

    await expect(page).toHaveURL(/there-is-no-such-page/)
    await expect(
      page.getByRole('heading', { name: /nothing lives at this address|rien à cette adresse/i }),
    ).toBeVisible()
    await expect(page.getByRole('link', { name: /back to the catalogue|retour au catalogue/i })).toBeVisible()
  })

  test('says a removed work is gone, not that something broke', async ({ page }) => {
    await page.goto('/work/00000000-0000-0000-0000-000000000000')

    await expect(
      page.getByRole('heading', {
        name: /no longer in the catalogue|n’est plus au catalogue/i,
      }),
    ).toBeVisible()
  })
})

test.describe('the bar on the narrowest phone', () => {
  test.skip(({ isMobile }) => !isMobile, 'the bar along the foot is a phone’s')
  test.use({ viewport: { width: 320, height: 640 } })

  test('keeps the last place of the bar on the screen, and the sheet opens', async ({ page }) => {
    await page.goto('/')

    const more = page.getByRole('button', { name: /^(more|plus)$/i })
    const box = await more.boundingBox()

    // Clipped past the edge, it could not be pressed and the navigation behind
    // it could not be reached at all.
    expect(box).not.toBeNull()
    expect(box!.x + box!.width).toBeLessThanOrEqual(320)

    await more.click()
    await expect(page.getByRole('dialog', { name: /^(more|plus)$/i }).getByRole('link', { name: /^collections$/i })).toBeVisible()
  })
})

test.describe('the bar on a tablet', () => {
  // Between the phone's menu and the wide bar's field: every control in the
  // bar at once, on a screen not much wider than they are.
  for (const width of [768, 820]) {
    test(`keeps every control on a ${width}px screen`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1024 })
      await page.goto('/')

      for (const name of [/^(sign in|se connecter|admin|administration)$/i, /^(search|rechercher)/i]) {
        const control = page.getByRole(/search/.test(name.source) ? 'button' : 'link', { name }).first()
        if (!(await control.count())) continue
        if (!(await control.isVisible())) continue
        const box = (await control.boundingBox())!
        expect(box.x + box.width, `${name} past the edge`).toBeLessThanOrEqual(width)
      }
      expect(await scrollsSideways(page)).toBe(false)
    })
  }
})

test.describe('reaching the interface', () => {
  test('offers a skip link before anything else', async ({ page }) => {
    await page.goto('/')
    const skip = page.getByRole('link', { name: /skip to content|aller au contenu/i })
    // The page is drawn once its language is in: a Tab before that lands nowhere.
    await expect(skip).toBeAttached()
    await page.keyboard.press('Tab')

    await expect(skip).toBeFocused()
  })

  test('marks the section you are in', async ({ page, isMobile }) => {
    test.skip(isMobile, 'the tabs are along the foot of the screen at this width')

    await page.goto('/browse?kind=series')
    await catalogueLoaded(page)

    // An underline rather than a fill, but it still has to be *somewhere*.
    const marked = await page.evaluate(() =>
      [...document.querySelectorAll('nav a')].some(
        (a) => a.querySelector('span')?.className.includes('scale-x-100'),
      ),
    )
    expect(marked).toBe(true)
  })

  test('marks one section at a time, not every tab sharing a path', async ({ page, isMobile }) => {
    test.skip(isMobile, 'the tabs are along the foot of the screen at this width')

    // The catalogue's tab is current on the catalogue whatever it is narrowed
    // to, and on nothing else; the schedule's is current on the schedule.
    for (const [path, expected] of [
      ['/browse', 'Browse'],
      ['/browse?kind=series', 'Browse'],
      ['/browse?kind=movie', 'Browse'],
      ['/calendar', 'Schedule'],
    ] as const) {
      await page.goto(path)
      await catalogueLoaded(page)

      // What is drawn: the bar along the foot of a phone is in the page at
      // every width, with `display: none` above a phone's.
      const current = await page.evaluate(() =>
        [...document.querySelectorAll('nav a[aria-current="page"]')]
          .filter((a) => a.getClientRects().length > 0)
          .map((a) => (a.textContent ?? '').trim()),
      )

      expect(current, `${path} should mark exactly one tab`).toEqual([expected])
    }
  })

  test('leaves no console errors behind', async ({ page }) => {
    const errors: string[] = []
    page.on('console', (message) => {
      if (message.type() === 'error') errors.push(message.text())
    })

    await page.goto('/')
    await catalogueLoaded(page)
    await page.locator('a[href^="/work/"]').first().click()
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()

    // Artwork is fetched from provider CDNs that may rate-limit a test run;
    // that is the network's problem, not the interface's.
    const ours = errors.filter((text) => !/Failed to load resource|ERR_/.test(text))
    expect(ours).toEqual([])
  })
})

/**
 * The administration's register of everything held: what records lack, as
 * filters kept in the address and as four marks on each row, a row's actions
 * behind one button, and an entry typed in by hand with every field the
 * server takes. What is made here is made by hand, one per project, and
 * removed at the end.
 */
test.describe('the register', () => {
  const USERNAME = process.env.AMS_E2E_USER
  const PASSWORD = process.env.AMS_E2E_PASSWORD
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  async function signIn(page: Page) {
    await page.goto('/login')
    await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
    await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
    await page.getByRole('button', { name: /sign in|se connecter/i }).click()
    await page.waitForURL('**/admin')
  }

  test('narrows to what records lack, and keeps it in the address', async ({ page }) => {
    await signIn(page)
    const asked = page.waitForRequest((r) => r.url().includes('/api/v1/items?') && r.url().includes('missing=poster'))
    await page.goto('/admin/catalogue?missing=poster')
    await asked

    const gaps = page.getByRole('group', { name: /what is missing|ce qui manque/i })
    const poster = gaps.getByRole('button', { name: /no poster|sans affiche/i })
    const overview = gaps.getByRole('button', { name: /no overview|sans synopsis/i })
    await expect(poster).toHaveAttribute('aria-pressed', 'true')
    await expect(overview).toHaveAttribute('aria-pressed', 'false')

    // Pressed together they narrow together, and the address says so.
    await overview.click()
    await expect(page).toHaveURL(/missing=poster(%2C|,)overview/)
    await page.reload()
    await expect(overview).toHaveAttribute('aria-pressed', 'true')
    await poster.click()
    await overview.click()
    await expect(page).not.toHaveURL(/missing=/)

    // The server's filter is what the marks say: no IMDb id among those it lists.
    const lacking = await page.request.get('/api/v1/items?missing=imdb&includeDisabled=true&limit=20')
    expect(lacking.ok()).toBe(true)
    const { items } = (await lacking.json()) as { items: { externalIds: { imdb?: string } }[] }
    expect(items.every((item) => !item.externalIds.imdb)).toBe(true)
    // A gap it does not know is refused, not passed over.
    expect((await page.request.get('/api/v1/items?missing=trailer')).status()).toBe(400)
  })

  test('a row says what its record lacks, and keeps its actions behind one button', async ({ page }, info) => {
    await signIn(page)
    const title = `Registre e2e ${info.project.name} ${Date.now()}`
    const created = await page.request.post('/api/v1/items', {
      data: { kind: 'movie', title, overview: 'Un film que personne n’a tourné.' },
    })
    expect(created.status()).toBe(201)
    const { id } = (await created.json()) as { id: string }

    try {
      await page.goto('/admin/catalogue')
      await page.getByLabel(/search titles|chercher un titre/i).fill(title)
      const row = page.locator(`tr[data-work="${id}"]`)
      await expect(row).toBeVisible()
      // The search answered, and not the register as it opened, which may
      // hold the work too: its answer, arriving after a click, would draw
      // the row anew, and the focus with it.
      await expect(page.locator('tr[data-work]')).toHaveCount(1)

      // Four marks, each said in words: no poster, an overview, no IMDb id,
      // and nothing to refresh a work entered by hand from.
      const marks = row.getByRole('list', { name: /^(completeness|complétude)$/i }).filter({ visible: true })
      await expect(marks.getByRole('img', { name: /^(no poster|sans affiche)$/i })).toBeVisible()
      await expect(marks.getByRole('img', { name: /^(has an overview|a un synopsis)$/i })).toBeVisible()
      await expect(marks.getByRole('img', { name: /^(no imdb id|sans identifiant imdb)$/i })).toBeVisible()
      await expect(marks.getByRole('img', { name: /entered by hand|saisie à la main/i })).toBeVisible()

      // Behind one button: a popover of buttons and links, not a menu.
      const more = row.getByRole('button', { name: pattern(`^(actions for|actions pour) ${escaped(title)}$`, 'i') })
      await more.click()
      const panel = page.locator(`#row-actions-${id}`)
      await expect(panel).toBeVisible()
      await expect(more).toHaveAttribute('aria-expanded', 'true')
      await expect(page.getByRole('menu')).toHaveCount(0)
      // A work entered by hand has nothing to be refreshed from, and says so.
      await expect(panel.getByRole('button', { name: /^(refresh|rafraîchir)$/i })).toBeDisabled()

      // Escape shuts it, with the focus back on its button.
      await page.keyboard.press('Escape')
      await expect(panel).toBeHidden()
      await expect(more).toBeFocused()

      await more.click()
      await panel.getByRole('link', { name: /open the public page|ouvrir la page publique/i }).click()
      await expect(page).toHaveURL(pattern(`/work/${id}$`))
    } finally {
      await page.request.delete(`/api/v1/items/${id}`)
    }
  })

  test('an entry by hand takes every field the server does, and says what is wrong before sending', async ({
    page,
  }, info) => {
    await signIn(page)
    // Refused by the server too: a series has no release in cinemas.
    expect(
      (await page.request.post('/api/v1/items', { data: { kind: 'series', title: 'x', status: 'released' } })).status(),
    ).toBe(400)

    await page.goto('/admin/catalogue')
    await page.getByRole('button', { name: /^(new entry|nouvelle fiche)$/i }).click()
    const title = `Saisie e2e ${info.project.name} ${Date.now()}`
    const imdb = `tt${String(Date.now()).slice(-8)}`
    await page.locator('#compose-title').fill(title)
    await page.locator('#compose-year').fill('99')
    await page.locator('#compose-imdb').fill('nm0000001')

    let posted = false
    page.on('request', (request) => {
      if (request.method() === 'POST' && request.url().endsWith('/api/v1/items')) posted = true
    })
    const submit = page.getByRole('button', { name: /^(create the entry|créer la fiche)$/i })
    await submit.click()
    // Said beside each field, the first one focused, and nothing sent.
    await expect(page.locator('#compose-year')).toBeFocused()
    await expect(page.locator('#compose-year')).toHaveAttribute('aria-invalid', 'true')
    await expect(page.locator('#compose-imdb')).toHaveAttribute('aria-invalid', 'true')
    expect(posted).toBe(false)

    await page.locator('#compose-year').fill('2034')
    await page.locator('#compose-imdb').fill(imdb)
    await page.locator('#compose-status').selectOption('ended')
    await page.locator('#compose-runtime').fill('48')
    await page.locator('#compose-released').fill('2034-03-02')
    await page.locator('#compose-network').fill('Arte')
    await page.locator('#compose-studio').fill('Atelier e2e')
    await page.locator('#compose-overview').fill('Une restauratrice de films muets.')
    await page.locator('#compose-genres').fill('Drame, drame, Mystère')
    const answered = page.waitForResponse((r) => r.request().method() === 'POST' && r.url().endsWith('/api/v1/items'))
    await submit.click()
    const response = await answered
    expect(response.status()).toBe(201)
    const work = (await response.json()) as {
      id: string
      genres: string[]
      externalIds: { imdb?: string }
    }

    try {
      expect(work).toMatchObject({
        kind: 'series',
        title,
        year: 2034,
        status: 'ended',
        runtime: 48,
        firstAired: '2034-03-02',
        network: 'Arte',
        studio: 'Atelier e2e',
        overview: 'Une restauratrice de films muets.',
      })
      // A genre typed twice is kept once.
      expect(work.genres).toEqual(['Drame', 'Mystère'])
      expect(work.externalIds.imdb).toBe(imdb)
    } finally {
      await page.request.delete(`/api/v1/items/${work.id}`)
    }
  })
})
