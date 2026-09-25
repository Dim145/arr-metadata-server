/**
 * One season, as a contact sheet.
 *
 * Every episode at once, in order, each with its frame, its edge code and
 * everything known about it — the page a work's season strip only hints at.
 * It reads the work the work's own page already fetched, so moving between
 * the two costs nothing.
 *
 * The code on each still is set like the edge print on a strip of film: the
 * one piece of the page that is the same in every language and every client,
 * and the one somebody matching a release to an episode is looking for.
 */

import { Link, useParams, useSearchParams } from 'react-router'

import { Elsewhere, Trail } from '../components/elsewhere'
import { Placement, finaleLabel } from '../components/episodes'
import { Artwork } from '../components/media'
import { Chip, EmptyState, Glyph, Skeleton } from '../components/ui'
import { ApiError } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useMe, useOrders, useTitle, useWork } from '../lib/hooks'
import { useI18n } from '../lib/i18n'
import { seasonLinks } from '../lib/links'
import {
  adjacentSeasons,
  airTime,
  airValue,
  airsWhen,
  episodeCode,
  episodesOf,
  hasAired,
  seasonPoster,
  seasonName,
  seasonNumbers,
} from '../lib/media'
import type { Episode, EpisodeOrder, MediaItem, PlacedEpisode } from '../lib/types'
import { NotFound, Unavailable } from './NotFound'

export function Season() {
  const { id = '', season = '' } = useParams()
  const [params] = useSearchParams()
  const number = Number(season)
  const work = useWork(id)
  const orders = useOrders(id)

  // Kept on screen through a refetch that failed; see the work's page.
  if (!work.data) {
    if (work.isPending) return <SeasonSkeleton />
    if (work.error instanceof ApiError && work.error.status === 404) return <NotFound work />
    return <Unavailable onRetry={() => void work.refetch()} />
  }

  const item = work.data
  // The order asked for, where the series is numbered that way. Not known
  // yet, the page waits rather than judge the season by the wrong numbers.
  const asked = params.get('order')
  if (asked && orders.isPending) return <SeasonSkeleton />
  const order = asked ? orders.data?.orders.find((o) => o.kind === asked) : undefined
  const numbers = order ? orderSeasons(order) : seasonNumbers(item)
  if (!Number.isInteger(number) || !numbers.includes(number)) {
    return <NotFound />
  }

  // Keyed by the season and the order, so the next one starts its images
  // afresh rather than inheriting whatever the last one's had to fall back to.
  return (
    <SeasonSheet
      key={`${number}:${order?.kind ?? ''}`}
      item={item}
      number={number}
      order={order}
      orders={orders.data?.orders ?? []}
    />
  )
}

/** The seasons an order has, specials last. */
function orderSeasons(order: EpisodeOrder): number[] {
  const numbers = [...new Set(order.episodes.map((e) => e.seasonNumber))]
  return numbers.sort((a, b) => (a === 0 ? 1 : b === 0 ? -1 : a - b))
}

/**
 * The work's episodes as an order places them in one of its seasons, each
 * with its place. An episode the work does not hold — a DVD extra TheTVDB
 * numbers and no provider aired — is left out.
 */
function placedIn(
  item: MediaItem,
  order: EpisodeOrder,
  season: number,
): { episode: Episode; placed: PlacedEpisode }[] {
  const byTvdb = new Map((item.episodes ?? []).filter((e) => e.tvdbId).map((e) => [e.tvdbId!, e]))
  return order.episodes
    .filter((p) => p.seasonNumber === season)
    .sort((a, b) => a.episodeNumber - b.episodeNumber)
    .flatMap((placed) => {
      const episode = byTvdb.get(placed.tvdbId)
      return episode ? [{ episode, placed }] : []
    })
}

function adjacent(numbers: number[], season: number): { before?: number; after?: number } {
  const at = numbers.indexOf(season)
  return {
    before: at > 0 ? numbers[at - 1] : undefined,
    after: at >= 0 ? numbers[at + 1] : undefined,
  }
}

