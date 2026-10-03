import { expect, test, type Page } from '@playwright/test'
import { pattern } from './support/regex'

/** A string as a regular expression matching it and nothing else. */
function literally(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/**
 * The administration side, signed in.
 *
 * The credential comes from the environment and has no default. A password in
 * a repository is a password, whatever it was for; and a suite that silently
 * signed in as `admin/admin` would pass against somebody's real instance.
 *
 *   AMS_E2E_USER=admin AMS_E2E_PASSWORD=… npm run test:e2e
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

const SCREENS = [
  ['/admin', /dashboard|tableau de bord/i],
  ['/admin/catalogue', /catalogue/i],
  ['/admin/discover', /^(import|importer)$/i],
  ['/admin/clients', /^(keys & network|clés & réseau)$/i],
  ['/admin/users', /^(members|membres)$/i],
  ['/admin/access', /^(opening & apis|ouverture & api)$/i],
  ['/admin/jobs', /jobs|tâches/i],
  ['/admin/audit', /audit|journal/i],
  ['/admin/settings', /settings|réglages/i],
] as const

async function signIn(page: Page) {
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

async function scrollsSideways(page: Page) {
  return page.evaluate(() => {
    window.scrollTo(600, 0)
    const moved = window.scrollX > 0
    window.scrollTo(0, 0)
    return moved
  })
}

test.describe('the administration side', () => {
  test.skip(
    !USERNAME || !PASSWORD,
    'set AMS_E2E_USER and AMS_E2E_PASSWORD to exercise the administration side',
  )

  test('a row’s buttons take the press, not the row behind them', async ({ page, isMobile }) => {
    test.skip(isMobile, 'the buttons are in the editor at this width')
    await signIn(page)
    await page.goto('/admin/catalogue')

    // By the pointer, as the row's title link stretches over the whole row:
    // the press must reach the button, and the dialog it opens is the proof.
    const remove = page.getByRole('button', { name: /^(delete|supprimer)$/i }).first()
    await expect(remove).toBeVisible()
    await remove.click()
    const dialog = page.getByRole('dialog')
    await expect(dialog).toBeVisible()
    await expect(page).toHaveURL(/\/admin\/catalogue$/)
    await dialog.getByRole('button', { name: /^(cancel|annuler)$/i }).click()
    await expect(dialog).toHaveCount(0)
  })

  test('turns an anonymous visitor away at the door', async ({ page }) => {
    await page.goto('/admin/settings')
    await expect(page).toHaveURL(/\/login/)
    await expect(page.getByRole('button', { name: /sign in|se connecter/i })).toBeVisible()
  })

  test('refuses a wrong password and says so beside the field', async ({ page }) => {
    await page.goto('/login')
    await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
    await page.getByLabel(/password|mot de passe/i).fill('not-the-password')
    await page.getByRole('button', { name: /sign in|se connecter/i }).click()

    await expect(page.getByRole('alert')).toBeVisible()
    await expect(page).toHaveURL(/\/login/)
  })

  test('opens the API documentation, rather than redirecting to itself', async ({ page }) => {
    // Swagger UI redirects its bare path to the same path with a slash, and the
    // normalisation Sonarr needs strips that slash again — so the address the
    // README gives looped until the browser gave up. And the page has to render
    // under this server's content security policy, which a CSP violation would
    // report as a console error.
    const errors: string[] = []
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text())
    })

    await signIn(page)
    await page.goto('/api/docs')

    await expect(page).toHaveURL(/\/api\/docs\/index\.html$/)
    await expect(page.locator('.swagger-ui .information-container')).toBeVisible()
    await expect(page.locator('.swagger-ui .opblock').first()).toBeVisible()

    expect(errors.filter((t) => /Content Security Policy|Refused to/i.test(t))).toEqual([])
  })

  test('opens every screen without scrolling sideways or logging an error', async ({ page }) => {
    const errors: string[] = []
    page.on('console', (m) => {
      if (m.type() === 'error') errors.push(m.text())
    })

    await signIn(page)

    for (const [path, heading] of SCREENS) {
      await page.goto(path)
      await expect(page.getByRole('heading', { name: heading }).first()).toBeVisible()
      expect(await scrollsSideways(page), `${path} scrolls sideways`).toBe(false)
    }

    expect(errors.filter((t) => !/Failed to load resource|ERR_/.test(t))).toEqual([])
  })

  test('is unmistakably a different place from the catalogue', async ({ page }) => {
    await signIn(page)

    // Whatever the chrome looks like, the way back has to be obvious.
    await expect(page.getByRole('link', { name: /back to the catalogue|retour au catalogue/i })).toBeVisible()
    await expect(page.getByRole('button', { name: /sign out|déconnecter/i })).toBeVisible()
  })

  test('speaks French throughout when asked to', async ({ page }) => {
    await signIn(page)
    await page.getByRole('button', { name: 'fr', exact: true }).click()
    await expect(page.locator('html')).toHaveAttribute('lang', 'fr')

    for (const [path] of SCREENS) {
      await page.goto(path)
      await page.waitForLoadState('networkidle')

      // Headings and controls are the strings a port most often leaves behind.
      const stranded = await page.evaluate(() => {
        const english = /\b(Dashboard|Settings|Catalogue entries|Sign out|Back to the catalogue|Search titles|New entry|All kinds|Edited by hand)\b/
        return [...document.querySelectorAll('h1, h2, h3, button, a, label, th')]
          .map((el) => (el.textContent ?? '').trim())
          .filter((text) => text && english.test(text))
      })

      expect(stranded, `${path} still shows English`).toEqual([])
    }
  })
})

test.describe('a locked field', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs a credential; see above')

  test('survives a round trip and says who owns it', async ({ page }) => {
    test.slow()
    await signIn(page)

    await page.goto('/admin/catalogue')
    await page.locator('a[href^="/admin/catalogue/"]').first().click()
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()

    // Sort title is chosen because nothing else reads it: locking it cannot
    // change what any client is served.
    const row = page.locator('li[data-field="sortTitle"]')
    const locked = row.getByText(/^(locked|verrouillé)\b/i)
    const unlock = row.getByRole('button', { name: /^(unlock|déverrouiller)$/i })

    // A run that failed halfway would otherwise leave this field locked and
    // every later run would start from a state it did not create.
    if (await unlock.count()) {
      await unlock.click()
      await expect(locked).toHaveCount(0)
    }

    await row.getByRole('button', { name: /^(edit|modifier)$/i }).click()
    await page.getByLabel(/^sort title$/i).fill('Zzz e2e lock probe')
    await page.getByRole('button', { name: /save and lock|enregistrer et verrouiller/i }).click()

    await expect(locked).toBeVisible()

    // And it must come back after a reload, or it was never stored.
    await page.reload()
    await expect(locked).toBeVisible()
    await expect(row).toContainText('Zzz e2e lock probe')

    await row.getByRole('button', { name: /^(unlock|déverrouiller)$/i }).click()
    await expect(locked).toHaveCount(0)
  })
})

test.describe('network access', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs a credential; see above')

  /**
   * A documentation range, so a rule left behind by a failed run cannot let
   * anything real through — and one address per project, because the two run
   * at the same time against the same server.
   */
  const probe = (project: string) => `203.0.113.${project === 'mobile' ? 21 : 20}`

  test('lets an address in and out again without a restart', async ({ page }, info) => {
    const address = probe(info.project.name)
    await signIn(page)
    await page.goto('/admin/clients')

    const rules = page.getByRole('region').filter({ hasText: /allowed addresses|adresses autorisées/i })
    await expect(rules).toBeVisible()

    // Clean up after a run that failed partway, or this one starts from a
    // state it did not create.
    const existing = rules.locator('li').filter({ hasText: address })
    if (await existing.count()) {
      await existing.getByRole('button', { name: /stop allowing|retirer/i }).click()
      await page.getByRole('button', { name: /stop allowing|retirer/i }).last().click()
      await expect(existing).toHaveCount(0)
    }

    await page.getByLabel(/address or block|adresse ou plage/i).fill(address)
    await page.getByLabel(/^(note)$/i).fill('e2e probe')
    await page.getByRole('button', { name: /^(allow|autoriser)$/i }).click()

    const added = rules.locator('li').filter({ hasText: address })
    await expect(added).toBeVisible()

    // It has to survive a reload, or it was never written down.
    await page.reload()
    await expect(rules.locator('li').filter({ hasText: address })).toBeVisible()

    await rules
      .locator('li')
      .filter({ hasText: address })
      .getByRole('button', { name: /stop allowing|retirer/i })
      .click()
    await page.getByRole('button', { name: /stop allowing|retirer/i }).last().click()
    await expect(rules.locator('li').filter({ hasText: address })).toHaveCount(0)
  })

  test('refuses something that is not an address, beside the field', async ({ page }) => {
    await signIn(page)
    await page.goto('/admin/clients')

    await page.getByLabel(/address or block|adresse ou plage/i).fill('everyone')
    await page.getByRole('button', { name: /^(allow|autoriser)$/i }).click()

    await expect(page.getByRole('alert').filter({ hasText: /CIDR/i })).toBeVisible()
  })

  test('names the callers it has seen', async ({ page, request }) => {
    await signIn(page)

    // Knock on a guarded route so there is something to show.
    await request.get('/v1/tvdb/shows/en/81189', {
      headers: { 'user-agent': 'Sonarr/4.0.20.3014 (e2e)' },
    })

    await page.goto('/admin/clients')
    const callers = page.getByRole('region').filter({ hasText: /who has been calling|qui appelle/i })
    await expect(callers).toBeVisible()

    // The address alone is not actionable; the name and the client are what
    // tell an operator which container to fix.
    await expect(callers.getByText('127.0.0.1').first()).toBeVisible()
    await expect(callers.getByText(/localhost/).first()).toBeVisible()
    // An address is written down once a minute at most, so the client named
    // is whichever knocked first in that minute: this test's Sonarr, or the
    // Radarr of the visitor tests running beside it.
    await expect(callers.getByText(/(Sonarr|Radarr)\//).first()).toBeVisible()
  })
})

test.describe('settings', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs a credential; see above')

  /**
   * Only one project writes the server scope.
   *
   * The two run at once against one server, and a setting said server-wide is
   * the one thing here that is not per-project: both workers would write the
   * same row and each would read the other's value back.
   */
  test('a server setting survives a reload, and one it cannot hold is refused', async ({
    page,
  }, info) => {
    test.skip(info.project.name !== 'desktop', 'the server scope is shared; one writer is enough')

    await signIn(page)
    await page.goto('/admin/settings')

    const limit = page.getByLabel(/results per search|résultats par recherche/i)
    const before = await limit.inputValue()
    const after = before === '11' ? '12' : '11'

    await limit.fill(after)
    await limit.blur()
    await expect(page.getByText(/^(saved|enregistré)$/i).first()).toBeVisible()

    // Stored, not merely accepted by the field.
    await page.reload()
    await expect(page.getByLabel(/results per search|résultats par recherche/i)).toHaveValue(after)

    // The server's own words, beside the field that was refused.
    const interval = page.getByLabel(/between sweeps|entre deux passages/i)
    await interval.fill('30')
    await interval.blur()
    await expect(page.getByRole('alert').filter({ hasText: /60/ })).toBeVisible()

    await page.reload()
    await expect(page.getByLabel(/between sweeps|entre deux passages/i)).not.toHaveValue('30')

    const restore = page.getByLabel(/results per search|résultats par recherche/i)
    await restore.fill(before)
    await restore.blur()
    await expect(page.getByText(/^(saved|enregistré)$/i).first()).toBeVisible()
  })

  /**
   * A rule, named, given settings of its own, and handed back to the server.
   *
   * Its own address per project, for the same reason the allowlist test uses
   * one: both run at once against the same list.
   */
  test('names a rule, overrides a setting on it, and lets it inherit again', async ({
    page,
  }, info) => {
    const address = `203.0.113.${info.project.name === 'mobile' ? 31 : 30}`
    const name = `probe-${info.project.name}`

    await signIn(page)
    await page.goto('/admin/clients')

    const rules = page.getByRole('region').filter({ hasText: /allowed addresses|adresses autorisées/i })
    const row = () => rules.locator('li').filter({ hasText: address }).first()

    // A run that failed partway would otherwise leave the rule behind — named,
    // once it got that far, so it is looked for by its name too.
    const leftover = rules.locator('li').filter({ hasText: pattern(`${literally(address)}|${literally(name)}`) })
    if (await leftover.count()) {
      await leftover.first().getByRole('button', { name: /stop allowing|retirer/i }).click()
      await page.getByRole('button', { name: /stop allowing|retirer/i }).last().click()
      await expect(leftover).toHaveCount(0)
    }

    await page.getByLabel(/address or block|adresse ou plage/i).fill(address)
    await page.getByRole('button', { name: /^(allow|autoriser)$/i }).click()
    await expect(row()).toBeVisible()

    // Unnamed, a rule says so: settings hang off the name, so the blank is an
    // invitation rather than a column nobody filled in.
    await expect(row().getByText(/not named yet|pas encore nommé/i)).toBeVisible()

    await row().getByRole('button', { name: /name this client|nommer ce client/i }).click()
    // The address stays on screen while it is being named.
    await expect(row().getByText(address)).toBeVisible()
    await row().getByLabel(/^(name|nom)$/i).fill(name)
    await row().getByRole('button', { name: /^(save|enregistrer)$/i }).click()

    await page.reload()
    await expect(rules.locator('li').filter({ hasText: name })).toBeVisible()

    await row().getByRole('button', { name: /^(settings|réglages)$/i }).click()
    const dialog = page.getByRole('dialog')
    await expect(dialog).toBeVisible()

    const language = dialog.getByLabel(/language of the answers|langue des réponses/i)

    // Read once the dialog has its values, not the instant it opens: this is a
    // plain read with no retry behind it, and an empty one here becomes an
    // assertion that the field ends up empty at the bottom of this test.
    await expect(language).not.toHaveValue('')
    const inherited = await language.inputValue()

    // Inherited: it says so, it is not editable, and the way out is offered.
    await expect(dialog.getByText(/^(inherited|hérité)$/i).first()).toBeVisible()
    await expect(language).toBeDisabled()

    await dialog.getByRole('button', { name: /set for this one|définir pour celui-ci/i }).first().click()
    await expect(dialog.getByText(/^(set here|défini ici)$/i).first()).toBeVisible()
    await expect(language).toBeEnabled()

    await language.fill('fr-CA')
    await language.blur()
    await expect(dialog.getByText(/^(saved|enregistré)$/i).first()).toBeVisible()

    await page.keyboard.press('Escape')
    await page.reload()
    await row().getByRole('button', { name: /^(settings|réglages)$/i }).click()

    const again = page.getByRole('dialog')
    await expect(again.getByText(/^(set here|défini ici)$/i).first()).toBeVisible()
    await expect(again.getByLabel(/language of the answers|langue des réponses/i)).toHaveValue('fr-CA')

    await again.getByRole('button', { name: /inherit again|hériter à nouveau/i }).first().click()
    await expect(again.getByText(/^(inherited|hérité)$/i).first()).toBeVisible()
    await expect(again.getByLabel(/language of the answers|langue des réponses/i)).toHaveValue(inherited)

    await page.keyboard.press('Escape')
    await row().getByRole('button', { name: /stop allowing|retirer/i }).click()
    await page.getByRole('button', { name: /stop allowing|retirer/i }).last().click()
    await expect(rules.locator('li').filter({ hasText: address })).toHaveCount(0)
  })
})

