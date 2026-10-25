import { expect, test, type Page } from '@playwright/test'
import { scan } from './support/axe'
import { escaped, pattern } from './support/regex'

/**
 * The import rules, on the Lists page: one written through its form — TMDB's
 * most popular films of the last three months, every Monday, into a
 * selection made for the test — shown with when it runs next, switched off,
 * changed, and deleted. It is never run: a run asks TMDB, which is not a
 * test's to spend. Server-wide: a rule runs on its own while it is on.
 */

const USERNAME = process.env.AMS_E2E_USER
const PASSWORD = process.env.AMS_E2E_PASSWORD

async function signIn(page: Page) {
  await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
  await page.goto('/login')
  await page.getByLabel(/username|identifiant/i).fill(USERNAME!)
  await page.getByLabel(/password|mot de passe/i).fill(PASSWORD!)
  await page.getByRole('button', { name: /sign in|se connecter/i }).click()
  await page.waitForURL('**/admin')
}

interface Rule {
  id: string
  name: string
  isEnabled: boolean
  nextAt?: string
  listId?: string
  cron: string
}

test.describe('the import rules', () => {
  test.skip(!USERNAME || !PASSWORD, 'needs AMS_E2E_USER and AMS_E2E_PASSWORD')

  test('a rule is written, shown with its next run, switched off, changed and deleted', async ({ page }) => {
    await signIn(page)
    const stamp = Date.now()
    const listName = `E2E top ${stamp}`
    const made = await page.request.post('/api/v1/lists', {
      data: { name: listName, kind: 'movie', mode: 'manual', isPublic: false },
    })
    expect(made.status()).toBe(201)
    const selection = (await made.json()) as { id: string }
    const ruleName = `E2E weekly top ${stamp}`

    try {
      await page.goto('/admin/lists#import-rules')
      const panel = page.getByRole('region', { name: /^import rules$/i })
      await panel.getByRole('button', { name: /^new rule$/i }).click()

      const form = page.getByRole('dialog').filter({ hasText: /a new import rule/i })
      await form.getByLabel(/^name$/i).fill(ruleName)
      await form.getByLabel(/^released in the last$/i).selectOption('90')
      await form.getByLabel(/^how many$/i).fill('20')
      await form.getByLabel(/^into the selection$/i).selectOption({ label: listName })
      await form.getByText(/^in place of its works$/i).click()
      await form.getByRole('button', { name: /^mondays at 05:00$/i }).click()
      // When it would run, said before anything is kept.
      await expect(form.getByText(/^next: /i)).toBeVisible()
      await scan(page, 'the rule’s form')
      await form.getByRole('button', { name: /^save$/i }).click()
      await expect(form).toBeHidden()

      // Its row: what it brings, where, when.
      const row = panel.getByRole('listitem').filter({ hasText: pattern(escaped(ruleName)) })
      await expect(row).toContainText(/TMDB’s 20 most popular films of the last three months/i)
      await expect(row).toContainText(pattern(escaped(listName)))
      await expect(row).toContainText('0 5 * * 1 UTC')
      await expect(row).toContainText(/not run yet/i)
      await scan(page, 'the import rules')

      const rules = (await (await page.request.get('/api/v1/import/rules')).json()) as Rule[]
      const rule = rules.find((one) => one.name === ruleName)!
      expect(rule.listId).toBe(selection.id)
      expect(new Date(rule.nextAt!).getUTCDay()).toBe(1)
      expect(new Date(rule.nextAt!).getUTCHours()).toBe(5)

      // Switched off: no next run.
      await row.getByRole('switch', { name: pattern(`^switched on: ${escaped(ruleName)}$`, 'i') }).click()
      await expect(row).toContainText(/switched off/i)
      await expect
        .poll(async () => ((await (await page.request.get('/api/v1/import/rules')).json()) as Rule[]).find((one) => one.id === rule.id))
        .toMatchObject({ isEnabled: false })

      // Changed.
      await row.getByRole('button', { name: pattern(`^change “${escaped(ruleName)}”$`, 'i') }).click()
      const edit = page.getByRole('dialog').filter({ hasText: pattern(`change “${escaped(ruleName)}”`, 'i') })
      await edit.getByLabel(/^name$/i).fill(`${ruleName} renamed`)
      await edit.getByRole('button', { name: /^save$/i }).click()
      await expect(edit).toBeHidden()
      await expect(panel.getByText(pattern(`${escaped(ruleName)} renamed`))).toBeVisible()

      // Nothing this machine alone reaches is followed.
      const refused = await page.request.post('/api/v1/import/rules', {
        data: { name: 'nope', source: 'feed', url: 'http://127.0.0.1:8479/api/v1/lists/x/ids.txt', cron: '0 5 * * 1' },
      })
      expect(refused.status()).toBe(400)

      // Deleted.
      await panel.getByRole('button', { name: pattern(`^delete: ${escaped(ruleName)} renamed$`, 'i') }).click()
      await page.getByRole('dialog').filter({ hasText: /delete this rule/i }).getByRole('button', { name: /^delete$/i }).click()
      await expect(panel.getByText(pattern(escaped(ruleName)))).toHaveCount(0)
    } finally {
      // The selection, and with it any rule the test left.
      await page.request.delete(`/api/v1/lists/${selection.id}`)
    }
  })
})
