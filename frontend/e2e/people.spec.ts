import { expect, test, type APIRequestContext, type Page } from '@playwright/test'
import { escaped, pattern } from './support/regex'

/**
 * The people the catalogue credits: their index, by letter and by job; a
 * person's page, read a job at a time and reached by their record or by
 * TMDB's id; and, for whoever keeps the catalogue, their records.
 *
 * Who is in the index is read through the API rather than named, so this
 * runs against whatever catalogue the instance holds and skips what it
 * cannot show. Nothing here writes: the records are the suite's, and the
 * editor's specs add and take away credits beside these.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

interface Card {
  id: string
  name: string
  tmdbId?: number
  works: number
  jobs: string[]
}

interface Index {
  people: Card[]
  total: number
  letters: string[]
}

interface Role {
  workId: string
  creditType: 'actor' | 'guest' | 'director' | 'writer' | 'producer'
}

/** The jobs in the order their tabs are drawn, and how each is named in either language. */
const JOBS = [
  ['acting', /^(acting|interprétation)\b/i],
  ['directing', /^(directing|réalisation)\b/i],
  ['writing', /^(writing|scénario)\b/i],
  ['producing', /^(producing|production)\b/i],
] as const

type Job = (typeof JOBS)[number][0]

const JOB_OF: Record<Role['creditType'], Job> = {
  actor: 'acting',
  guest: 'acting',
  director: 'directing',
  writer: 'writing',
  producer: 'producing',
}

/** A page of the index, as the API answers it. */
async function index(request: APIRequestContext, search = ''): Promise<Index> {
  const response = await request.get(`/api/v1/people${search}`)
  expect(response.status(), `/api/v1/people${search}`).toBe(200)
  return (await response.json()) as Index
}

/** Somebody's jobs, each with the works they did it on, in the order of the tabs. */
async function jobsOf(request: APIRequestContext, id: string): Promise<{ job: Job; works: string[] }[]> {
  const person = (await (await request.get(`/api/v1/people/${encodeURIComponent(id)}`)).json()) as { roles: Role[] }
  return JOBS.map(([job]) => ({
    job,
    works: [...new Set(person.roles.filter((role) => JOB_OF[role.creditType] === job).map((role) => role.workId))],
  })).filter((entry) => entry.works.length > 0)
}

/** The job a page opens on: the one with the most works, the first of those with as many. */
function opening(jobs: { job: Job; works: string[] }[]): Job | undefined {
  let most: { job: Job; works: string[] } | undefined
  for (const entry of jobs) if (!most || entry.works.length > most.works.length) most = entry
  return most?.job
}

const named = (job: Job) => JOBS.find(([one]) => one === job)![1]

/** The row of letters. */
const letters = (page: Page) => page.getByRole('navigation', { name: /first letter|initiale/i })

/** The tabs of a person's page. */
const tabs = (page: Page) => page.getByRole('navigation', { name: /by job|par métier/i })

/**
 * A choice of a few, wherever the width draws it: a row of radios on a wide
 * screen, a menu on a phone. A radio is chosen by its label, as a pointer
 * chooses it: the radio itself is drawn as the pill round it.
 */
async function choose(page: Page, name: RegExp, value: string, label: RegExp) {
  const menu = page.getByRole('combobox', { name })
  if (await menu.isVisible()) {
    await menu.selectOption(value)
    return
  }
  await page.getByRole('radiogroup', { name }).getByRole('radio', { name: label }).locator('xpath=..').click()
}

