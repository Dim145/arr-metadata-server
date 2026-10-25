/**
 * The catalogue, narrowed and ordered.
 *
 * Every filter lives in the URL, so a view is a link: a genre, a decade and a
 * sort can be sent to somebody or kept in a bookmark, and the back button
 * returns to the list you were looking at rather than to its defaults.
 *
 * The filters are the catalogue's own. What a genre, a network or a language
 * list offers comes from what is stored, counted under the filters already on,
 * so a count is how many works choosing it would leave. The rarer ones — a
 * person, a keyword, a running time, a country, a rating, a collection — are
 * a sheet away, "More filters", so the rail stays a rail.
 * On a wide screen they stand in a rail beside the results, what is applied
 * listed at its head as chips that take it off again; on a phone they are one
 * tap away in a sheet, and the chips stand above the results.
 *
 * A search is in the order of how well each work answers it — the server
 * ranks it, misspellings forgiven — unless another order is chosen. The
 * results are laid out as a grid of posters, to know a work by its cover, or
 * as a list, to read and compare: `?view=list` in the address, and what the
 * reader chose last is remembered in this browser for the address that says
 * nothing. The next page is asked for as the end of the list comes near; the
 * button that asks for it stays, for whoever reaches it first. A link at the
 * head of the rail goes past the filters to the results, and one before the
 * first card comes back.
 *
 * A genre's, a network's or a year's own page lists its works with this same
 * list, its subject pinned: always asked for, never offered as a choice nor
 * as a chip to take off, and the page's own heading in place of this one.
 */

import { useQuery } from '@tanstack/react-query'
import { type ReactNode, useEffect, useRef, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router'

import { PosterCard, PosterGrid } from '../components/media'
import { Button, Dialog, EmptyState, Glyph, Input, Label, Select, Skeleton } from '../components/ui'
import { WorkRow } from '../components/WorkRow'
import { api, query } from '../lib/api'
import { cn } from '../lib/cn'
import { drawWork } from '../lib/draw'
import * as fmt from '../lib/format'
import { useCollections, useTitle } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { GenreFilter } from '../components/GenreFilter'
import { genreLabel, languageGroups, languageName, listedGenres, statusLabel } from '../lib/labels'
import type { CatalogueFacets, Facet, ItemPage, Person as PersonData } from '../lib/types'

const PAGE = 36

// The most one page will ask for, whatever the URL says. The server clamps at
// 500; stopping short of it keeps a shared link from being a way to make this
// server sort its whole catalogue for an anonymous visitor.
const MAX_SHOWN = 500

/**
 * How far below the screen the end of the list is when the next page is asked
 * for: a screen or so ahead, so that a reader scrolling at an ordinary pace
 * never meets the end.
 */
const AHEAD = '0px 0px 1200px 0px'

/** The orders on offer: each is a sort and the direction that reads best. */
const ORDERS = {
  // How well each work answers the search: only with something searched.
  relevant: { sort: 'relevance', order: '' },
  popular: { sort: 'popularity', order: '' },
  rated: { sort: 'rating', order: '' },
  newest: { sort: 'release', order: '' },
  oldest: { sort: 'release', order: 'asc' },
  title: { sort: 'title', order: '' },
  titleDesc: { sort: 'title', order: 'desc' },
  added: { sort: 'added', order: '' },
  longest: { sort: 'runtime', order: '' },
  shortest: { sort: 'runtime', order: 'asc' },
  lastAired: { sort: 'lastAired', order: '' },
  episodes: { sort: 'episodes', order: '' },
} as const

type OrderKey = keyof typeof ORDERS

const SCORES = [5, 6, 7, 8, 9]

type View = 'grid' | 'list'

/** Where the layout the reader chose last is kept: in this browser, and nowhere else. */
const VIEW_KEY = 'ams.browse.view'

/** The layout chosen last, or the grid where nothing was or storage cannot be read. */
function rememberedView(): View {
  try {
    return localStorage.getItem(VIEW_KEY) === 'list' ? 'list' : 'grid'
  } catch {
    // Private browsing, or storage disabled: the grid is fine.
    return 'grid'
  }
}

function rememberView(view: View) {
  try {
    localStorage.setItem(VIEW_KEY, view)
  } catch {
    // The choice still applies to this visit.
  }
}

/** A number from a URL, made into one that means something. */
function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min
  }
  return Math.min(Math.max(Math.trunc(value), min), max)
}

/** A count of minutes, or an id, from a URL: digits, or nothing the server would refuse. */
function digits(value: string | null): string {
  return value && /^\d{1,6}$/.test(value.trim()) ? value.trim() : ''
}

