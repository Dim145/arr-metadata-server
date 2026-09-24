import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Page } from '@playwright/test'

/**
 * Every screen, scanned against WCAG 2.1 AA.
 *
 * An automated scan finds perhaps a third of what a person with a screen reader
 * would — contrast, names, roles, structure — but it finds it every time, which
 * is the part that regresses quietly. Everything else about access is tested
 * by hand and written down in the specs beside this one.
 *
 * The works are looked up rather than hard-coded, so the scan runs against
 * whatever catalogue the instance holds.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

const WCAG = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']

async function scan(page: Page, label: string) {
  // Mid-fade, text is measured at part of its opacity: a dialog scanned the
  // moment it opened failed on colours that pass once it has finished arriving.
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== 'running'))

  const results = await new AxeBuilder({ page }).withTags(WCAG).analyze()

  const violations = results.violations.map((v) => ({
    rule: v.id,
    impact: v.impact,
    help: v.help,
    where: v.nodes.slice(0, 3).map((n) => n.target.join(' ')),
    count: v.nodes.length,
  }))

  expect(violations, `${label} has accessibility violations`).toEqual([])
}

async function firstWork(page: Page, kind: 'series' | 'movie') {
  const response = await page.request.get(`/api/v1/items?kind=${kind}&limit=1`)
  const { items } = await response.json()
  return items[0]?.id as string | undefined
}

interface Work {
  id: string
  episodes?: { seasonNumber: number; episodeNumber: number }[]
  credits?: { tmdbPersonId?: number }[]
}

/** A series with an episode outside the specials, and where it leads. */
async function aSeriesWithEpisodes(page: Page) {
  const { items } = await (await page.request.get('/api/v1/items?kind=series&limit=10')).json()
  for (const { id } of items as { id: string }[]) {
    const work = (await (await page.request.get(`/api/v1/items/${id}`)).json()) as Work
    const episode = work.episodes?.find((e) => e.seasonNumber > 0)
    if (episode) {
      return { work, episode, person: work.credits?.find((c) => c.tmdbPersonId)?.tmdbPersonId }
    }
  }
  return undefined
}

test.describe('the catalogue, to a visitor', () => {
  for (const [label, path] of [
    ['the front page', '/'],
    ['a browse page', '/browse?kind=series'],
    ['an empty result', '/browse?q=zzzqqq'],
    ['a narrowed and ordered list', '/browse?kind=series&genre=Drama&yearFrom=2000&order=rated'],
    ['the schedule', '/calendar'],
    ['sign-in', '/login'],
  ] as const) {
    test(`${label} meets WCAG 2.1 AA`, async ({ page }) => {
      await page.goto(path)
      await page.waitForLoadState('networkidle')
      await scan(page, label)
    })
  }

  test('a work meets WCAG 2.1 AA', async ({ page }) => {
    const id = await firstWork(page, 'series')
    test.skip(!id, 'the catalogue holds no series')

    await page.goto(`/work/${id}`)
    await page.waitForLoadState('networkidle')
    await scan(page, 'a work')
  })

  test('a film meets WCAG 2.1 AA', async ({ page }) => {
    const id = await firstWork(page, 'movie')
    test.skip(!id, 'the catalogue holds no film')

    await page.goto(`/work/${id}`)
    await page.waitForLoadState('networkidle')
    await scan(page, 'a film')
  })

  test('a season, an episode and a person meet WCAG 2.1 AA', async ({ page }) => {
    const found = await aSeriesWithEpisodes(page)
    test.skip(!found, 'the catalogue holds no series with episodes')
    const { work, episode, person } = found!

    const pages = [
      `/work/${work.id}/season/${episode.seasonNumber}`,
      `/work/${work.id}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`,
      ...(person ? [`/person/${person}`] : []),
    ]

    for (const path of pages) {
      await page.goto(path)
      await page.waitForLoadState('networkidle')
      await scan(page, path)
    }
  })

  test('the artwork viewer meets WCAG 2.1 AA', async ({ page }) => {
    const id = await firstWork(page, 'series')
    test.skip(!id, 'the catalogue holds no series')

    // From the record, not the page: the page has not drawn its gallery yet
    // when it first answers, and a check made then skipped every time.
    const work = await (await page.request.get(`/api/v1/items/${id}`)).json()
    const own = (work.images ?? []).filter(
      (i: { seasonNumber?: number | null; coverType: string }) =>
        (i.seasonNumber === undefined || i.seasonNumber === null) &&
        ['poster', 'fanart', 'landscape', 'banner', 'clearlogo', 'clearart'].includes(i.coverType),
    )
    test.skip(!own.length, 'the series has no artwork')

    await page.goto(`/work/${id}`)
    const opener = page.getByRole('button', { name: /^(open artwork|ouvrir l’image) 1$/i })
    await expect(opener).toBeVisible()
    await opener.click()
    await expect(page.getByRole('dialog')).toBeVisible()
    await page.waitForLoadState('networkidle')
    await scan(page, 'the artwork viewer')
  })

  test('the filters, opened, meet WCAG 2.1 AA', async ({ page }) => {
    await page.goto('/browse')
    await page.waitForLoadState('networkidle')

    // A phone keeps them in a sheet; a wide screen has them out already.
    const open = page.getByRole('button', { name: /^(filters|filtres)/i })
    if (await open.isVisible()) {
      await open.click()
      await expect(page.getByRole('dialog')).toBeVisible()
    }

    await scan(page, 'the filters')
  })
})

test.describe('the administration side', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs a credential; see admin.spec.ts')

  test('every screen meets WCAG 2.1 AA', async ({ page }) => {
    await page.goto('/login')
    await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
    await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
    await page.getByRole('button', { name: /sign in|se connecter/i }).click()
    await page.waitForURL('**/admin')

    const id = await firstWork(page, 'series')
    const found = await aSeriesWithEpisodes(page)
    const screens = [
      '/admin',
      '/admin/catalogue',
      '/admin/catalogue?refreshFailed=1',
      '/admin/discover',
      '/admin/clients',
      '/admin/jobs',
      '/admin/audit',
      '/admin/settings',
      ...(id ? [`/admin/catalogue/${id}`] : []),
      // An episode's fields, opened from its public page.
      ...(found
        ? [
            `/admin/catalogue/${found.work.id}?season=${found.episode.seasonNumber}&episode=${found.episode.episodeNumber}`,
          ]
        : []),
    ]

    for (const path of screens) {
      await page.goto(path)
      await page.waitForLoadState('networkidle')
      await scan(page, path)
    }
  })
})
