import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { scan } from './support/axe'

/**
 * IMDb's titles by region and its episodes, as the interface shows them: an
 * episode's IMDb figure on its page, the lists downloaded whole on the
 * Sources page, and their two tasks on the Jobs page.
 *
 * The lists themselves are half a gigabyte and fifty megabytes from IMDb, and
 * nothing here downloads them. An episode's figure is the answer to the
 * page's own request, given here as the server gives it once the episodes are
 * read: so this runs against whatever the instance holds, read or not.
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

interface Episode {
  id: string
  seasonNumber: number
  episodeNumber: number
}

/** A regular episode of one of the most popular series. */
async function anEpisode(request: APIRequestContext): Promise<{ workId: string; episode: Episode } | undefined> {
  const { items } = await (await request.get('/api/v1/items?kind=series&limit=10')).json()
  for (const { id } of items as { id: string }[]) {
    const work = (await (await request.get(`/api/v1/items/${id}`)).json()) as { episodes?: Episode[] }
    const episode = work.episodes?.find((e) => e.seasonNumber > 0)
    if (episode) return { workId: id, episode }
  }
  return undefined
}

/** What the server answers for the episode's season, as `on` says. */
async function answer(page: Page, workId: string, episode: Episode, on: boolean) {
  await page.route(`**/api/v1/items/${workId}/imdb/episodes**`, (route) =>
    route.fulfill({
      json: {
        on,
        episodes: on
          ? [
              {
                episodeId: episode.id,
                seasonNumber: episode.seasonNumber,
                episodeNumber: episode.episodeNumber,
                imdbId: 'tt0959621',
                rating: 8.6,
                votes: 41203,
                matchedBy: 'numbers',
              },
            ]
          : [],
      },
    }),
  )
}

test.describe('an episode IMDb numbers alike', () => {
  test('shows IMDb’s figure, says it was matched by its number, and leads to IMDb’s page', async ({
    page,
    request,
  }) => {
    const found = await anEpisode(request)
    test.skip(!found, 'the catalogue holds no series with episodes')
    const { workId, episode } = found!
    await answer(page, workId, episode, true)

    await page.goto(`/work/${workId}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`)
    const figure = page.locator('[data-imdb-rating]')
    await expect(figure).toBeVisible()
    // Named, for whoever cannot see the caption beside it.
    await expect(figure.getByRole('img', { name: /IMDb/ })).toBeVisible()
    await expect(figure).toContainText(/by number|d’après son numéro/)

    // Offered beside the other sources' and in the record, with what the
    // match rests on said there in full; every one opens beside the
    // catalogue, and tells IMDb nothing of where it came from.
    const imdb = page.locator('a[href="https://www.imdb.com/title/tt0959621/"]')
    await expect(imdb.first()).toBeVisible()
    for (const link of await imdb.all()) {
      await expect(link).toHaveAttribute('target', '_blank')
      await expect(link).toHaveAttribute('rel', /noreferrer/)
    }
    await expect(page.getByText(/same season and number|même saison et de même numéro/)).toBeVisible()

    await scan(page, 'an episode with IMDb’s figure')
  })

  test('shows nothing of IMDb while its episodes are not read', async ({ page, request }) => {
    const found = await anEpisode(request)
    test.skip(!found, 'the catalogue holds no series with episodes')
    const { workId, episode } = found!
    await answer(page, workId, episode, false)

    await page.goto(`/work/${workId}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`)
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()
    await expect(page.locator('[data-imdb-rating]')).toHaveCount(0)
    await expect(page.locator('a[href^="https://www.imdb.com/title/"]')).toHaveCount(0)
  })
})

test.describe('IMDb’s lists, for whoever keeps the catalogue', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('the Sources page says what each list brings, whether it is on, and when it last ran', async ({ page }) => {
    await signIn(page)
    await page.goto('/admin/sources')

    const lists = (await (await page.request.get('/api/v1/sources/datasets')).json()) as {
      id: string
      task: string
      on: boolean
      imported?: { rows: number } | null
    }[]
    expect(lists.map((list) => list.id)).toEqual(['imdb-ratings', 'imdb-akas', 'imdb-episodes', 'anime-lists'])

    const panel = page.locator('#datasets')
    await expect(panel).toBeVisible()
    for (const list of lists) {
      const row = panel.locator(`[data-dataset="${list.id}"]`)
      await expect(row).toBeVisible()
      // Said in a word beside its lamp, not by the lamp's colour alone.
      await expect(row.getByText(list.on ? /^(On|Activée)$/ : /^(Off|Désactivée)$/)).toBeVisible()
      if (!list.imported) await expect(row).toContainText(/Not downloaded yet|Pas encore téléchargée/)
      // The way to its task, where it is run now, stopped or scheduled.
      await expect(row.locator(`a[href="/admin/jobs#task-${list.task}"]`)).toBeVisible()
    }

    await scan(page, 'the Sources page')
  })

  test('the Jobs page has a card for IMDb’s titles and one for its episodes', async ({ page }) => {
    await signIn(page)
    await page.goto('/admin/jobs')
    for (const id of ['imdb.akas', 'imdb.episodes']) {
      const card = page.locator(`[id="task-${id}"]`)
      await expect(card).toBeVisible()
      // Run now, or again once it has run: named with the task, for whoever
      // moves from card to card.
      await expect(card.getByRole('button', { name: /^(Run now|Run again|Lancer|Relancer): IMDb/ })).toBeAttached()
    }
  })
})
