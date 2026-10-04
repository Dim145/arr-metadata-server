/**
 * The catalogue in numbers: what it holds, when it was made, what it is
 * about, where it came from and how it is rated. Bars drawn in the page's
 * own ink, no chart library: a count, a name, a length — and each a way into
 * the catalogue narrowed to what it counts.
 */

import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router'

import { EmptyState, Glyph, Label, SectionTitle, Skeleton } from '../components/ui'
import { api } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useTitle } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { genreLabel, languageGroups, statusLabel } from '../lib/labels'
import type { Figures as FiguresData } from '../lib/types'

/** A bar: its name as shown, its count, and where in the catalogue it leads. */
interface Bar {
  key: string
  name: string
  count: number
  to?: string
}

/** The browse page, narrowed. */
function browse(params: Record<string, string>): string {
  return `/browse?${new URLSearchParams(params).toString()}`
}

export function Figures() {
  const { t, lang, locale } = useI18n()
  useTitle(t.figures.label)

  const figures = useQuery({
    queryKey: ['figures'],
    queryFn: () => api.get<FiguresData>('/figures'),
    staleTime: 5 * 60_000,
  })

  if (figures.isPending) {
    return (
      <div className="pt-10 pb-12">
        <Skeleton className="h-4 w-24" />
        <Skeleton className="mt-4 h-10 w-2/3" />
        <div className="mt-10 grid gap-px sm:grid-cols-2 lg:grid-cols-4">
          {Array.from({ length: 4 }, (_, index) => (
            <Skeleton key={index} className="h-24 w-full" />
          ))}
        </div>
      </div>
    )
  }
  if (figures.isError) {
    return (
      <div className="pt-10 pb-12">
        <EmptyState title={t.figures.loadFailed} />
      </div>
    )
  }

  const data = figures.data
  const headline: [string, number][] = [
    [t.figures.works, data.total],
    [t.nav.series, data.series],
    [t.nav.films, data.movies],
    [t.figures.episodes, data.episodes],
  ]

  // Each count as the catalogue filters it: a decade is a span of years, a
  // genre is the genre as the server files it, said here in the reader's
  // language. A score bucket is not a filter the catalogue has — "8 to 9" is
  // not "8 or more" — so those bars stay bars.
  const decades: Bar[] = data.decades.map((c) => {
    const year = Number.parseInt(c.name, 10)
    return {
      key: c.name,
      name: c.name,
      count: c.count,
      to: Number.isFinite(year) ? browse({ yearFrom: String(year), yearTo: String(year + 9) }) : undefined,
    }
  })
  const genres: Bar[] = data.genres.map((c) => ({
    key: c.name,
    name: genreLabel(c.name, lang),
    count: c.count,
    to: browse({ genre: c.name }),
  }))
  const networks: Bar[] = data.networks.map((c) => ({ key: c.name, name: c.name, count: c.count, to: browse({ network: c.name }) }))
  // One tongue spelt two ways by two providers is one bar, asked for by both.
  const languages: Bar[] = languageGroups(data.languages, (c) => c.name, (c) => c.count, locale)
    .map((group) => ({
      key: group.codes.join(','),
      name: group.label,
      count: group.count,
      to: browse({ language: group.codes.join(',') }),
    }))
    .sort((a, b) => b.count - a.count)
  const scores: Bar[] = data.scores.map((c) => ({ key: c.name, name: t.figures.score(c.name), count: c.count }))
  const statuses: Bar[] = data.statuses.map((c) => ({
    key: c.name,
    name: statusLabel(c.name, t) ?? c.name,
    count: c.count,
    to: browse({ status: c.name }),
  }))

  return (
    <div className="pt-10 pb-12">
      <header className="rise mb-8">
        <Label>{t.figures.label}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{t.figures.title}</h1>
        <p className="mt-3 max-w-prose text-sm leading-relaxed text-bone-dim">{t.figures.lead}</p>
      </header>

      <dl className="rise grid gap-px overflow-hidden rounded-panel border border-rule bg-rule sm:grid-cols-2 lg:grid-cols-4">
        {headline.map(([label, value]) => (
          <div key={label} className="bg-ink-raised px-5 py-4">
            <dt className="label">{label}</dt>
            <dd className="mt-1 font-display text-3xl text-bone tabular-nums">{fmt.count(value, locale)}</dd>
          </div>
        ))}
      </dl>
      <p className="mt-3 font-mono text-xs text-bone-faint tabular-nums">{t.figures.addedRecently(data.addedRecently)}</p>

      <div className="mt-12 grid gap-x-12 gap-y-12 lg:grid-cols-2">
        <Bars title={t.figures.decades} bars={decades} />
        <Bars title={t.figures.genres} bars={genres} />
        <Bars title={t.figures.networks} bars={networks} />
        <Bars title={t.figures.languages} bars={languages} />
        <Bars title={t.figures.scores} bars={scores} />
        <Bars title={t.figures.statuses} bars={statuses} />
      </div>
    </div>
  )
}

/**
 * A list of counts as bars, the longest bar the largest count. A bar with
 * somewhere to lead is a link, the whole row of it.
 */
function Bars({ title, bars }: { title: string; bars: Bar[] }) {
  const { t, locale } = useI18n()
  const most = Math.max(...bars.map((bar) => bar.count), 1)
  const row = 'grid grid-cols-[minmax(0,8rem)_1fr_auto] items-center gap-3 rounded-card px-2 py-1.5 text-sm'

  return (
    <section>
      <SectionTitle>{title}</SectionTitle>
      {bars.length === 0 ? (
        <p className="flex items-center gap-2 text-sm text-bone-faint">
          <Glyph name="reel" className="size-4" />
          {t.figures.none}
        </p>
      ) : (
        <ol className="-mx-2 space-y-0.5">
          {bars.map((bar) => {
            const inner = (
              <>
                <span className="truncate text-bone transition-colors duration-150 group-hover:text-vermillion" title={bar.name}>
                  {bar.name}
                </span>
                <span className="h-2 overflow-hidden rounded-full bg-ink-high" aria-hidden>
                  <span
                    className="block h-full rounded-full bg-vermillion/80 transition-colors duration-150 group-hover:bg-vermillion"
                    style={{ width: `${Math.max(2, Math.round((bar.count / most) * 100))}%` }}
                  />
                </span>
                <span className="font-mono text-xs text-bone-faint tabular-nums">{fmt.count(bar.count, locale)}</span>
              </>
            )
            return (
              <li key={bar.key}>
                {bar.to ? (
                  <Link
                    to={bar.to}
                    title={t.figures.open(bar.name)}
                    className={cn(row, 'group transition-colors duration-150 hover:bg-ink-high')}
                  >
                    {inner}
                  </Link>
                ) : (
                  <span className={row}>{inner}</span>
                )}
              </li>
            )
          })}
        </ol>
      )}
    </section>
  )
}
