/**
 * The catalogue in numbers: what it holds, when it was made, what it is
 * about, where it came from and how it is rated. Bars drawn in the page's
 * own ink, no chart library: a count, a name, a length.
 */

import { useQuery } from '@tanstack/react-query'

import { EmptyState, Glyph, Label, SectionTitle, Skeleton } from '../components/ui'
import { api } from '../lib/api'
import * as fmt from '../lib/format'
import { useTitle } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { languageName, statusLabel } from '../lib/labels'
import type { Count, Figures as FiguresData } from '../lib/types'

export function Figures() {
  const { t, locale } = useI18n()
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
        <Bars title={t.figures.decades} counts={data.decades} />
        <Bars title={t.figures.genres} counts={data.genres} />
        <Bars title={t.figures.networks} counts={data.networks} />
        <Bars title={t.figures.languages} counts={data.languages.map((c) => ({ ...c, name: languageName(c.name, locale) ?? c.name }))} />
        <Bars title={t.figures.scores} counts={data.scores.map((c) => ({ ...c, name: t.figures.score(c.name) }))} />
        <Bars title={t.figures.statuses} counts={data.statuses.map((c) => ({ ...c, name: statusLabel(c.name, t) ?? c.name }))} />
      </div>
    </div>
  )
}

/** A list of counts as bars, the longest bar the largest count. */
function Bars({ title, counts }: { title: string; counts: Count[] }) {
  const { t, locale } = useI18n()
  const most = Math.max(...counts.map((c) => c.count), 1)
  return (
    <section>
      <SectionTitle>{title}</SectionTitle>
      {counts.length === 0 ? (
        <p className="flex items-center gap-2 text-sm text-bone-faint">
          <Glyph name="reel" className="size-4" />
          {t.figures.none}
        </p>
      ) : (
        <ol className="space-y-2">
          {counts.map((count) => (
            <li key={count.name} className="grid grid-cols-[minmax(0,8rem)_1fr_auto] items-center gap-3 text-sm">
              <span className="truncate text-bone" title={count.name}>
                {count.name}
              </span>
              <span className="h-2 overflow-hidden rounded-full bg-ink-high" aria-hidden>
                <span
                  className="block h-full rounded-full bg-vermillion/80"
                  style={{ width: `${Math.max(2, Math.round((count.count / most) * 100))}%` }}
                />
              </span>
              <span className="font-mono text-xs text-bone-faint tabular-nums">{fmt.count(count.count, locale)}</span>
            </li>
          ))}
        </ol>
      )}
    </section>
  )
}
