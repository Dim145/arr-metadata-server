import { expect, test } from '@playwright/test'

/**
 * The lamp: the page is dark unless the reader or the system asks for
 * daylight, and what the reader asks for is remembered.
 */
test.describe('the theme', () => {
  test('the lamp switches the page to daylight, and it stays lit', async ({ page, isMobile }) => {
    test.skip(isMobile, 'the lamp is in the menu at this width')
    await page.goto('/')
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')

    const lamp = page.getByRole('button', { name: /light theme|thème clair/i }).first()
    await expect(lamp).toHaveAttribute('aria-pressed', 'false')
    await lamp.click()
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light')
    await expect(lamp).toHaveAttribute('aria-pressed', 'true')

    // Remembered: the next page is lit before anything renders.
    await page.reload()
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light')
    await page.getByRole('button', { name: /light theme|thème clair/i }).first().click()
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')
  })

  test('the system’s daylight is followed until the reader chooses', async ({ browser }) => {
    const context = await browser.newContext({ colorScheme: 'light' })
    const page = await context.newPage()
    await page.goto('/')
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light')
    await context.close()
  })
})