/** The filters the URL carries, read once. */
function read(params: URLSearchParams) {
  const list = (name: string) =>
    (params.get(name) ?? '')
      .split(',')
      .map((v) => v.trim())
      .filter(Boolean)

  const term = params.get('q') ?? ''
  // A search is in the order of how well each work answers it unless another
  // was chosen; with nothing searched, that order means nothing.
  const fallback: OrderKey = term ? 'relevant' : 'popular'
  const asked = (Object.keys(ORDERS) as OrderKey[]).find((key) => key === params.get('order'))
  const order = asked && (asked !== 'relevant' || term) ? asked : fallback
  // `?year=2008`, as links made before the range existed still say.
  const year = params.get('year') ?? ''
  const country = (params.get('country') ?? '').trim().toUpperCase()

  return {
    kind: params.get('kind') ?? '',
    term,
    // As the catalogue lists them: a series' page links "Action & Adventure",
    // which is its "Action" and "Adventure", the chips on offer.
    genres: listedGenres(list('genre')),
    // All of them unless asked otherwise; the address says `any` in a word.
    genreMode: params.get('genreMode') === 'any' ? ('any' as const) : ('all' as const),
    keyword: params.get('keyword') ?? '',
    yearFrom: params.get('yearFrom') ?? year,
    yearTo: params.get('yearTo') ?? year,
    minRating: params.get('minRating') ?? '',
    status: params.get('status') ?? '',
    language: params.get('language') ?? '',
    network: params.get('network') ?? '',
    // A TMDB id, or a name as the credits give it.
    person: (params.get('person') ?? '').trim(),
    runtimeMin: digits(params.get('runtimeMin')),
    runtimeMax: digits(params.get('runtimeMax')),
    country: /^[A-Z]{2}$/.test(country) ? country : '',
    certification: (params.get('certification') ?? '').trim(),
    collection: digits(params.get('collection')),
    manualOnly: params.get('manual') === '1',
    order,
    // Straight off the URL, so it can be anything: `?shown=abc` sent
    // `limit=NaN` and `?shown=99999999` asked a public surface for the whole
    // catalogue in one page. The server clamps too; this is so the link behaves.
    shown: clamp(Number(params.get('shown') ?? PAGE), PAGE, MAX_SHOWN),
  }
}

type Filters = ReturnType<typeof read>

/**
 * What a page pins the list to, in the address's words: a genre's slug, a
 * network's name, a range of years. Asked for with whatever else is chosen.
 */
export type Pinned = Partial<Record<'genre' | 'network' | 'yearFrom' | 'yearTo', string>>

/** The filters the "More filters" sheet holds, by their names in the address. */
const MORE = ['person', 'keyword', 'runtimeMin', 'runtimeMax', 'country', 'certification', 'collection'] as const

/** Where the results are, and the filters, for the links that go from one to the other. */
const RESULTS_ID = 'browse-results'
const FILTERS_ID = 'browse-filters'

/** A link that shows itself only when it has the focus: the shells' "skip to content", in place. */
const SKIP_LINK =
  'sr-only focus:not-sr-only focus:fixed focus:top-3 focus:left-3 focus:z-50 focus:rounded-card ' +
  'focus:bg-vermillion focus:px-4 focus:py-2 focus:text-sm focus:font-medium focus:text-ink'

