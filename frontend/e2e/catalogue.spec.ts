import { expect, test, type Page } from '@playwright/test'

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

/**
 * Whether the page can be dragged sideways.
 *
 * Asserted on every page because it is the failure that hides: a child that
 * scrolls horizontally on purpose will, without containment, make the whole
 * document scroll, and nothing about the layout looks wrong until you try.
 */
async function scrollsSideways(page: Page) {
  return page.evaluate(() => {
    window.scrollTo(600, 0)
    const moved = window.scrollX > 0
    window.scrollTo(0, 0)
    return moved
  })
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

    // Both kinds are offered, and each row says how many it is a window onto.
    await expect(page.getByRole('heading', { name: /^(Series|Séries)$/ })).toBeVisible()
    await expect(page.getByRole('heading', { name: /^(Films)$/ })).toBeVisible()
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

  test('offers what it finds as the search is typed, and opens one with the keys', async ({
    page,
    request,
  }) => {
    const { items } = await (await request.get('/api/v1/items?kind=series&limit=1')).json()
    const work = items[0] as { id: string; title: string } | undefined
    test.skip(!work, 'the catalogue holds no series')

    await page.goto('/')
    const reveal = page.getByRole('button', { name: /^(search|rechercher)/i })
    if (await reveal.isVisible()) await reveal.click()

    const box = page.getByRole('combobox').first()
    await box.fill(work!.title.slice(0, 4))
    const option = page.getByRole('option', { name: new RegExp(work!.title.slice(0, 4), 'i') }).first()
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
    await expect(page.locator(`main a[href^="/work/${longest!.id}/season/"]:not([href*="/episode/"])`)).toHaveCount(
      seasons.size,
    )
    // The episode that aired last and the one airing next, and no more.
    expect(await page.locator(`main a[href*="/episode/"]`).count()).toBeLessThanOrEqual(2)
  })
})

/**
 * Where the header's secondary controls are.
 *
 * On a phone the language toggle and the way in live in the menu — together
 * with the logo, search and menu buttons they were wider than the screen, and
 * at 320px the menu button itself was pushed off the edge. Opening the menu
 * first, when there is one, is what a person on a phone does.
 */
async function openMenuIfNarrow(page: Page) {
  const menu = page.getByRole('button', { name: /^(menu)$/i })
  if (await menu.isVisible()) {
    await menu.click()
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
  test.use({ viewport: { width: 320, height: 640 } })

  test('keeps the menu button on the screen, and the menu opens', async ({ page }) => {
    await page.goto('/')

    const menu = page.getByRole('button', { name: /^(menu)$/i })
    const box = await menu.boundingBox()

    // Clipped past the edge, it could not be pressed and the navigation behind
    // it could not be reached at all.
    expect(box).not.toBeNull()
    expect(box!.x + box!.width).toBeLessThanOrEqual(320)

    await menu.click()
    await expect(page.getByRole('link', { name: /^(series|séries)$/i }).last()).toBeVisible()
  })
})

test.describe('the bar on a tablet', () => {
  // Between the phone's menu and the wide bar's field: every control in the
  // bar at once, on a screen not much wider than they are.
  for (const width of [768, 820]) {
    test(`keeps every control on a ${width}px screen`, async ({ page }) => {
      await page.setViewportSize({ width, height: 1024 })
      await page.goto('/')

      for (const name of [/^(sign in|se connecter|admin|administration)$/i, /^(search|rechercher)/i, /^(menu)$/i]) {
        const control = page.getByRole(/menu/.test(name.source) ? 'button' : 'link', { name }).first()
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
    await page.keyboard.press('Tab')

    const skip = page.getByRole('link', { name: /skip to content|aller au contenu/i })
    await expect(skip).toBeFocused()
  })

  test('marks the section you are in', async ({ page, isMobile }) => {
    test.skip(isMobile, 'the tabs are behind the menu at this width')

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
    test.skip(isMobile, 'the tabs are behind the menu at this width')

    // Browse, Series and Films all point at /browse and differ only by a query
    // parameter, which a path comparison cannot tell apart — so all three lit
    // at once. One is current, and it is the one whose parameter matches.
    for (const [path, expected] of [
      ['/browse', 'Browse'],
      ['/browse?kind=series', 'Series'],
      ['/browse?kind=movie', 'Films'],
    ] as const) {
      await page.goto(path)
      await catalogueLoaded(page)

      const current = await page.evaluate(() =>
        [...document.querySelectorAll('nav a[aria-current="page"]')].map((a) =>
          (a.textContent ?? '').trim(),
        ),
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