function SeasonSheet({
  item,
  number,
  order,
  orders,
}: {
  item: MediaItem
  number: number
  /** The order the sheet is numbered in, where it is not the aired one. */
  order?: EpisodeOrder
  orders: EpisodeOrder[]
}) {
  const { t, locale } = useI18n()
  const me = useMe()

  // A season's own name and note belong to the aired order; another order's
  // seasons are only numbers.
  const meta = order ? undefined : item.seasons?.find((s) => s.seasonNumber === number)
  const rows = order
    ? placedIn(item, order, number)
    : episodesOf(item, number).map((episode) => ({ episode, placed: undefined }))
  const episodes = rows.map((r) => r.episode)
  const name = seasonName(meta?.title, number, t.work.season)
  const sheet = seasonPoster(item, number)
  const numbers = order ? orderSeasons(order) : seasonNumbers(item)
  const { before, after } = order ? adjacent(numbers, number) : adjacentSeasons(item, number)
  const suffix = order ? `?order=${encodeURIComponent(order.kind)}` : ''
  const orderLabel = (kind?: string) => (kind ? (t.season.order[kind] ?? kind) : t.season.order.official)
  useTitle(order ? `${name} · ${orderLabel(order.kind)}` : name, item.title)

  // The earliest date and the latest, whatever order the numbers put them in:
  // specials are numbered as they were found, not as they aired.
  const dates = episodes.map((e) => e.airDate).filter((d): d is string => !!d).sort()
  const first = dates[0] ?? meta?.airDate
  const last = dates.at(-1)
  const minutes = episodes.reduce((sum, e) => sum + (e.runtime ?? 0), 0)

  return (
    <article className="pt-8 pb-12">
      <Trail
        steps={[
          { label: t.nav.series, to: '/browse?kind=series' },
          { label: item.title, to: `/work/${item.id}` },
          { label: name },
        ]}
      />

      <header className="rise mt-4 flex flex-col gap-6 sm:flex-row sm:items-end">
        <div className="w-28 shrink-0 sm:w-36">
          {sheet ? (
            <Artwork
              url={sheet}
              role="poster"
              eager
              alt={t.a11y.poster(`${item.title} — ${name}`)}
              className="w-full rounded-plate border border-rule-bright shadow-[var(--shadow-plate)]"
            />
          ) : (
            <div className="grid aspect-2/3 w-full place-items-center rounded-plate border border-rule bg-ink-high">
              <Glyph name="tv" className="size-7 text-bone-faint" />
            </div>
          )}
        </div>

        <div className="min-w-0 flex-1">
          <Link
            to={`/work/${item.id}`}
            className="label inline-flex min-h-11 items-center transition-colors duration-150 hover:text-vermillion"
          >
            {item.title}
          </Link>
          <h1 className="font-display text-3xl leading-tight font-medium text-bone sm:text-4xl">{name}</h1>
          <p className="mt-2 font-mono text-xs text-bone-faint tabular-nums">
            {[
              order ? orderLabel(order.kind) : undefined,
              t.work.episodeCount(episodes.length),
              first ? (last && fmt.year(last) !== fmt.year(first) ? `${fmt.year(first)}–${fmt.year(last)}` : fmt.year(first)) : undefined,
              minutes ? t.season.watchTime(fmt.runtime(minutes, locale) ?? '') : undefined,
            ]
              .filter(Boolean)
              .join(' · ')}
          </p>
          {meta?.overview ? (
            <p className="mt-4 max-w-[68ch] text-[0.9375rem] leading-relaxed text-bone-dim">{meta.overview}</p>
          ) : null}

          <div className="mt-5 flex flex-wrap items-center gap-3">
            <Elsewhere links={seasonLinks(item, meta)} />
            {me.data?.canWrite ? (
              <Link
                to={`/admin/catalogue/${item.id}?season=${number}`}
                className="inline-flex min-h-11 items-center gap-1.5 rounded-full border border-brass-deep px-3.5 text-[0.8125rem] font-medium text-brass transition-colors duration-150 hover:bg-brass/10"
              >
                <Glyph name="pencil" className="size-3.5" />
                {t.season.edit}
              </Link>
            ) : null}
          </div>
        </div>
      </header>

      {/* Between seasons: the neighbours by name, and every season by number
          for a series with too many to walk through one at a time — as links,
          which a select that moved on every change was not: an arrow key on
          it opened another season each time it was pressed. */}
      <nav
        aria-label={t.season.seasons}
        className="rise mt-8 border-y border-rule py-3"
        style={{ animationDelay: '60ms' }}
      >
        {/* Where TheTVDB numbers the series more than one way, the way a
            reader knows it: the DVDs, straight through. The aired order stays
            the page's own, and the one every client is served. */}
        {orders.length ? (
          <div className="mb-3">
            <div role="group" aria-label={t.season.orders} className="flex flex-wrap items-center gap-1.5">
              <span className="label mr-1">{t.season.orders}</span>
              {[undefined, ...orders].map((o) => {
                const kind = o?.kind
                const current = order?.kind === kind
                const seasons = o ? orderSeasons(o) : seasonNumbers(item)
                const target = seasons.includes(number) ? number : seasons[0]
                return (
                  <Link
                    key={kind ?? 'official'}
                    to={`/work/${item.id}/season/${target}${kind ? `?order=${encodeURIComponent(kind)}` : ''}`}
                    aria-current={current ? 'page' : undefined}
                    className={cn(
                      'inline-flex min-h-11 items-center rounded-full border px-3 text-sm transition-colors duration-150',
                      current
                        ? 'border-vermillion bg-vermillion/15 text-bone'
                        : 'border-rule text-bone-dim hover:border-rule-bright hover:text-bone',
                    )}
                  >
                    {orderLabel(kind)}
                  </Link>
                )
              })}
            </div>
            {order ? <p className="mt-2 text-xs text-bone-faint">{t.season.orderNote}</p> : null}
          </div>
        ) : null}
        <div className="flex items-center justify-between gap-3">
          <SeasonStep item={item} number={before} direction="before" suffix={suffix} />
          <SeasonStep item={item} number={after} direction="after" suffix={suffix} />
        </div>
        {/* One row, scrolled where it is wider than the screen: wrapped, a
            phone put the specials alone on a second line. */}
        {numbers.length > 2 ? (
          <ol className="-mx-4 mt-2 flex items-center gap-1.5 overflow-x-auto px-4 py-1 [scrollbar-width:none] sm:mx-0 sm:justify-center sm:px-0">
            {numbers.map((n) => {
              const title = seasonName(item.seasons?.find((s) => s.seasonNumber === n)?.title, n, t.work.season)
              const current = n === number

              return (
                <li key={n} className="shrink-0">
                  <Link
                    to={`/work/${item.id}/season/${n}${suffix}`}
                    aria-current={current ? 'page' : undefined}
                    title={title}
                    className={cn(
                      'inline-flex min-h-11 min-w-11 items-center justify-center rounded-full border px-3 font-mono text-sm tabular-nums',
                      'transition-colors duration-150',
                      current
                        ? 'border-vermillion bg-vermillion/15 text-bone'
                        : 'border-rule text-bone-dim hover:border-rule-bright hover:text-bone',
                    )}
                  >
                    {/* The number on the pill, the season's whole name to a
                        screen reader — which begins with what is shown. */}
                    {n === 0 ? (
                      t.work.season(0)
                    ) : (
                      <>
                        <span className="sr-only">{t.season.jump} </span>
                        {n}
                        {title !== t.work.season(n) ? <span className="sr-only"> — {title}</span> : null}
                      </>
                    )}
                  </Link>
                </li>
              )
            })}
          </ol>
        ) : null}
      </nav>

      {rows.length ? (
        <ol className="stagger mt-8 space-y-4">
          {rows.map(({ episode, placed }) => (
            <EpisodeEntry key={episode.id} item={item} episode={episode} placed={placed} />
          ))}
        </ol>
      ) : (
        <EmptyState title={t.season.empty} hint={t.season.emptyHint} />
      )}

      <p className="mt-8 flex items-start gap-2 text-xs leading-relaxed text-bone-faint">
        <Glyph name="cloud" className="mt-0.5 size-3.5 shrink-0 text-slate" />
        {t.work.numbering} {t.season.localTime}
      </p>
    </article>
  )
}