export function Browse({ pinned, lead }: { pinned?: Pinned; lead?: ReactNode } = {}) {
  const { t, lang, locale } = useI18n()
  const [params, setParams] = useSearchParams()
  const [sheet, setSheet] = useState(false)
  const [more, setMore] = useState(false)
  // What the address says wins, so a link is the view it was sent as; a bare
  // address is the one the reader last chose.
  const [remembered, setRemembered] = useState<View>(rememberedView)
  const asked = params.get('view')
  const view: View = asked === 'list' || asked === 'grid' ? asked : remembered

  const f = read(params)
  const { sort, order } = ORDERS[f.order]

  // A pinned genre is asked for with every one chosen beside it, all of
  // them: "any" of them would reach past the page's own.
  const genres = pinned?.genre ? [pinned.genre, ...f.genres.filter((g) => g !== pinned.genre)] : f.genres
  const narrowing = {
    kind: f.kind,
    term: f.term,
    genre: genres.join(','),
    genreMode: f.genreMode === 'any' && !pinned?.genre ? 'any' : '',
    keyword: f.keyword,
    yearFrom: pinned?.yearFrom ?? f.yearFrom,
    yearTo: pinned?.yearTo ?? f.yearTo,
    minRating: f.minRating,
    status: f.status,
    originalLanguage: f.language,
    network: pinned?.network ?? f.network,
    person: f.person,
    runtimeMin: f.runtimeMin,
    runtimeMax: f.runtimeMax,
    country: f.country,
    certification: f.certification,
    collection: f.collection,
    manualOnly: f.manualOnly,
  }

  // A page that leads the list with its own heading names the tab too.
  useTitle(
    lead ? undefined : f.term ? t.browse.quoted(f.term) : f.kind === 'series' ? t.nav.series : f.kind === 'movie' ? t.nav.films : t.nav.browse,
  )

  // Counted under the other filters, so asked again as they change; the last
  // counts stay up meanwhile rather than the rail emptying on every tap.
  const facets = useQuery({
    queryKey: ['facets', narrowing],
    queryFn: () => api.get<CatalogueFacets>(`/facets${query(narrowing)}`),
    staleTime: 60_000,
    placeholderData: (previous) => previous,
  })

  const results = useQuery({
    queryKey: ['browse', f, pinned, lang],
    queryFn: () =>
      api.get<ItemPage>(`/items${query({ ...narrowing, sort, order, limit: f.shown, language: lang })}`),
    placeholderData: (previous) => previous,
  })

  // Who a person given by their TMDB id is, for the chip that names them:
  // the same read their page makes, so a reader who came from it pays nothing.
  const personId = /^\d+$/.test(f.person) ? f.person : ''
  const person = useQuery({
    queryKey: ['person', personId, lang],
    queryFn: () => api.get<PersonData>(`/people/${encodeURIComponent(personId)}${query({ language: lang })}`),
    enabled: Boolean(personId),
    staleTime: 10 * 60_000,
    retry: false,
  })
  const collections = useCollections()

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
    // The order and the view are how to read the list, not what is in it:
    // they stay.
    const current = new URLSearchParams(window.location.search)
    const kept = new URLSearchParams()
    for (const name of ['order', 'view']) {
      const value = current.get(name)
      if (value) kept.set(name, value)
    }
    setParams(kept, { replace: true })
  }

  /** Lay the results out this way: in the address, for a link, and in memory, for the next visit. */
  const choose = (next: View) => {
    rememberView(next)
    setRemembered(next)
    const address = new URLSearchParams(window.location.search)
    // The grid is what a bare address means; only the list is worth saying.
    if (next === 'list') address.set('view', 'list')
    else address.delete('view')
    setParams(address, { replace: true })
  }

  const names = {
    person: person.data?.name,
    collection: collections.data?.collections.find((c) => String(c.tmdbId) === f.collection)?.name ?? undefined,
  }
  const applied = activeFilters(f, t, lang, locale, names)
  const items = results.data?.items ?? []
  const total = results.data?.total ?? 0
  const moreOn = MORE.filter((name) => f[name]).length

  // One of these, at random: the question a catalogue is opened with in the
  // evening. Drawn under the filters as they stand, from the count the list
  // already holds.
  const navigate = useNavigate()
  const [drawing, setDrawing] = useState(false)
  const [drawFailed, setDrawFailed] = useState(false)
  const draw = async () => {
    setDrawing(true)
    setDrawFailed(false)
    try {
      const work = await drawWork({ ...narrowing, sort, order, language: lang }, total)
      // Nothing to draw is said, not left as a button that did nothing.
      if (work) navigate(`/work/${work.id}`)
      else setDrawFailed(true)
    } catch {
      setDrawFailed(true)
    } finally {
      setDrawing(false)
    }
  }

  // The next page, asked for as the end of the list comes near. Not while a
  // page is on its way — the list it would extend is not the one on screen
  // yet — and never past what one page may hold. Where the browser has no
  // observer, the button below is the way on, as it always is too.
  const end = useRef<HTMLDivElement>(null)
  const further = items.length < total && f.shown < MAX_SHOWN
  const busy = results.isFetching
  // The results being filled — the first read, a page more, other filters'
  // list — not a refetch of the one on screen (see the top of ui/feedback.tsx).
  const filling = results.isPending || (results.isFetching && results.isPlaceholderData)
  const shown = f.shown

  // And only on the reader's way down. On a screen tall enough to hold the
  // end of the first page, the observer alone asked for the second as soon
  // as the first was drawn, and the page was not the one its address names.
  // The reader is on their way once the page scrolls, or a wheel, a finger
  // or a key tries to, since this list — these filters, this order — was
  // drawn; until then the button is the way on.
  const listed = JSON.stringify([narrowing, sort, order])
  const [movedOn, setMovedOn] = useState<string | null>(null)
  const moved = movedOn === listed
  useEffect(() => {
    if (moved) return
    const onMove = () => setMovedOn(listed)
    const onKey = (event: KeyboardEvent) => {
      if (['PageDown', 'End', 'ArrowDown', ' '].includes(event.key)) onMove()
    }
    const passive = { passive: true } as const
    window.addEventListener('scroll', onMove, passive)
    window.addEventListener('wheel', onMove, passive)
    window.addEventListener('touchmove', onMove, passive)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('scroll', onMove)
      window.removeEventListener('wheel', onMove)
      window.removeEventListener('touchmove', onMove)
      window.removeEventListener('keydown', onKey)
    }
  }, [moved, listed])

  useEffect(() => {
    const node = end.current
    if (!node || !further || busy || !moved || typeof IntersectionObserver === 'undefined') return
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          observer.disconnect()
          change({ shown: String(shown + PAGE) })
        }
      },
      { rootMargin: AHEAD },
    )
    observer.observe(node)
    return () => observer.disconnect()
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `change` reads the address as it is when it runs: never stale
  }, [further, busy, shown, moved])

  const heading = f.term
    ? t.browse.quoted(f.term)
    : f.kind === 'series'
      ? t.nav.series
      : f.kind === 'movie'
        ? t.nav.films
        : t.home.title

  // The orders this list can be read in: by how well each answers, only for a search.
  const orders = (Object.keys(ORDERS) as OrderKey[]).filter((key) => key !== 'relevant' || f.term)
  const defaultOrder: OrderKey = f.term ? 'relevant' : 'popular'

  return (
    <div className={lead ? 'pb-12' : 'pt-10 pb-12'}>
      <header className="mb-6 flex flex-wrap items-end justify-between gap-4">
        <div>
          {lead ?? (
            <>
              <Label>{t.browse.title}</Label>
              <h1 className="mt-2 font-display text-3xl font-medium text-bone sm:text-4xl">{heading}</h1>
            </>
          )}
          {/* Laid out before the count is known: a line that appeared with
              it pushed the whole page down by its height, a layout shift on
              every arrival. */}
          <p
            className={cn('mt-2 font-mono text-sm text-bone-faint tabular-nums', !results.isSuccess && 'invisible')}
            aria-live="polite"
          >
            {results.isSuccess ? t.browse.results(total) : ' '}
          </p>
        </div>

        {/* On a phone the order takes a line of its own under the two
            buttons: beside them it had room for "Most popu". Its own width,
            all on one line, from a tablet up. */}
        <div className="flex w-full flex-wrap items-end gap-2 sm:w-auto sm:flex-nowrap">
          <Button className="lg:hidden" onClick={() => setSheet(true)}>
            <Glyph name="sliders" className="size-4" />
            {t.browse.filters}
            {applied.length ? (
              <span className="rounded-full bg-vermillion px-1.5 font-mono text-caption text-ink tabular-nums">
                {applied.length}
              </span>
            ) : null}
          </Button>

          {/* Beside the order, away from the filters: the filters say what,
              the order and the draw say in which order it comes. */}
          <Button
            onClick={() => void draw()}
            // Not while the list is the last filters' still, whose count it would draw from.
            disabled={drawing || !total || results.isPlaceholderData}
            aria-label={t.browse.random}
            title={t.browse.randomHint}
          >
            <Glyph name="shuffle" className="size-4" />
            <span className="hidden sm:inline">{drawing ? t.browse.drawing : t.browse.random}</span>
          </Button>
          {drawFailed ? (
            <span role="alert" className="self-center text-sm text-vermillion">
              {t.common.actionFailed}
            </span>
          ) : null}

          {/* How the results are laid out: a grid to know them by their
              covers, a list to read and compare. */}
          <div role="group" aria-label={t.browse.view} className="flex rounded-full border border-rule">
            {(['grid', 'list'] as const).map((value) => (
              <button
                key={value}
                type="button"
                onClick={() => choose(value)}
                aria-pressed={view === value}
                aria-label={value === 'grid' ? t.browse.viewGrid : t.browse.viewList}
                title={value === 'grid' ? t.browse.viewGrid : t.browse.viewList}
                className={cn(
                  'grid size-11 cursor-pointer place-items-center rounded-full transition-colors duration-150',
                  view === value ? 'bg-ink-top text-bone' : 'text-bone-faint hover:text-bone',
                )}
              >
                <Glyph name={value} className="size-4" />
              </button>
            ))}
          </div>

          <div className="min-w-0 basis-full sm:basis-auto">
            <label htmlFor="browse-order" className="label mb-1.5 block">
              {t.browse.sort}
            </label>
            <Select
              id="browse-order"
              value={f.order}
              onChange={(e) => update('order', e.target.value === defaultOrder ? '' : e.target.value)}
            >
              {orders.map((key) => (
                <option key={key} value={key}>
                  {t.browse.orders[key]}
                </option>
              ))}
            </Select>
          </div>
        </div>
      </header>

      <div className="lg:grid lg:grid-cols-[15.5rem_minmax(0,1fr)] lg:gap-10">
        <aside id={FILTERS_ID} tabIndex={-1} aria-label={t.browse.filters} className="hidden outline-none lg:block">
          <div className="sticky top-24 max-h-[calc(100dvh-7rem)] overflow-y-auto overscroll-contain pr-2 pb-6">
            {/* Past every filter at once, for whoever goes by the keyboard. */}
            <a href={`#${RESULTS_ID}`} className={SKIP_LINK}>
              {t.a11y.toResults}
            </a>
            {/* What is applied, at the head of the rail: read before the
                choices, and taken off where it is read. */}
            {applied.length ? (
              <div className="mb-7 border-b border-rule pb-6">
                <div className="mb-2.5 flex items-center justify-between gap-2">
                  <span className="label">{t.browse.applied}</span>
                  <Button size="sm" variant="quiet" className="whitespace-nowrap" onClick={clearAll}>
                    {t.browse.clearAll}
                  </Button>
                </div>
                <AppliedChips applied={applied} change={change} />
              </div>
            ) : null}
            <FilterPanel
              filters={f}
              facets={facets.data}
              change={change}
              pinned={pinned}
              idPrefix="rail"
              onMore={() => setMore(true)}
              moreOn={moreOn}
            />
          </div>
        </aside>

        <div className="min-w-0">
          {applied.length ? (
            <div className="mb-6 flex flex-wrap items-center gap-2 lg:hidden">
              <AppliedChips applied={applied} change={change} />
              <Button size="sm" variant="quiet" onClick={clearAll}>
                {t.browse.clearAll}
              </Button>
            </div>
          ) : null}

          {results.isError ? (
            // A failure is not an empty list: "no work matches" would send the
            // reader off removing filters over what may be a server restart.
            <EmptyState
              kind="error"
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
            view === 'list' ? (
              <ul aria-hidden className="divide-y divide-rule border-y border-rule">
                {Array.from({ length: 6 }, (_, index) => (
                  <li key={index} className="grid grid-cols-[4.5rem_minmax(0,1fr)] gap-x-4 px-3 py-4 sm:grid-cols-[5.5rem_minmax(0,1fr)] sm:px-4">
                    <Skeleton className="aspect-2/3 w-full" />
                    <div className="space-y-2.5">
                      <Skeleton className="h-6 w-2/3" />
                      <Skeleton className="h-3 w-24" />
                      <Skeleton className="h-4 w-full" />
                      <Skeleton className="h-4 w-4/5" />
                    </div>
                  </li>
                ))}
              </ul>
            ) : (
              <PosterGrid busy className="lg:grid-cols-4 xl:grid-cols-5 min-[90rem]:grid-cols-6">
                {Array.from({ length: 12 }, (_, index) => (
                  <div key={index} className="space-y-2.5">
                    <Skeleton className="aspect-2/3 w-full" />
                    <Skeleton className="h-4 w-3/4" />
                    <Skeleton className="h-3 w-10" />
                  </div>
                ))}
              </PosterGrid>
            )
          ) : items.length === 0 ? (
            <EmptyState
              kind={applied.length ? 'filtered' : 'empty'}
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
              {view === 'list' ? (
                <ul
                  id={RESULTS_ID}
                  tabIndex={-1}
                  aria-busy={filling || undefined}
                  className="stagger divide-y divide-rule border-y border-rule outline-none"
                >
                  {items.map((item) => (
                    <WorkRow key={item.id} item={item} to={`/work/${item.id}`} />
                  ))}
                </ul>
              ) : (
                // The cards far below the fold are laid out as they come near,
                // by the grid itself; a link before the first goes back to the
                // filters.
                <PosterGrid
                  id={RESULTS_ID}
                  skip={{ to: FILTERS_ID, label: t.a11y.toFilters }}
                  busy={filling}
                  className="lg:grid-cols-4 xl:grid-cols-5 min-[90rem]:grid-cols-6"
                >
                  {items.map((item) => (
                    <PosterCard key={item.id} item={item} to={`/work/${item.id}`} kind={!f.kind} />
                  ))}
                </PosterGrid>
              )}

              {items.length < total ? (
                <div ref={end} className="mt-12 flex flex-col items-center gap-2">
                  {f.shown < MAX_SHOWN ? (
                    <>
                      <Button onClick={() => update('shown', String(f.shown + PAGE))} disabled={results.isFetching}>
                        {results.isFetching && !results.isPlaceholderData ? t.browse.loadingMore : t.browse.loadMore}
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
        <FilterPanel
          filters={f}
          facets={facets.data}
          change={change}
          pinned={pinned}
          idPrefix="sheet"
          onMore={() => setMore(true)}
          moreOn={moreOn}
        />
      </Dialog>

      <Sheet
        id="more-filters"
        open={more}
        title={t.browse.moreFilters}
        onClose={() => setMore(false)}
        footer={
          <>
            {moreOn ? (
              <Button variant="quiet" onClick={() => change(Object.fromEntries(MORE.map((name) => [name, ''])))}>
                {t.browse.clearMore}
              </Button>
            ) : null}
            <Button variant="primary" onClick={() => setMore(false)}>
              {results.isSuccess ? t.browse.show(total) : t.nav.close}
            </Button>
          </>
        }
      >
        <MoreFilters filters={f} facets={facets.data} change={change} />
      </Sheet>
    </div>
  )
}

/**
 * A sheet over the page: from the foot of the screen on a phone, in the
 * middle of a wider one, entering through `.sheet-in`'s starting style and
 * leaving the same way. What it holds is drawn from its first opening on and
 * kept, so that it leaves whole and the fields in it follow the address
 * while it is closed.
 */
function Sheet({
  id,
  open,
  title,
  onClose,
  children,
  footer,
}: {
  id: string
  open: boolean
  title: string
  onClose: () => void
  children: ReactNode
  footer: ReactNode
}) {
  const { t } = useI18n()
  const ref = useRef<HTMLDialogElement>(null)
  const [drawn, setDrawn] = useState(open)
  if (open && !drawn) setDrawn(true)

  useEffect(() => {
    const node = ref.current
    if (!node) return
    if (open && !node.open) node.showModal()
    if (!open && node.open) node.close()
  }, [open])

  return (
    <dialog
      ref={ref}
      id={id}
      aria-labelledby={`${id}-title`}
      // Escape, which the browser handles, and whatever else closes a dialog.
      onClose={onClose}
      onMouseDown={(event) => {
        // The dark around the sheet: the dialog itself is what a press there lands on.
        if (event.target === event.currentTarget) onClose()
      }}
      className={cn(
        'sheet-in mx-0 mt-auto mb-0 max-h-[85dvh] w-full max-w-none overflow-hidden rounded-t-plate border border-b-0 border-rule',
        'bg-(--surface-overlay) p-0 text-bone backdrop:bg-ink/80',
        'sm:m-auto sm:max-h-[calc(100dvh-2rem)] sm:w-[calc(100vw-2rem)] sm:max-w-lg sm:rounded-panel sm:border-b',
      )}
    >
      {drawn ? (
        <div className="flex max-h-[85dvh] flex-col sm:max-h-[calc(100dvh-2rem)]">
          <header className="flex shrink-0 items-center justify-between border-b border-rule py-1 pr-1 pl-5">
            <h2 id={`${id}-title`} className="font-display text-lg font-medium text-bone">
              {title}
            </h2>
            <button
              type="button"
              onClick={onClose}
              aria-label={t.nav.close}
              className="grid size-11 cursor-pointer place-items-center rounded-card text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
            >
              <Glyph name="close" className="size-5" />
            </button>
          </header>
          <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-5 py-4 text-sm leading-relaxed text-bone-dim">
            {children}
          </div>
          <footer className="flex shrink-0 flex-wrap items-center justify-end gap-2 border-t border-rule px-5 py-3 pb-[max(0.75rem,env(safe-area-inset-bottom))]">
            {footer}
          </footer>
        </div>
      ) : null}
    </dialog>
  )
}

/** One applied filter, as a chip that takes it off again. */
interface Applied {
  key: string
  label: string
  /** What the parameters hold once this is removed. */
  clear: Record<string, string>
}

/** The chips of what is applied, each a button that takes it off. */
function AppliedChips({ applied, change }: { applied: Applied[]; change: (changes: Record<string, string>) => void }) {
  const { t } = useI18n()
  return (
    <div className="flex flex-wrap items-center gap-2">
      {applied.map((filter) => (
        <button
          key={filter.key}
          type="button"
          onClick={() => change(filter.clear)}
          className={cn(
            'hit inline-flex min-h-9 max-w-full cursor-pointer items-center gap-1.5 rounded-full border border-rule-bright px-3',
            'text-body-sm text-bone transition-colors duration-150 hover:border-vermillion-deep hover:text-vermillion',
          )}
          aria-label={t.browse.remove(filter.label)}
        >
          <span className="truncate">{filter.label}</span>
          <Glyph name="close" className="size-3 shrink-0" />
        </button>
      ))}
    </div>
  )
}

/** A country, as the reader's language names it: `FR` is France, or Frankreich. */
function countryName(code: string, locale: string): string {
  try {
    return new Intl.DisplayNames([locale], { type: 'region', fallback: 'code' }).of(code) ?? code
  } catch {
    return code
  }
}

function activeFilters(
  f: Filters,
  t: ReturnType<typeof useI18n>['t'],
  lang: ReturnType<typeof useI18n>['lang'],
  locale: string,
  names: { person?: string; collection?: string },
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
  if (f.person) out.push(one('person', t.browse.withPerson(names.person ?? f.person), 'person'))
  if (f.keyword) out.push(one('keyword', `# ${f.keyword}`, 'keyword'))
  if (f.yearFrom && f.yearFrom === f.yearTo) {
    out.push({ key: 'year', label: f.yearFrom, clear: { yearFrom: '', yearTo: '', year: '' } })
  } else {
    if (f.yearFrom) out.push(one('yearFrom', t.browse.since(f.yearFrom), 'yearFrom'))
    if (f.yearTo) out.push(one('yearTo', t.browse.until(f.yearTo), 'yearTo'))
  }
  if (f.runtimeMin || f.runtimeMax) {
    out.push({
      key: 'runtime',
      label: t.browse.runtimeChip(f.runtimeMin ? Number(f.runtimeMin) : undefined, f.runtimeMax ? Number(f.runtimeMax) : undefined),
      clear: { runtimeMin: '', runtimeMax: '' },
    })
  }
  if (f.minRating) {
    out.push(one('minRating', t.browse.atLeast(fmt.score(Number(f.minRating), locale) ?? f.minRating), 'minRating'))
  }
  if (f.status) out.push(one('status', statusLabel(f.status, t) ?? f.status, 'status'))
  if (f.language) out.push(one('language', languageName(f.language, locale) ?? f.language, 'language'))
  if (f.network) out.push(one('network', f.network, 'network'))
  if (f.country) out.push(one('country', t.browse.madeIn(countryName(f.country, locale)), 'country'))
  if (f.certification) out.push(one('certification', t.browse.rated(f.certification), 'certification'))
  if (f.collection) {
    out.push(one('collection', names.collection ?? t.collections.unnamed(Number(f.collection)), 'collection'))
  }
  if (f.manualOnly) out.push(one('manual', t.browse.manualOnly, 'manual'))

  return out
}

/**
 * The languages on offer, one choice per tongue however many ways the
 * catalogue spells it, each choice asking for every spelling at once. The
 * value the URL holds is kept as it is — an older link's single code is
 * still the group's choice — and offered on its own where nothing counts it.
 */
function languageChoices(
  facets: Facet[],
  current: string,
  locale: string,
): { value: string; label: string; count?: number }[] {
  const choices: { value: string; label: string; count?: number }[] = languageGroups(
    facets,
    (facet) => facet.value,
    (facet) => facet.count,
    locale,
  ).map((group) => ({
    value: group.codes.includes(current) ? current : group.codes.join(','),
    label: group.label,
    count: group.count,
  }))
  if (current && !choices.some((choice) => choice.value === current)) {
    choices.push({ value: current, label: languageName(current, locale) ?? current })
  }
  return choices
}

/**
 * A value the URL holds that the counts do not list — a network outside the
 * forty most common, followed from a work's page — is still offered, or the
 * select would read "any" over a list that is filtered.
 */
function withCurrent(list: Facet[], current: string): { value: string; count?: number }[] {
  return current && !list.some((facet) => facet.value === current) ? [...list, { value: current }] : list
}

/**
 * Every filter, as one panel. The same component stands in the rail and in
 * the phone's sheet, so the two can never offer different things.
 */
function FilterPanel({
  filters: f,
  facets,
  change,
  pinned,
  idPrefix,
  onMore,
  moreOn,
}: {
  filters: Filters
  facets?: CatalogueFacets
  change: (changes: Record<string, string>) => void
  /** What the page pins the list to: not offered as a choice. */
  pinned?: Pinned
  idPrefix: string
  /** Open the sheet of the rarer filters. */
  onMore: () => void
  /** How many of those are on. */
  moreOn: number
}) {
  const { t, locale } = useI18n()
  const id = (name: string) => `${idPrefix}-${name}`
  const update = (name: string, value: string) => change({ [name]: value })

  const toggleGenre = (genre: string) => {
    const next = f.genres.includes(genre) ? f.genres.filter((g) => g !== genre) : [...f.genres, genre]
    // How they combine only means something for two or more.
    change({ genre: next.join(','), genreMode: next.length >= 2 && f.genreMode === 'any' ? 'any' : '' })
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
                'hit relative grid min-h-9 cursor-pointer place-items-center rounded-full px-2 text-body-sm',
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
                // emptied the list while their selects read "any". So does a
                // collection, which only films belong to.
                onChange={() =>
                  change({ kind: value, status: '', network: '', ...(value === 'series' ? { collection: '' } : {}) })
                }
                className="sr-only"
              />
              {value === '' ? t.browse.all : value === 'series' ? t.browse.series : t.browse.films}
            </label>
          ))}
        </div>
      </fieldset>

      {facets?.genres.length || f.genres.length ? (
        <GenreFilter
          // The page's own genre is every work's here, and no choice.
          genres={(facets?.genres ?? []).filter((genre) => !pinned?.genre || genre.slug !== pinned.genre)}
          selected={f.genres}
          mode={f.genreMode}
          onToggle={toggleGenre}
          onMode={(mode) => update('genreMode', mode === 'any' ? 'any' : '')}
          idPrefix={idPrefix}
        />
      ) : null}

      {pinned?.yearFrom || pinned?.yearTo ? null : (
        <fieldset>
          <legend className="label mb-2">{t.browse.years}</legend>
          <YearRange filters={f} facets={facets} update={update} id={id} />
        </fieldset>
      )}

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
            {languageChoices(facets?.languages ?? [], f.language, locale).map((language) => (
              <option key={language.value} value={language.value}>
                {language.label}
                {language.count !== undefined ? ` (${language.count})` : ''}
              </option>
            ))}
          </Select>
        </div>
      ) : null}

      {!pinned?.network && (facets?.networks.length || f.network) ? (
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

      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          variant={f.manualOnly ? 'primary' : 'ghost'}
          aria-pressed={f.manualOnly}
          onClick={() => update('manual', f.manualOnly ? '' : '1')}
        >
          <Glyph name="lock" className="size-4" />
          {t.browse.manualOnly}
        </Button>
        <Button size="sm" onClick={onMore} aria-haspopup="dialog">
          <Glyph name="plus" className="size-4" />
          {t.browse.moreFilters}
          {moreOn ? (
            <span className="rounded-full bg-vermillion px-1.5 font-mono text-caption text-ink tabular-nums">
              {moreOn}
            </span>
          ) : null}
        </Button>
      </div>
    </div>
  )
}

/**
 * The rarer filters: who is credited, a keyword, how long a work runs, where
 * it was made and how the reader's country rates it, the saga a film belongs
 * to. Each is applied as it is chosen — a field when it is left or Enter is
 * pressed — as the rail's are.
 */
function MoreFilters({
  filters: f,
  facets,
  change,
}: {
  filters: Filters
  facets?: CatalogueFacets
  change: (changes: Record<string, string>) => void
}) {
  const { t, locale } = useI18n()
  const collections = useCollections()
  const update = (name: string, value: string) => change({ [name]: value })
  const region = facets?.region ? countryName(facets.region, locale) : undefined
  const sagas = collections.data?.collections ?? []

  return (
    <div className="space-y-7">
      <fieldset className="space-y-4">
        <legend className="label mb-3">{t.browse.creditsAndWords}</legend>
        <CommittedField
          id="more-person"
          label={t.browse.person}
          hint={t.browse.personHint}
          value={f.person}
          onCommit={(value) => update('person', value)}
        />
        <CommittedField
          id="more-keyword"
          label={t.browse.keyword}
          hint={t.browse.keywordHint}
          value={f.keyword}
          onCommit={(value) => update('keyword', value)}
          // As wide as the keyword typed, within the sheet: a word, not a line.
          className="w-auto max-w-full min-w-[14ch] field-sizing-content"
        />
      </fieldset>

      <fieldset>
        <legend className="label mb-3">{t.browse.runtime}</legend>
        <div className="grid grid-cols-2 gap-2">
          {(['runtimeMin', 'runtimeMax'] as const).map((name) => (
            <CommittedField
              key={name}
              id={`more-${name}`}
              label={name === 'runtimeMin' ? t.browse.runtimeMin : t.browse.runtimeMax}
              value={f[name]}
              numeric
              suffix={t.browse.minutes}
              onCommit={(value) => update(name, /^\d{1,5}$/.test(value) ? value : '')}
            />
          ))}
        </div>
      </fieldset>

      <fieldset className="space-y-4">
        <legend className="label mb-3">{t.browse.origin}</legend>
        <div>
          <label htmlFor="more-country" className="mb-1 block text-xs text-bone-faint">
            {t.browse.country}
          </label>
          <Select id="more-country" value={f.country} onChange={(e) => update('country', e.target.value)}>
            <option value="">{t.browse.anyCountry}</option>
            {withCurrent(facets?.countries ?? [], f.country)
              .map((country) => ({ ...country, label: countryName(country.value, locale) }))
              .map((country) => (
                <option key={country.value} value={country.value}>
                  {country.label}
                  {country.count !== undefined ? ` (${country.count})` : ''}
                </option>
              ))}
          </Select>
        </div>
        <div>
          <label htmlFor="more-certification" className="mb-1 block text-xs text-bone-faint">
            {t.browse.certification(region)}
          </label>
          <Select id="more-certification" value={f.certification} onChange={(e) => update('certification', e.target.value)}>
            <option value="">{t.browse.anyCertification}</option>
            {withCurrent(facets?.certifications ?? [], f.certification).map((rating) => (
              <option key={rating.value} value={rating.value}>
                {rating.value}
                {rating.count !== undefined ? ` (${rating.count})` : ''}
              </option>
            ))}
          </Select>
        </div>
      </fieldset>

      {/* A series belongs to no collection: offered with films in view. */}
      {(sagas.length && f.kind !== 'series') || f.collection ? (
        <div>
          <label htmlFor="more-collection" className="label mb-2 block">
            {t.browse.collection}
          </label>
          <Select id="more-collection" value={f.collection} onChange={(e) => update('collection', e.target.value)}>
            <option value="">{t.browse.anyCollection}</option>
            {sagas.map((saga) => (
              <option key={saga.tmdbId} value={String(saga.tmdbId)}>
                {saga.name ?? t.collections.unnamed(saga.tmdbId)} ({saga.count})
              </option>
            ))}
            {f.collection && !sagas.some((saga) => String(saga.tmdbId) === f.collection) ? (
              <option value={f.collection}>{t.collections.unnamed(Number(f.collection))}</option>
            ) : null}
          </Select>
        </div>
      ) : null}
    </div>
  )
}

/**
 * A text the list is narrowed by, applied when the field is left or Enter
 * pressed — not per keystroke, which would ask the server for a list at every
 * letter of a name — and set again when the address moves under it.
 */
function CommittedField({
  id,
  label,
  hint,
  value,
  onCommit,
  numeric = false,
  suffix,
  className,
}: {
  id: string
  label: string
  hint?: string
  value: string
  onCommit: (value: string) => void
  numeric?: boolean
  suffix?: string
  className?: string
}) {
  const [typed, setTyped] = useState(value)
  const [seen, setSeen] = useState(value)
  if (seen !== value) {
    setSeen(value)
    setTyped(value)
  }
  const commit = () => {
    const clean = typed.trim()
    if (clean !== value) onCommit(clean)
  }
  const hintId = hint ? `${id}-hint` : undefined

  return (
    <div className="flex flex-col">
      <label htmlFor={id} className="mb-1 block text-xs text-bone-faint">
        {label}
      </label>
      <div className="flex items-center gap-2">
        <Input
          id={id}
          value={typed}
          inputMode={numeric ? 'numeric' : undefined}
          aria-describedby={hintId}
          onChange={(e) => setTyped(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => e.key === 'Enter' && commit()}
          className={cn(numeric && 'tabular-nums', className)}
        />
        {suffix ? <span className="shrink-0 text-xs text-bone-faint">{suffix}</span> : null}
      </div>
      {hint ? (
        <p id={hintId} className="mt-1 text-xs text-bone-faint">
          {hint}
        </p>
      ) : null}
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
  facets?: CatalogueFacets
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
