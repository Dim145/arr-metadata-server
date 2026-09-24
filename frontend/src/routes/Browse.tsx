/**
 * The catalogue, narrowed and ordered.
 *
 * Every filter lives in the URL, so a view is a link: a genre, a decade and a
 * sort can be sent to somebody or kept in a bookmark, and the back button
 * returns to the list you were looking at rather than to its defaults.
 *
 * The filters are the catalogue's own. What a genre, a network or a language
 * list offers comes from what is stored, counted under the filters already on,
 * so a count is how many works choosing it would leave.
 * On a wide screen they stand in a rail beside the results; on a phone they
 * are one tap away in a sheet, and what is applied is always shown above the
 * results as chips that take it off again.
 */

import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import { useSearchParams } from 'react-router'

import { PosterCard, PosterGrid } from '../components/media'
import { Button, Dialog, EmptyState, Glyph, Input, Label, Select, Skeleton } from '../components/ui'
import { api, query } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useTitle } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { genreLabel, languageName, listedGenres, statusLabel } from '../lib/labels'
import type { Facet, Facets, ItemPage } from '../lib/types'

const PAGE = 36

// The most one page will ask for, whatever the URL says. The server clamps at
// 500; stopping short of it keeps a shared link from being a way to make this
// server sort its whole catalogue for an anonymous visitor.
const MAX_SHOWN = 500

/** The orders on offer: each is a sort and the direction that reads best. */
const ORDERS = {
  popular: { sort: 'popularity', order: '' },
  rated: { sort: 'rating', order: '' },
  newest: { sort: 'release', order: '' },
  oldest: { sort: 'release', order: 'asc' },
  title: { sort: 'title', order: '' },
  titleDesc: { sort: 'title', order: 'desc' },
  added: { sort: 'added', order: '' },
} as const

type OrderKey = keyof typeof ORDERS

const SCORES = [5, 6, 7, 8, 9]

/** A number from a URL, made into one that means something. */
function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min
  }
  return Math.min(Math.max(Math.trunc(value), min), max)
}

/** The filters the URL carries, read once. */
function read(params: URLSearchParams) {
  const list = (name: string) =>
    (params.get(name) ?? '')
      .split(',')
      .map((v) => v.trim())
      .filter(Boolean)

  const order = (Object.keys(ORDERS) as OrderKey[]).find((key) => key === params.get('order')) ?? 'popular'
  // `?year=2008`, as links made before the range existed still say.
  const year = params.get('year') ?? ''

  return {
    kind: params.get('kind') ?? '',
    term: params.get('q') ?? '',
    // As the catalogue lists them: a series' page links "Action & Adventure",
    // which is its "Action" and "Adventure", the chips on offer.
    genres: listedGenres(list('genre')),
    keyword: params.get('keyword') ?? '',
    yearFrom: params.get('yearFrom') ?? year,
    yearTo: params.get('yearTo') ?? year,
    minRating: params.get('minRating') ?? '',
    status: params.get('status') ?? '',
    language: params.get('language') ?? '',
    network: params.get('network') ?? '',
    manualOnly: params.get('manual') === '1',
    order,
    // Straight off the URL, so it can be anything: `?shown=abc` sent
    // `limit=NaN` and `?shown=99999999` asked a public surface for the whole
    // catalogue in one page. The server clamps too; this is so the link behaves.
    shown: clamp(Number(params.get('shown') ?? PAGE), PAGE, MAX_SHOWN),
  }
}

type Filters = ReturnType<typeof read>