function SeasonStep({
  item,
  number,
  direction,
  suffix = '',
}: {
  item: MediaItem
  number?: number
  direction: 'before' | 'after'
  /** The order the page is numbered in, carried along. */
  suffix?: string
}) {
  const { t } = useI18n()

  if (number === undefined) return <span className="hidden sm:block sm:w-40" />

  const name = seasonName(
    suffix ? undefined : item.seasons?.find((s) => s.seasonNumber === number)?.title,
    number,
    t.work.season,
  )

  return (
    <Link
      to={`/work/${item.id}/season/${number}${suffix}`}
      rel={direction === 'before' ? 'prev' : 'next'}
      className={cn(
        'group inline-flex min-h-11 items-center gap-2 rounded-full px-3 text-sm text-bone-dim',
        'transition-colors duration-150 hover:bg-ink-high hover:text-bone',
        direction === 'after' && 'flex-row-reverse',
      )}
    >
      <Glyph name={direction === 'before' ? 'chevronLeft' : 'chevronRight'} className="size-4" />
      <span>
        <span className="sr-only">{direction === 'before' ? t.season.previous : t.season.next} </span>
        {name}
      </span>
    </Link>
  )
}

/** One frame of the sheet. The whole row is the way to the episode's page. */
function EpisodeEntry({
  item,
  episode,
  placed,
}: {
  item: MediaItem
  episode: Episode
  /** Where the episode stands in the order the sheet is numbered in. */
  placed?: PlacedEpisode
}) {
  const { t, locale } = useI18n()

  // The way to the episode's page is its aired place; what is printed on the
  // still is its place in the order shown.
  const to = `/work/${item.id}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`
  const shown = placed
    ? { ...episode, seasonNumber: placed.seasonNumber, episodeNumber: placed.episodeNumber }
    : episode
  const absolute = placed?.absoluteNumber ?? episode.absoluteEpisodeNumber
  const when = airTime(episode)
  const upcoming = when !== undefined && !hasAired(when)
  const time = when?.moment ? fmt.clock(when.moment.toISOString(), locale) : undefined

  return (
    // An anchor only in the aired order: another order can number two
    // episodes the same, and an id has to be one thing.
    <li id={placed ? undefined : `episode-${episode.episodeNumber}`}>
      <Link
        to={to}
        className={cn(
          'group grid gap-4 rounded-panel border border-rule bg-ink-raised p-3 sm:grid-cols-[14rem_minmax(0,1fr)] sm:p-4',
          'transition-colors duration-150 hover:border-rule-bright hover:bg-ink-high',
        )}
      >
        <div className="relative aspect-video overflow-hidden rounded-card bg-ink-high">
          {episode.image ? (
            <Artwork
              url={episode.image}
              role="still"
              alt=""
              className="size-full object-cover"
            />
          ) : (
            <div className="grid size-full place-items-center">
              <Glyph name="film" className="size-5 text-bone-faint" />
            </div>
          )}
          {/* Edge print: the code as a film strip carries its frame numbers. */}
          <span className="absolute bottom-1.5 left-1.5 rounded-card bg-ink/85 px-1.5 py-0.5 font-mono text-[0.6875rem] font-medium tracking-wider text-bone tabular-nums">
            {episodeCode(shown)}
          </span>
        </div>

        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            {episode.finaleType ? <Chip tone="accent">{finaleLabel(episode.finaleType, t)}</Chip> : null}
            {upcoming && when ? (
              <Chip tone="provider">
                <Glyph name="clock" className="size-3" />
                {airsWhen(when, locale)}
              </Chip>
            ) : null}
            {episode.isManual ? (
              <Chip tone="manual">
                <Glyph name="lock" className="size-3" />
                {t.work.manualEntry}
              </Chip>
            ) : null}
          </div>

          <h2 className="mt-1 text-base leading-snug font-medium text-bone transition-colors duration-150 group-hover:text-vermillion">
            {episode.title || t.episode.untitled(shown.episodeNumber)}
          </h2>

          <p className="mt-1 font-mono text-[0.6875rem] text-bone-faint tabular-nums">
            {[
              fmt.weekdayDate(airValue(when), locale),
              time,
              fmt.runtime(episode.runtime, locale),
              episode.rating?.value ? `★ ${fmt.score(episode.rating.value, locale)}` : undefined,
              // Last, and only where it says something the code does not: in
              // a first season the two are the same number. Above the title,
              // on every episode after the first season, it read as a heading.
              absolute && absolute !== shown.episodeNumber ? t.episode.absolute(absolute) : undefined,
            ]
              .filter(Boolean)
              .join(' · ') || t.episode.noDate}
          </p>

          {episode.overview ? (
            <p className="mt-2 line-clamp-3 max-w-[70ch] text-sm leading-relaxed text-bone-dim">{episode.overview}</p>
          ) : null}

          <Placement episode={episode} className="mt-2" />
        </div>
      </Link>
    </li>
  )
}

function SeasonSkeleton() {
  return (
    <div className="space-y-6 pt-8">
      <Skeleton className="h-4 w-64" />
      <div className="flex gap-6">
        <Skeleton className="aspect-2/3 w-28 sm:w-36" />
        <div className="flex-1 space-y-3 pt-16">
          <Skeleton className="h-9 w-1/2" />
          <Skeleton className="h-4 w-1/3" />
        </div>
      </div>
      {Array.from({ length: 4 }, (_, index) => (
        <Skeleton key={index} className="h-36 w-full" />
      ))}
    </div>
  )
}
