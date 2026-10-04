import { expect, test, type Page } from '@playwright/test'
import { pattern } from './support/regex'

/**
 * The work editor: a record in five tabs — the fields, the seasons and their
 * episodes, the artwork, the people and titles, the work elsewhere — with the
 * address saying which is open.
 *
 * What is added by hand is added to a work made here and removed at the end,
 * one per project: the two projects run at once against one server, and a
 * work they shared they would also edit under each other.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

type Episode = { seasonNumber: number; episodeNumber: number; title: string }
type Series = { id: string; title: string; seasons: { seasonNumber: number }[]; episodes: Episode[] }

/**
 * A series the providers filled: two seasons at least, and a few episodes.
 * `skip` passes over the first such series: the two projects run at once
 * against one server, and a test that locks something on a work must not
 * share it with the same test on the other project.
 */
async function aSeries(page: Page, skip = 0): Promise<Series | undefined> {
  const { items } = await (await page.request.get('/api/v1/items?kind=series&limit=20')).json()
  let passed = 0
  for (const work of items as { id: string }[]) {
    const full = (await (await page.request.get(`/api/v1/items/${work.id}`)).json()) as Series
    if ((full.seasons?.length ?? 0) >= 2 && (full.episodes?.length ?? 0) >= 3) {
      if (passed === skip) return full
      passed += 1
    }
  }
  return undefined
}

/** Which series a locking test takes: the first for one project, the next for the other. */
const ownSeries = (project: string) => (project === 'mobile' ? 1 : 0)

const code = (episode: Pick<Episode, 'seasonNumber' | 'episodeNumber'>) =>
  `S${String(episode.seasonNumber).padStart(2, '0')}E${String(episode.episodeNumber).padStart(2, '0')}`

/** Whether the page is wider than the screen — which a phone pans, and nobody wants. */
async function scrollsSideways(page: Page) {
  return page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth + 1)
}

