/**
 * The catalogue, filtered.
 *
 * Every filter lives in the URL, so a view is a link: a year, a kind and a
 * search term can be sent to somebody or kept in a bookmark, and the back
 * button returns to the list you were looking at rather than to its defaults.
 */

import { useQuery } from '@tanstack/react-query'
import { useSearchParams } from 'react-router'

import { PosterCard, PosterGrid } from '../components/media'
import { Button, EmptyState, Glyph, Label, Select, Skeleton } from '../components/ui'
import { api, query } from '../lib/api'
import * as fmt from '../lib/format'
import { useI18n } from '../lib/i18n'
import type { ItemPage } from '../lib/types'

const PAGE = 36

export function Browse() {
  const { t, lang, locale } = useI18n()
  const [params, setParams] = useSearchParams()

  const kind = params.get('kind') ?? ''
  const term = params.get('q') ?? ''
  const year = params.get('year') ?? ''
  const manualOnly = params.get('manual') === '1'
  const shown = Number(params.get('shown') ?? PAGE)

  const results = useQuery({
    queryKey: ['browse', { kind, term, year, manualOnly, shown, lang }],
    queryFn: () =>
      api.get<ItemPage>(
        `/items${query({
          kind,
          term,
          year,
          manualOnly,
          limit: shown,
          language: lang,
        })}`,
      ),
  })

  /** Change one filter, keep the rest, and start the page count over. */
  function update(name: string, value: string) {
    const next = new URLSearchParams(params)

    // Narrowing the list while holding a deep page would show a page that no
    // longer exists. Asking for more of the same list must not reset itself.
    if (name !== 'shown') {
      next.delete('shown')
    }

    if (value) {
      next.set(name, value)
    } else {
      next.delete(name)
    }

    setParams(next, { replace: true })
  }

  const filtered = Boolean(kind || term || year || manualOnly)
  const items = results.data?.items ?? []
  const total = results.data?.total ?? 0

  return (
    <div className="pt-10">
      <header className="mb-8">
        <Label>{t.browse.title}</Label>
        <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">
          {term ? `« ${term} »` : kind === 'series' ? t.nav.series : kind === 'movie' ? t.nav.films : t.home.title}
        </h1>
        {results.isSuccess ? (
          <p className="mt-2 font-mono text-sm text-bone-faint tabular-nums">
            {t.browse.results(total)}
          </p>
        ) : null}
      </header>

      <div className="mb-8 flex flex-wrap items-end gap-3 border-y border-rule py-4">
        <div className="min-w-32">
          <label htmlFor="filter-kind" className="label mb-1.5 block">
            {t.browse.kind}
          </label>
          <Select id="filter-kind" value={kind} onChange={(e) => update('kind', e.target.value)}>
            <option value="">{t.browse.all}</option>
            <option value="series">{t.browse.series}</option>
            <option value="movie">{t.browse.films}</option>
          </Select>
        </div>

        <div className="min-w-28">
          <label htmlFor="filter-year" className="label mb-1.5 block">
            {t.browse.year}
          </label>
          <Select id="filter-year" value={year} onChange={(e) => update('year', e.target.value)}>
            <option value="">{t.browse.anyYear}</option>
            {years().map((value) => (
              <option key={value} value={value}>
                {value}
              </option>
            ))}
          </Select>
        </div>

        <Button
          variant={manualOnly ? 'primary' : 'ghost'}
          aria-pressed={manualOnly}
          onClick={() => update('manual', manualOnly ? '' : '1')}
        >
          <Glyph name="lock" className="size-4" />
          {t.browse.manualOnly}
        </Button>

        {filtered ? (
          <Button variant="quiet" onClick={() => setParams(new URLSearchParams(), { replace: true })}>
            <Glyph name="close" className="size-3.5" />
            {t.browse.clear}
          </Button>
        ) : null}
      </div>

      {results.isPending ? (
        <PosterGrid>
          {Array.from({ length: 12 }, (_, index) => (
            <div key={index} className="space-y-2.5">
              <Skeleton className="aspect-2/3 w-full" />
              <Skeleton className="h-4 w-3/4" />
              <Skeleton className="h-3 w-10" />
            </div>
          ))}
        </PosterGrid>
      ) : items.length === 0 ? (
        <EmptyState
          title={t.browse.noResults}
          hint={t.browse.noResultsHint}
          action={
            filtered ? (
              <Button
                className="mt-2"
                onClick={() => setParams(new URLSearchParams(), { replace: true })}
              >
                {t.browse.clear}
              </Button>
            ) : undefined
          }
        />
      ) : (
        <>
          <PosterGrid>
            {items.map((item) => (
              <PosterCard key={item.id} item={item} to={`/work/${item.id}`} />
            ))}
          </PosterGrid>

          {items.length < total ? (
            <div className="mt-12 flex flex-col items-center gap-2">
              <Button onClick={() => update('shown', String(shown + PAGE))}>
                {t.browse.loadMore}
              </Button>
              <span className="font-mono text-xs text-bone-faint tabular-nums">
                {fmt.count(items.length, locale)} {t.common.of} {fmt.count(total, locale)}
              </span>
            </div>
          ) : null}
        </>
      )}
    </div>
  )
}

/** Something to pick from, newest first, without asking the server. */
function years(): number[] {
  const now = new Date().getFullYear()
  return Array.from({ length: 80 }, (_, index) => now - index)
}
