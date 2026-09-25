import { expect, test } from '@playwright/test'

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
    await expect(page.getByRole('option', { name: new RegExp(work!.title.slice(0, 6).replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'i') }).first()).toBeVisible()
    await box.press('Escape')
    await expect(box).toBeHidden()
    await expect(page).toHaveURL(/kind=movie/)

    // The button in the bar opens it too.
    await page.getByRole('button', { name: /command palette|palette de commandes/i }).click()
    await expect(box).toBeVisible()
  })
})