export function Browse() {
  const { t, lang, locale } = useI18n()
  const [params, setParams] = useSearchParams()
  const [sheet, setSheet] = useState(false)

  const f = read(params)
  const { sort, order } = ORDERS[f.order]

  const narrowing = {
    kind: f.kind,
    term: f.term,
    genre: f.genres.join(','),
    keyword: f.keyword,
    yearFrom: f.yearFrom,
    yearTo: f.yearTo,
    minRating: f.minRating,
    status: f.status,
    originalLanguage: f.language,
    network: f.network,
    manualOnly: f.manualOnly,
  }

  useTitle(f.term ? t.browse.quoted(f.term) : f.kind === 'series' ? t.nav.series : f.kind === 'movie' ? t.nav.films : t.nav.browse)

  // Counted under the other filters, so asked again as they change; the last
  // counts stay up meanwhile rather than the rail emptying on every tap.
  const facets = useQuery({
    queryKey: ['facets', narrowing],
    queryFn: () => api.get<Facets>(`/facets${query(narrowing)}`),
    staleTime: 60_000,
    placeholderData: (previous) => previous,
  })

  const results = useQuery({
    queryKey: ['browse', f, lang],
    queryFn: () =>
      api.get<ItemPage>(`/items${query({ ...narrowing, sort, order, limit: f.shown, language: lang })}`),
    placeholderData: (previous) => previous,
  })

  /** Change some filters, keep the rest, and start the page count over. */
  function change(changes: Record<string, string>) {
    // From the address as it is now, not as this render saw it: a year field
    // left by tapping a genre commits the year and then the genre in one go,
    // and the second built on the first's stale address and dropped the year.
    const next = new URLSearchParams(window.location.search)

    // Narrowing the list while holding a deep page would show a page that no
    // longer exists. Asking for more of the same list must not reset itself.
    if (!('shown' in changes)) {
      next.delete('shown')
    }
    // The old single-year form gives way to the range as soon as either end moves.
    if ('yearFrom' in changes || 'yearTo' in changes) {
      if (next.has('year')) {
        next.set('yearFrom', next.get('yearFrom') ?? next.get('year') ?? '')
        next.set('yearTo', next.get('yearTo') ?? next.get('year') ?? '')
        next.delete('year')
      }
    }

    for (const [name, value] of Object.entries(changes)) {
      if (value) {
        next.set(name, value)
      } else {
        next.delete(name)
      }
    }

    setParams(next, { replace: true })
  }

  const update = (name: string, value: string) => change({ [name]: value })

  const clearAll = () => {
    // The order is how to read the list, not what is in it: it stays.
    const order = new URLSearchParams(window.location.search).get('order')
    setParams(order ? { order } : {}, { replace: true })
  }

  const applied = activeFilters(f, t, lang, locale)
  const items = results.data?.items ?? []
  const total = results.data?.total ?? 0

  const heading = f.term
    ? t.browse.quoted(f.term)
    : f.kind === 'series'
      ? t.nav.series
      : f.kind === 'movie'
        ? t.nav.films
        : t.home.title

  return (
    <div className="pt-10 pb-12">
      <header className="mb-6 flex flex-wrap items-end justify-between gap-4">
        <div>
          <Label>{t.browse.title}</Label>
          <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{heading}</h1>
          {results.isSuccess ? (
            <p className="mt-2 font-mono text-sm text-bone-faint tabular-nums" aria-live="polite">
              {t.browse.results(total)}
            </p>
          ) : null}
        </div>

        <div className="flex items-end gap-2">
          <Button className="lg:hidden" onClick={() => setSheet(true)}>
            <Glyph name="sliders" className="size-4" />
            {t.browse.filters}
            {applied.length ? (
              <span className="rounded-full bg-vermillion px-1.5 font-mono text-[0.6875rem] text-ink tabular-nums">
                {applied.length}
              </span>
            ) : null}
          </Button>

          <div>
            <label htmlFor="browse-order" className="label mb-1.5 block">
              {t.browse.sort}
            </label>
            <Select id="browse-order" value={f.order} onChange={(e) => update('order', e.target.value === 'popular' ? '' : e.target.value)}>
              {(Object.keys(ORDERS) as OrderKey[]).map((key) => (
                <option key={key} value={key}>
                  {t.browse.orders[key]}
                </option>
              ))}
            </Select>
          </div>
        </div>
      </header>

      <div className="lg:grid lg:grid-cols-[15.5rem_minmax(0,1fr)] lg:gap-10">
        <aside aria-label={t.browse.filters} className="hidden lg:block">
          <div className="sticky top-24 max-h-[calc(100dvh-7rem)] overflow-y-auto overscroll-contain pr-2 pb-6">
            <FilterPanel filters={f} facets={facets.data} change={change} idPrefix="rail" />
          </div>
        </aside>

        <div className="min-w-0">
          {applied.length ? (
            <div className="mb-6 flex flex-wrap items-center gap-2">
              {applied.map((filter) => (
                <button
                  key={filter.key}
                  type="button"
                  onClick={() => change(filter.clear)}
                  className={cn(
                    'hit inline-flex min-h-9 cursor-pointer items-center gap-1.5 rounded-full border border-rule-bright px-3',
                    'text-[0.8125rem] text-bone transition-colors duration-150 hover:border-vermillion-deep hover:text-vermillion',
                  )}
                  aria-label={t.browse.remove(filter.label)}
                >
                  {filter.label}
                  <Glyph name="close" className="size-3" />
                </button>
              ))}
              <Button size="sm" variant="quiet" onClick={clearAll}>
                {t.browse.clear}
              </Button>
            </div>
          ) : null}

          {results.isError ? (
            // A failure is not an empty list: "no work matches" would send the
            // reader off removing filters over what may be a server restart.
            <EmptyState
              title={t.common.error}
              hint={t.browse.loadFailed}
              action={
                <div className="mt-2 flex flex-wrap justify-center gap-2">
                  <Button size="sm" onClick={() => void results.refetch()}>
                    <Glyph name="refresh" className="size-3.5" />
                    {t.common.retry}
                  </Button>
                  {applied.length ? (
                    <Button size="sm" variant="quiet" onClick={clearAll}>
                      {t.browse.clear}
                    </Button>
                  ) : null}
                </div>
              }
            />
          ) : results.isPending ? (
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
              title={applied.length ? t.browse.noResults : t.home.empty}
              hint={
                f.term
                  ? t.browse.noResultsTermHint
                  : applied.length
                    ? t.browse.noResultsFilterHint
                    : t.home.emptyHint
              }
              action={
                applied.length ? (
                  <div className="mt-2 flex flex-wrap justify-center gap-2">
                    {/* With one filter, taking it off and clearing are the
                        same thing: one button says it. */}
                    {(applied.length > 1 ? applied.slice(-2) : []).map((filter) => (
                      <Button key={filter.key} size="sm" onClick={() => change(filter.clear)}>
                        <Glyph name="close" className="size-3.5" />
                        {t.browse.without(filter.label)}
                      </Button>
                    ))}
                    <Button size="sm" variant="quiet" onClick={clearAll}>
                      {t.browse.clear}
                    </Button>
                  </div>
                ) : undefined
              }
            />
          ) : (
            <div className={cn('transition-opacity duration-150', results.isPlaceholderData && 'opacity-60')}>
              <PosterGrid className="lg:grid-cols-4 xl:grid-cols-5">
                {items.map((item) => (
                  <PosterCard key={item.id} item={item} to={`/work/${item.id}`} kind={!f.kind} />
                ))}
              </PosterGrid>

              {items.length < total ? (
                <div className="mt-12 flex flex-col items-center gap-2">
                  {f.shown < MAX_SHOWN ? (
                    <>
                      <Button onClick={() => update('shown', String(f.shown + PAGE))} disabled={results.isFetching}>
                        {t.browse.loadMore}
                      </Button>
                      <span className="font-mono text-xs text-bone-faint tabular-nums">
                        {fmt.count(items.length, locale)} {t.common.of} {fmt.count(total, locale)}
                      </span>
                    </>
                  ) : (
                    // As far as one page goes; a button that asked for more
                    // and got the same page back would say nothing at all.
                    <p className="max-w-md text-center text-sm text-bone-faint">
                      {t.browse.capped(fmt.count(items.length, locale), fmt.count(total, locale))}
                    </p>
                  )}
                </div>
              ) : null}
            </div>
          )}
        </div>
      </div>

      <Dialog
        open={sheet}
        title={t.browse.filters}
        onClose={() => setSheet(false)}
        footer={
          <>
            {applied.length ? (
              <Button variant="quiet" onClick={clearAll}>
                {t.browse.clear}
              </Button>
            ) : null}
            <Button variant="primary" onClick={() => setSheet(false)}>
              {results.isSuccess ? t.browse.show(total) : t.nav.close}
            </Button>
          </>
        }
      >
        <FilterPanel filters={f} facets={facets.data} change={change} idPrefix="sheet" />
      </Dialog>
    </div>
  )
}

