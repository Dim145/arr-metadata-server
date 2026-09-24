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

import { Link, useParams } from 'react-router'

import { Elsewhere, Trail } from '../components/elsewhere'
import { Placement, finaleLabel } from '../components/episodes'
import { Artwork } from '../components/media'
import { Chip, EmptyState, Glyph, Skeleton } from '../components/ui'
import { ApiError } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useMe, useTitle, useWork } from '../lib/hooks'
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
  poster,
  seasonName,
  seasonNumbers,
} from '../lib/media'
import type { Episode, MediaItem } from '../lib/types'
import { NotFound, Unavailable } from './NotFound'

export function Season() {
  const { id = '', season = '' } = useParams()
  const number = Number(season)
  const work = useWork(id)

  // Kept on screen through a refetch that failed; see the work's page.
  if (!work.data) {
    if (work.isPending) return <SeasonSkeleton />
    if (work.error instanceof ApiError && work.error.status === 404) return <NotFound work />
    return <Unavailable onRetry={() => void work.refetch()} />
  }

  const item = work.data
  if (!Number.isInteger(number) || !seasonNumbers(item).includes(number)) {
    return <NotFound />
  }

  // Keyed by the season, so the next one starts its images afresh rather than
  // inheriting whatever the last one's had to fall back to.
  return <SeasonSheet key={number} item={item} number={number} />
}

function SeasonSheet({ item, number }: { item: MediaItem; number: number }) {
  const { t, locale } = useI18n()
  const me = useMe()

  const meta = item.seasons?.find((s) => s.seasonNumber === number)
  const episodes = episodesOf(item, number)
  const name = seasonName(meta?.title, number, t.work.season)
  const sheet = poster(item, number) ?? poster(item)
  const { before, after } = adjacentSeasons(item, number)
  const numbers = seasonNumbers(item)
  useTitle(name, item.title)

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
        <div className="flex items-center justify-between gap-3">
          <SeasonStep item={item} number={before} direction="before" />
          <SeasonStep item={item} number={after} direction="after" />
        </div>
        {numbers.length > 2 ? (
          <ol className="mt-2 flex flex-wrap items-center justify-center gap-1.5">
            {numbers.map((n) => {
              const title = seasonName(item.seasons?.find((s) => s.seasonNumber === n)?.title, n, t.work.season)
              const current = n === number

              return (
                <li key={n}>
                  <Link
                    to={`/work/${item.id}/season/${n}`}
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

      {episodes.length ? (
        <ol className="stagger mt-8 space-y-4">
          {episodes.map((episode) => (
            <EpisodeEntry key={episode.id} item={item} episode={episode} />
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
}: {
  item: MediaItem
  number?: number
  direction: 'before' | 'after'
}) {
  const { t } = useI18n()

  if (number === undefined) return <span className="hidden sm:block sm:w-40" />

  const name = seasonName(item.seasons?.find((s) => s.seasonNumber === number)?.title, number, t.work.season)

  return (
    <Link
      to={`/work/${item.id}/season/${number}`}
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
function EpisodeEntry({ item, episode }: { item: MediaItem; episode: Episode }) {
  const { t, locale } = useI18n()

  const to = `/work/${item.id}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`
  const when = airTime(episode)
  const upcoming = when !== undefined && !hasAired(when)
  const time = when?.moment ? fmt.clock(when.moment.toISOString(), locale) : undefined

  return (
    <li id={`episode-${episode.episodeNumber}`}>
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
            {episodeCode(episode)}
          </span>
        </div>

        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            {/* Only where it says something the code does not: in a first
                season the two are the same number. */}
            {episode.absoluteEpisodeNumber && episode.absoluteEpisodeNumber !== episode.episodeNumber ? (
              <span className="font-mono text-[0.6875rem] text-bone-faint tabular-nums">
                {t.work.absoluteNumber} {episode.absoluteEpisodeNumber}
              </span>
            ) : null}
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
            {episode.title || t.episode.untitled(episode.episodeNumber)}
          </h2>

          <p className="mt-1 font-mono text-[0.6875rem] text-bone-faint tabular-nums">
            {[
              fmt.weekdayDate(airValue(when), locale),
              time,
              fmt.runtime(episode.runtime, locale),
              episode.rating?.value ? `★ ${fmt.score(episode.rating.value, locale)}` : undefined,
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
