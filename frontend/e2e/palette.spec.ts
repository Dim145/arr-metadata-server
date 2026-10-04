import { expect, test } from '@playwright/test'
import { escaped, pattern } from './support/regex'

/**
 * The command palette: opened from the keyboard anywhere, it finds a place
 * to go and a work by its title, and closes as it came.
 */
test.describe('the command palette', () => {
  test('opens with the keyboard, goes to a place, and finds a work', async ({ page, isMobile }) => {
    test.skip(isMobile, 'a keyboard shortcut is a desktop thing')
    const { items } = await (await page.request.get('/api/v1/items?limit=1')).json()
    const work = items[0] as { id: string; title: string } | undefined

    await page.goto('/')
    await page.keyboard.press('ControlOrMeta+k')
    const box = page.getByRole('combobox', { name: /command palette|palette de commandes/i })
    await expect(box).toBeVisible()
    await expect(box).toBeFocused()

    // A place: the films, in either language, then Enter.
    await box.fill('Fil')
    await expect(page.getByRole('option', { name: /films/i })).toBeVisible()
    await box.press('Enter')
    await expect(page).toHaveURL(/kind=movie/)
    await expect(box).toBeHidden()

    // A work, by the start of its title; Escape closes without going.
    test.skip(!work, 'no work in the catalogue')
    await page.keyboard.press('ControlOrMeta+k')
    await box.fill(work!.title.slice(0, 6))
    await expect(page.getByRole('option', { name: pattern(escaped(work!.title.slice(0, 6)), 'i') }).first()).toBeVisible()
    await box.press('Escape')
    await expect(box).toBeHidden()
    await expect(page).toHaveURL(/kind=movie/)

    // The button in the bar opens it too.
    await page.getByRole('button', { name: /command palette|palette de commandes/i }).click()
    await expect(box).toBeVisible()
  })

  test('offers what was opened lately before anything is typed, and a work at random', async ({ page, isMobile }) => {
    test.skip(isMobile, 'a keyboard shortcut is a desktop thing')
    const { items } = await (await page.request.get('/api/v1/items?limit=1')).json()
    const work = items[0] as { id: string; title: string } | undefined
    test.skip(!work, 'no work in the catalogue')

    await page.goto(`/work/${work!.id}`)
    await expect(page.getByRole('heading', { level: 1 })).toBeVisible()

    await page.goto('/')
    await page.keyboard.press('ControlOrMeta+k')
    const box = page.getByRole('combobox', { name: /command palette|palette de commandes/i })
    await expect(box).toBeVisible()

    // The work just opened, listed before any place, with nothing typed.
    const options = page.getByRole('option')
    await expect(options.first()).toContainText(work!.title)

    // A word typed is a search: the recent give way to it.
    await box.fill('zzz')
    await expect(options.filter({ hasText: work!.title })).toHaveCount(0)
    await box.fill('')

    // And a draw from the whole catalogue lands on a work.
    await page.getByRole('option', { name: /at random|au hasard/i }).click()
    await page.waitForURL(/\/work\/[^/]+$/)
    await expect(box).toBeHidden()
  })
})