/** One applied filter, as a chip that takes it off again. */
interface Applied {
  key: string
  label: string
  /** What the parameters hold once this is removed. */
  clear: Record<string, string>
}

function activeFilters(
  f: Filters,
  t: ReturnType<typeof useI18n>['t'],
  lang: ReturnType<typeof useI18n>['lang'],
  locale: string,
): Applied[] {
  const out: Applied[] = []

  const one = (key: string, label: string, param: string): Applied => ({ key, label, clear: { [param]: '' } })

  // The search narrows the list like any filter, and comes off the same way.
  if (f.term) out.push(one('q', t.browse.quoted(f.term), 'q'))
  if (f.kind) out.push(one('kind', f.kind === 'series' ? t.browse.series : t.browse.films, 'kind'))
  for (const genre of f.genres) {
    out.push({
      key: `genre:${genre}`,
      label: genreLabel(genre, lang),
      clear: { genre: f.genres.filter((g) => g !== genre).join(',') },
    })
  }
  if (f.keyword) out.push(one('keyword', `# ${f.keyword}`, 'keyword'))
  if (f.yearFrom && f.yearFrom === f.yearTo) {
    out.push({ key: 'year', label: f.yearFrom, clear: { yearFrom: '', yearTo: '', year: '' } })
  } else {
    if (f.yearFrom) out.push(one('yearFrom', t.browse.since(f.yearFrom), 'yearFrom'))
    if (f.yearTo) out.push(one('yearTo', t.browse.until(f.yearTo), 'yearTo'))
  }
  if (f.minRating) {
    out.push(one('minRating', t.browse.atLeast(fmt.score(Number(f.minRating), locale) ?? f.minRating), 'minRating'))
  }
  if (f.status) out.push(one('status', statusLabel(f.status, t) ?? f.status, 'status'))
  if (f.language) out.push(one('language', languageName(f.language, locale) ?? f.language, 'language'))
  if (f.network) out.push(one('network', f.network, 'network'))
  if (f.manualOnly) out.push(one('manual', t.browse.manualOnly, 'manual'))

  return out
}

