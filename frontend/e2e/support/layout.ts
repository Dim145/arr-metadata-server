import type { Page } from '@playwright/test'

/**
 * Whether the page can be dragged sideways.
 *
 * Asserted on every page because it is the failure that hides: a child that
 * scrolls horizontally on purpose will, without containment, make the whole
 * document scroll, and nothing about the layout looks wrong until you try.
 *
 * Measured once a view transition has finished: for the 220ms one lasts,
 * the browser draws the page at the window's full width, the scrollbar's
 * gutter included, and the document can move by the gutter's width until
 * the page settles. A test that arrived by a link would otherwise measure
 * the crossing, and a slow machine arrives during it.
 */
export async function scrollsSideways(page: Page) {
  await page.waitForFunction(() => {
    try {
      return !document.documentElement.matches(':active-view-transition')
    } catch {
      // An engine that does not know the pseudo-class runs no transition.
      return true
    }
  })
  return page.evaluate(() => {
    window.scrollTo(600, 0)
    const moved = window.scrollX > 0
    window.scrollTo(0, 0)
    return moved
  })
}
