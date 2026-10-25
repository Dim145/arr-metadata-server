/**
 * A screen, or what is open on it, scanned against WCAG 2.1 AA, as
 * `accessibility.spec.ts` scans the pages: for the views a spec opens on its
 * way — a step of a wizard, a dialog — which the scan of every page never
 * reaches.
 */

import AxeBuilder from '@axe-core/playwright'
import { expect, type Page } from '@playwright/test'

const WCAG = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']

export async function scan(page: Page, label: string): Promise<void> {
  // Mid-fade, text is measured at part of its opacity: wait until what the
  // page animates has arrived.
  await page.waitForFunction(() =>
    document.getAnimations().every((a) => a.playState !== 'running' || a.timeline !== document.timeline),
  )
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