test.describe('importing a work', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs a credential; see above')

  /** One work per project: both run at once, against one catalogue. */
  const target = (project: string) =>
    project === 'mobile' ? 'Blade Runner 2099' : 'Blade Runner: Black Lotus'

  test('searches the providers, takes one, and opens what it stored', async ({ page }, info) => {
    test.slow()
    await signIn(page)
    await page.goto('/admin/discover')

    // Nothing has been asked yet, and the screen says so rather than showing an
    // empty list that looks like a search with no results.
    await expect(page.getByText(/nothing searched yet|aucune recherche/i)).toBeVisible()

    await page.getByLabel(/^(title|titre)$/i).fill('Blade Runner')
    await page.getByRole('button', { name: /search the providers|chercher chez les sources/i }).click()

    const found = page.getByRole('region').filter({
      hasText: /what the providers have|ce que les sources ont/i,
    })
    const failed = page.getByText(/could not be reached|n’ont pas pu être jointes/i)
    await expect(failed.or(found.locator('li').first()).first()).toBeVisible({ timeout: 60_000 })

    // A machine with no provider credentials cannot exercise this, and saying
    // so is better than a failure that looks like the interface's fault.
    test.skip((await failed.count()) > 0, 'no provider could be reached from here')

    const row = found.locator('li').filter({ hasText: target(info.project.name) }).first()

    // A work this server already holds offers no second import.
    if (await row.getByText(/already held|déjà détenue/i).count()) {
      await expect(row.getByRole('button', { name: /^(import|importer)$/i })).toHaveCount(0)
      return
    }

    await row.getByRole('button', { name: /^(import|importer)$/i }).click()
    await expect(row.getByText(/^(imported|importée)$/i)).toBeVisible({ timeout: 90_000 })

    await row.getByRole('link', { name: /open the entry|ouvrir la fiche/i }).click()
    await page.waitForURL(/\/admin\/catalogue\/[^/]+$/)
    await expect(page.getByRole('heading', { level: 1 })).toContainText(target(info.project.name))

    // Put the catalogue back as it was found, so the next run has something to
    // import: the screen would otherwise correctly refuse to do it twice.
    const id = page.url().split('/').pop()
    expect((await page.request.delete(`/api/v1/items/${id}`)).ok()).toBe(true)
  })
})

