import AxeBuilder from '@axe-core/playwright'
import { expect, test, type Page } from '@playwright/test'

/**
 * The programmer's note: written in place by whoever maintains the catalogue,
 * in English and in French; read by a visitor in theirs, the other standing
 * in until it is written; listed in the administration, written again and
 * taken away there. The front page offers none to write: a note on its work
 * of the day is begun in the administration, and read under the plate.
 *
 * Serial: a note belongs to the catalogue, and every page of the genre reads
 * it — the other specs' genre pages among them. Each test takes its notes
 * away again, whatever became of it.
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

/** The page, scanned against WCAG 2.1 AA once nothing on it is still arriving. */
async function scan(page: Page, label: string) {
  await page.waitForFunction(() =>
    document.getAnimations().every((a) => a.playState !== 'running' || a.timeline !== document.timeline),
  )
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze()
  const violations = results.violations.map((v) => ({ rule: v.id, where: v.nodes.slice(0, 3).map((n) => n.target.join(' ')) }))
  expect(violations, `${label} has accessibility violations`).toEqual([])
}

/** The notes the tests write, taken away without an assertion. */
const WRITTEN = ['genre/drama/en', 'genre/drama/fr', 'year/1999/en']

test.describe('the programmer’s note', () => {
  test.skip(!USERNAME || !PASSWORD, 'set AMS_E2E_USER and AMS_E2E_PASSWORD to an administrator to run the note tests')

  test.afterEach(async ({ page }) => {
    for (const note of WRITTEN) {
      await page.request.delete(`/api/v1/editorial/${note}`)
    }
  })

  test('is written in place in two languages, and read by a visitor in theirs', async ({ page, browser }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
    await signIn(page)
    for (const note of WRITTEN) await page.request.delete(`/api/v1/editorial/${note}`)

    await page.goto('/genre/drama')
    await expect(page.getByRole('heading', { level: 1, name: 'Drama' })).toBeVisible()
    await page.getByRole('button', { name: 'Write a programmer’s note' }).click()
    await page.getByLabel('The note, in English').fill('The *quiet* ones, and [the crimes](/genre/crime) beside them.')
    // What it will look like, beneath the field, before it is saved.
    await expect(page.getByText('As it will read')).toBeVisible()
    await scan(page, 'the note being written in place')
    await page.getByRole('button', { name: 'Save the note' }).click()

    const note = page.getByRole('complementary', { name: 'Programmer’s note' })
    await expect(note.locator('em')).toHaveText('quiet')
    await expect(note.getByRole('link', { name: 'the crimes' })).toHaveAttribute('href', '/genre/crime')

    // A link to another site is said before anything is sent, and Escape
    // puts the note back as it was.
    await note.getByRole('button', { name: 'Edit the note' }).click()
    await page.getByLabel('The note, in English').fill('See [elsewhere](https://example.com).')
    await expect(page.getByRole('alert').filter({ hasText: 'A link leads to a page of this site' })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Save the note' })).toBeDisabled()
    await page.getByLabel('The note, in English').press('Escape')
    await expect(note.locator('em')).toHaveText('quiet')

    // In French, the English note stands in, and the editor is told so.
    const french = await page.context().newPage()
    await french.addInitScript(() => localStorage.setItem('ams.lang', 'fr'))
    await french.goto('/genre/drama')
    await expect(french.getByRole('heading', { level: 1, name: 'Drame' })).toBeVisible()
    await expect(french.getByText(/^Affiché en anglais/)).toBeVisible()
    await french.getByRole('button', { name: 'Modifier le mot' }).click()
    await french.getByLabel('Le mot, en français').fill('Les *calmes*, et [les crimes](/genre/crime) à côté.')
    await french.getByRole('button', { name: 'Enregistrer le mot' }).click()
    await expect(french.getByRole('complementary', { name: 'Le mot du programmateur' }).locator('em')).toHaveText('calmes')
    await french.close()

    // A visitor reads theirs, in either language, and is offered nothing to write.
    const visitor = await browser.newContext()
    for (const [language, words, label] of [
      ['fr', 'Les calmes, et les crimes à côté.', 'Le mot du programmateur'],
      ['en', 'The quiet ones, and the crimes beside them.', 'Programmer’s note'],
    ] as const) {
      const reader = await visitor.newPage()
      await reader.addInitScript((code) => localStorage.setItem('ams.lang', code), language)
      await reader.goto('/genre/drama')
      await expect(reader.getByRole('complementary', { name: label })).toContainText(words)
      await expect(reader.getByRole('button', { name: /Modifier le mot|Edit the note/ })).toHaveCount(0)
      await reader.close()
    }

    // A link to the page carries the note, in the catalogue's own language.
    const html = await (await visitor.request.get('/genre/drama')).text()
    expect(html).toMatch(/<meta property="og:description" content="(The quiet ones|Les calmes)/)
    await visitor.close()
  })

  test('is listed in the administration, written again and taken away there', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
    await signIn(page)
    const written = await page.request.put('/api/v1/editorial/year/1999/en', {
      data: { body: 'The last year of the century.' },
    })
    expect(written.ok(), await written.text()).toBeTruthy()

    await page.goto('/admin/editorial')
    await expect(page.getByRole('heading', { level: 1, name: 'Programmer’s notes' })).toBeVisible()
    await expect(page.getByText('The last year of the century.')).toBeVisible()
    await scan(page, 'the notes in the administration')
    await page.getByRole('button', { name: 'Edit the note on 1999 (en)' }).click()
    await page.getByLabel('The note', { exact: true }).fill('The last year of the *century*.')
    await page.getByRole('button', { name: 'Save the note' }).click()
    await expect(page.getByText('The last year of the century.')).toBeVisible()

    await page.goto('/year/1999')
    await expect(page.getByRole('complementary', { name: 'Programmer’s note' }).locator('em')).toHaveText('century')

    await page.goto('/admin/editorial')
    await page.getByRole('button', { name: 'Take the note on 1999 (en) away' }).click()
    await page.getByRole('dialog').getByRole('button', { name: 'Take it away' }).click()
    await expect(page.getByText('The last year of the century.')).toHaveCount(0)
  })

  test('is not offered on the front page, and one begun in the administration is read there', async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem('ams.lang', 'en'))
    await signIn(page)

    // The work of the day, by the way to its page; who is looking is known by
    // then — the bar offers the administration — and nothing under the plate
    // offers a note to write.
    await page.goto('/')
    const open = page.getByRole('link', { name: 'See the details' })
    await expect(open).toBeVisible()
    await expect(page.getByRole('banner').getByRole('link', { name: 'Admin', exact: true })).toBeVisible()
    await expect(page.getByRole('button', { name: 'Write a programmer’s note' })).toHaveCount(0)
    const id = (await open.getAttribute('href'))?.replace(/^\/work\//, '')
    expect(id).toBeTruthy()

    try {
      await page.goto('/admin/editorial')
      await page.getByRole('button', { name: 'Write a note' }).click()
      const form = page.locator('form').filter({ has: page.getByRole('button', { name: 'Save the note' }) })
      await form.getByLabel('About').selectOption('work')
      await form.getByLabel('Which one').fill(id!)
      await form.getByLabel('The note', { exact: true }).fill('Why this one, *today*.')
      await form.getByRole('button', { name: 'Save the note' }).click()
      await expect(page.getByText('Why this one, today.')).toBeVisible()

      await page.goto('/')
      await expect(page.getByRole('complementary', { name: 'Programmer’s note' }).locator('em')).toHaveText('today')
    } finally {
      await page.request.delete(`/api/v1/editorial/work/${encodeURIComponent(id!)}/en`)
    }
  })
})