/**
 * Every filter, as one panel. The same component stands in the rail and in
 * the phone's sheet, so the two can never offer different things.
 */
function FilterPanel({
  filters: f,
  facets,
  change,
  idPrefix,
}: {
  filters: Filters
  facets?: Facets
  change: (changes: Record<string, string>) => void
  idPrefix: string
}) {
  const { t, lang, locale } = useI18n()
  const id = (name: string) => `${idPrefix}-${name}`
  const update = (name: string, value: string) => change({ [name]: value })

  // A value the URL holds that the counts do not list — a network outside the
  // forty most common, followed from a work's page — is still offered, or the
  // select would read "any" over a list that is filtered.
  const withCurrent = (list: Facet[], current: string): { value: string; count?: number }[] =>
    current && !list.some((facet) => facet.value === current) ? [...list, { value: current }] : list

  const toggleGenre = (genre: string) => {
    const next = f.genres.includes(genre) ? f.genres.filter((g) => g !== genre) : [...f.genres, genre]
    update('genre', next.join(','))
  }

  return (
    <div className="space-y-7">
      <fieldset>
        <legend className="label mb-2">{t.browse.kind}</legend>
        <div className="grid grid-cols-3 gap-1 rounded-full border border-rule p-1">
          {(['', 'series', 'movie'] as const).map((value) => (
            <label
              key={value || 'all'}
              className={cn(
                'hit relative grid min-h-9 cursor-pointer place-items-center rounded-full px-2 text-[0.8125rem]',
                'transition-colors duration-150 has-focus-visible:outline-2 has-focus-visible:outline-vermillion',
                f.kind === value ? 'bg-ink-top text-bone' : 'text-bone-dim hover:text-bone',
              )}
            >
              <input
                type="radio"
                name={id('kind')}
                value={value}
                checked={f.kind === value}
                // Statuses and networks belong to one kind — "continuing" is a
                // series', a studio a film's — and kept across a switch they
                // emptied the list while their selects read "any".
                onChange={() => change({ kind: value, status: '', network: '' })}
                className="sr-only"
              />
              {value === '' ? t.browse.all : value === 'series' ? t.browse.series : t.browse.films}
            </label>
          ))}
        </div>
      </fieldset>

      {facets?.genres.length ? (
        <fieldset>
          <legend className="label mb-2">{t.work.genres}</legend>
          <div className="flex flex-wrap gap-x-1.5 gap-y-3">
            {facets.genres.map((genre) => {
              const on = f.genres.includes(genre.value)
              return (
                <button
                  key={genre.value}
                  type="button"
                  aria-pressed={on}
                  onClick={() => toggleGenre(genre.value)}
                  className={cn(
                    'hit inline-flex min-h-8 cursor-pointer items-center gap-1.5 rounded-full border px-2.5 text-xs',
                    'transition-colors duration-150',
                    on
                      ? 'border-vermillion bg-vermillion/15 text-bone'
                      : 'border-rule-bright text-bone-dim hover:border-bone-faint hover:text-bone',
                  )}
                >
                  {on ? <Glyph name="check" className="size-3 text-vermillion" /> : null}
                  {genreLabel(genre.value, lang)}
                  <span className="font-mono text-[0.625rem] text-bone-faint tabular-nums">{genre.count}</span>
                </button>
              )
            })}
          </div>
        </fieldset>
      ) : null}

      <fieldset>
        <legend className="label mb-2">{t.browse.years}</legend>
        <YearRange filters={f} facets={facets} update={update} id={id} />
      </fieldset>

      <div>
        <label htmlFor={id('score')} className="label mb-1.5 block">
          {t.browse.score}
        </label>
        <Select id={id('score')} value={f.minRating} onChange={(e) => update('minRating', e.target.value)}>
          <option value="">{t.browse.anyScore}</option>
          {SCORES.map((score) => (
            <option key={score} value={score}>
              {t.browse.atLeast(fmt.score(score, locale) ?? String(score))}
            </option>
          ))}
        </Select>
      </div>

      {facets?.statuses.length || f.status ? (
        <div>
          <label htmlFor={id('status')} className="label mb-1.5 block">
            {t.work.status}
          </label>
          <Select id={id('status')} value={f.status} onChange={(e) => update('status', e.target.value)}>
            <option value="">{t.browse.anyStatus}</option>
            {withCurrent(facets?.statuses ?? [], f.status).map((status) => (
              <option key={status.value} value={status.value}>
                {statusLabel(status.value, t)}
                {status.count !== undefined ? ` (${status.count})` : ''}
              </option>
            ))}
          </Select>
        </div>
      ) : null}

      {facets?.languages.length || f.language ? (
        <div>
          <label htmlFor={id('language')} className="label mb-1.5 block">
            {t.work.originalLanguage}
          </label>
          <Select id={id('language')} value={f.language} onChange={(e) => update('language', e.target.value)}>
            <option value="">{t.browse.anyLanguage}</option>
            {withCurrent(facets?.languages ?? [], f.language).map((language) => (
              <option key={language.value} value={language.value}>
                {languageName(language.value, locale)}
                {language.count !== undefined ? ` (${language.count})` : ''}
              </option>
            ))}
          </Select>
        </div>
      ) : null}

      {facets?.networks.length || f.network ? (
        <div>
          <label htmlFor={id('network')} className="label mb-1.5 block">
            {t.browse.network}
          </label>
          <Select id={id('network')} value={f.network} onChange={(e) => update('network', e.target.value)}>
            <option value="">{t.browse.anyNetwork}</option>
            {withCurrent(facets?.networks ?? [], f.network).map((network) => (
              <option key={network.value} value={network.value}>
                {network.value}
                {network.count !== undefined ? ` (${network.count})` : ''}
              </option>
            ))}
          </Select>
        </div>
      ) : null}

      <Button
        size="sm"
        variant={f.manualOnly ? 'primary' : 'ghost'}
        aria-pressed={f.manualOnly}
        onClick={() => update('manual', f.manualOnly ? '' : '1')}
      >
        <Glyph name="lock" className="size-4" />
        {t.browse.manualOnly}
      </Button>
    </div>
  )
}