/**
 * Two episodes of a series nothing else in the suite reads: the last regular
 * season's last two. The first season's are what the public pages' tests
 * open, at the same moment, and a title changed under one of them made it fail.
 */
async function aQuietSeason(page: Page) {
  const { items } = await (await page.request.get('/api/v1/items?kind=series&limit=10')).json()
  for (const { id } of items as { id: string }[]) {
    const work = await (await page.request.get(`/api/v1/items/${id}`)).json()
    const regular = (work.episodes ?? []).filter((e: { seasonNumber: number }) => e.seasonNumber > 0)
    if (!regular.length) continue
    const season = Math.max(...regular.map((e: { seasonNumber: number }) => e.seasonNumber))
    const episodes = regular
      .filter((e: { seasonNumber: number }) => e.seasonNumber === season)
      .sort((a: { episodeNumber: number }, b: { episodeNumber: number }) => a.episodeNumber - b.episodeNumber)
      .slice(-2)
    if (episodes.length === 2) return { id: work.id as string, season, episodes }
  }
  return undefined
}

test.describe('an episode’s field', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs a credential; see above')

  test('is locked in the episode’s own scope, and its page shows it', async ({ page }, info) => {
    test.slow()
    await signIn(page)

    const found = await aQuietSeason(page)
    test.skip(!found, 'the catalogue holds no season of two episodes')

    // One episode per project: the two run at the same time against one server.
    const episode = found!.episodes[info.project.name === 'mobile' ? 0 : 1]
    const number = episode.episodeNumber as number
    const scope = `episode:${found!.season}x${number}`
    const editor = `/admin/catalogue/${found!.id}?season=${found!.season}&episode=${number}`
    const publicPage = `/work/${found!.id}/season/${found!.season}/episode/${number}`
    const probe = `Zzz e2e episode probe ${info.project.name}`

    // However the test ends, the lock goes: left behind on a real instance,
    // the probe would be the title Sonarr is given for that episode.
    const unlock = () =>
      page.request.delete(`/api/v1/items/${found!.id}/overrides/${encodeURIComponent(scope)}/title`)

    try {
      await unlock()

      await page.goto(editor)
      const row = page.locator(`#episode-fields-${found!.season}x${number} li[data-field="title"]`)
      await expect(row).toBeVisible()

      await row.getByRole('button', { name: /^(edit|modifier)$/i }).click()
      await row.getByRole('textbox').fill(probe)
      await row.getByRole('button', { name: /save and lock|enregistrer et verrouiller/i }).click()
      await expect(row.getByText(/^(locked|verrouillé)\b/i)).toBeVisible()

      await page.goto(publicPage)
      await expect(page.getByRole('heading', { level: 1 })).toHaveText(probe)

      await page.goto(editor)
      await row.getByRole('button', { name: /^(unlock|déverrouiller)$/i }).click()
      await expect(row.getByText(/^(locked|verrouillé)\b/i)).toHaveCount(0)

      await page.goto(publicPage)
      await expect(page.getByRole('heading', { level: 1 })).not.toHaveText(probe)
    } finally {
      await unlock()
    }
  })
})

