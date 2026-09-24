/**
 * A season of the catalogue, set as a festival programme.
 *
 * The season's name in the display face; beneath it a strip of film, a frame
 * for each of its weeks, showing how many works each brings and where the
 * reader is in it; then the programme — new series, series back for a new
 * season, films, and what is still airing from the season before, each a card
 * with the day it opens, its next episode and its trailer.
 *
 * The chart is the catalogue's own, so a season is what this server holds. For
 * whoever maintains it, what else TMDB lists for the season follows, to import.
 * Every filter lives in the address, and all of them are applied here, over a
 * season's worth of works the server sends at once.
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Link, Navigate, useLocation, useParams, useSearchParams } from 'react-router'

import { Trailer } from '../components/Theatre'
import { ExternalLink } from '../components/elsewhere'
import { Artwork, Score } from '../components/media'
import { Button, Chip, EmptyState, Genre, Glyph, Label, Select, Skeleton } from '../components/ui'
import { ApiError, api, query } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useMe, useTitle } from '../lib/hooks'
import { genreLabel, languageName, listedGenres } from '../lib/labels'
import { useI18n } from '../lib/i18n'
import { identifierLink } from '../lib/links'
import { headlineRating, poster } from '../lib/media'
import {
  SEASONS,
  type SeasonName,
  type SeasonRef,
  around,
  chartQuery,
  formatDay,
  isSeason,
  mondayOf,
  seasonOf,
  seasonPath,
  stepSeason,
  weekday,
  weeksOf,
} from '../lib/seasons'
import type { MediaItem, SeasonCandidate, SeasonChart as ChartData, SeasonEntry, SeasonEntryKind } from '../lib/types'
import { NotFound } from './NotFound'

/** The kinds, in the order the programme runs them. */
const KINDS: SeasonEntryKind[] = ['newSeries', 'newSeason', 'film', 'continuing']
const TRAILERS = ['all', 'with', 'without'] as const
const SORTS = ['popularity', 'date', 'score', 'title'] as const

type Sort = (typeof SORTS)[number]

export function Seasons() {
  const { year, season } = useParams()

  // `/seasons` on its own is the season the reader is in.
  if (year === undefined) return <Navigate to={seasonPath(seasonOf(new Date()))} replace />

  const number = Number(year)
  if (!Number.isInteger(number) || number < 1890 || number > 2100 || !isSeason(season)) {
    return <NotFound />
  }

  // Keyed, so another season starts with nothing of this one's state.
  return <Programme key={`${number}-${season}`} at={{ year: number, season }} />
}

/** The filters the address carries. */
function read(params: URLSearchParams) {
  const type = params.get('type')
  const trailer = params.get('trailer')
  const sort = params.get('sort')
  return {
    type: (KINDS as string[]).includes(type ?? '') ? (type as SeasonEntryKind) : undefined,
    trailer: (TRAILERS as readonly string[]).includes(trailer ?? '') ? (trailer as (typeof TRAILERS)[number]) : 'all',
    language: params.get('language') ?? '',
    // As the catalogue lists them: an older address asking for a series'
    // "Action & Adventure" asks for its "Action" and "Adventure".
    genres: listedGenres(
      (params.get('genre') ?? '')
        .split(',')
        .map((g) => g.trim())
        .filter(Boolean),
    ),
    sort: (SORTS as readonly string[]).includes(sort ?? '') ? (sort as Sort) : 'popularity',
  }
}

type Filters = ReturnType<typeof read>