test.describe('the work editor', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('is a record in tabs, and the address says which is open', async ({ page }) => {
    await signIn(page)
    const series = await aSeries(page)
    test.skip(!series, 'the catalogue holds no series with seasons')

    await page.goto(`/admin/catalogue/${series!.id}`)
    await expect(page.getByRole('heading', { level: 1 })).toContainText(series!.title)
    const tabs = page.getByRole('tablist', { name: /sections/i })
    await expect(tabs.getByRole('tab')).toHaveCount(5)
    await expect(tabs.getByRole('tab', { name: /^(record|fiche)/i })).toHaveAttribute('aria-selected', 'true')

    // The record first: the fields in their groups, and the sources beside them.
    await expect(page.locator('li[data-field="title"]')).toBeVisible()
    await expect(page.locator('#group-identity')).toBeVisible()
    await expect(page.locator('#group-broadcast')).toBeVisible()
    await expect(page.locator('#sources')).toBeVisible()
    expect(await scrollsSideways(page)).toBe(false)

    await tabs.getByRole('tab', { name: /seasons|saisons/i }).click()
    await expect(page).toHaveURL(/tab=seasons/)
    await expect(page.locator('#season-fields')).toBeVisible()
    await expect(page.locator('#episodes')).toBeVisible()
    expect(await scrollsSideways(page)).toBe(false)

    await tabs.getByRole('tab', { name: /artwork|images/i }).click()
    await expect(page).toHaveURL(/tab=artwork/)
    await expect(page.locator('#artwork')).toBeVisible()
    expect(await scrollsSideways(page)).toBe(false)

    await tabs.getByRole('tab', { name: /people|générique/i }).click()
    await expect(page.locator('#credits')).toBeVisible()
    await expect(page.locator('#titles')).toBeVisible()
    await expect(page.locator('#translations')).toBeVisible()

    await tabs.getByRole('tab', { name: /elsewhere|ailleurs/i }).click()
    await expect(page.locator('#identifiers')).toBeVisible()
    await expect(page.locator('#ratings')).toBeVisible()
    await expect(page.locator('#orders')).toBeVisible()
    expect(await scrollsSideways(page)).toBe(false)

    // A reload keeps the tab: the address carries it.
    await page.reload()
    await expect(tabs.getByRole('tab', { name: /elsewhere|ailleurs/i })).toHaveAttribute('aria-selected', 'true')

    // The anchors the public pages and the old editor used still land on
    // the tab that holds them.
    await page.goto(`/admin/catalogue/${series!.id}#artwork`)
    await expect(tabs.getByRole('tab', { name: /artwork|images/i })).toHaveAttribute('aria-selected', 'true')
    await expect(page.locator('#artwork')).toBeVisible()

    // The arrow keys walk the tabs.
    await tabs.getByRole('tab', { name: /artwork|images/i }).focus()
    await page.keyboard.press('ArrowRight')
    await expect(tabs.getByRole('tab', { name: /people|générique/i })).toHaveAttribute('aria-selected', 'true')
  })

  test('groups the fields, filters them, and knows the fields it could not edit before', async ({ page }) => {
    await signIn(page)
    const series = await aSeries(page)
    test.skip(!series, 'the catalogue holds no series with seasons')

    await page.goto(`/admin/catalogue/${series!.id}`)
    // What TheTVDB adds to a homonym's name, and the country a rating is of:
    // held by the server, and now on the record.
    await expect(page.locator('li[data-field="titleQualifier"]')).toBeVisible()
    await expect(page.locator('li[data-field="contentRatingCountry"]')).toBeVisible()
    // A film's fields stay off a series' record until asked for.
    const other = page.locator('#group-other')
    await expect(other).toBeVisible()
    if (!(await other.locator('li[data-field="inCinemas"]').count())) {
      await other.getByRole('button', { name: /show|afficher/i }).click()
    }
    await expect(other.locator('li[data-field="inCinemas"]')).toBeVisible()

    // Filtered to the empty fields, the title — never empty — goes. The
    // radios are the browser's own, hidden under their labels, so the label
    // is what is pressed.
    const filter = page.getByRole('radiogroup', { name: /^(show|afficher)$/i })
    await filter.getByText(/^(empty|vides)/i).click()
    await expect(page.getByRole('radio', { name: /^(empty|vides)/i })).toBeChecked()
    await expect(page.locator('li[data-field="title"]')).toHaveCount(0)
    await filter.getByText(/^(all|tous)/i).click()
    await expect(page.locator('li[data-field="title"]')).toBeVisible()
  })

  test('a season is chosen from the rail, and an episode opens onto every field of its own', async ({ page }) => {
    await signIn(page)
    const series = await aSeries(page)
    test.skip(!series, 'the catalogue holds no series with seasons')

    const numbers = [...new Set(series!.episodes.map((e) => e.seasonNumber))].filter((n) => n > 0).sort((a, b) => a - b)
    const target = numbers[numbers.length - 1]!
    const first = series!.episodes
      .filter((e) => e.seasonNumber === target)
      .sort((a, b) => a.episodeNumber - b.episodeNumber)[0]!

    await page.goto(`/admin/catalogue/${series!.id}?tab=seasons`)
    const rail = page.getByRole('navigation', { name: /^(seasons|saisons)$/i })
    await rail.getByRole('button', { name: pattern(`S${String(target).padStart(2, '0')}`) }).click()
    await expect(page).toHaveURL(pattern(`season=${target}`))
    await expect(page.locator('#season-fields').getByRole('heading', { level: 2 })).toBeVisible()

    // The season's own identifiers, editable now; TMDB's and TheTVDB's.
    await expect(page.locator('#season-fields li[data-field="tvdbId"]')).toBeVisible()
    await expect(page.locator('#season-fields li[data-field="tmdbId"]')).toBeVisible()

    const row = page.locator('#episodes').getByRole('button', { name: pattern(code(first)) })
    await row.click()
    await expect(row).toHaveAttribute('aria-expanded', 'true')
    const fields = page.locator(`#episode-fields-${target}x${first.episodeNumber}`)
    await expect(fields.locator('li[data-field="title"]')).toBeVisible()
    await expect(fields.locator('li[data-field="airDateUtc"]')).toBeVisible()
    // The episode's own identifiers: new here. Where a special belongs is a
    // question a regular episode is not asked.
    await expect(fields.locator('li[data-field="tvdbId"]')).toBeVisible()
    await expect(fields.locator('li[data-field="airedAfterSeasonNumber"]')).toHaveCount(0)
    await expect(fields.getByRole('link', { name: /episode page|page de l’épisode/i })).toBeVisible()

    // Closed again on a second press.
    await row.click()
    await expect(row).toHaveAttribute('aria-expanded', 'false')
    await expect(fields).toHaveCount(0)

    // A special is asked it, in its own season.
    const special = series!.episodes.filter((e) => e.seasonNumber === 0).sort((a, b) => a.episodeNumber - b.episodeNumber)[0]
    if (special) {
      await rail.getByRole('button', { name: /S00/ }).click()
      const specialRow = page.locator('#episodes').getByRole('button', { name: pattern(code(special)) })
      await specialRow.click()
      await expect(page.locator(`#episode-fields-0x${special.episodeNumber} li[data-field="airedAfterSeasonNumber"]`)).toBeVisible()
    }
  })

  test('a work made by hand is built up from the tabs, with every field the server takes', async ({ page }, info) => {
    test.slow()
    await signIn(page)
    const created = await page.request.post('/api/v1/items', {
      data: { kind: 'series', title: `Éditeur e2e ${info.project.name} ${Date.now()}`, year: 2033 },
    })
    expect(created.status()).toBe(201)
    const { id } = (await created.json()) as { id: string }

    try {
      // ── A season, then an episode in it, with the fields the row form never offered.
      await page.goto(`/admin/catalogue/${id}?tab=seasons`)
      await page.getByRole('button', { name: /add a season|ajouter une saison/i }).click()
      await page.locator('#season-number').fill('1')
      await page.locator('#season-title').fill('Première partie')
      await page.locator('#season-overview').fill('Tout commence.')
      await page.locator('form', { has: page.locator('#season-number') }).getByRole('button', { name: /^(add|ajouter)$/i }).click()
      await expect(page.locator('#season-fields li[data-field="title"]')).toContainText('Première partie')
      await expect(page.locator('#season-fields li[data-field="overview"]')).toContainText('Tout commence.')

      await page.getByRole('button', { name: /add an episode|ajouter un épisode/i }).click()
      await page.locator('#episode-number').fill('1')
      await page.locator('#episode-title').fill('Pilote')
      await page.locator('#episode-runtime').fill('42')
      await page.locator('#episode-absolute').fill('1')
      await page.locator('#episode-overview').fill('Un début.')
      await page.locator('form', { has: page.locator('#episode-number') }).getByRole('button', { name: /^(add|ajouter)$/i }).click()
      const episode = page.locator('#episodes').getByRole('button', { name: /S01E01/ })
      await expect(episode).toBeVisible()
      await expect(episode).toHaveAttribute('aria-expanded', 'true')
      await expect(page.locator('#episode-fields-1x1 li[data-field="runtime"]')).toContainText('42')
      await expect(page.locator('#episode-fields-1x1 li[data-field="overview"]')).toContainText('Un début.')
      await expect(page.locator('#episode-fields-1x1 li[data-field="absoluteEpisodeNumber"]')).toContainText('1')
      // Theirs, so it can go — and its row says so.
      await expect(episode.getByText(/^(yours|à vous)$/i)).toBeVisible()

      // ── A credit with a photograph, then taken away again.
      await page.goto(`/admin/catalogue/${id}?tab=people`)
      await page.getByRole('button', { name: /add a credit|ajouter au générique/i }).click()
      await page.locator('#credit-name').fill('Jane Doe')
      await page.locator('#credit-character').fill('Elle-même')
      await page.locator('#credit-image').fill('https://people.example.invalid/jane.jpg')
      await page.locator('form', { has: page.locator('#credit-name') }).getByRole('button', { name: /^(add|ajouter)$/i }).click()
      const credit = page.locator('#credits li', { hasText: 'Jane Doe' })
      await expect(credit).toBeVisible()
      await expect(credit).toContainText(/elle-même/i)
      await expect(credit.locator('img')).toHaveAttribute('src', /people\.example\.invalid/)
      await credit.getByRole('button', { name: /^(delete|supprimer)$/i }).click()
      await page.getByRole('dialog').getByRole('button', { name: /^(delete|supprimer)$/i }).click()
      await expect(credit).toHaveCount(0)

      // ── An alternative title with its kind and its country.
      await page.getByRole('button', { name: /add a title|ajouter un titre/i }).click()
      await page.locator('#alt-title').fill('Editeur.E2E.FRENCH')
      await page.locator('#alt-type').fill('release')
      await page.locator('#alt-language').fill('fr')
      await page.locator('form', { has: page.locator('#alt-title') }).getByRole('button', { name: /^(add|ajouter)$/i }).click()
      const title = page.locator('#titles li', { hasText: 'Editeur.E2E.FRENCH' })
      await expect(title).toBeVisible()
      await expect(title).toContainText(/france/i)

      // ── The season's poster, by address, from the season's own panel: a
      //    choice that locks, and a picture filed with the season.
      await page.goto(`/admin/catalogue/${id}?tab=seasons&season=1`)
      await page.locator('#season-fields').getByRole('button', { name: /change the poster|changer l’affiche/i }).click()
      const picker = page.getByRole('dialog')
      await picker.getByRole('tab', { name: /an address|une adresse/i }).click()
      await picker.getByLabel(/picture’s address|adresse de l’image/i).fill('https://two.example.invalid/saison-1.jpg')
      await picker.getByRole('button', { name: /choose and lock|choisir et verrouiller/i }).click()
      await expect(picker).toBeHidden()
      // Shown as the season's poster at once. No lock is said on a work no
      // source could overrule — the editor's rule for every edit of it.
      await expect(page.locator('#season-fields img[src*="saison-1.jpg"]')).toBeAttached()

      // The artwork tab keeps to the work's own pictures, and leads to the season's.
      await page.goto(`/admin/catalogue/${id}?tab=artwork`)
      await expect(page.locator('#artwork [data-season-images]')).toBeVisible()
      await expect(page.locator('#artwork section', { hasText: /season artwork|images de saison/i })).toHaveCount(0)
      await page.locator('#artwork').getByRole('link', { name: /open the seasons|ouvrir les saisons/i }).click()
      await expect(page).toHaveURL(/tab=seasons/)

      // A picture added to the work itself, by address, in a language.
      await page.goto(`/admin/catalogue/${id}?tab=artwork`)
      await page.locator('#artwork').getByRole('button', { name: /add an image|ajouter une image/i }).click()
      await page.locator('#image-kind').selectOption('poster')
      await page.locator('#image-language').fill('fr')
      await picker.getByRole('tab', { name: /an address|une adresse/i }).click()
      await picker.getByLabel(/picture’s address|adresse de l’image/i).fill('https://two.example.invalid/affiche-fr.jpg')
      await picker.getByRole('button', { name: /^(add|ajouter)$/i }).click()
      await expect(picker).toBeHidden()
      await expect(page.locator('#artwork li', { hasText: 'two.example.invalid' })).toBeVisible()

      // Filed with the season, as the server keeps a season's own pictures,
      // and named as its choice.
      type Picture = { url: string; seasonNumber?: number; isManual: boolean }
      const work = (await (await page.request.get(`/api/v1/items/${id}`)).json()) as {
        images?: Picture[]
        seasons: { seasonNumber: number; images?: Picture[]; primaryImages?: { poster?: string } }[]
        alternativeTitles: { title: string; titleType?: string; language?: string }[]
      }
      const season = work.seasons.find((one) => one.seasonNumber === 1)
      const poster = season?.images?.find((image) => image.url.includes('saison-1.jpg'))
      expect(poster, 'the poster is filed with season 1').toBeDefined()
      expect(poster?.isManual).toBe(true)
      expect(season?.primaryImages?.poster, 'and is the one chosen').toBeDefined()
      expect(work.images?.some((image) => image.url.includes('affiche-fr.jpg'))).toBe(true)
      const alt = work.alternativeTitles.find((one) => one.title === 'Editeur.E2E.FRENCH')
      expect(alt?.titleType).toBe('release')
      expect(alt?.language).toBe('fr')
    } finally {
      await page.request.delete(`/api/v1/items/${id}`)
    }
  })

  test('a season’s poster is chosen among the pictures on offer, locks, and is unlocked again', async ({ page }, info) => {
    await signIn(page)
    const series = await aSeries(page, ownSeries(info.project.name))
    test.skip(!series, 'the catalogue holds no series with seasons')
    const target = series!.seasons.map((s) => s.seasonNumber).filter((n) => n > 0).sort((a, b) => a - b)[0]!
    const scope = `season:${target}`

    try {
      await page.goto(`/admin/catalogue/${series!.id}?tab=seasons&season=${target}`)
      const head = page.locator('#season-fields')
      await head.getByRole('button', { name: /change the poster|changer l’affiche/i }).click()
      const picker = page.getByRole('dialog')
      await expect(picker.getByRole('heading', { name: /choose the poster|choisir l’affiche/i })).toBeVisible()

      // One of the pictures on offer where there are any — the last, which is
      // not the one in place — else an address.
      const offered = picker.getByRole('radio')
      if ((await offered.count()) > 1) {
        // The radio itself is hidden under its picture: the label is what is pressed.
        await offered.last().locator('xpath=..').click()
        await expect(offered.last()).toBeChecked()
      } else {
        await picker.getByRole('tab', { name: /an address|une adresse/i }).click()
        await picker.getByLabel(/picture’s address|adresse de l’image/i).fill('https://season.example.invalid/poster.jpg')
      }
      await picker.getByRole('button', { name: /choose and lock|choisir et verrouiller/i }).click()
      await expect(picker).toBeHidden()

      const chosen = head.locator('[data-field="primaryPoster"]')
      await expect(chosen).toContainText(/chosen poster|affiche choisie/i)
      const locks = (await (await page.request.get(`/api/v1/items/${series!.id}/overrides`)).json()) as { scope: string; field: string }[]
      expect(locks.some((lock) => lock.scope === scope && lock.field === 'primaryPoster')).toBe(true)
      const work = (await (await page.request.get(`/api/v1/items/${series!.id}`)).json()) as {
        seasons: { seasonNumber: number; images?: { id: string }[]; primaryImages?: { poster?: string } }[]
      }
      const season = work.seasons.find((s) => s.seasonNumber === target)!
      expect(season.primaryImages?.poster).toBeDefined()
      expect(season.images?.[0]?.id, 'the chosen poster leads the season’s own').toBe(season.primaryImages?.poster)

      // Taken off again from the same place.
      await chosen.getByRole('button', { name: /^(unlock|déverrouiller)$/i }).click()
      await expect(chosen).toHaveCount(0)
      const after = (await (await page.request.get(`/api/v1/items/${series!.id}/overrides`)).json()) as { scope: string; field: string }[]
      expect(after.some((lock) => lock.scope === scope && lock.field === 'primaryPoster')).toBe(false)
    } finally {
      await page.request.delete(`/api/v1/items/${series!.id}/overrides/${encodeURIComponent(scope)}/primaryPoster`)
    }
  })

  test('an episode’s picture is chosen through the picker, and locks its field', async ({ page }, info) => {
    await signIn(page)
    const series = await aSeries(page, ownSeries(info.project.name))
    test.skip(!series, 'the catalogue holds no series with seasons')
    const first = series!.episodes.filter((e) => e.seasonNumber > 0).sort((a, b) => a.seasonNumber - b.seasonNumber || a.episodeNumber - b.episodeNumber)[0]!
    const scope = `episode:${first.seasonNumber}x${first.episodeNumber}`

    try {
      await page.goto(`/admin/catalogue/${series!.id}?tab=seasons&season=${first.seasonNumber}&episode=${first.episodeNumber}`)
      const fields = page.locator(`#episode-fields-${first.seasonNumber}x${first.episodeNumber}`)
      const row = fields.locator('li[data-field="image"]')
      await expect(row).toBeVisible()
      await row.getByRole('button', { name: /^(edit|modifier)$/i }).click()

      const picker = page.getByRole('dialog')
      await expect(picker.getByRole('heading', { name: /episode’s picture|image de l’épisode/i })).toBeVisible()
      await picker.getByRole('tab', { name: /an address|une adresse/i }).click()
      await picker.getByLabel(/picture’s address|adresse de l’image/i).fill('https://stills.example.invalid/pilot.jpg')
      await picker.getByRole('button', { name: /choose and lock|choisir et verrouiller/i }).click()
      await expect(picker).toBeHidden()

      await expect(row).toContainText('stills.example.invalid')
      await expect(row.getByRole('button', { name: /^(unlock|déverrouiller)$/i })).toBeVisible()
      await row.getByRole('button', { name: /^(unlock|déverrouiller)$/i }).click()
      await expect(row.getByRole('button', { name: /^(edit|modifier)$/i })).toBeVisible()
    } finally {
      await page.request.delete(`/api/v1/items/${series!.id}/overrides/${encodeURIComponent(scope)}/image`)
    }
  })

  test('the masthead changes the work’s own poster through the same picker', async ({ page }, info) => {
    await signIn(page)
    const series = await aSeries(page, ownSeries(info.project.name))
    test.skip(!series, 'the catalogue holds no series with seasons')

    try {
      await page.goto(`/admin/catalogue/${series!.id}`)
      await page.getByRole('button', { name: /change the poster|changer l’affiche/i }).click()
      const picker = page.getByRole('dialog')
      await picker.getByRole('tab', { name: /an address|une adresse/i }).click()
      await picker.getByLabel(/picture’s address|adresse de l’image/i).fill('https://poster.example.invalid/lead.jpg')
      await picker.getByRole('button', { name: /choose and lock|choisir et verrouiller/i }).click()
      await expect(picker).toBeHidden()

      const work = (await (await page.request.get(`/api/v1/items/${series!.id}`)).json()) as {
        images?: { id: string; url: string }[]
        primaryImages?: { poster?: string }
      }
      expect(work.primaryImages?.poster).toBeDefined()
      expect(work.images?.find((image) => image.id === work.primaryImages?.poster)?.url).toBe('https://poster.example.invalid/lead.jpg')
    } finally {
      await page.request.delete(`/api/v1/items/${series!.id}/overrides/item/primaryPoster`)
    }
  })

  test('a work made by hand shows none of its edits as locks, until it is given a source', async ({ page }, info) => {
    await signIn(page)
    const created = await page.request.post('/api/v1/items', {
      data: { kind: 'series', title: `Sans source e2e ${info.project.name} ${Date.now()}`, year: 2035 },
    })
    expect(created.status()).toBe(201)
    const { id } = (await created.json()) as { id: string }

    try {
      // An edit, which the server files as an override all the same.
      const edited = await page.request.put(`/api/v1/items/${id}/overrides`, {
        data: { field: 'overview', value: 'Écrit à la main.' },
      })
      expect(edited.ok()).toBe(true)

      await page.goto(`/admin/catalogue/${id}`)
      const row = page.locator('li[data-field="overview"]')
      await expect(row).toContainText('Écrit à la main.')
      // Nothing refreshes a work with no source, so nothing is locked
      // against anything: no padlock, no unlock, no filter for them.
      await expect(row.getByRole('button', { name: /^(unlock|déverrouiller)$/i })).toHaveCount(0)
      await expect(page.getByRole('radio', { name: /^(locked|verrouillés)/i })).toHaveCount(0)

      // Given a source, the same edit is a lock again — it keeps the
      // overview from what that source says. An identifier is one work's:
      // each project gives its own.
      const sourced = await page.request.put(`/api/v1/items/${id}/overrides`, {
        data: { field: 'externalIds', value: { tvdb: info.project.name === 'desktop' ? 998877 : 998878 } },
      })
      expect(sourced.ok()).toBe(true)
      await page.reload()
      await expect(row.getByRole('button', { name: /^(unlock|déverrouiller)$/i })).toBeVisible()
      await expect(page.getByRole('radio', { name: /^(locked|verrouillés)/i })).toBeVisible()
    } finally {
      await page.request.delete(`/api/v1/items/${id}`)
    }
  })

  test('the identifiers are edited, locked and unlocked from the Elsewhere tab', async ({ page }, info) => {
    await signIn(page)
    const created = await page.request.post('/api/v1/items', {
      data: { kind: 'series', title: `Identifiants e2e ${info.project.name} ${Date.now()}`, year: 2034 },
    })
    expect(created.status()).toBe(201)
    const { id } = (await created.json()) as { id: string }
    // One per project: an identifier another work goes by is refused.
    const tvdb = info.project.name === 'mobile' ? '987654301' : '987654302'

    try {
      await page.goto(`/admin/catalogue/${id}?tab=elsewhere`)
      const panel = page.locator('#identifiers')
      await expect(panel).toContainText(/none: the work is known here alone|aucun : l’œuvre n’est connue qu’ici/i)
      await panel.getByRole('button', { name: /^(edit|modifier)$/i }).click()
      await page.locator('#id-tvdb').fill(tvdb)
      await panel.getByRole('button', { name: /save and lock|enregistrer et verrouiller/i }).click()
      await expect(panel).toContainText(/locked by hand|verrouillés à la main/i)
      await expect(panel.getByRole('link', { name: pattern(tvdb) })).toBeVisible()

      const stored = (await (await page.request.get(`/api/v1/items/${id}`)).json()) as { externalIds: { tvdb?: number } }
      expect(stored.externalIds.tvdb).toBe(Number(tvdb))

      await panel.getByRole('button', { name: /^(unlock|déverrouiller)$/i }).click()
      await expect(panel).not.toContainText(/locked by hand|verrouillés à la main/i)
    } finally {
      await page.request.delete(`/api/v1/items/${id}`)
    }
  })

  test('the record can be kept from the clients, and given back to them', async ({ page }, info) => {
    await signIn(page)
    const created = await page.request.post('/api/v1/items', {
      data: { kind: 'movie', title: `Servie e2e ${info.project.name} ${Date.now()}`, year: 2035 },
    })
    expect(created.status()).toBe(201)
    const { id } = (await created.json()) as { id: string }

    try {
      await page.goto(`/admin/catalogue/${id}`)
      // A film's record: its release group, and no seasons tab.
      await expect(page.locator('#group-release')).toBeVisible()
      await expect(page.getByRole('tablist', { name: /sections/i }).getByRole('tab')).toHaveCount(4)

      const served = page.getByRole('switch', { name: /^(enabled|activée)$/i })
      await expect(served).toHaveAttribute('aria-checked', 'true')
      await served.click()
      await page.getByRole('dialog').getByRole('button', { name: /^(disable|désactiver)$/i }).click()
      await expect(served).toHaveAttribute('aria-checked', 'false')
      await expect(page.getByRole('heading', { level: 1 }).locator('..').getByText(/^(disabled|désactivée)$/i)).toBeVisible()

      const kept = (await (await page.request.get(`/api/v1/items/${id}`)).json()) as { isEnabled: boolean }
      expect(kept.isEnabled).toBe(false)

      await served.click()
      await expect(served).toHaveAttribute('aria-checked', 'true')
    } finally {
      await page.request.delete(`/api/v1/items/${id}`)
    }
  })
})