test.describe('the index of people', () => {
  test('lists everybody credited, says which letters nobody is filed under, and narrows to one', async ({
    page,
    request,
  }) => {
    const everybody = await index(request, '?limit=1')
    test.skip(!everybody.total, 'nobody is credited in this catalogue')
    const letter = everybody.letters.find((one) => one !== '#') ?? everybody.letters[0]!
    const under = await index(request, `?letter=${encodeURIComponent(letter)}&limit=1`)

    await page.goto('/people')
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(/^(people|personnes)$/i)
    await expect(page.locator('main a[href^="/people/"]').first()).toBeVisible()

    // A letter nobody is filed under is said so, where the keyboard passes,
    // and leads nowhere.
    const bar = letters(page)
    const missing = [...'ABCDEFGHIJKLMNOPQRSTUVWXYZ#'].filter((one) => !everybody.letters.includes(one))
    await expect(bar.locator('[aria-disabled="true"]')).toHaveCount(missing.length)
    await expect(bar.getByRole('link', { name: /^(all|toutes)$/i })).toHaveAttribute('aria-current', 'page')

    await bar.getByRole('link', { name: letter, exact: true }).click()
    await expect(page).toHaveURL(pattern(`[?&]letter=${escaped(encodeURIComponent(letter))}(&|$)`))
    await expect(bar.getByRole('link', { name: letter, exact: true })).toHaveAttribute('aria-current', 'page')
    await expect(page.getByText(pattern(`^${under.total} (person|people|personnes?)$`, 'i'))).toBeVisible()
    await expect(page.locator(`main a[href="/people/${under.people[0]!.id}"]`)).toBeVisible()

    // Every letter again, as the address said before.
    await bar.getByRole('link', { name: /^(all|toutes)$/i }).click()
    await expect(page).not.toHaveURL(/letter=/)
  })

  test('narrows to a job, and a person opened from it reads their work a job at a time', async ({
    page,
    request,
  }) => {
    // Directing, where anybody did it; acting otherwise.
    const directing = await index(request, '?job=directing&limit=1')
    const job: Job = directing.total ? 'directing' : 'acting'
    const listed = directing.total ? directing : await index(request, '?job=acting&limit=1')
    test.skip(!listed.total, 'nobody is credited in this catalogue')
    const person = listed.people[0]!
    const jobs = await jobsOf(request, person.id)

    await page.goto('/people')
    await expect(page.locator('main a[href^="/people/"]').first()).toBeVisible()
    await choose(page, /^(job|métier)$/i, job, named(job))
    await expect(page).toHaveURL(pattern(`[?&]job=${job}(&|$)`))
    const card = page.locator(`main a[href="/people/${person.id}"]`)
    await expect(card).toBeVisible()

    await card.click()
    await expect(page).toHaveURL(pattern(`/people/${escaped(person.id)}$`))
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(person.name)

    // A tab for each job they did, with how many works it holds; the one
    // with the most open.
    const row = tabs(page)
    await expect(row.getByRole('link')).toHaveCount(jobs.length)
    const first = opening(jobs)!
    await expect(row.locator('[aria-current="page"]')).toHaveAccessibleName(named(first))
    const shown = jobs.find((entry) => entry.job === first)!
    await expect(row.locator('[aria-current="page"]')).toContainText(String(shown.works.length))
    await expect(page.locator(`main ol a[href="/work/${shown.works[0]}"]`).first()).toBeVisible()

    // Another job, in the address; and the order, kept with it.
    const other = jobs.find((entry) => entry.job !== first)
    if (other) {
      await row.getByRole('link', { name: named(other.job) }).click()
      await expect(page).toHaveURL(pattern(`[?&]role=${other.job}(&|$)`))
      await expect(row.getByRole('link', { name: named(other.job) })).toHaveAttribute('aria-current', 'page')
      await expect(page.locator(`main ol a[href="/work/${other.works[0]}"]`).first()).toBeVisible()
    }
    await page.getByRole('radiogroup', { name: /^(order|ordre)$/i }).getByRole('radio', { name: /^(title|titre)$/i }).locator('xpath=..').click()
    await expect(page).toHaveURL(/[?&]sort=title(&|$)/)
    await page.reload()
    await expect(
      page.getByRole('radiogroup', { name: /^(order|ordre)$/i }).getByRole('radio', { name: /^(title|titre)$/i }),
    ).toBeChecked()
  })

  test('finds somebody by a part of their name, across every letter', async ({ page, request }) => {
    const everybody = await index(request, '?limit=1')
    test.skip(!everybody.total, 'nobody is credited in this catalogue')
    const person = everybody.people[0]!

    await page.goto('/people')
    await page.getByLabel(/^(find somebody|trouver quelqu’un)$/i).fill(person.name)
    await expect(page).toHaveURL(/[?&]term=/)
    // The bar's own search box is for titles: it is left as it was.
    const bar = await page
      .locator('form[role="search"] input[role="combobox"]')
      .evaluateAll((boxes) => boxes.map((box) => (box as HTMLInputElement).value))
    expect(bar.filter(Boolean)).toEqual([])
    // The letters wait while a name is looked for.
    await expect(letters(page)).toHaveCount(0)
    await expect(page.locator(`main a[href="/people/${person.id}"]`)).toBeVisible()

    await page.getByLabel(/^(find somebody|trouver quelqu’un)$/i).fill('')
    await expect(page).not.toHaveURL(/[?&]term=/)
    await expect(letters(page)).toBeVisible()
  })
})