test.describe('an episode’s air time', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs a credential; see above')

  test('typed without its seconds or zone, is stored as the instant Sonarr reads', async ({ page }, info) => {
    test.slow()
    await signIn(page)

    const found = await aQuietSeason(page)
    test.skip(!found, 'the catalogue holds no season of two episodes')

    const episode = found!.episodes[info.project.name === 'mobile' ? 0 : 1]
    const number = episode.episodeNumber as number
    const scope = `episode:${found!.season}x${number}`
    const editor = `/admin/catalogue/${found!.id}?season=${found!.season}&episode=${number}`
    const unlock = () =>
      page.request.delete(`/api/v1/items/${found!.id}/overrides/${encodeURIComponent(scope)}/airDateUtc`)

    try {
      await unlock()
      await page.goto(editor)
      const row = page.locator(`#episode-fields-${found!.season}x${number} li[data-field="airDateUtc"]`)
      await expect(row).toBeVisible()

      // The field says what it holds, and the hint how to type it.
      await expect(row.getByText(/date and time|date et heure/i).first()).toBeVisible()
      await row.getByRole('button', { name: /^(edit|modifier)$/i }).click()
      await row.getByRole('textbox').fill('2009-03-22T21:00')
      await row.getByRole('button', { name: /save and lock|enregistrer et verrouiller/i }).click()
      await expect(row.getByText(/^(locked|verrouillé)\b/i)).toBeVisible()
      // The value's own line, not the type's example, which reads the same.
      await expect(row.locator('p', { hasText: /^2009-03-22T21:00:00Z$/ })).toBeVisible()

      // What the server holds is the whole instant, which its clients parse.
      const overrides = await (await page.request.get(`/api/v1/items/${found!.id}/overrides`)).json()
      const stored = (overrides as { scope: string; field: string; value: unknown }[]).find(
        (o) => o.scope === scope && o.field === 'airDateUtc',
      )
      expect(stored?.value).toBe('2009-03-22T21:00:00Z')

      // A day alone is not an instant, and the server says so where it was typed.
      await row.getByRole('button', { name: /^(unlock|déverrouiller)$/i }).click()
      await expect(row.getByText(/^(locked|verrouillé)\b/i)).toHaveCount(0)
      await row.getByRole('button', { name: /^(edit|modifier)$/i }).click()
      await row.getByRole('textbox').fill('2009-03-22')
      await row.getByRole('button', { name: /save and lock|enregistrer et verrouiller/i }).click()
      await expect(row.getByText(/expects a date-time/i)).toBeVisible()
    } finally {
      await unlock()
    }
  })
})

