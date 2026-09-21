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

    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
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

    const described = await page.evaluate(() =>
      [...document.querySelectorAll('a[href^="/work/"] img')].every(
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

  test('a search from the bar lands on a filtered list', async ({ page }) => {
    await page.goto('/')
    await catalogueLoaded(page)

    const reveal = page.getByRole('button', { name: /^(search|rechercher)$/i })
    if (await reveal.isVisible()) {
      await reveal.click()
    }

    const box = page.getByRole('searchbox').first()
    await box.fill('breaking')
    await box.press('Enter')

    await expect(page).toHaveURL(/\/browse\?q=breaking/)
    await expect(page.getByRole('heading', { level: 1 })).toContainText('breaking')
    await expect(page.locator('a[href^="/work/"]')).toHaveCount(1)
  })

  test('a filter is carried in the URL, so the view can be linked', async ({ page }) => {
    await page.goto('/browse?kind=movie')
    await catalogueLoaded(page)

    const kinds = await page.evaluate(() =>
      [...document.querySelectorAll('a[href^="/work/"]')].length,
    )
    expect(kinds).toBeGreaterThan(0)

    // The control reflects the URL rather than its own default.
    await expect(page.getByLabel(/kind|type/i)).toHaveValue('movie')
  })

  test('says so plainly when a filter matches nothing', async ({ page }) => {
    await page.goto('/browse?q=zzzzznothinghere')

    await expect(page.getByText(/no work matches|aucune œuvre/i)).toBeVisible()
    await expect(page.getByRole('button', { name: /clear|effacer/i }).first()).toBeVisible()
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

  test('scrolls a season inside its own strip, not the page', async ({ page }) => {
    await page.goto('/browse?kind=series')
    await catalogueLoaded(page)
    await page.locator('a[href^="/work/"]').first().click()
    await expect(page.getByRole('heading', { name: /^(Seasons|Saisons)$/ })).toBeVisible()

    const moved = await page.evaluate(async () => {
      const strip = document.querySelector<HTMLElement>('.snap-x')
      if (!strip || strip.scrollWidth <= strip.clientWidth) return null

      strip.scrollTo(400, 0)
      await new Promise((r) => setTimeout(r, 150))
      return { inside: strip.scrollLeft > 0, page: window.scrollX > 0 }
    })

    expect(moved, 'no season strip had anything to scroll').not.toBeNull()
    expect(moved!.inside).toBe(true)
    expect(moved!.page).toBe(false)
  })
})

test.describe('language', () => {
  test('switches the whole interface and survives a reload', async ({ page }) => {
    await page.goto('/')
    await catalogueLoaded(page)

    await page.getByRole('button', { name: 'fr', exact: true }).click()
    await expect(page.locator('html')).toHaveAttribute('lang', 'fr')
    await expect(page.getByRole('link', { name: /se connecter/i })).toBeVisible()

    await page.reload()
    await expect(page.locator('html')).toHaveAttribute('lang', 'fr')

    await page.getByRole('button', { name: 'en', exact: true }).click()
    await expect(page.locator('html')).toHaveAttribute('lang', 'en')
  })

  test('asks the server for the same language it is showing', async ({ page }) => {
    await page.goto('/')
    await catalogueLoaded(page)

    const asked = page.waitForRequest((request) => request.url().includes('language=fr'))
    await page.getByRole('button', { name: 'fr', exact: true }).click()

    expect((await asked).url()).toContain('language=fr')
  })
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