function Programme({ at }: { at: SeasonRef }) {
  const { t, lang, locale } = useI18n()
  const [params, setParams] = useSearchParams()
  const location = useLocation()
  const me = useMe()
  // The trailer last asked for stays mounted once closed, so the dialog is
  // closed rather than torn down, and focus goes back to the button that
  // opened it.
  const [trailer, setTrailer] = useState<{ id: string; title: string } | null>(null)
  const [playing, setPlaying] = useState(false)
  const onTrailer = useCallback((id: string, title: string) => {
    setTrailer({ id, title })
    setPlaying(true)
  }, [])

  const name = t.seasons.names[at.season]
  useTitle(t.seasons.title(name, at.year))

  // The reader's day, the one the server counts what has aired from: taken
  // once, so a page left open past midnight does not ask again mid-read.
  const [today] = useState(() => fmt.localDay(new Date()))
  const chart = useQuery(chartQuery(at, lang, today))

  const search = params.toString()
  const f = useMemo(() => read(new URLSearchParams(search)), [search])

  /** Change some filters, keep the rest: from the address as it is now. */
  const change = useCallback(
    (changes: Record<string, string>) => {
      const next = new URLSearchParams(window.location.search)
      for (const [key, value] of Object.entries(changes)) {
        if (value) next.set(key, value)
        else next.delete(key)
      }
      setParams(next, { replace: true })
    },
    [setParams],
  )

  // Worked out once per season and set of filters, not on every render: a
  // trailer opening, or the header re-rendering, leaves the programme alone.
  const derived = useMemo(() => {
    const works = new Map((chart.data?.works ?? []).map((w) => [w.id, w]))
    const all = (chart.data?.entries ?? []).filter((e) => works.has(e.workId))
    // Genres as the catalogue lists them: a series' "Action & Adventure" is
    // the "Action" and "Adventure" of the films beside it.
    const genresOf = new Map([...works.values()].map((w) => [w.id, listedGenres(w.genres)]))
    const wanted = f.genres

    // Each filter's own values are counted under the others, so a count is
    // what choosing it would leave.
    const passes = (entry: SeasonEntry, except?: 'type' | 'language' | 'trailer') => {
      const work = works.get(entry.workId)!
      return (
        (except === 'type' || !f.type || entry.kind === f.type) &&
        (except === 'language' || !f.language || work.originalLanguage === f.language) &&
        (except === 'trailer' ||
          f.trailer === 'all' ||
          (f.trailer === 'with') === Boolean(work.trailerYoutubeId)) &&
        wanted.every((g) => genresOf.get(work.id)!.includes(g))
      )
    }

    const visible = sorted(all.filter((e) => passes(e)), f.sort, works, locale)
    const typed = all.filter((e) => passes(e, 'type'))
    return {
      works,
      all,
      visible,
      byKind: count(typed.map((e) => e.kind)),
      total: typed.length,
      byLanguage: count(
        all.filter((e) => passes(e, 'language')).flatMap((e) => works.get(e.workId)?.originalLanguage ?? []),
      ),
      byGenre: count(visible.flatMap((e) => genresOf.get(e.workId) ?? [])),
      // The order genres are offered in is the season's, not the filtered
      // list's: a chip that moved when it was pressed took the focus with it.
      genreOrder: [...count(all.flatMap((e) => genresOf.get(e.workId) ?? []))]
        .sort(([a, x], [b, y]) => y - x || a.localeCompare(b))
        .map(([genre]) => genre),
    }
  }, [chart.data, f, locale])
  const { works, all, visible } = derived
  const filtered = Boolean(f.type || f.language || f.genres.length || f.trailer !== 'all')

  // A frame of the strip, followed: the week's section, once it is drawn —
  // once for each time a frame is followed, and not again when the season is
  // read afresh, after an import say, with the reader elsewhere on the page.
  const followed = useRef<string | null>(null)
  useEffect(() => {
    if (!location.hash || !chart.data || followed.current === location.key) return
    let id: string
    try {
      id = decodeURIComponent(location.hash.slice(1))
    } catch {
      return
    }
    const target = document.getElementById(id)
    if (!target) return
    followed.current = location.key
    const still = window.matchMedia('(prefers-reduced-motion: reduce)').matches
    target.scrollIntoView({ behavior: still ? 'auto' : 'smooth', block: 'start' })
    target.focus({ preventScroll: true })
  }, [location.hash, location.key, chart.data, f.sort])

  const future = chart.data ? chart.data.from > today : false

  return (
    <div className="pt-10 pb-16">
      <Header
        at={at}
        total={chart.isSuccess ? new Set(visible.map((e) => e.workId)).size : undefined}
        from={chart.data?.from}
        to={chart.data?.to}
      />

      {chart.isPending ? (
        <ProgrammeSkeleton />
      ) : chart.isError ? (
        <EmptyState
          title={t.common.error}
          hint={t.seasons.loadFailed}
          action={
            <Button className="mt-2" onClick={() => void chart.refetch()}>
              <Glyph name="refresh" className="size-4" />
              {t.common.retry}
            </Button>
          }
        />
      ) : (
        <>
          <Strip from={chart.data.from} to={chart.data.to} today={today} entries={visible} params={params} />

          {/* A season the catalogue holds nothing of has nothing to filter. */}
          {all.length ? (
            <Controls
              filters={f}
              change={change}
              byKind={derived.byKind}
              total={derived.total}
              byLanguage={derived.byLanguage}
              byGenre={derived.byGenre}
              genreOrder={derived.genreOrder}
              filtered={filtered}
            />
          ) : null}

          {visible.length ? (
            <Sections entries={visible} works={works} sort={f.sort} chart={chart.data} today={today} onTrailer={onTrailer} />
          ) : (
            <EmptyState
              title={filtered ? t.seasons.emptyFiltered : future ? t.seasons.emptyFuture : t.seasons.empty}
              hint={t.seasons.emptyHint}
              action={
                filtered ? (
                  <Button size="sm" className="mt-2" onClick={() => setParams(f.sort === 'popularity' ? {} : { sort: f.sort }, { replace: true })}>
                    {t.seasons.clear}
                  </Button>
                ) : undefined
              }
            />
          )}

          {me.data?.canWrite ? <Candidates at={at} filters={f} /> : null}
        </>
      )}

      <p className="mt-12 max-w-[70ch] text-xs leading-relaxed text-bone-faint">{t.seasons.note}</p>

      {trailer ? (
        <Trailer youtubeId={trailer.id} title={trailer.title} open={playing} onClose={() => setPlaying(false)} />
      ) : null}
    </div>
  )
}

/** How many of each value a list holds. */
function count(values: string[]): Map<string, number> {
  const counts = new Map<string, number>()
  for (const value of values) counts.set(value, (counts.get(value) ?? 0) + 1)
  return counts
}

function sorted(entries: SeasonEntry[], sort: Sort, works: Map<string, MediaItem>, locale: string): SeasonEntry[] {
  // One collator for the whole sort, rather than one per comparison.
  const collator = new Intl.Collator(locale, { sensitivity: 'base' })
  const title = (e: SeasonEntry) => works.get(e.workId)?.title ?? ''
  const byTitle = (a: SeasonEntry, b: SeasonEntry) => collator.compare(title(a), title(b))
  const score = (e: SeasonEntry) => headlineRating(works.get(e.workId)?.ratings)?.value ?? -1
  const popularity = (e: SeasonEntry) => works.get(e.workId)?.popularity ?? -1

  return [...entries].sort((a, b) => {
    switch (sort) {
      case 'date':
        return a.starts.localeCompare(b.starts) || byTitle(a, b)
      case 'score':
        return score(b) - score(a) || byTitle(a, b)
      case 'title':
        return byTitle(a, b)
      default:
        return popularity(b) - popularity(a) || byTitle(a, b)
    }
  })
}