test.describe('a provider’s answer', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs a credential; see above')

  test('can be read as it was received', async ({ page }) => {
    await signIn(page)

    // Chosen from the record: the sources are read separately from the page,
    // and a check made as the page first answered skipped every time.
    const { items } = await (await page.request.get('/api/v1/items?limit=10')).json()
    let id: string | undefined
    for (const item of items as { id: string }[]) {
      const snapshots = await (await page.request.get(`/api/v1/items/${item.id}/snapshots`)).json()
      if (Array.isArray(snapshots) && snapshots.length) {
        id = item.id
        break
      }
    }
    test.skip(!id, 'no work has a stored answer')

    await page.goto(`/admin/catalogue/${id}`)
    const raw = page.getByRole('button', { name: /^(what .+ answered|réponse brute de .+)$/i }).first()
    await expect(raw).toBeVisible()
    await raw.click()

    const dialog = page.getByRole('dialog')
    await expect(dialog).toBeVisible()
    await expect(dialog.getByRole('region')).toContainText('{')
    await expect(dialog.getByRole('button', { name: /^(copy|copier)$/i })).toBeVisible()
  })
})

test.describe('a locked genre', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs a credential; see above')

  test('is what the catalogue is filtered by, as soon as it is saved', async ({ page }, info) => {
    test.slow()
    await signIn(page)

    // The least popular films, one per project — the two run at the same time
    // against one server, and sharing a work they shared its lock. Nothing
    // else in the suite reads their genres, and a film's editor has a single
    // genres field, with no seasons below it.
    const { items } = await (await page.request.get('/api/v1/items?kind=movie&limit=50')).json()
    const work = (items as { id: string }[]).at(info.project.name === 'mobile' ? -2 : -1)
    test.skip(!work, 'the catalogue holds too few films')

    const genre = `Zzz e2e genre ${info.project.name}`
    const listed = `/browse?genre=${encodeURIComponent(genre)}`
    const unlock = () => page.request.delete(`/api/v1/items/${work!.id}/overrides/item/genres`)

    try {
      await unlock()

      await page.goto(`/admin/catalogue/${work!.id}`)
      const row = page.locator('li[data-field="genres"]').first()
      await row.getByRole('button', { name: /^(edit|modifier)$/i }).click()
      await row.getByRole('textbox').fill(genre)
      await row.getByRole('button', { name: /save and lock|enregistrer et verrouiller/i }).click()
      await expect(row.getByText(/^(locked|verrouillé)\b/i)).toBeVisible()

      // Filtered by the genre it is shown with now, not the one its
      // providers gave it — and counted under it beside the list.
      await page.goto(listed)
      await expect(page.locator(`a[href="/work/${work!.id}"]`)).toBeVisible()
      await expect(page.locator('a[href^="/work/"]')).toHaveCount(1)

      const facets = await (await page.request.get(`/api/v1/facets?genre=${encodeURIComponent(genre)}`)).json()
      expect(facets.genres).toContainEqual({ value: genre, count: 1 })
    } finally {
      await unlock()
    }

    await page.goto(listed)
    await expect(page.getByText(/no work matches|aucune œuvre/i)).toBeVisible()
  })
})

