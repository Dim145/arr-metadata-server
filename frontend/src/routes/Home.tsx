/**
 * The way in.
 *
 * A catalogue's front page should show the catalogue, not explain it: one work
 * given the room a poster deserves, then rows of everything else. The featured
 * work is whichever the providers rank highest, which is as close to an opinion
 * as this server is willing to have.
 */

import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router'

import { Artwork, Placeholder, PosterCard, PosterShelf, Score } from '../components/media'
import { EmptyState, Genre, Glyph, Label, SectionTitle, Skeleton } from '../components/ui'
import { useMe } from '../lib/hooks'
import { api, query } from '../lib/api'
import * as fmt from '../lib/format'
import { useI18n } from '../lib/i18n'
import { airTime, backdrop, episodeCode, headlineRating, logo, poster } from '../lib/media'
import { forget, useRecent } from '../lib/recent'
import { chartQuery, formatDay, seasonOf, seasonPath } from '../lib/seasons'
import type { Airing, Calendar, ItemPage, MediaItem } from '../lib/types'

export function Home() {
  const { t, lang } = useI18n()

  const series = useQuery({
    queryKey: ['home', 'series', lang],
    queryFn: () => api.get<ItemPage>(`/items${query({ kind: 'series', limit: 12, language: lang })}`),
  })

  const films = useQuery({
    queryKey: ['home', 'movie', lang],
    queryFn: () => api.get<ItemPage>(`/items${query({ kind: 'movie', limit: 12, language: lang })}`),
  })

  const added = useQuery({
    queryKey: ['home', 'added', lang],
    queryFn: () => api.get<ItemPage>(`/items${query({ sort: 'added', limit: 12, language: lang })}`),
  })

  const loading = series.isPending || films.isPending
  const everything = [...(series.data?.items ?? []), ...(films.data?.items ?? [])]
  // The work at the top: one of the most followed series and films, a
  // different one each day rather than the same one for ever — the same one
  // all day, so a page left open and reloaded does not shuffle.
  const heroes = [...(series.data?.items ?? []).slice(0, 4), ...(films.data?.items ?? []).slice(0, 4)].filter(
    (item) => backdrop(item),
  )
  const now = new Date()
  const dayOfYear = Math.floor((now.getTime() - new Date(now.getFullYear(), 0, 0).getTime()) / 86_400_000)
  const featured = heroes.length ? heroes[dayOfYear % heroes.length] : everything[0]

  if (loading) {
    return <HomeSkeleton />
  }

  if (series.isError || films.isError) {
    return (
      <div className="pt-16">
        <EmptyState title={t.home.loadFailed} />
      </div>
    )
  }

  if (!everything.length) {
    return <FirstVisit />
  }

  return (
    <div className="pb-8">
      {featured ? <Featured item={featured} /> : null}

      <div className="mt-16 space-y-16">
        <ThisWeek />

        <RecentlyViewed />

        <ThisSeason />

        {series.data?.items.length ? (
          <Row
            title={t.home.allSeries}
            to="/browse?kind=series"
            items={series.data.items}
            total={series.data.total}
          />
        ) : null}

        {films.data?.items.length ? (
          <Row
            title={t.home.allFilms}
            to="/browse?kind=movie"
            items={films.data.items}
            total={films.data.total}
          />
        ) : null}

        {added.data?.items.length ? (
          <Row
            title={t.home.recentlyAdded}
            to="/browse?order=added"
            items={added.data.items}
            total={added.data.total}
            mixed
          />
        ) : null}
      </div>
    </div>
  )
}

/**
 * The work at the top.
 *
 * The backdrop is laid edge to edge and cut with a hard gradient rather than a
 * blur, so the text sits on ink instead of on a smeared image — cheaper to
 * paint and far easier to read.
 */