// ─── the header and the way to other seasons ────────────────────────────────

function Header({ at, total, from, to }: { at: SeasonRef; total?: number; from?: string; to?: string }) {
  const { t, locale } = useI18n()
  const { search } = useLocation()
  const before = stepSeason(at, -1)
  const after = stepSeason(at, 1)
  const now = seasonOf(new Date())
  const here = now.year === at.year && now.season === at.season
  const name = (ref: SeasonRef) => t.seasons.title(t.seasons.names[ref.season], ref.year)
  // Another season, looked at the same way: films in Japanese stay films in
  // Japanese from one season to the next.
  const toward = (ref: SeasonRef) => ({ pathname: seasonPath(ref), search })

  const range =
    from && to
      ? new Intl.DateTimeFormat(locale, { day: 'numeric', month: 'long', year: 'numeric', timeZone: 'UTC' }).formatRange(
          new Date(`${from}T00:00:00Z`),
          new Date(`${to}T00:00:00Z`),
        )
      : undefined

  return (
    <header className="rise">
      <div className="flex flex-wrap items-end justify-between gap-x-8 gap-y-4">
        <div className="min-w-0">
          <Label>{t.seasons.label}</Label>
          <h1 className="mt-2 font-display text-5xl leading-[0.95] font-medium tracking-tight text-bone sm:text-7xl">
            {t.seasons.names[at.season]}{' '}
            <span className="text-bone-faint tabular-nums">{at.year}</span>
          </h1>
          <p className="mt-3 font-mono text-sm text-bone-faint tabular-nums">
            {range}
            {/* Said again as the filters narrow it, as the catalogue's count is. */}
            <span aria-live="polite">{total !== undefined ? `${range ? ' · ' : ''}${t.seasons.count(total)}` : ''}</span>
          </p>
        </div>

        <div className="flex items-center gap-1">
          <Link
            to={toward(before)}
            rel="prev"
            className="inline-flex min-h-11 items-center gap-1.5 rounded-full px-3 text-sm text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
          >
            <Glyph name="chevronLeft" className="size-4" />
            <span className="sr-only">{t.seasons.previous} </span>
            {name(before)}
          </Link>
          <Link
            to={toward(after)}
            rel="next"
            className="inline-flex min-h-11 items-center gap-1.5 rounded-full px-3 text-sm text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
          >
            <span className="sr-only">{t.seasons.next} </span>
            {name(after)}
            <Glyph name="chevronRight" className="size-4" />
          </Link>
        </div>
      </div>

      {/* The year, stepped either way, and its four seasons: links, so a
          reader walking the calendar keeps a history to walk back through. On
          a phone the seasons take a row of their own, the width of it. */}
      <nav aria-label={t.seasons.picker} className="mt-6 flex flex-wrap items-center gap-x-3 gap-y-3">
        <div className="flex items-center">
          <Link
            to={toward({ year: at.year - 1, season: at.season })}
            aria-label={t.seasons.previousYear(name({ year: at.year - 1, season: at.season }))}
            className="grid size-11 place-items-center rounded-full text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
          >
            <Glyph name="chevronLeft" className="size-4" />
          </Link>
          <span className="min-w-[4.5ch] text-center font-mono text-sm text-bone tabular-nums">{at.year}</span>
          <Link
            to={toward({ year: at.year + 1, season: at.season })}
            aria-label={t.seasons.nextYear(name({ year: at.year + 1, season: at.season }))}
            className="grid size-11 place-items-center rounded-full text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
          >
            <Glyph name="chevronRight" className="size-4" />
          </Link>
        </div>
        <ol
          aria-label={t.seasons.year(at.year)}
          className="order-last grid w-full grid-cols-4 gap-1 rounded-full border border-rule p-1 sm:order-none sm:flex sm:w-auto"
        >
          {SEASONS.map((season: SeasonName) => {
            const current = season === at.season
            return (
              <li key={season} className="flex">
                <Link
                  to={toward({ year: at.year, season })}
                  aria-current={current ? 'page' : undefined}
                  className={cn(
                    'hit inline-flex min-h-9 flex-1 items-center justify-center rounded-full px-3.5 text-[0.8125rem] transition-colors duration-150',
                    current ? 'bg-ink-top text-bone' : 'text-bone-dim hover:text-bone',
                  )}
                >
                  {t.seasons.names[season]}
                </Link>
              </li>
            )
          })}
        </ol>
        {here ? null : (
          <Link
            to={toward(now)}
            className="ml-auto inline-flex min-h-11 items-center gap-1.5 rounded-full px-3 text-sm text-vermillion transition-colors duration-150 hover:bg-vermillion/10 sm:ml-0"
          >
            <Glyph name="calendar" className="size-4" />
            {t.seasons.current}
          </Link>
        )}
      </nav>
    </header>
  )
}

// ─── the strip ───────────────────────────────────────────────────────────────

/**
 * The season as a strip of film: a frame a week, the works it brings standing
 * in it, and a playhead on the day the reader is on. A frame leads to its
 * week in the programme, set out by date.
 */