/**
 * From one year to another. Applied when the field is left or Enter pressed,
 * not per keystroke: typing 1990 would otherwise list the year 1, 19 and 199
 * on the way there.
 */
function YearRange({
  filters: f,
  facets,
  update,
  id,
}: {
  filters: Filters
  facets?: Facets
  update: (name: string, value: string) => void
  id: (name: string) => string
}) {
  const { t } = useI18n()
  const [from, setFrom] = useState(f.yearFrom)
  const [to, setTo] = useState(f.yearTo)
  const [seen, setSeen] = useState({ from: f.yearFrom, to: f.yearTo })

  // The URL moved under the fields — a chip removed, a link followed.
  if (seen.from !== f.yearFrom || seen.to !== f.yearTo) {
    setSeen({ from: f.yearFrom, to: f.yearTo })
    setFrom(f.yearFrom)
    setTo(f.yearTo)
  }

  const commit = (name: 'yearFrom' | 'yearTo', value: string) => {
    const clean = /^\d{4}$/.test(value.trim()) ? value.trim() : ''
    if (clean !== (name === 'yearFrom' ? f.yearFrom : f.yearTo)) update(name, clean)
  }

  return (
    <div className="grid grid-cols-2 gap-2">
      <div>
        <label htmlFor={id('from')} className="mb-1 block text-xs text-bone-faint">
          {t.browse.from}
        </label>
        <Input
          id={id('from')}
          inputMode="numeric"
          placeholder={facets?.yearMin ? String(facets.yearMin) : undefined}
          value={from}
          onChange={(e) => setFrom(e.target.value)}
          onBlur={() => commit('yearFrom', from)}
          onKeyDown={(e) => e.key === 'Enter' && commit('yearFrom', from)}
          className="tabular-nums"
        />
      </div>
      <div>
        <label htmlFor={id('to')} className="mb-1 block text-xs text-bone-faint">
          {t.browse.to}
        </label>
        <Input
          id={id('to')}
          inputMode="numeric"
          placeholder={facets?.yearMax ? String(facets.yearMax) : undefined}
          value={to}
          onChange={(e) => setTo(e.target.value)}
          onBlur={() => commit('yearTo', to)}
          onKeyDown={(e) => e.key === 'Enter' && commit('yearTo', to)}
          className="tabular-nums"
        />
      </div>
    </div>
  )
}