test.describe('a person', () => {
  test('is the same page by TMDB’s id as by their record', async ({ page, request }) => {
    const known = (await index(request, '?limit=60')).people.find((person) => person.tmdbId)
    test.skip(!known, 'nobody credited here is known by a TMDB id')
    const person = known!

    // One record, by either address.
    const byRecord = await (await request.get(`/api/v1/people/${person.id}`)).json()
    const byTmdb = await (await request.get(`/api/v1/people/${person.tmdbId}`)).json()
    expect(byTmdb.id).toBe(byRecord.id)

    await page.goto(`/person/${person.tmdbId}`)
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(person.name)
    // The older address stays as it was: it names the person on every server.
    await expect(page).toHaveURL(pattern(`/person/${person.tmdbId}$`))
    await expect(tabs(page).locator('[aria-current="page"]')).toHaveCount(1)
    await expect(page.locator(`a[href="https://www.themoviedb.org/person/${person.tmdbId}"]`)).toHaveAttribute('target', '_blank')

    await page.goto(`/people/${person.id}`)
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(person.name)
    await expect(tabs(page).locator('[aria-current="page"]')).toHaveCount(1)
  })

  test('nobody by that record says so', async ({ page }) => {
    await page.goto('/people/no-such-record')
    await expect(
      page.getByRole('heading', { name: /nothing lives at this address|rien à cette adresse/i }),
    ).toBeVisible()
  })
})

test.describe('the people, for whoever keeps the catalogue', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  async function signIn(page: Page) {
    await page.goto('/login')
    await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
    await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
    await page.getByRole('button', { name: /sign in|se connecter/i }).click()
    await page.waitForURL('**/admin')
  }

  test('lists the records, filters them, and opens one with its credits', async ({ page }) => {
    await signIn(page)
    const credited = await index(page.request, '?limit=1')
    test.skip(!credited.total, 'nobody is credited in this catalogue')
    const person = credited.people[0]!

    await page.goto('/admin/people')
    await expect(page.getByRole('heading', { level: 1 })).toHaveText(/^(people|personnes)$/i)
    const list = page.getByRole('region', { name: /^(people|personnes)$/i })
    await expect(list.locator('a[data-person]').first()).toBeVisible()

    // Those known by no TMDB id: no row says one.
    const without = page.getByRole('button', { name: /^(no tmdb id|sans identifiant tmdb)$/i })
    await without.click()
    await expect(page).toHaveURL(/[?&]withoutTmdb=1(&|$)/)
    await expect(without).toHaveAttribute('aria-pressed', 'true')
    await expect(list.locator('a[data-person]', { hasText: /TMDB \d/ })).toHaveCount(0)
    await without.click()
    await expect(page).not.toHaveURL(/withoutTmdb=/)

    // Everybody, credited on anything or not.
    await page.getByRole('radiogroup', { name: /^(who|qui)$/i }).getByRole('radio', { name: /^(everybody|tout le monde)$/i }).locator('xpath=..').click()
    await expect(page).toHaveURL(/[?&]all=1(&|$)/)

    // A name searched, and the record opened from its row: the focus goes
    // with it, and its credits are there to be chosen.
    await page.getByLabel(/^(search a name|chercher un nom)/i).fill(person.name)
    await expect(page).toHaveURL(/[?&]term=/)
    const row = list.locator(`a[data-person="${person.id}"]`)
    await expect(row).toBeVisible()
    await row.click()
    await expect(page).toHaveURL(pattern(`[?&]id=${escaped(person.id)}(&|$)`))
    const record = page.getByRole('region', { name: person.name, exact: true })
    await expect(record.getByRole('heading', { level: 2, name: person.name, exact: true })).toBeFocused()
    // One credit, by its own box rather than a work's, which chooses them all.
    const credit = record.locator('label input[type="checkbox"]').first()
    await expect(credit).toBeVisible()
    await expect(record.getByRole('button', { name: /^(split off|scinder)$/i })).toBeDisabled()
    await credit.check()
    await expect(record.getByText(/^(1 credit chosen|1 crédit choisi)$/i)).toBeVisible()
    await expect(record.getByRole('button', { name: /^(split off|scinder)$/i })).toBeEnabled()

    // Merging asks whom into, and nothing happens until it is said.
    await record.getByRole('button', { name: /^(merge into…|fusionner dans…)$/i }).click()
    const dialog = page.getByRole('dialog')
    await expect(dialog.getByLabel(/^(find somebody|trouver quelqu’un)$/i)).toBeFocused()
    await dialog.getByRole('button', { name: /^(cancel|annuler)$/i }).click()
    await expect(dialog).toBeHidden()

    // Closed, the list is back, and the focus with the row it came from.
    await record.getByRole('link', { name: /^(every person|toutes les personnes)$/i }).click()
    await expect(page).not.toHaveURL(/[?&]id=/)
    await expect(row).toBeFocused()
  })
})
