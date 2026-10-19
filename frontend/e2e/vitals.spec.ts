import { expect, test, type Page } from '@playwright/test'

/**
 * The Core Web Vitals of the pages a visitor opens first: the front page,
 * the catalogue, a work, the season chart.
 *
 * Measured the way the `web-vitals` library measures them, without the
 * library — observers set before the page's own scripts run:
 *
 * - LCP, the largest contentful paint: the last candidate the browser
 *   reports before anyone touches the page.
 * - CLS, the layout shift: the worst session window of shifts no input
 *   caused — shifts less than a second apart, five seconds at most.
 * - INP, the interaction to next paint, approximated: a test has no reader
 *   to interact, so one press on an empty part of the page is timed, and
 *   reported rather than held to a threshold — one press is not the 98th
 *   percentile of a visit.
 *
 * Held to Google's "good": LCP within 2.5 s, CLS under 0.1. Each page is
 * opened once before it is measured, so that a server just started — its
 * caches cold, its thumbnails not yet cut — is not what is measured; the
 * measured load has the browser's cache from that first one too, so what is
 * timed is the page — its scripts, its data, its layout — rather than the
 * network between the browser and the providers' image servers.
 *
 * Asked for, not run with the rest: timings taken beside a score of other
 * workers pressing on the same server measure the crowd. Run it alone, on
 * the desktop project, against a server nothing else is using:
 *
 *   VITALS=1 playwright test e2e/vitals.spec.ts --project desktop --workers 1
 */

/** What the page measured of itself (see `observe`). */
interface Vitals {
  /** Milliseconds from the navigation to the largest paint. */
  lcp?: number
  /** What that paint was, as a selector would name it. */
  lcpElement?: string
  /** The worst session window. */
  cls: number
  /** The slowest interaction, in milliseconds — when one took 16 or more, the least the browser reports. */
  inp?: number
}

/**
 * The observers, installed into every page before its own scripts. Run in
 * the page, so it says everything it needs itself.
 */
function observe() {
  const vitals: { lcp?: number; lcpElement?: string; cls: number; inp?: number } = { cls: 0 }
  ;(window as unknown as { __vitals: typeof vitals }).__vitals = vitals

  const named = (element: Element | null | undefined) =>
    element
      ? `${element.tagName.toLowerCase()}${element.id ? `#${element.id}` : ''}${
          element.getAttribute('class') ? `.${(element.getAttribute('class') ?? '').trim().split(/\s+/).slice(0, 3).join('.')}` : ''
        }`
      : undefined

  new PerformanceObserver((list) => {
    const last = list.getEntries().at(-1) as LargestContentfulPaint | undefined
    if (!last) return
    vitals.lcp = last.startTime
    vitals.lcpElement = named(last.element)
  }).observe({ type: 'largest-contentful-paint', buffered: true })

  let session = 0
  let first = 0
  let latest = 0
  new PerformanceObserver((list) => {
    for (const entry of list.getEntries() as (PerformanceEntry & { value: number; hadRecentInput: boolean })[]) {
      if (entry.hadRecentInput) continue
      if (session && entry.startTime - latest < 1000 && entry.startTime - first < 5000) {
        session += entry.value
      } else {
        session = entry.value
        first = entry.startTime
      }
      latest = entry.startTime
      vitals.cls = Math.max(vitals.cls, session)
    }
  }).observe({ type: 'layout-shift', buffered: true })

  new PerformanceObserver((list) => {
    for (const entry of list.getEntries() as PerformanceEventTiming[]) {
      if (entry.interactionId) vitals.inp = Math.max(vitals.inp ?? 0, entry.duration)
    }
  }).observe({ type: 'event', durationThreshold: 16, buffered: true } as PerformanceObserverInit)
}

/** What the page has measured of itself so far. */
async function measured(page: Page): Promise<Vitals> {
  return page.evaluate(() => (window as unknown as { __vitals: Vitals }).__vitals)
}

/**
 * Opens `path` once to warm the server, then again to measure it: settled —
 * the network quiet, a beat for the last picture to be drawn — then pressed
 * once, on the empty margin beside the page, for the interaction.
 */
async function vitalsOf(page: Page, path: string): Promise<Vitals> {
  await page.goto(path)
  await page.waitForLoadState('networkidle')

  await page.goto(path)
  await page.waitForLoadState('networkidle')
  await page.waitForTimeout(500)
  const loaded = await measured(page)

  // Left of the centred column, where only the page's background is.
  await page.mouse.click(8, 450)
  await page.waitForTimeout(300)
  return { ...loaded, inp: (await measured(page)).inp }
}

/** Holds a page to the thresholds, and says what it measured either way. */
function judge(name: string, vitals: Vitals) {
  test.info().annotations.push({
    type: `vitals: ${name}`,
    description:
      `LCP ${vitals.lcp === undefined ? '—' : `${Math.round(vitals.lcp)} ms`}` +
      `${vitals.lcpElement ? ` (${vitals.lcpElement})` : ''}` +
      ` · CLS ${vitals.cls.toFixed(3)}` +
      ` · INP ≈ ${vitals.inp === undefined ? 'under 16 ms' : `${Math.round(vitals.inp)} ms`}`,
  })
  expect(vitals.lcp, `${name}: a largest contentful paint`).toBeDefined()
  expect(vitals.lcp ?? Number.POSITIVE_INFINITY, `${name}: LCP`).toBeLessThanOrEqual(2500)
  expect(vitals.cls, `${name}: CLS`).toBeLessThanOrEqual(0.1)
}

test.describe('Core Web Vitals', () => {
  test.skip(!process.env.VITALS, 'asked for with VITALS=1, run alone on a quiet server')
  // A phone's are a slower processor and network, which a desktop browser
  // pretending to be one does not have: measured at desktop width only.
  test.skip(({ isMobile }) => isMobile, 'measured on the desktop project')

  test.beforeEach(async ({ page }) => {
    await page.addInitScript(observe)
  })

  test('of the front page', async ({ page }) => {
    judge('home', await vitalsOf(page, '/'))
  })

  test('of the catalogue', async ({ page }) => {
    judge('browse', await vitalsOf(page, '/browse'))
  })

  test('of a work', async ({ page, request }) => {
    const { items } = (await (await request.get('/api/v1/items?limit=1')).json()) as { items?: { id: string }[] }
    const work = items?.[0]
    test.skip(!work, 'no work in the catalogue')
    judge('work', await vitalsOf(page, `/work/${work!.id}`))
  })

  test('of the season chart', async ({ page }) => {
    judge('seasons', await vitalsOf(page, '/seasons'))
  })
})