function Strip({
  from,
  to,
  today,
  entries,
  params,
}: {
  from: string
  to: string
  today: string
  entries: SeasonEntry[]
  params: URLSearchParams
}) {
  const { t, locale } = useI18n()
  const reel = useRef<HTMLDivElement>(null)
  const weeks = weeksOf(from, to)
  const openings = count(entries.filter((e) => e.kind !== 'continuing').map((e) => mondayOf(e.starts)))
  const most = Math.max(1, ...openings.values())
  const shown = openings.size > 0

  // Too narrow a screen for thirteen frames a finger can hit, and the strip
  // scrolls: to the week the reader is in, when the season holds it.
  useEffect(() => {
    const strip = reel.current
    const now = strip?.querySelector<HTMLElement>('[data-today]')
    if (!strip || !now || strip.scrollWidth <= strip.clientWidth) return
    // Where the frame sits along the strip, whatever it is measured from.
    const left = now.getBoundingClientRect().left - strip.getBoundingClientRect().left + strip.scrollLeft
    strip.scrollLeft = left - (strip.clientWidth - now.offsetWidth) / 2
  }, [shown])

  const day = (value: string, options: Intl.DateTimeFormatOptions) => formatDay(value, locale, options)

  const dated = new URLSearchParams(params)
  dated.set('sort', 'date')

  // Nothing opens: the programme below says so, and an empty strip would only
  // repeat it.
  if (!shown) return null

  return (
    <nav aria-label={t.seasons.strip} className="rise mt-10" style={{ animationDelay: '60ms' }}>
      <div ref={reel} className="overflow-x-auto overscroll-x-contain rounded-card border border-rule [scrollbar-width:thin]">
      <ol className="film-strip flex min-w-max divide-x divide-rule sm:min-w-0">
        {weeks.map((week, index) => {
          const n = openings.get(week.monday) ?? 0
          const holdsToday = today >= week.first && today <= week.last
          const month = index === 0 || week.first.slice(5, 7) !== weeks[index - 1]!.first.slice(5, 7)
          const label = [t.seasons.frame(day(week.first, { day: 'numeric', month: 'long' }), n), holdsToday ? t.seasons.today : '']
            .filter(Boolean)
            .join(' · ')

          const inner = (
            <>
              <span className={cn('font-mono text-[0.6875rem] tabular-nums', n ? 'text-bone' : 'text-transparent')} aria-hidden>
                {n}
              </span>
              <span
                aria-hidden
                className={cn('w-full max-w-6 rounded-t-[2px]', n ? 'bg-vermillion/75' : 'bg-rule')}
                style={{ height: n ? `${Math.max(12, (n / most) * 52)}%` : '2px' }}
              />
              <span aria-hidden className="mt-1 flex flex-col items-center font-mono text-[0.625rem] leading-tight text-bone-faint tabular-nums">
                {day(week.first, { day: 'numeric' })}
                <span className={cn('uppercase', month ? 'text-bone-dim' : 'invisible')}>
                  {day(week.first, { month: 'short' })}
                </span>
              </span>
              {holdsToday ? (
                <span
                  aria-hidden
                  className="pointer-events-none absolute inset-y-0 w-0.5 bg-vermillion"
                  style={{ left: `${((weekday(today) + 0.5) / 7) * 100}%` }}
                />
              ) : null}
            </>
          )

          const frame = cn(
            'relative flex h-28 min-w-0 flex-1 flex-col items-center justify-end px-0.5 pt-2 pb-1 sm:px-1',
            holdsToday && 'bg-vermillion/[0.06]',
          )

          return (
            <li key={week.monday} data-today={holdsToday || undefined} className="flex min-w-11 flex-1">
              {n ? (
                <Link
                  to={{ search: dated.toString(), hash: `week-${week.monday}` }}
                  aria-label={label}
                  aria-current={holdsToday ? 'date' : undefined}
                  className={cn(frame, 'transition-colors duration-150 hover:bg-ink-high')}
                >
                  {inner}
                </Link>
              ) : (
                <span className={frame} aria-label={label} role="img">
                  {inner}
                </span>
              )}
            </li>
          )
        })}
      </ol>
      </div>
    </nav>
  )
}

// ─── filters ─────────────────────────────────────────────────────────────────

