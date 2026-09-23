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

test.describe('the catalogue, to a visitor', () => {
  for (const [label, path] of [
    ['the front page', '/'],
    ['a browse page', '/browse?kind=series'],
    ['an empty result', '/browse?q=zzzqqq'],
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
    const screens = [
      '/admin',
      '/admin/catalogue',
      '/admin/discover',
      '/admin/clients',
      '/admin/jobs',
      '/admin/audit',
      '/admin/settings',
      ...(id ? [`/admin/catalogue/${id}`] : []),
    ]

    for (const path of screens) {
      await page.goto(path)
      await page.waitForLoadState('networkidle')
      await scan(page, path)
    }
  })
})