test.describe('the poster and the background to lead with', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('none by default; the star chooses one, and takes it off again', async ({ page }) => {
    await signIn(page)
    // A work of its own, removed again: nothing any other test reads.
    const created = await page.request.post('/api/v1/items', {
      data: { kind: 'movie', title: `Deux affiches ${Date.now()}`, year: 2032 },
    })
    expect(created.status()).toBe(201)
    const { id } = (await created.json()) as { id: string }

    try {
      for (const host of ['one', 'two']) {
        const added = await page.request.post(`/api/v1/items/${id}/images`, {
          data: { coverType: 'poster', url: `https://${host}.example.invalid/poster.jpg`, sortOrder: 0 },
        })
        expect(added.ok()).toBe(true)
      }
      type Work = { primaryImages?: { poster?: string }; images: { id: string; url: string; coverType: string }[] }
      const read = async () => (await (await page.request.get(`/api/v1/items/${id}`)).json()) as Work
      expect((await read()).primaryImages, 'none is chosen by default').toBeUndefined()

      // The pictures are a tab of the record now; the anchor opens it.
      await page.goto(`/admin/catalogue/${id}#artwork`)
      const artwork = page.locator('#artwork')
      const row = (host: string) => artwork.locator('li').filter({ hasText: `${host}.example.invalid` })
      const star = (host: string) => row(host).getByRole('button', { name: /lead with this one|montrer celle-ci en premier|no longer lead|ne plus la montrer/i })

      await expect(star('two')).toHaveAttribute('aria-pressed', 'false')
      await star('two').click()
      await expect(star('two')).toHaveAttribute('aria-pressed', 'true')
      await expect(row('two').getByText(/^(primary|principale)$/i)).toBeVisible()
      await expect(star('one')).toHaveAttribute('aria-pressed', 'false')

      // The server leads with it: named, and first among the posters.
      const chosen = await read()
      const two = chosen.images.find((image) => image.url.includes('two.example.invalid'))!
      expect(chosen.primaryImages?.poster).toBe(two.id)
      expect(chosen.images.filter((image) => image.coverType === 'poster')[0].id).toBe(two.id)

      await star('two').click()
      await expect(star('two')).toHaveAttribute('aria-pressed', 'false')
      expect((await read()).primaryImages).toBeUndefined()
    } finally {
      await page.request.delete(`/api/v1/items/${id}`)
    }
  })
})