function Controls({
  filters: f,
  change,
  byKind,
  total,
  byLanguage,
  byGenre,
  genreOrder,
  filtered,
}: {
  filters: Filters
  change: (changes: Record<string, string>) => void
  byKind: Map<string, number>
  total: number
  byLanguage: Map<string, number>
  byGenre: Map<string, number>
  genreOrder: string[]
  filtered: boolean
}) {
  const { t, lang, locale } = useI18n()

  const toggleGenre = (genre: string) => {
    const next = f.genres.includes(genre) ? f.genres.filter((g) => g !== genre) : [...f.genres, genre]
    change({ genre: next.join(',') })
  }
  // Every genre of the season, in the season's order, whatever the filters
  // leave: a chip keeps its place when pressed, and so does the focus on it.
  // A genre asked for that the season lacks is offered too, to be taken off.
  const genres = [...genreOrder, ...f.genres.filter((g) => !genreOrder.includes(g))]
  const languages = [...new Set([...(f.language ? [f.language] : []), ...byLanguage.keys()])].sort(
    (a, b) => (byLanguage.get(b) ?? 0) - (byLanguage.get(a) ?? 0),
  )

  return (
    <div className="rise mt-8 space-y-5" style={{ animationDelay: '100ms' }}>
      {/* One kind at a time, or all of them: a choice of one, so radios, as
          the catalogue's own kind is. */}
      <fieldset>
        <legend className="sr-only">{t.seasons.kind}</legend>
        <div className="flex flex-wrap gap-2">
          {([undefined, ...KINDS] as (SeasonEntryKind | undefined)[]).map((kind) => {
            const n = kind ? (byKind.get(kind) ?? 0) : total
            const on = f.type === kind
            if (kind && !n && !on) return null
            return (
              <label
                key={kind ?? 'all'}
                className={cn(
                  'relative inline-flex min-h-11 cursor-pointer items-center gap-2 rounded-full border px-4 text-sm transition-colors duration-150',
                  'has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-vermillion',
                  on
                    ? 'border-vermillion bg-vermillion/12 text-bone'
                    : 'border-rule-bright text-bone-dim hover:border-bone-faint hover:text-bone',
                )}
              >
                <input
                  type="radio"
                  name="season-kind"
                  value={kind ?? ''}
                  checked={on}
                  onChange={() => change({ type: kind ?? '' })}
                  className="sr-only"
                />
                {on ? <Glyph name="check" className="size-3.5 text-vermillion" /> : null}
                {kind ? t.seasons.kinds[kind] : t.seasons.kinds.all}
                <span className="font-mono text-xs text-bone-faint tabular-nums">{n}</span>
              </label>
            )
          })}
        </div>
      </fieldset>

      <div className="flex flex-wrap items-end gap-x-6 gap-y-4">
        <fieldset>
          <legend className="label mb-1.5">{t.seasons.trailer}</legend>
          <div className="flex gap-1 rounded-full border border-rule p-1">
            {TRAILERS.map((value) => (
              <label
                key={value}
                className={cn(
                  'hit relative inline-flex min-h-9 cursor-pointer items-center rounded-full px-3 text-[0.8125rem]',
                  'transition-colors duration-150 has-focus-visible:outline-2 has-focus-visible:outline-vermillion',
                  f.trailer === value ? 'bg-ink-top text-bone' : 'text-bone-dim hover:text-bone',
                )}
              >
                <input
                  type="radio"
                  name="season-trailer"
                  value={value}
                  checked={f.trailer === value}
                  onChange={() => change({ trailer: value === 'all' ? '' : value })}
                  className="sr-only"
                />
                {t.seasons.trailers[value]}
              </label>
            ))}
          </div>
        </fieldset>

        {languages.length ? (
          <div>
            <label htmlFor="season-language" className="label mb-1.5 block">
              {t.seasons.language}
            </label>
            <Select id="season-language" value={f.language} onChange={(e) => change({ language: e.target.value })}>
              <option value="">{t.seasons.anyLanguage}</option>
              {languages.map((code) => (
                <option key={code} value={code}>
                  {languageName(code, locale)} ({byLanguage.get(code) ?? 0})
                </option>
              ))}
            </Select>
          </div>
        ) : null}

        <div>
          <label htmlFor="season-sort" className="label mb-1.5 block">
            {t.seasons.sort}
          </label>
          <Select id="season-sort" value={f.sort} onChange={(e) => change({ sort: e.target.value === 'popularity' ? '' : e.target.value })}>
            {SORTS.map((sort) => (
              <option key={sort} value={sort}>
                {t.seasons.sorts[sort]}
              </option>
            ))}
          </Select>
        </div>

        {filtered ? (
          <Button size="sm" variant="quiet" onClick={() => change({ type: '', trailer: '', language: '', genre: '' })}>
            <Glyph name="close" className="size-3.5" />
            {t.seasons.clear}
          </Button>
        ) : null}
      </div>

      {genres.length ? (
        <div role="group" aria-label={t.seasons.genres} className="flex flex-wrap gap-x-1.5 gap-y-3">
          {genres.map((genre) => {
            const on = f.genres.includes(genre)
            const n = byGenre.get(genre) ?? 0
            return (
              <button
                key={genre}
                type="button"
                aria-pressed={on}
                onClick={() => toggleGenre(genre)}
                className={cn(
                  'hit inline-flex min-h-8 cursor-pointer items-center gap-1.5 rounded-full border px-2.5 text-xs transition-colors duration-150',
                  on
                    ? 'border-vermillion bg-vermillion/15 text-bone'
                    : n
                      ? 'border-rule-bright text-bone-dim hover:border-bone-faint hover:text-bone'
                      : 'border-rule text-bone-faint hover:text-bone-dim',
                )}
              >
                {on ? <Glyph name="check" className="size-3 text-vermillion" /> : null}
                {genreLabel(genre, lang)}
                <span className="font-mono text-[0.625rem] text-bone-faint tabular-nums">{n}</span>
              </button>
            )
          })}
        </div>
      ) : null}
    </div>
  )
}

// ─── the programme ───────────────────────────────────────────────────────────

/**
 * By week when the reader asked for dates — what is still airing first, then
 * a section for each week that opens something — and by kind otherwise.
 */
