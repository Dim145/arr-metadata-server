/**
 * Ways out of the catalogue, and ways back up it.
 *
 * Both are about where a reader stands: a trail up from an episode to its
 * season and its series, and the pages the same work has on the sites this
 * one was assembled from. The second always opens beside the catalogue rather
 * than over it, and says so to a screen reader, which cannot see the glyph.
 */

import { Fragment } from 'react'
import { Link } from 'react-router'

import { cn } from '../lib/cn'
import { useI18n } from '../lib/i18n'
import { providerName } from '../lib/labels'
import type { Destination } from '../lib/links'
import { Glyph } from './ui'

/** A link to another site, opened in a new tab. */
export function ExternalLink({
  href,
  children,
  className,
}: {
  href: string
  children: React.ReactNode
  className?: string
}) {
  const { t } = useI18n()

  return (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      className={cn(
        'inline-flex items-center gap-1 underline decoration-rule-bright underline-offset-2',
        'transition-colors duration-150 hover:text-bone hover:decoration-bone-faint',
        className,
      )}
    >
      {children}
      <Glyph name="external" className="size-3 shrink-0 opacity-70" />
      <span className="sr-only"> {t.elsewhere.newTab}</span>
    </a>
  )
}

/**
 * The same work on the sites it came from, as a row of pills.
 *
 * Slate, the colour of what a machine supplied: these are the providers'
 * pages, not anything a person here decided.
 */
export function Elsewhere({
  links,
  className,
}: {
  links: Destination[]
  className?: string
}) {
  const { t } = useI18n()

  if (!links.length) return null

  return (
    <ul aria-label={t.elsewhere.label} className={cn('flex flex-wrap gap-2', className)}>
      {links.map((link) => (
        <li key={link.source + link.href}>
          <a
            href={link.href}
            target="_blank"
            rel="noreferrer"
            className={cn(
              'inline-flex min-h-11 items-center gap-1.5 rounded-full border border-slate-deep px-3.5',
              'text-[0.8125rem] font-medium text-slate',
              'transition-colors duration-150 hover:border-slate hover:bg-slate/10 hover:text-bone',
            )}
          >
            {providerName(link.source)}
            <Glyph name="external" className="size-3" />
            <span className="sr-only"> {t.elsewhere.newTab}</span>
          </a>
        </li>
      ))}
    </ul>
  )
}

/**
 * Where a page sits, from the catalogue down.
 *
 * The last step is where the reader already is: said, not linked, and marked
 * as the current page for anyone who cannot see which one is plain text.
 *
 * One line at every width. Wrapped on a phone, each step's touch height made
 * the trail two tall rows with a gap between them; now a long title is cut
 * short instead, and the steps either side of it keep their full names.
 */
export function Trail({ steps }: { steps: { label: string; to?: string }[] }) {
  const { t } = useI18n()

  // The one step that gives way when the line is too short: the longest link,
  // which is the work's title. Letting every step shrink cut "Series" and
  // "Season 1" short on a phone as well, for a few pixels each.
  const yielding = steps.reduce(
    (best, step, index) =>
      step.to && (best < 0 || step.label.length > (steps[best]?.label.length ?? 0)) ? index : best,
    -1,
  )

  return (
    <nav aria-label={t.elsewhere.trail}>
      <ol className="flex min-w-0 items-center gap-x-1.5 overflow-hidden font-mono text-[0.6875rem] tracking-[0.12em] whitespace-nowrap uppercase">
        {steps.map((step, index) => (
          <Fragment key={`${step.label}-${index}`}>
            {index > 0 ? (
              <li aria-hidden className="shrink-0 text-bone-faint">
                <Glyph name="chevronRight" className="size-3" />
              </li>
            ) : null}
            <li className={index === yielding ? 'min-w-0 max-w-[18rem]' : 'shrink-0'}>
              {step.to ? (
                <Link
                  to={step.to}
                  title={step.label}
                  className="block min-h-11 truncate py-3.5 leading-4 text-bone-faint transition-colors duration-150 hover:text-bone"
                >
                  {step.label}
                </Link>
              ) : (
                <span aria-current="page" className="block min-h-11 py-3.5 leading-4 text-bone-dim">
                  {step.label}
                </span>
              )}
            </li>
          </Fragment>
        ))}
      </ol>
    </nav>
  )
}