function Featured({ item }: { item: MediaItem }) {
  const { t, locale } = useI18n()
  const art = backdrop(item)
  const sheet = poster(item)
  const mark = logo(item)
  const rating = headlineRating(item.ratings)

  return (
    <section className="relative mx-[calc(50%-50vw)] w-screen">
      <div className="relative h-[19rem] overflow-hidden sm:h-[24rem] lg:h-[28rem]">
        {art ? (
          <Artwork
            url={art}
            role="backdrop"
            eager
            alt=""
            aria-hidden
            fetchPriority="high"
            className="fade-in size-full object-cover object-top"
          />
        ) : (
          <div className="size-full bg-ink-raised" />
        )}

        {/* Solid where the text sits, then a flat wash the whole way across —
            not a fade to clear. A gradient that lifts near the right edge puts
            body copy on whatever the photograph happens to be doing there, and
            on a bright frame that is unreadable. The backdrop is texture. */}
        <div className="absolute inset-0 bg-[linear-gradient(to_right,var(--color-ink)_0,var(--color-ink)_22%,color-mix(in_srgb,var(--color-ink)_86%,transparent)_52%,color-mix(in_srgb,var(--color-ink)_86%,transparent)_100%)]" />
        <div className="absolute inset-x-0 bottom-0 h-32 bg-gradient-to-t from-ink to-transparent" />
      </div>

      <div className="absolute inset-0">
        <div className="mx-auto flex h-full max-w-7xl items-end px-4 pb-8 sm:px-6">
          <div className="flex items-end gap-5">
            {sheet ? (
              <Artwork
                url={sheet}
                role="poster"
                eager
                alt={t.a11y.poster(item.title)}
                className="strike hidden w-28 rounded-plate border border-rule-bright shadow-[var(--shadow-plate)] sm:block lg:w-36"
                fallback={
                  <Placeholder
                    title={item.title}
                    kind={item.kind}
                    label={t.a11y.poster(item.title)}
                    className="hidden aspect-2/3 w-28 rounded-plate border border-rule-bright shadow-[var(--shadow-plate)] sm:grid lg:w-36"
                  />
                }
              />
            ) : null}

            <div className="strike max-w-2xl" style={{ animationDelay: '80ms' }}>
              <Label>{item.kind === 'series' ? t.nav.series : t.nav.films}</Label>

              {/* The work's own mark where it has one, as a cinema's front
                  does: the title stays the heading, for whoever reads rather
                  than looks. A light hairline round it keeps a dark logo from
                  sinking into the ink. */}
              {mark ? (
                <>
                  <h1 className="sr-only">{item.title}</h1>
                  <Artwork
                    url={mark}
                    role="logo"
                    eager
                    alt=""
                    className="mt-3 max-h-20 w-auto max-w-[min(100%,26rem)] object-contain object-left [filter:drop-shadow(0_0_1px_rgb(255_255_255/0.55))_drop-shadow(0_8px_18px_rgb(0_0_0/0.65))] sm:max-h-28 lg:max-h-36"
                  />
                </>
              ) : (
                <h1 className="mt-2 font-display text-3xl leading-[1.05] font-medium text-bone sm:text-4xl lg:text-5xl">
                  {item.title}
                </h1>
              )}

              <div className="mt-4 flex flex-wrap items-center gap-x-4 gap-y-2">
                {item.year ? (
                  <span className="font-mono text-sm text-bone-dim tabular-nums">{item.year}</span>
                ) : null}
                {fmt.runtime(item.runtime, locale) ? (
                  <span className="text-sm text-bone-dim">{fmt.runtime(item.runtime, locale)}</span>
                ) : null}
                {item.genres.slice(0, 3).map((genre) => (
                  <Genre key={genre} name={genre} />
                ))}
                {/* On a phone the score ring made the line wrap; the page it
                    opens says it. */}
                {rating?.value ? (
                  <span className="hidden sm:block">
                    <Score value={rating.value} size="sm" />
                  </span>
                ) : null}
              </div>

              {item.overview ? (
                <p className="mt-4 line-clamp-2 max-w-xl text-sm leading-relaxed text-bone-dim sm:line-clamp-3">
                  {item.overview}
                </p>
              ) : null}

              <Link
                to={`/work/${item.id}`}
                className="mt-5 inline-flex min-h-11 items-center gap-2 rounded-full bg-[linear-gradient(135deg,var(--color-vermillion),color-mix(in_srgb,var(--color-vermillion)_70%,var(--color-brass)))] px-5 text-sm font-medium text-ink shadow-[var(--shadow-lift)] transition-[filter] duration-200 hover:brightness-110"
              >
                {t.home.open}
                <Glyph name="chevronRight" className="size-4" />
              </Link>
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}

function Row({
  title,
  to,
  items,
  total,
  mixed = false,
}: {
  title: string
  to: string
  items: MediaItem[]
  total: number
  /** Series and films together, so each card says which it is. */
  mixed?: boolean
}) {
  const { t, locale } = useI18n()

  return (
    <section>
      <SectionTitle
        action={
          <Link
            to={to}
            className="flex min-h-11 items-center gap-1.5 text-sm text-bone-dim transition-colors duration-200 hover:text-vermillion"
          >
            {t.home.seeAll}
            <span className="font-mono text-xs text-bone-faint tabular-nums">
              {fmt.count(total, locale)}
            </span>
            <Glyph name="chevronRight" className="size-3.5" />
          </Link>
        }
      >
        {title}
      </SectionTitle>

      <PosterShelf label={title}>
        {items.map((item) => (
          <PosterCard key={item.id} item={item} to={`/work/${item.id}`} kind={mixed} />
        ))}
      </PosterShelf>
    </section>
  )
}

/**
 * The next seven days, as a strip of frames: what is on, when, in the
 * reader's own time. Left out when nothing is.
 */
function ThisWeek() {
  const { t, lang, locale } = useI18n()

  // Today and the six days after it, as the reader's calendar has them — and
  // the same dates at midnight UTC, where an episode with only a date is
  // stored: from this hour on, tonight's episode was already "past".
  const today = new Date()
  today.setHours(0, 0, 0, 0)
  const end = new Date(today)
  end.setDate(today.getDate() + 7)
  const utcMidnight = (d: Date) => Date.UTC(d.getFullYear(), d.getMonth(), d.getDate())
  const from = new Date(Math.min(today.getTime(), utcMidnight(today))).toISOString()
  const to = new Date(Math.max(end.getTime(), utcMidnight(end))).toISOString()
  const first = fmt.localDay(today)
  const last = fmt.localDay(new Date(end.getTime() - 1))

  const week = useQuery({
    queryKey: ['home', 'week', first, lang],
    queryFn: () => api.get<Calendar>(`/calendar${query({ from, to, language: lang })}`),
  })

  const works = new Map((week.data?.works ?? []).map((w) => [w.id, w]))
  const episodes = (week.data?.episodes ?? [])
    .filter((a) => {
      const day = airTime(a.episode)?.day
      return day !== undefined && day >= first && day <= last
    })
    .slice(0, 16)

  if (!episodes.length) return null

  return (
    <section>
      <SectionTitle
        action={
          <Link
            to="/calendar"
            className="flex min-h-11 items-center gap-1.5 text-sm text-bone-dim transition-colors duration-200 hover:text-vermillion"
          >
            {t.home.fullSchedule}
            <Glyph name="chevronRight" className="size-3.5" />
          </Link>
        }
      >
        {t.home.thisWeek}
      </SectionTitle>

      <div
        role="region"
        aria-label={t.home.thisWeek}
        className="-mt-2 -mr-4 -ml-2 flex snap-x snap-mandatory scroll-pl-2 gap-3 overflow-x-auto pt-2 pr-4 pb-2 pl-2 sm:-mr-6 sm:pr-6"
      >
        {episodes.map((airing) => (
          <Upcoming key={`${airing.workId}-${airing.episode.id}`} airing={airing} work={works.get(airing.workId)} locale={locale} />
        ))}
      </div>
    </section>
  )
}

/**
 * What was opened lately, from this browser's own memory: the way back to
 * the work left half-read, before the rows of everything else. Nothing when
 * there is nothing — a shelf that says "nothing yet" is a shelf for its own
 * sake.
 */
function RecentlyViewed() {
  const { t } = useI18n()
  const recent = useRecent()

  if (!recent.length) return null

  return (
    <section>
      <SectionTitle
        action={
          <button
            type="button"
            onClick={forget}
            title={t.home.recentHint}
            className="flex min-h-11 cursor-pointer items-center gap-1.5 text-sm text-bone-dim transition-colors duration-200 hover:text-vermillion"
          >
            <Glyph name="close" className="size-3.5" />
            {t.home.recentClear}
          </button>
        }
      >
        {t.home.recent}
      </SectionTitle>

      <PosterShelf label={t.home.recent}>
        {recent.map((work) => (
          <PosterCard
            key={work.id}
            to={`/work/${work.id}`}
            kind
            // As much of the work as the shelf kept: enough for a card.
            item={{
              id: work.id,
              title: work.title,
              kind: work.kind,
              year: work.year,
              images: work.poster
                ? [{ id: `recent-${work.id}`, coverType: 'poster', url: work.poster, sortOrder: 0, isManual: false }]
                : [],
              ratings: work.rating ? [work.rating] : [],
              isManual: false,
              externalIds: {},
            }}
          />
        ))}
      </PosterShelf>
    </section>
  )
}

/**
 * What the season brings: its new series, the series back for another
 * season, its films, the most followed first. Left out when it brings nothing
 * the catalogue holds.
 */
function ThisSeason() {
  const { t, lang, locale } = useI18n()
  const at = seasonOf(new Date())

  // The season page's own query, so going there from here costs nothing.
  const chart = useQuery(chartQuery(at, lang))

  const works = new Map((chart.data?.works ?? []).map((w) => [w.id, w]))
  const held = (chart.data?.entries ?? []).filter((e) => works.has(e.workId))
  const fresh = held
    .filter((e) => e.kind !== 'continuing')
    .sort((a, b) => (works.get(b.workId)!.popularity ?? -1) - (works.get(a.workId)!.popularity ?? -1))

  if (!fresh.length) return null

  const day = (value: string) => formatDay(value, locale, { day: 'numeric', month: 'short' })

  return (
    <section>
      <SectionTitle
        action={
          <Link
            to={seasonPath(at)}
            className="flex min-h-11 items-center gap-1.5 text-sm text-bone-dim transition-colors duration-200 hover:text-vermillion"
          >
            {t.home.wholeSeason}
            {/* The season's works, as its page counts them. */}
            <span className="font-mono text-xs text-bone-faint tabular-nums">
              {fmt.count(new Set(held.map((e) => e.workId)).size, locale)}
            </span>
            <Glyph name="chevronRight" className="size-3.5" />
          </Link>
        }
      >
        {t.home.thisSeason(at.season, at.year)}
      </SectionTitle>

      <PosterShelf label={t.home.thisSeason(at.season, at.year)}>
        {fresh.slice(0, 12).map((entry) => {
          const work = works.get(entry.workId)!
          const kind =
            entry.kind === 'film'
              ? t.seasons.badges.film
              : entry.kind === 'newSeries'
                ? t.seasons.badges.newSeries
                : t.seasons.badges.season(entry.seasonNumber ?? 0)
          return (
            <PosterCard
              key={`${entry.workId}-${entry.seasonNumber ?? ''}`}
              item={work}
              to={
                entry.kind === 'newSeason' && entry.seasonNumber !== undefined
                  ? `/work/${work.id}/season/${entry.seasonNumber}`
                  : `/work/${work.id}`
              }
              note={`${kind} · ${day(entry.starts)}`}
            />
          )
        })}
      </PosterShelf>
    </section>
  )
}

function Upcoming({ airing, work, locale }: { airing: Airing; work?: MediaItem; locale: string }) {
  const { t } = useI18n()
  const { episode } = airing
  const image = episode.image ?? (work ? backdrop(work) : undefined)
  const when = airTime(episode)
  // Today and tomorrow in words; after that the weekday, which within a week
  // says which day without a date.
  const soon = when && when.day <= fmt.localDay(new Date(Date.now() + 86_400_000))
  const day = !when
    ? undefined
    : soon
      ? fmt.dayDistance(when.day, locale)
      : new Intl.DateTimeFormat(locale, { weekday: 'short', timeZone: 'UTC' }).format(new Date(`${when.day}T00:00:00Z`))
  const time = when?.moment ? fmt.clock(when.moment.toISOString(), locale) : undefined

  return (
    <Link
      to={`/work/${airing.workId}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`}
      className="group w-64 shrink-0 snap-start overflow-hidden rounded-panel border border-rule bg-ink-raised transition-colors duration-150 hover:border-rule-bright"
    >
      <div className="relative aspect-video bg-ink-high">
        {image ? (
          <Artwork url={image} role="still" alt="" className="size-full object-cover" />
        ) : (
          <div className="grid size-full place-items-center">
            <Glyph name="tv" className="size-5 text-bone-faint" />
          </div>
        )}
        <span className="absolute top-1.5 left-1.5 rounded-card bg-ink/85 px-1.5 py-0.5 font-mono text-[0.6875rem] font-medium text-bone tabular-nums first-letter:uppercase">
          {[day, time].filter(Boolean).join(' · ')}
        </span>
      </div>
      <div className="space-y-0.5 p-3">
        <p className="truncate text-sm font-medium text-bone transition-colors duration-150 group-hover:text-vermillion">
          {work?.title ?? '—'}
        </p>
        <p className="truncate text-xs text-bone-faint">
          <span className="font-mono">{episodeCode(episode)}</span> · {episode.title || t.episode.untitled(episode.episodeNumber)}
        </p>
      </div>
    </Link>
  )
}

function HomeSkeleton() {
  return (
    <div className="pb-8">
      <div className="mx-[calc(50%-50vw)] h-[19rem] w-screen sm:h-[24rem] lg:h-[28rem]">
        <Skeleton className="size-full rounded-none" />
      </div>
      <div className="mt-16 space-y-4">
        <Skeleton className="h-7 w-40" />
        <PosterShelf>
          {Array.from({ length: 6 }, (_, index) => (
            <div key={index} aria-hidden className="space-y-2.5">
              <Skeleton className="aspect-2/3 w-full" />
              <Skeleton className="h-4 w-3/4" />
            </div>
          ))}
        </PosterShelf>
      </div>
    </div>
  )
}

/**
 * The catalogue before anything is in it: what this server is for, and the
 * three things that fill it, said once, on the page a first visit lands on.
 * Whoever may write is pointed at the screens; a reader is told who does.
 */
function FirstVisit() {
  const { t } = useI18n()
  const me = useMe()
  const writes = me.data?.canWrite === true
  const administers = me.data?.isAdmin === true
  const steps = [
    writes
      ? null
      : {
          key: 'signIn',
          title: t.home.first.signIn,
          body: t.home.first.signInHint,
          to: '/login',
          label: t.home.first.signInAction,
        },
    {
      key: 'import',
      title: t.home.first.import,
      body: t.home.first.importHint,
      to: writes ? '/admin/discover' : undefined,
      label: t.home.first.importAction,
    },
    {
      key: 'connect',
      title: t.home.first.connect,
      body: t.home.first.connectHint,
      // The allowlist is the administrator's; a writer who is not one is
      // told what to do, not where.
      to: administers ? '/admin/clients' : undefined,
      label: t.home.first.connectAction,
    },
  ].filter((step) => step !== null)

  return (
    <div className="pt-16 pb-12">
      <header className="rise mx-auto max-w-2xl text-center">
        <Glyph name="reel" className="mx-auto size-10 text-vermillion" />
        <h1 className="mt-5 font-display text-3xl font-medium text-bone sm:text-4xl">{t.home.first.title}</h1>
        <p className="mt-3 text-sm leading-relaxed text-bone-dim">{t.home.first.lead}</p>
      </header>
      {/* Who is looking decides the steps; until that is known, none are
          shown rather than shown and renumbered. */}
      {me.isPending ? null : (
      <ol className="stagger mx-auto mt-10 grid max-w-4xl gap-4 sm:grid-cols-3">
        {steps.map((step, index) => (
          <li key={step.key} className="plate flex flex-col gap-2 rounded-panel border border-rule bg-ink-raised p-5">
            <span className="font-mono text-xs text-vermillion tabular-nums">{String(index + 1).padStart(2, '0')}</span>
            <h2 className="font-display text-lg text-bone">{step.title}</h2>
            <p className="text-sm leading-relaxed text-bone-dim">{step.body}</p>
            {step.to ? (
              <Link
                to={step.to}
                className="mt-auto inline-flex min-h-11 items-center gap-1.5 pt-2 text-sm text-vermillion underline-offset-4 hover:underline"
              >
                {step.label}
                <Glyph name="chevronRight" className="size-3.5" />
              </Link>
            ) : null}
          </li>
        ))}
      </ol>
      )}
      {writes ? (
        <p className="mx-auto mt-8 max-w-2xl text-center text-xs text-bone-faint">
          {t.home.first.docs}{' '}
          <a href="/api/docs" className="underline underline-offset-2 transition-colors duration-150 hover:text-bone">
            /api/docs
          </a>
        </p>
      ) : null}
    </div>
  )
}