const Sections = memo(function Sections({
  entries,
  works,
  sort,
  chart,
  today,
  onTrailer,
}: {
  entries: SeasonEntry[]
  works: Map<string, MediaItem>
  sort: Sort
  chart: ChartData
  today: string
  onTrailer: (id: string, title: string) => void
}) {
  const { t, locale } = useI18n()
  const { from } = chart

  const groups: { key: string; id?: string; title: string; entries: SeasonEntry[] }[] = []

  if (sort === 'date') {
    const early = entries.filter((e) => e.starts < from)
    if (early.length) groups.push({ key: 'before', title: t.seasons.before, entries: early })

    const weekly = new Map<string, SeasonEntry[]>()
    for (const entry of entries.filter((e) => e.starts >= from)) {
      const monday = mondayOf(entry.starts)
      weekly.set(monday, [...(weekly.get(monday) ?? []), entry])
    }
    for (const [monday, list] of weekly) {
      const first = monday < from ? from : monday
      groups.push({
        key: monday,
        id: `week-${monday}`,
        title: t.seasons.week(formatDay(first, locale, { day: 'numeric', month: 'long' })),
        entries: list,
      })
    }
  } else {
    for (const kind of KINDS) {
      const list = entries.filter((e) => e.kind === kind)
      if (list.length) groups.push({ key: kind, title: t.seasons.kinds[kind], entries: list })
    }
  }

  return (
    <div className="mt-12 space-y-14">
      {groups.map((group) => (
        <section
          key={group.key}
          id={group.id}
          tabIndex={group.id ? -1 : undefined}
          aria-labelledby={`season-group-${group.key}`}
          className="scroll-mt-24 outline-none"
        >
          <div className="mb-5 flex items-baseline gap-3 border-b border-rule pb-2">
            <h2 id={`season-group-${group.key}`} className="font-display text-2xl font-medium text-bone first-letter:uppercase">
              {group.title}
            </h2>
            <span className="font-mono text-xs text-bone-faint tabular-nums">{group.entries.length}</span>
          </div>
          <ul className="stagger grid gap-4 lg:grid-cols-2">
            {group.entries.map((entry) => (
              <li key={`${entry.workId}-${entry.seasonNumber ?? ''}`}>
                <ProgrammeCard entry={entry} work={works.get(entry.workId)!} chart={chart} today={today} onTrailer={onTrailer} />
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  )
})

/** One work of the programme: what it is, when it opens, what it is about. */
function ProgrammeCard({
  entry,
  work,
  chart,
  today,
  onTrailer,
}: {
  entry: SeasonEntry
  work: MediaItem
  chart: ChartData
  today: string
  onTrailer: (id: string, title: string) => void
}) {
  const { t, locale } = useI18n()
  const rating = headlineRating(work.ratings)
  const art = poster(work)

  const to =
    entry.seasonNumber !== undefined && (entry.kind === 'newSeason' || entry.kind === 'continuing')
      ? `/work/${work.id}/season/${entry.seasonNumber}`
      : `/work/${work.id}`

  // A day of another year than the season's says which: a series that began
  // the autumn before, an episode due after New Year.
  const short = (day: string) =>
    formatDay(day, locale, {
      weekday: 'short',
      day: 'numeric',
      month: 'short',
      year: day.slice(0, 4) === String(chart.year) ? undefined : 'numeric',
    })
  const when = (day: string) => [short(day), around(day, locale)].filter(Boolean).join(' · ')

  const opens =
    entry.kind === 'continuing'
      ? t.seasons.began(short(entry.starts))
      : entry.kind === 'film'
        ? (entry.starts >= today ? t.seasons.releases : t.seasons.released)(when(entry.starts))
        : (entry.starts >= today ? t.seasons.premieres : t.seasons.premiered)(when(entry.starts))

  const badge =
    entry.kind === 'film'
      ? t.seasons.badges.film
      : entry.kind === 'continuing'
        ? t.seasons.badges.continuing
        : entry.kind === 'newSeries'
          ? t.seasons.badges.newSeries
          : t.seasons.badges.season(entry.seasonNumber ?? 0)

  // That every episode is out is news while the season is: said of every
  // card of a season long past, it would only be noise.
  const running = chart.from <= today && today <= chart.to
  const done = entry.episodes !== undefined && Boolean(entry.aired) && entry.aired! >= entry.episodes
  const facts = [
    entry.episodes !== undefined
      ? entry.aired && entry.aired < entry.episodes
        ? t.seasons.aired(entry.aired, entry.episodes)
        : t.seasons.episodes(entry.episodes)
      : undefined,
    done && running ? t.seasons.finished : undefined,
    entry.kind === 'film' ? fmt.runtime(work.runtime, locale) : undefined,
  ].filter(Boolean)

  return (
    // The title is the card's one link, stretched over the whole of it: one
    // stop for a keyboard and one name for a screen reader, with the genres
    // and the trailer raised above it to stay their own.
    // On a phone the synopsis and what follows it run under the poster too,
    // at the card's full width rather than a column of a few words.
    <article className="group relative grid h-full grid-cols-[5.5rem_minmax(0,1fr)] grid-rows-[auto_1fr] gap-x-4 gap-y-3 rounded-panel border border-rule bg-ink-raised p-3 transition-colors duration-150 hover:border-rule-bright hover:bg-ink-high sm:grid-cols-[7rem_minmax(0,1fr)] sm:gap-y-0 sm:p-4">
      <div className="self-start overflow-hidden rounded-card border border-rule bg-ink-high sm:row-span-2">
        {art ? (
          <Artwork url={art} role="thumb" alt="" className="aspect-2/3 w-full object-cover" />
        ) : (
          <div className="grid aspect-2/3 place-items-center">
            <Glyph name={work.kind === 'series' ? 'tv' : 'film'} className="size-5 text-bone-faint" />
          </div>
        )}
      </div>

      <div className="min-w-0">
        <div className="flex items-start gap-2">
          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1">
            <Chip tone={entry.kind === 'continuing' ? 'provider' : 'accent'}>{badge}</Chip>
            {work.network ?? work.studio ? (
              <span className="truncate text-xs text-bone-faint">{work.network ?? work.studio}</span>
            ) : null}
          </div>
          {rating?.value ? <Score value={rating.value} votes={rating.votes} size="sm" /> : null}
        </div>

        <h3 className="mt-2 font-display text-xl leading-snug font-medium text-bone">
          <Link
            to={to}
            className="transition-colors duration-150 after:absolute after:inset-0 after:rounded-panel after:content-[''] group-hover:text-vermillion focus-visible:outline-none focus-visible:after:outline-2 focus-visible:after:outline-offset-2 focus-visible:after:outline-vermillion"
          >
            {work.title}
          </Link>
        </h3>

        <p className="mt-1 text-sm text-bone-dim">{opens}</p>
        {facts.length ? (
          <p className="mt-0.5 font-mono text-xs text-bone-faint tabular-nums">{facts.join(' · ')}</p>
        ) : null}
        {entry.nextEpisode ? (
          <p className="mt-1 inline-flex items-center gap-1.5 text-xs text-vermillion">
            <span aria-hidden className="size-1.5 rounded-full bg-vermillion" />
            {t.seasons.nextEpisode(entry.nextEpisode.episodeNumber, when(entry.nextEpisode.airDate))}
          </p>
        ) : null}
      </div>

      <div className="col-span-2 flex min-w-0 flex-col sm:col-span-1 sm:col-start-2">
        {work.overview ? (
          <p className="line-clamp-3 text-sm leading-relaxed text-bone-dim sm:mt-3">{work.overview}</p>
        ) : null}

        {/* The trailer keeps its corner however many rows the genres take. */}
        <div className="mt-auto flex items-end gap-3 pt-3">
          <div className="flex min-w-0 flex-1 flex-wrap gap-1.5">
            {/* Named as the filters above name them. */}
            {listedGenres(work.genres).slice(0, 3).map((genre) => (
              <Genre key={genre} name={genre} to={`/browse?genre=${encodeURIComponent(genre)}`} className="relative z-10" />
            ))}
          </div>
          {work.trailerYoutubeId ? (
            <Button
              size="sm"
              variant="quiet"
              className="relative z-10 shrink-0"
              onClick={() => onTrailer(work.trailerYoutubeId!, work.title)}
              aria-label={t.seasons.trailerOf(work.title)}
            >
              <Glyph name="play" className="size-3.5" />
              {t.trailer.play}
            </Button>
          ) : null}
        </div>
      </div>
    </article>
  )
}

// ─── what else premieres ─────────────────────────────────────────────────────

/**
 * What TMDB lists for the season that the catalogue does not hold, for
 * whoever maintains it: the same kind and language asked for above, and a way
 * to import each.
 */
function Candidates({ at, filters: f }: { at: SeasonRef; filters: Filters }) {
  const { t, lang } = useI18n()
  const queryClient = useQueryClient()
  // By kind and id: TMDB numbers its series and its films apart, and a series
  // can share a number with a film.
  const [imported, setImported] = useState<Record<string, string>>({})
  const keyOf = (c: SeasonCandidate) => `${c.kind}-${c.tmdbId}`

  // New seasons and what carries on are the catalogue's own works: TMDB's
  // list is of premieres.
  const wanted = !f.type || f.type === 'newSeries' || f.type === 'film'

  const listed = useQuery({
    queryKey: ['season-candidates', at.year, at.season, f.language, lang],
    queryFn: () =>
      api.get<SeasonCandidate[]>(
        `/seasons/${at.year}/${at.season}/candidates${query({ language: lang, originalLanguage: f.language })}`,
      ),
    enabled: wanted,
    retry: false,
    // Asked again each time the panel is drawn — the server keeps TMDB's
    // list, and says afresh which of it the catalogue now holds — so what was
    // imported from here is not offered again on the way back.
    staleTime: 0,
  })

  // Several can be on their way at once, each its own.
  const [state, setState] = useState<Record<string, 'busy' | 'failed'>>({})
  const mark = (c: SeasonCandidate, to?: 'busy' | 'failed') =>
    setState(({ [keyOf(c)]: _, ...rest }) => (to ? { ...rest, [keyOf(c)]: to } : rest))

  const bring = useMutation({
    mutationFn: (c: SeasonCandidate) => api.post<MediaItem>('/discover/import', { kind: c.kind, tmdbId: c.tmdbId }),
    onMutate: (c) => mark(c, 'busy'),
    onSuccess: (item, c) => {
      mark(c)
      setImported((was) => ({ ...was, [keyOf(c)]: item.id }))
      // The work is in the catalogue now: in this season, in its lists and
      // counts, and on the front page.
      for (const queryKey of [['season', at.year, at.season], ['items'], ['stats'], ['home']]) {
        void queryClient.invalidateQueries({ queryKey })
      }
    },
    onError: (_, c) => mark(c, 'failed'),
  })

  // No TMDB key: nothing to offer, and nothing to say about it here.
  if (!wanted || (listed.error instanceof ApiError && listed.error.status === 503)) return null

  const shown = (listed.data ?? []).filter(
    (c) => !f.type || (f.type === 'film' ? c.kind === 'movie' : c.kind === 'series'),
  )

  return (
    <section aria-labelledby="season-candidates" className="mt-16 rounded-plate border border-brass-deep/60 bg-brass/[0.03] p-5 sm:p-6">
      <div className="flex flex-wrap items-baseline gap-3">
        <Glyph name="discover" className="size-4 self-center text-brass" />
        <h2 id="season-candidates" className="font-display text-2xl font-medium text-bone">
          {t.seasons.candidates.title}
        </h2>
        {listed.isSuccess ? (
          <span className="font-mono text-xs text-bone-faint tabular-nums">{shown.length}</span>
        ) : null}
      </div>
      <p className="mt-2 max-w-[70ch] text-sm leading-relaxed text-bone-dim">{t.seasons.candidates.lead}</p>

      {listed.isPending ? (
        <div className="mt-5 grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
          {Array.from({ length: 6 }, (_, i) => (
            <Skeleton key={i} className="h-28 w-full" />
          ))}
        </div>
      ) : listed.isError ? (
        <p role="status" className="mt-4 text-sm text-bone-faint">
          {t.seasons.candidates.unavailable}
        </p>
      ) : shown.length === 0 ? (
        <p className="mt-4 text-sm text-bone-faint">{t.seasons.candidates.none}</p>
      ) : (
        // Series and films apart, as the programme above has them.
        (['series', 'movie'] as const).map((kind) => {
          const list = shown.filter((c) => c.kind === kind)
          if (!list.length) return null
          return (
            <section key={kind} aria-labelledby={`season-candidates-${kind}`} className="mt-6">
              <h3 id={`season-candidates-${kind}`} className="label flex items-baseline gap-2">
                {kind === 'series' ? t.seasons.candidates.seriesGroup : t.seasons.candidates.filmGroup}
                <span className="tabular-nums">{list.length}</span>
              </h3>
              <ul className="mt-3 grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
                {list.map((c) => (
                  <CandidateCard
                    key={keyOf(c)}
                    candidate={c}
                    imported={imported[keyOf(c)]}
                    busy={state[keyOf(c)] === 'busy'}
                    failed={state[keyOf(c)] === 'failed'}
                    onImport={() => bring.mutate(c)}
                  />
                ))}
              </ul>
            </section>
          )
        })
      )}
    </section>
  )
}

function CandidateCard({
  candidate: c,
  imported,
  busy,
  failed,
  onImport,
}: {
  candidate: SeasonCandidate
  imported?: string
  busy: boolean
  failed: boolean
  onImport: () => void
}) {
  const { t, locale } = useI18n()
  return (
    <li className="flex gap-3 rounded-panel border border-rule bg-ink-raised p-3">
      <div className="w-14 shrink-0 self-start overflow-hidden rounded-card border border-rule bg-ink-high">
        {c.poster ? (
          <Artwork url={c.poster} role="thumb" sizes="56px" alt="" className="aspect-2/3 w-full object-cover" />
        ) : (
          <div className="grid aspect-2/3 place-items-center">
            <Glyph name={c.kind === 'series' ? 'tv' : 'film'} className="size-4 text-bone-faint" />
          </div>
        )}
      </div>
      <div className="flex min-w-0 flex-1 flex-col">
        <p className="truncate text-sm font-medium text-bone" title={c.title}>
          {c.title}
        </p>
        <p className="mt-0.5 truncate font-mono text-[0.6875rem] text-bone-faint">
          {[
            c.premiere
              ? new Intl.DateTimeFormat(locale, { day: 'numeric', month: 'short', timeZone: 'UTC' }).format(
                  new Date(`${c.premiere}T00:00:00Z`),
                )
              : t.seasons.candidates.noDate,
            c.originalLanguage ? languageName(c.originalLanguage, locale) : undefined,
          ]
            .filter(Boolean)
            .join(' · ')}
        </p>
        {c.overview ? <p className="mt-1 line-clamp-2 text-xs leading-relaxed text-bone-dim">{c.overview}</p> : null}
        <div className="mt-auto flex flex-wrap items-center gap-2 pt-2">
          {imported ? (
            <Link
              to={`/work/${imported}`}
              className="hit inline-flex min-h-9 items-center gap-1.5 text-sm text-moss transition-colors duration-150 hover:text-bone"
            >
              <Glyph name="check" className="size-3.5" />
              {t.seasons.candidates.imported} · {t.seasons.candidates.open}
            </Link>
          ) : (
            <Button size="sm" disabled={busy} onClick={onImport} aria-label={`${t.seasons.candidates.import}: ${c.title}`}>
              <Glyph name="download" className="size-3.5" />
              {busy ? t.seasons.candidates.importing : t.seasons.candidates.import}
            </Button>
          )}
          {/* Its page on TMDB, to judge it by before bringing it in. */}
          <ExternalLink
            href={identifierLink('tmdb', c.tmdbId, c.kind)!}
            className="hit ml-auto inline-flex min-h-9 items-center text-xs text-bone-faint"
          >
            TMDB
          </ExternalLink>
          {failed ? (
            <span role="alert" className="basis-full text-xs text-vermillion">
              {t.seasons.candidates.failed}
            </span>
          ) : null}
        </div>
      </div>
    </li>
  )
}

function ProgrammeSkeleton() {
  return (
    <div className="mt-10 space-y-8">
      <Skeleton className="h-28 w-full" />
      <div className="flex gap-2">
        {Array.from({ length: 4 }, (_, i) => (
          <Skeleton key={i} className="h-11 w-32 rounded-full" />
        ))}
      </div>
      <div className="grid gap-4 lg:grid-cols-2">
        {Array.from({ length: 6 }, (_, i) => (
          <Skeleton key={i} className="h-52 w-full" />
        ))}
      </div>
    </div>
  )
}
