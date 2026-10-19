/**
 * The top of a work's page: its backdrop, its poster, its name, its year and
 * what airs next — and what can be done from there.
 */

import { useQuery } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { Link } from 'react-router'

import { ExternalLink } from '../../components/elsewhere'
import { Artwork, Placeholder, Score, VEILED, Veil, useVeiled } from '../../components/media'
import { ShareButton } from '../../components/Share'
import { Button, Chip, Genre, Glyph, Provenance } from '../../components/ui'
import { api } from '../../lib/api'
import { cn } from '../../lib/cn'
import { feeds, webcal } from '../../lib/feeds'
import * as fmt from '../../lib/format'
import { useMe } from '../../lib/hooks'
import { useI18n } from '../../lib/i18n'
import { providerName } from '../../lib/labels'
import { airTime, airsWhen, backdrop, episodeCode, episodesOf, hasAired, headlineRating, nextEpisode, poster, seasonName } from '../../lib/media'
import { releaseDay, releasedNotAired } from '../../lib/platforms'
import { countryName } from '../../lib/releases'
import { around, formatDay } from '../../lib/seasons'
import { POSTER } from '../../lib/transitions'
import type { CuratedLists, MediaItem } from '../../lib/types'
import { ratedIn } from './record'

export function Plate({ item, region, onTrailer }: { item: MediaItem; region: string; onTrailer: () => void }) {
  const { t, lang, locale } = useI18n()
  const me = useMe()
  // A calendar app carries no credential, so the subscription is offered only
  // where the catalogue is open — and only for a work with a date to give.
  const subscribable =
    me.data?.publicBrowse === true &&
    (item?.kind === 'series' || Boolean(item?.inCinemas || item?.digitalRelease || item?.physicalRelease))
  // The selections this work is part of: the public ones, and for whoever
  // maintains them the private ones too.
  const inLists = useQuery({
    queryKey: ['work-lists', item.id],
    queryFn: () => api.get<CuratedLists>(`/items/${item.id}/lists`),
    staleTime: 60_000,
  })
  const art = backdrop(item)
  const sheet = poster(item)
  const rating = headlineRating(item.ratings)
  const maker = item.network ?? item.studio
  const rated = ratedIn(item, region)
  // A work for adults keeps its pictures veiled until the reader asks.
  const veiled = useVeiled(item)
  // What AniList calls the entry, where it says more than series or film.
  const format =
    item.format && !(item.kind === 'series' && item.format === 'TV') && !(item.kind === 'movie' && item.format === 'MOVIE')
      ? (t.work.format[item.format] ?? item.format)
      : undefined

  return (
    <header className="relative mx-[calc(50%-50vw)] w-screen">
      <div className="relative h-56 overflow-hidden sm:h-72 lg:h-[22rem]">
        {art ? (
          <Artwork
            url={art}
            role="backdrop"
            eager
            alt=""
            aria-hidden
            fetchPriority="high"
            className={cn('fade-in size-full object-cover object-top', veiled && VEILED)}
          />
        ) : (
          <div className="size-full bg-ink-raised" />
        )}
        {/* Hard gradients, not a blur: the text needs a seat, and frosting the
            whole strip would cost a compositor pass on every scrolled frame.
            The wash stays flat rather than clearing at the right edge — see the
            note in Home. */}
        <div className="absolute inset-0 bg-[linear-gradient(to_right,var(--color-ink)_0,var(--color-ink)_18%,color-mix(in_srgb,var(--color-ink)_84%,transparent)_48%,color-mix(in_srgb,var(--color-ink)_84%,transparent)_100%)]" />
        <div className="absolute inset-x-0 bottom-0 h-24 bg-gradient-to-t from-ink to-transparent" />
      </div>

      <div className="mx-auto max-w-7xl px-4 sm:px-6">
        <div className="-mt-24 flex flex-col gap-6 sm:-mt-28 sm:flex-row sm:items-end">
          {/* Named for the view transition: the card that was opened travels
              onto this box. No entrance of its own, then — an arrival drawn
              twice is drawn wrong. A veiled poster is clipped to its frame,
              whose blur would otherwise spill past the edge. */}
          <div
            className={cn('relative w-32 shrink-0 sm:w-40 lg:w-48', veiled && 'overflow-hidden rounded-plate')}
            data-travels={POSTER}
            style={{ viewTransitionName: POSTER }}
          >
            {sheet ? (
              <Artwork
                url={sheet}
                role="poster"
                eager
                alt={t.a11y.poster(item.title)}
                className={cn(
                  'aspect-2/3 w-full rounded-plate border border-rule-bright object-cover shadow-[var(--shadow-plate)]',
                  veiled && VEILED,
                )}
                fallback={
                  <Placeholder
                    title={item.title}
                    kind={item.kind}
                    label={t.a11y.poster(item.title)}
                    className="aspect-2/3 w-full rounded-plate border border-rule-bright shadow-[var(--shadow-plate)]"
                  />
                }
              />
            ) : (
              <Placeholder
                title={item.title}
                kind={item.kind}
                label={t.a11y.poster(item.title)}
                className="aspect-2/3 w-full rounded-plate border border-rule"
              />
            )}
            {veiled && (sheet || art) ? <Veil id={item.id} button /> : null}
          </div>

          {/* A container, so the title's size follows this column's width
              (`text-plate`, in `cqi`) rather than the window's: beside a
              poster on a tablet the column is half the screen. */}
          <div className="strike @container min-w-0 flex-1 pb-1" style={{ animationDelay: '70ms' }}>
            <div className="mb-2 flex flex-wrap items-center gap-2">
              <Chip tone="accent">
                <Glyph name={item.kind === 'series' ? 'tv' : 'film'} className="size-3" />
                {item.kind === 'series' ? t.nav.series : t.nav.films}
              </Chip>
              {item.isManual ? <Provenance manual label={t.work.manualEntry} /> : null}
              {format ? <Chip>{format}</Chip> : null}
              {rated ? (
                <Chip>
                  {/* The reader's country's, else the record's own with the
                      country it is of. */}
                  <span aria-hidden>
                    {rated.certification}
                    {rated.country && rated.country !== region ? ` · ${rated.country}` : ''}
                  </span>
                  <span className="sr-only">
                    {rated.country
                      ? t.work.ratedIn(rated.certification, countryName(rated.country, locale))
                      : rated.certification}
                  </span>
                </Chip>
              ) : null}
            </div>

            {/* Title and year at one size, told apart by weight — the move every
                one of the reference catalogues makes, and it costs nothing. */}
            <h1 className="font-display text-plate leading-[1.08] text-bone">
              <span className="font-medium">{item.title}</span>
              {item.year ? (
                <Link
                  to={`/browse?yearFrom=${item.year}&yearTo=${item.year}`}
                  className="ml-3 font-normal text-bone-dim opacity-80 transition-opacity duration-150 hover:opacity-100"
                  aria-label={t.work.sameYear(item.year)}
                >
                  {item.year}
                </Link>
              ) : null}
            </h1>

            {item.originalTitle && item.originalTitle !== item.title ? (
              <p className="mt-1.5 text-sm text-bone-faint italic">{item.originalTitle}</p>
            ) : null}

            {item.tagline ? (
              <p className="mt-2.5 max-w-[60ch] font-display text-base leading-snug text-bone-dim sm:text-lg">
                {item.tagline}
              </p>
            ) : null}

            <div className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-3">
              {rating?.value ? (
                <div className="flex items-center gap-3">
                  <Score
                    value={rating.value}
                    votes={rating.votes}
                    source={providerName(rating.source)}
                    size="md"
                  />
                  {/* Whose figure it is. One number with no name beside it read
                      as the catalogue's own verdict, and it moves between
                      sources as they are switched on. */}
                  <div className="leading-tight">
                    <p aria-hidden className="label">
                      {providerName(rating.source)}
                    </p>
                    {rating.votes ? (
                      <p className="mt-1 font-mono text-caption text-bone-faint tabular-nums">
                        {t.work.votes(fmt.count(rating.votes, locale), rating.votes)}
                      </p>
                    ) : null}
                  </div>
                </div>
              ) : null}

              <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-sm text-bone-dim">
                {fmt.runtime(item.runtime, locale) ? (
                  <span>{fmt.runtime(item.runtime, locale)}</span>
                ) : null}
                {maker ? (
                  <Link
                    to={`/browse?network=${encodeURIComponent(maker)}`}
                    className="underline decoration-rule-bright underline-offset-4 transition-colors duration-150 hover:text-bone hover:decoration-bone-faint"
                  >
                    {maker}
                  </Link>
                ) : null}
                {item.genres.slice(0, 4).map((genre) => (
                  <Genre key={genre} name={genre} to={`/browse?genre=${encodeURIComponent(genre)}`} />
                ))}
              </div>
            </div>

            {item.kind === 'series' && item.status !== 'ended' ? <NextUp item={item} /> : null}

            <div className="mt-5 flex flex-wrap items-center gap-2">
                {/* The trailer and the address are along the foot of a phone,
                    where the thumb is; up here they are for a wider screen. */}
                {item.trailerYoutubeId ? (
                  <Button onClick={onTrailer} className="max-md:hidden">
                    <Glyph name="play" className="size-4" />
                    {t.trailer.play}
                  </Button>
                ) : null}
                {item.themeMusic ? <ThemeMusic key={item.themeMusic} url={item.themeMusic} /> : null}
                {/* The address, handed on: the server's own preview of the
                    work travels with it wherever it lands. */}
                <span className="contents max-md:hidden">
                  <ShareButton item={item} />
                </span>
                {item.homepage ? (
                  <ExternalLink href={item.homepage} className="min-h-11 px-3 text-sm text-bone-dim">
                    {/* A Fan-Kai's is its page on the wiki, and says so. */}
                    {onFankaiWiki(item.homepage) ? providerName('fankaiwiki') : t.work.homepage}
                  </ExternalLink>
                ) : null}
                {/* The work's dates as a subscription a calendar app keeps
                    current: its episodes as they air, or a film's release day. */}
                {subscribable ? (
                  <a
                    href={webcal(feeds.work(item.id, lang))}
                    title={t.feeds.workHint}
                    className="inline-flex min-h-11 items-center gap-1.5 rounded-full px-3 text-sm text-bone-dim transition-colors duration-150 hover:bg-ink-high hover:text-bone"
                  >
                    <Glyph name="rss" className="size-4" />
                    {t.feeds.work}
                  </a>
                ) : null}
                {me.data?.canWrite ? (
                  <Link
                    to={`/admin/catalogue/${item.id}`}
                    className="inline-flex min-h-11 items-center gap-1.5 rounded-full border border-brass-deep px-4 text-sm font-medium text-brass transition-colors duration-150 hover:bg-brass/10"
                  >
                    <Glyph name="pencil" className="size-3.5" />
                    {t.work.edit}
                  </Link>
                ) : null}
              </div>

            {inLists.data?.lists.length ? (
              <div className="mt-4 flex flex-wrap items-center gap-2">
                <span className="label">{t.lists.inLists}</span>
                {inLists.data.lists.map((list) => (
                  <Link
                    key={list.id}
                    to={`/lists/${list.slug}`}
                    className="inline-flex min-h-9 items-center rounded-full border border-rule px-3 text-sm text-bone transition-colors duration-150 hover:border-rule-bright hover:text-vermillion"
                  >
                    {list.name}
                  </Link>
                ))}
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </header>
  )
}

/** A day in full, without its year while it is this year's: `Monday 12 October`. */
function dayInFull(day: string, locale: string): string {
  return formatDay(day, locale, {
    weekday: 'long',
    day: 'numeric',
    month: 'long',
    year: day.slice(0, 4) === String(new Date().getFullYear()) ? undefined : 'numeric',
  })
}

/**
 * What airs next, on the plate: the one line a running series is opened for,
 * read before anything scrolls. The code, the title, the day and how far off
 * it is, then how far the season it belongs to has got — counted as the
 * season's card counts it below, and worded as the season chart words it:
 * "2 of 8 aired" once the season is under way, "8 episodes" before it is.
 * A platform's day is the one its midnight opens, as the episode's page says
 * it; a network's, the reader's own. Nothing for a series that has ended, or
 * one with nothing announced.
 */
function NextUp({ item }: { item: MediaItem }) {
  const { t, locale } = useI18n()
  const next = nextEpisode(item)
  const when = next ? airTime(next) : undefined
  if (!next || !when) return null

  const episodes = episodesOf(item, next.seasonNumber)
  const aired = episodes.filter((e) => {
    const at = airTime(e)
    return at !== undefined && hasAired(at)
  }).length
  const released = releasedNotAired(item.network, when)
  const day = released ? releaseDay(when) : when.day
  // Tonight's by the hour; past that in days, then weeks, as the chart says
  // it. A platform's is a day, never hours.
  const distance =
    !released && when.moment && day === fmt.localDay(new Date()) ? airsWhen(when, locale) : around(day, locale)
  const season = seasonName(
    item.seasons?.find((s) => s.seasonNumber === next.seasonNumber)?.title,
    next.seasonNumber,
    t.work.season,
  )

  return (
    // A gap between the rows a phone wraps it into, as wide as the reach of
    // the targets in them.
    <p className="mt-4 flex flex-wrap items-center gap-x-3 gap-y-3 text-sm text-bone-dim">
      {/* The label inside the way to the episode: a link named for what it
          is, "Next episode: S05E03 · Title", not for a code alone. */}
      <Link
        to={`/work/${item.id}/season/${next.seasonNumber}/episode/${next.episodeNumber}`}
        className="hit group inline-flex min-h-8 min-w-0 items-center gap-2"
      >
        <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-vermillion" />
        <span className="shrink-0 font-medium text-vermillion">{t.work.nextUp}</span>{' '}
        <span className="min-w-0 text-bone underline decoration-rule-bright underline-offset-4 transition-colors duration-150 group-hover:text-vermillion group-hover:decoration-vermillion">
          <span className="font-mono text-xs text-bone-faint">{episodeCode(next)}</span>
          {' · '}
          {next.title || t.episode.untitled(next.episodeNumber)}
        </span>
      </Link>
      <span>{[dayInFull(day, locale), distance].filter(Boolean).join(' · ')}</span>
      <Link
        to={`/work/${item.id}/season/${next.seasonNumber}`}
        className="hit inline-flex min-h-8 items-center font-mono text-xs text-bone-faint tabular-nums underline decoration-rule-bright underline-offset-4 transition-colors duration-150 hover:text-bone hover:decoration-bone-faint"
      >
        <span className="sr-only">{season}, </span>
        {aired > 0 && aired < episodes.length
          ? t.work.airedOf(aired, episodes.length)
          : t.work.episodeCount(episodes.length)}
      </Link>
    </p>
  )
}

/**
 * The work's theme, played on demand: nothing is fetched until somebody
 * presses the button, and leaving the page stops it.
 */
function ThemeMusic({ url }: { url: string }) {
  const { t } = useI18n()
  const audio = useRef<HTMLAudioElement>(null)
  const [playing, setPlaying] = useState(false)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    const element = audio.current
    return () => element?.pause()
  }, [])

  return (
    <>
      {/* Unavailable, it says so in words: a disabled button shows no tooltip. */}
      <Button
        aria-pressed={failed ? undefined : playing}
        title={failed ? undefined : t.work.themeHint}
        disabled={failed}
        onClick={() => {
          const element = audio.current
          if (!element) return
          if (element.paused) {
            // A pause pressed before playback began rejects the play() it
            // interrupted: that is the reader's doing, not a broken file.
            element.play().catch((error: unknown) => {
              if (!(error instanceof DOMException && error.name === 'AbortError')) setFailed(true)
            })
          } else {
            element.pause()
          }
        }}
      >
        <Glyph name={failed ? 'alert' : playing ? 'pause' : 'play'} className="size-4" />
        {failed ? t.work.themeFailed : t.work.theme}
      </Button>
      {/* eslint-disable-next-line jsx-a11y/media-has-caption -- a theme tune, which no source captions */}
      <audio
        ref={audio}
        src={url}
        preload="none"
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onEnded={() => setPlaying(false)}
        onError={() => {
          setPlaying(false)
          setFailed(true)
        }}
      />
    </>
  )
}

/** Whether an address is a page of the Fankai wiki, where a Fan-Kai's homepage leads. */
function onFankaiWiki(address: string) {
  try {
    return new URL(address).hostname === 'fan-kai.fandom.com'
  } catch {
    return false
  }
}
