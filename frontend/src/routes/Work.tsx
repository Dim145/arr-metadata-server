/**
 * One work, in full.
 *
 * The page is a catalogue entry: a plate at the top, then the record. Seasons
 * are stacked scrollers rather than tabs, so a whole series is on one page and
 * nothing is hidden behind a state the URL does not carry.
 */

import { useQuery } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { Link, useParams } from 'react-router'

import { ExternalLink } from '../components/elsewhere'
import { Artwork, PosterCard, PosterGrid, Score, PosterShelf } from '../components/media'
import { Lightbox, Trailer, useLightbox } from '../components/Theatre'
import {
  Button,
  Chip,
  Field,
  Genre,
  Glyph,
  Panel,
  PanelHead,
  Provenance,
  SectionTitle,
  Select,
  Skeleton,
} from '../components/ui'
import { ApiError, api, query } from '../lib/api'
import { feeds, webcal } from '../lib/feeds'
import { useMe, useTitle, useWork } from '../lib/hooks'
import { identifierLink } from '../lib/links'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { jobLabel, languageName, providerName, statusLabel, titleOrigin } from '../lib/labels'
import { useI18n } from '../lib/i18n'
import {
  airTime,
  airValue,
  airsWhen,
  backdrop,
  cast,
  crew,
  episodesOf,
  episodeCode,
  hasAired,
  headlineRating,
  imagesOf,
  latestEpisode,
  nextEpisode,
  poster,
  ratingsOf,
  seasonName,
  seasonNumbers,
  seasonPoster,
} from '../lib/media'
import type { Credit, Episode, ItemPage, MediaItem, CuratedLists, Relation, WhereToWatch, Similar } from '../lib/types'
import { NotFound, Unavailable } from './NotFound'

export function Work() {
  const { id = '' } = useParams()
  const work = useWork(id)
  useTitle(work.data?.title)

  // What is on screen stays through a refetch that failed; only a read that
  // never came back decides between "gone" and "try again".
  if (!work.data) {
    if (work.isPending) return <WorkSkeleton />
    if (work.error instanceof ApiError && work.error.status === 404) return <NotFound work />
    return <Unavailable onRetry={() => void work.refetch()} />
  }

  const item = work.data

  return (
    // Keyed by the work, so moving from one to another starts every section
    // afresh: the gallery's tab and the lists opened out were the last one's.
    <article key={item.id} className="pb-8">
      <Plate item={item} />

      {/* The facts sit beside the page on a wide screen, and straight after the
          synopsis on a phone: status, dates, network and identifiers are what
          the page is opened for, and at the foot of a long page they were
          past the cast, the seasons and every picture. */}
      <div className="mt-12 grid gap-x-12 gap-y-14 lg:grid-cols-[minmax(0,1fr)_20rem] lg:grid-rows-[auto_1fr]">
        <div className="min-w-0 space-y-14 lg:col-start-1 lg:row-start-1">
          <Synopsis item={item} />
          {item.kind === 'series' ? <OnAir item={item} /> : null}
        </div>

        <aside className="space-y-6 lg:col-start-2 lg:row-span-2 lg:row-start-1">
          <Record item={item} />
          <WhereToWatch item={item} />
          <Ratings item={item} />
          <Identifiers item={item} />
        </aside>

        <div className="min-w-0 space-y-14 lg:col-start-1 lg:row-start-2">
          <CastList credits={cast(item.credits)} />
          {item.kind === 'series' ? <Seasons item={item} /> : null}
          <Gallery item={item} />
          {item.kind === 'movie' && item.collectionTmdbId ? <Collection item={item} /> : null}
          <Related item={item} />
          <Similar item={item} />
          <AlsoKnownAs item={item} />
        </div>
      </div>
    </article>
  )
}

/* ── The plate ────────────────────────────────────────────────────────────── */

function Plate({ item }: { item: MediaItem }) {
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
  const [trailer, setTrailer] = useState(false)
  const art = backdrop(item)
  const sheet = poster(item)
  const rating = headlineRating(item.ratings)
  const maker = item.network ?? item.studio

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
            className="fade-in size-full object-cover object-top"
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
          <div className="strike w-32 shrink-0 sm:w-40 lg:w-48">
            {sheet ? (
              <Artwork
                url={sheet}
                role="poster"
                eager
                alt={t.a11y.poster(item.title)}
                className="w-full rounded-plate border border-rule-bright shadow-[var(--shadow-plate)]"
              />
            ) : (
              <div className="grid aspect-2/3 w-full place-items-center rounded-plate border border-rule bg-ink-high">
                <Glyph name={item.kind === 'series' ? 'tv' : 'film'} className="size-8 text-bone-faint" />
              </div>
            )}
          </div>

          <div className="strike min-w-0 flex-1 pb-1" style={{ animationDelay: '70ms' }}>
            <div className="mb-2 flex flex-wrap items-center gap-2">
              <Chip tone="accent">
                <Glyph name={item.kind === 'series' ? 'tv' : 'film'} className="size-3" />
                {item.kind === 'series' ? t.nav.series : t.nav.films}
              </Chip>
              {item.isManual ? <Provenance manual label={t.work.manualEntry} /> : null}
              {item.contentRating ? <Chip>{item.contentRating}</Chip> : null}
            </div>

            {/* Title and year at one size, told apart by weight — the move every
                one of the reference catalogues makes, and it costs nothing. */}
            <h1 className="font-display text-3xl leading-[1.08] text-bone sm:text-4xl lg:text-5xl">
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
                      <p className="mt-1 font-mono text-[0.6875rem] text-bone-faint tabular-nums">
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

            {item.trailerYoutubeId || item.homepage || subscribable || me.data?.canWrite ? (
              <div className="mt-5 flex flex-wrap items-center gap-2">
                {item.trailerYoutubeId ? (
                  <Button onClick={() => setTrailer(true)}>
                    <Glyph name="play" className="size-4" />
                    {t.trailer.play}
                  </Button>
                ) : null}
                {item.homepage ? (
                  <ExternalLink href={item.homepage} className="min-h-11 px-3 text-sm text-bone-dim">
                    {t.work.homepage}
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
            ) : null}

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

            {item.trailerYoutubeId ? (
              <Trailer
                youtubeId={item.trailerYoutubeId}
                title={item.title}
                open={trailer}
                onClose={() => setTrailer(false)}
              />
            ) : null}
          </div>
        </div>
      </div>
    </header>
  )
}

/* ── Sections ─────────────────────────────────────────────────────────────── */

function Synopsis({ item }: { item: MediaItem }) {
  const { t, lang } = useI18n()

  return (
    <section>
      <SectionTitle>{t.work.overview}</SectionTitle>
      {item.overview ? (
        <p className="max-w-[68ch] text-[0.9375rem] leading-relaxed text-bone-dim">
          {item.overview}
        </p>
      ) : (
        <p className="text-sm text-bone-faint italic">{t.work.noOverview}</p>
      )}

      {crew(item.credits).length ? (
        <dl className="mt-8 grid gap-x-8 gap-y-4 sm:grid-cols-2 lg:grid-cols-3">
          {crew(item.credits, 6).map((person) => (
            <div key={person.id}>
              <dt className="label">{jobLabel(person.characterName ?? person.creditType, lang)}</dt>
              <dd className="mt-0.5 text-sm text-bone">{person.personName}</dd>
            </div>
          ))}
        </dl>
      ) : null}
    </section>
  )
}

/**
 * Cast as a list of rows with circular portraits.
 *
 * Circles rather than rectangles: a rectangular head-crop beside a page full of
 * posters reads as a poster that failed to load, and a row list reflows to one
 * column on a phone without a second layout.
 */
function CastList({ credits }: { credits: Credit[] }) {
  const { t } = useI18n()
  const [whole, setWhole] = useState(false)
  const list = useRef<HTMLUListElement>(null)
  // The button goes once pressed, and the focus with it: to the first name it
  // brought out, rather than to the page, which carried on tabbing past them.
  const revealed = useRef<number | null>(null)
  useEffect(() => {
    if (revealed.current === null) return
    list.current?.querySelectorAll<HTMLElement>(':scope > li')[revealed.current]?.focus()
    revealed.current = null
  }, [whole])

  if (!credits.length) return null

  // The top of the bill, then the rest a press away: three rows at the widest,
  // six names on a phone, where each takes a row of its own.
  const hidden = (index: number) => !whole && (index >= WIDE_CAST ? 'hidden' : index >= NARROW_CAST ? 'max-sm:hidden' : '')
  const more = !whole && credits.length > NARROW_CAST

  return (
    <section>
      <SectionTitle>{t.work.cast}</SectionTitle>
      <ul ref={list} className="stagger grid gap-x-8 gap-y-5 sm:grid-cols-2 xl:grid-cols-3">
        {credits.map((person, index) => {
          const face = person.image ? (
            <Artwork
              url={person.image}
              role="headshot"
              alt={t.a11y.headshot(person.personName)}
              className="size-14 shrink-0 rounded-full border border-rule object-cover"
            />
          ) : (
            <div className="grid size-14 shrink-0 place-items-center rounded-full border border-rule bg-ink-high">
              <Glyph name="user" className="size-5 text-bone-faint" />
            </div>
          )
          const words = (
            <div className="min-w-0">
              <p className="truncate text-sm font-medium text-bone transition-colors duration-150 group-hover:text-vermillion">
                {person.personName}
              </p>
              {person.characterName ? (
                <p className="truncate text-sm text-bone-faint">{person.characterName}</p>
              ) : null}
            </div>
          )

          return (
            <li key={person.id} tabIndex={-1} className={cn('rounded-card', hidden(index))}>
              {/* A person TMDB has an id for has a page: everything else they
                  are in, as far as this catalogue holds it. */}
              {person.tmdbPersonId ? (
                <Link to={`/person/${person.tmdbPersonId}`} className="group flex items-center gap-3.5 rounded-card">
                  {face}
                  {words}
                </Link>
              ) : (
                <div className="flex items-center gap-3.5">
                  {face}
                  {words}
                </div>
              )}
            </li>
          )
        })}
      </ul>
      {more ? (
        <Button
          size="sm"
          variant="ghost"
          className={cn('mt-6', credits.length <= WIDE_CAST && 'sm:hidden')}
          onClick={() => {
            revealed.current = window.matchMedia('(min-width: 640px)').matches ? WIDE_CAST : NARROW_CAST
            setWhole(true)
          }}
        >
          {t.work.wholeCast(credits.length)}
        </Button>
      ) : null}
    </section>
  )
}

/** How much of the cast shows before the rest is asked for. */
const WIDE_CAST = 9
const NARROW_CAST = 6

/**
 * Every season, as a shelf of its posters, each the way to its page.
 *
 * A season's episodes are its own page's. Laid out here, season after season,
 * a series of twenty-four seasons was twelve hundred cards — ten thousand
 * nodes and a page ten thousand pixels tall — most of it never looked at.
 * What the page is opened to choose is a season; the episode that aired last
 * and the one airing next are already above.
 */
function Seasons({ item }: { item: MediaItem }) {
  const { t } = useI18n()
  const shelf = useRef<HTMLUListElement>(null)
  const numbers = seasonNumbers(item).filter((n) => episodesOf(item, n).length)
  const now = latestEpisode(item) ?? nextEpisode(item)

  // A running series opens on the season it is at, not on its first, where
  // the shelf is wider than the screen; one that has ended, on its first.
  useEffect(() => {
    if (item.status === 'ended') return
    const list = shelf.current
    const at = list?.querySelector<HTMLElement>('[data-current]')
    if (!list || !at || list.scrollWidth <= list.clientWidth) return
    // Where the card sits along the shelf, whatever it is measured from.
    const left = at.getBoundingClientRect().left - list.getBoundingClientRect().left + list.scrollLeft
    list.scrollLeft = left - list.clientWidth + at.offsetWidth * 2
  }, [])

  if (!numbers.length) return null

  return (
    <section>
      <SectionTitle>{t.work.seasons}</SectionTitle>

      <p className="mb-6 flex items-start gap-2 text-xs leading-relaxed text-bone-faint">
        <Glyph name="cloud" className="mt-0.5 size-3.5 shrink-0 text-slate" />
        {/* A Fan-Kai is numbered by Fankai alone: TheTVDB has never seen it. */}
        {item.externalIds.fankai && !item.externalIds.tvdb ? t.work.numberingFankai : t.work.numbering}
      </p>

      {/* Bleeds past the container on the right so a clipped card shows there
          is more; each card is a link, so a keyboard walks the shelf by
          tabbing and the browser brings each one into view. */}
      <ul
        ref={shelf}
        aria-label={t.work.seasons}
        // Room above and to the left for a card's focus ring, which a scroller
        // would otherwise cut off.
        className="-mt-2 -mr-4 -ml-2 flex snap-x snap-mandatory scroll-pl-2 gap-4 overflow-x-auto pt-2 pr-4 pb-3 pl-2 sm:-mr-6 sm:pr-6"
      >
        {numbers.map((number) => (
          <li
            key={number}
            data-current={now?.seasonNumber === number || undefined}
            className="w-36 shrink-0 snap-start sm:w-40"
          >
            <SeasonCard item={item} season={number} />
          </li>
        ))}
      </ul>
    </section>
  )
}

function SeasonCard({ item, season }: { item: MediaItem; season: number }) {
  const { t, locale } = useI18n()
  const episodes = episodesOf(item, season)
  const meta = item.seasons?.find((s) => s.seasonNumber === season)
  const art = seasonPoster(item, season)

  // "Season 3" and "Specials" are placeholders this interface already has in
  // the reader's language; see `seasonName`.
  const name = seasonName(meta?.title, season, t.work.season)
  // Days as the reader's calendar has them, as the latest and next episodes
  // above are dated.
  const days = episodes
    .map((e) => airTime(e)?.day)
    .filter((d): d is string => !!d)
    .sort()
  const first = days[0] ?? meta?.airDate?.slice(0, 10)
  const aired = episodes.filter((e) => {
    const when = airTime(e)
    return when !== undefined && hasAired(when)
  }).length
  // A season under way says how far along it is; one still to come, when —
  // and only one whose first day is ahead: a past season with its episodes
  // undated is not coming.
  const airing = season > 0 && aired > 0 && aired < episodes.length && days.length === episodes.length
  const coming = season > 0 && aired === 0 && first !== undefined && first > fmt.localDay(new Date())

  return (
    <Link to={`/work/${item.id}/season/${season}`} className="group block focus-visible:outline-offset-4">
      <div className="relative aspect-2/3 overflow-hidden rounded-panel border border-rule bg-ink-high transition-colors duration-150 group-hover:border-vermillion/45">
        {art ? (
          <Artwork url={art} role="card" sizes="160px" alt="" className="size-full object-cover" />
        ) : (
          <div className="grid size-full place-items-center">
            <Glyph name="tv" className="size-6 text-bone-faint" />
          </div>
        )}
        {airing || coming ? (
          <span className="absolute top-2 left-2">
            <Chip tone={airing ? 'accent' : 'neutral'} className="bg-ink/85">
              {airing ? t.work.seasonAiring : t.work.seasonComing}
            </Chip>
          </span>
        ) : null}
        {airing ? (
          <span aria-hidden className="absolute inset-x-0 bottom-0 h-1 bg-ink/70">
            <span className="block h-full bg-vermillion" style={{ width: `${(aired / episodes.length) * 100}%` }} />
          </span>
        ) : null}
      </div>
      <h3 className="mt-2.5 line-clamp-2 text-sm leading-snug font-medium text-bone transition-colors duration-150 group-hover:text-vermillion">
        {name}
      </h3>
      <p className="mt-0.5 font-mono text-[0.6875rem] text-bone-faint tabular-nums">
        {[
          airing ? t.work.airedOf(aired, episodes.length) : t.work.episodeCount(episodes.length),
          coming ? fmt.shortDate(first, locale) : first ? fmt.year(first) : undefined,
        ]
          .filter(Boolean)
          .join(' · ')}
      </p>
      <span className="sr-only">{item.title}</span>
    </Link>
  )
}

/**
 * Where a running series stands: what aired last, and what is next.
 *
 * The one question a series page is most often opened to answer, and the one
 * a list of seasons makes the reader work out.
 */
function OnAir({ item }: { item: MediaItem }) {
  const { t } = useI18n()
  const next = nextEpisode(item)
  const latest = latestEpisode(item)

  if (!next && (!latest || item.status === 'ended')) return null

  return (
    <section aria-label={t.work.onAir} className="grid gap-3 sm:grid-cols-2">
      {latest ? <Milestone item={item} episode={latest} label={t.work.latestEpisode} /> : null}
      {next ? <Milestone item={item} episode={next} label={t.work.nextEpisode} upcoming /> : null}
    </section>
  )
}

function Milestone({
  item,
  episode,
  label,
  upcoming,
}: {
  item: MediaItem
  episode: Episode
  label: string
  upcoming?: boolean
}) {
  const { t, locale } = useI18n()
  const when = airTime(episode)
  const time = when?.moment ? fmt.clock(when.moment.toISOString(), locale) : undefined

  return (
    <Link
      to={`/work/${item.id}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`}
      className={cn(
        'group flex items-center gap-4 rounded-panel border p-3 transition-colors duration-150',
        upcoming
          ? 'border-vermillion-deep bg-vermillion/[0.05] hover:border-vermillion'
          : 'border-rule bg-ink-raised hover:border-rule-bright',
      )}
    >
      <div className="relative aspect-video w-28 shrink-0 overflow-hidden rounded-card bg-ink-high">
        {episode.image ? (
          <Artwork url={episode.image} role="still" alt="" className="size-full object-cover" />
        ) : (
          <div className="grid size-full place-items-center">
            <Glyph name={upcoming ? 'clock' : 'film'} className="size-4 text-bone-faint" />
          </div>
        )}
      </div>
      <div className="min-w-0">
        <p className={cn('label', upcoming && 'text-vermillion')}>{label}</p>
        <p className="mt-1 truncate text-sm font-medium text-bone transition-colors duration-150 group-hover:text-vermillion">
          <span className="font-mono text-xs text-bone-faint">{episodeCode(episode)}</span>{' '}
          {episode.title || t.episode.untitled(episode.episodeNumber)}
        </p>
        <p className="mt-0.5 text-xs text-bone-faint">
          {[fmt.longDate(airValue(when), locale), time].filter(Boolean).join(' · ')}
          {upcoming && when ? ` · ${airsWhen(when, locale)}` : ''}
        </p>
      </div>
    </Link>
  )
}

/** What each kind of artwork is called, in the order the gallery offers them. */
const GALLERY_KINDS = ['poster', 'fanart', 'landscape', 'banner', 'clearlogo', 'clearart', 'characterart'] as const

type GalleryKind = (typeof GALLERY_KINDS)[number]

/**
 * Every picture the providers filed for the work, by kind — the light table.
 *
 * A grid of the set and a lightbox for one at a time. The first twelve of a
 * kind are drawn; a work with sixty posters asks before drawing the rest.
 *
 * The kinds are tabs in the full sense: one stop for the Tab key, the arrow
 * keys between them, and the panel named after the tab that shows it.
 */
function Gallery({ item }: { item: MediaItem }) {
  const { t } = useI18n()
  const kinds = GALLERY_KINDS.filter((kind) => imagesOf(item, kind).length)
  const [kind, setKind] = useState<GalleryKind | undefined>(undefined)
  const [all, setAll] = useState(false)
  const lightbox = useLightbox()
  const tabs = useRef<(HTMLButtonElement | null)[]>([])

  const current = kind && kinds.includes(kind) ? kind : kinds[0]
  if (!current) return null

  const images = imagesOf(item, current)
  const shown = all ? images : images.slice(0, 12)
  const wide = current !== 'poster'
  const name = (value: string) => (t.gallery.kind as Record<string, string>)[value] ?? value

  const choose = (value: GalleryKind) => {
    setKind(value)
    setAll(false)
  }

  const onTabKey = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const at = kinds.indexOf(current)
    const next =
      event.key === 'ArrowRight'
        ? (at + 1) % kinds.length
        : event.key === 'ArrowLeft'
          ? (at - 1 + kinds.length) % kinds.length
          : event.key === 'Home'
            ? 0
            : event.key === 'End'
              ? kinds.length - 1
              : undefined
    const target = next === undefined ? undefined : kinds[next]
    if (next === undefined || !target) return

    event.preventDefault()
    choose(target)
    tabs.current[next]?.focus()
  }

  return (
    <section>
      <SectionTitle>{t.gallery.title}</SectionTitle>

      <div role="tablist" aria-label={t.gallery.kinds} onKeyDown={onTabKey} className="mb-5 flex flex-wrap gap-2">
        {kinds.map((value, index) => (
          <button
            key={value}
            ref={(node) => {
              tabs.current[index] = node
            }}
            id={`gallery-tab-${value}`}
            type="button"
            role="tab"
            aria-selected={value === current}
            aria-controls="gallery-panel"
            tabIndex={value === current ? 0 : -1}
            onClick={() => choose(value)}
            className={cn(
              'inline-flex min-h-11 cursor-pointer items-center gap-2 rounded-full border px-4 text-sm',
              'transition-colors duration-150',
              value === current
                ? 'border-vermillion-deep bg-vermillion/10 text-bone'
                : 'border-rule-bright text-bone-dim hover:text-bone',
            )}
          >
            {name(value)}
            <span className="font-mono text-xs text-bone-faint tabular-nums">{imagesOf(item, value).length}</span>
          </button>
        ))}
      </div>

      <div role="tabpanel" id="gallery-panel" aria-labelledby={`gallery-tab-${current}`}>
        <ul
          className={cn(
            'grid gap-3',
            wide ? 'grid-cols-2 sm:grid-cols-3' : 'grid-cols-3 sm:grid-cols-4 md:grid-cols-6',
          )}
        >
          {shown.map((image, index) => (
            <li key={image.id}>
              <button
                type="button"
                onClick={() => lightbox.open(index)}
                aria-label={t.gallery.open(index + 1)}
                className={cn(
                  'block w-full cursor-zoom-in overflow-hidden rounded-card border border-rule bg-ink-high',
                  'transition-colors duration-150 hover:border-vermillion/60',
                  current === 'poster' ? 'aspect-2/3' : current === 'banner' ? 'aspect-[758/140]' : 'aspect-video',
                )}
              >
                <Artwork
                  url={image.url}
                  role={current === 'poster' ? 'card' : 'still'}
                  alt=""
                  className={cn(
                    'size-full',
                    current === 'clearlogo' || current === 'clearart' || current === 'characterart'
                      ? 'object-contain p-3'
                      : 'object-cover',
                  )}
                />
              </button>
            </li>
          ))}
        </ul>

        {images.length > shown.length ? (
          <div className="mt-4 flex justify-center">
            <Button size="sm" onClick={() => setAll(true)}>
              {t.gallery.showAll(images.length)}
            </Button>
          </div>
        ) : null}
      </div>

      <Lightbox
        images={images}
        index={lightbox.index}
        onIndex={lightbox.move}
        onClose={lightbox.close}
        title={item.title}
      />
    </section>
  )
}

/** The other films TMDB files in the same collection, in the order they came out. */
function Collection({ item }: { item: MediaItem }) {
  const { t, lang } = useI18n()

  const others = useQuery({
    queryKey: ['collection', item.collectionTmdbId, lang],
    queryFn: () =>
      api.get<ItemPage>(
        `/items${query({
          collection: item.collectionTmdbId,
          kind: 'movie',
          sort: 'release',
          order: 'asc',
          limit: 24,
          language: lang,
        })}`,
      ),
  })

  const films = (others.data?.items ?? []).filter((film) => film.id !== item.id)
  if (!films.length) return null

  return (
    <section>
      <SectionTitle>{t.work.collection}</SectionTitle>
      <PosterGrid className="lg:grid-cols-4 xl:grid-cols-5">
        {films.map((film) => (
          <PosterCard key={film.id} item={film} to={`/work/${film.id}`} />
        ))}
      </PosterGrid>
      <Link
        to={`/collections/${item.collectionTmdbId}`}
        className="-ml-3 mt-4 inline-flex min-h-11 items-center gap-1.5 rounded-full px-3 text-sm text-vermillion transition-colors duration-150 hover:bg-vermillion/10"
      >
        {t.work.wholeCollection}
        <Glyph name="chevronRight" className="size-3.5" />
      </Link>
    </section>
  )
}

/**
 * The catalogue's own works in the same vein — its kind, sharing its genres,
 * keywords, network or decade — for a reader who liked this one. Nothing
 * where nothing is close enough.
 */
/**
 * The works AniList files beside this one, nearest first: what it follows and
 * what follows it, then the stories beside it, then what it was drawn from.
 * A work the catalogue holds leads to its page; the rest lead to AniList.
 */
function Related({ item }: { item: MediaItem }) {
  const { t } = useI18n()
  const related = item.relations ?? []
  if (!related.length) return null

  return (
    <section>
      <SectionTitle>{t.work.related}</SectionTitle>
      <p className="-mt-3 mb-4 text-xs text-bone-faint">{t.work.relatedHint}</p>
      <PosterShelf label={t.work.related}>
        {related.map((relation) => (
          <RelationCard key={relation.id} relation={relation} />
        ))}
      </PosterShelf>
    </section>
  )
}

function RelationCard({ relation }: { relation: Relation }) {
  const { t } = useI18n()
  const kind = t.work.relation[relation.relationType] ?? t.work.relation.OTHER
  const format = relation.format ? (t.work.format[relation.format] ?? relation.format) : undefined
  const caption = [relation.year, format].filter(Boolean).join(' · ')
  const label = `${relation.title} · ${kind}${relation.workId ? '' : ` · ${t.work.notHeld}`}`
  const className = 'group block focus-visible:outline-offset-4'
  const face = (
    <>
      <div
        className={cn(
          'relative aspect-2/3 overflow-hidden rounded-panel border border-rule bg-ink-high',
          'shadow-[var(--shadow-lift)] transition-colors duration-150 group-hover:border-vermillion/45',
          !relation.workId && 'opacity-80 group-hover:opacity-100',
        )}
      >
        {relation.image ? (
          <Artwork url={relation.image} role="card" alt="" className="size-full object-cover" />
        ) : (
          <div className="flex size-full items-center justify-center font-display text-3xl text-bone-faint">
            {relation.title.slice(0, 1)}
          </div>
        )}
        <div className="absolute right-0 bottom-0 left-0 bg-gradient-to-t from-ink/85 to-transparent px-2 pt-6 pb-2">
          <span className="label text-bone">{kind}</span>
        </div>
      </div>
      <div className="mt-2.5 space-y-0.5">
        <h3 className="line-clamp-2 text-sm leading-snug font-medium text-bone transition-colors duration-200 group-hover:text-vermillion">
          {relation.title}
        </h3>
        <p className="font-mono text-[0.6875rem] text-bone-faint tabular-nums">
          {relation.workId ? caption : [caption, t.work.onAnilist].filter(Boolean).join(' · ')}
        </p>
      </div>
    </>
  )

  return relation.workId ? (
    <Link to={`/work/${relation.workId}`} aria-label={label} className={className}>
      {face}
    </Link>
  ) : (
    <a
      href={`https://anilist.co/${relation.medium === 'manga' ? 'manga' : 'anime'}/${relation.externalId}`}
      target="_blank"
      rel="noreferrer noopener"
      aria-label={label}
      className={className}
    >
      {face}
    </a>
  )
}

function Similar({ item }: { item: MediaItem }) {
  const { t, lang } = useI18n()

  const alike = useQuery({
    queryKey: ['similar', item.id, lang],
    queryFn: () => api.get<Similar>(`/items/${item.id}/similar${query({ language: lang })}`),
    staleTime: 10 * 60_000,
    retry: false,
  })

  const works = alike.data?.items ?? []
  if (!works.length) return null

  return (
    <section>
      <SectionTitle>{t.work.similar}</SectionTitle>
      <PosterShelf label={t.work.similar}>
        {works.map((work) => (
          <PosterCard key={work.id} item={work} to={`/work/${work.id}`} />
        ))}
      </PosterShelf>
    </section>
  )
}

/**
 * The other names a work goes by, and what it is about.
 *
 * Alternative titles are how a work is found from a release name or a
 * translated poster; keywords are how TMDB and AniList say what it is about,
 * and each leads to the rest of the catalogue that shares it.
 */
/** The sources api.radarr.video files a title under, where others file its kind. */
const RADARR_SOURCES = new Set(['tmdb', 'mappings', 'user', 'indexer'])

function AlsoKnownAs({ item }: { item: MediaItem }) {
  const { t, locale } = useI18n()
  const [all, setAll] = useState(false)

  // The work's own names are on its plate already, whatever case a provider
  // wrote them in: "BREAKING BAD" is not another name for Breaking Bad.
  const own = new Set([item.title, item.originalTitle].filter(Boolean).map((n) => n!.trim().toLocaleLowerCase()))
  const titles = [
    ...new Map((item.alternativeTitles ?? []).map((a) => [a.title.trim().toLocaleLowerCase(), a])).values(),
  ].filter((a) => a.title.trim() && !own.has(a.title.trim().toLocaleLowerCase()))
  const keywords = item.keywords.filter((k) => k.trim())

  if (!titles.length && !keywords.length) return null

  const shown = all ? titles : titles.slice(0, 10)

  return (
    <section>
      {/* Named for what it holds: a work with keywords and no other title
          had them under "Also known as". */}
      <SectionTitle>{titles.length ? t.work.alsoKnownAs : t.work.keywords}</SectionTitle>

      {titles.length ? (
        <>
          <ul className="grid gap-x-8 gap-y-2 sm:grid-cols-2">
            {shown.map((alt) => {
              // Where the title is used, and what kind of title it is: the
              // second is a fixed word from AniList or MyAnimeList, or TMDB's
              // own note, which is free text in whatever language it came in.
              // Radarr's titles say where Radarr found them instead, which is
              // no use to a reader, and a language rather than a place.
              const type = alt.titleType?.toLowerCase()
              const fromRadarr = type !== undefined && RADARR_SOURCES.has(type)
              const kind =
                type === undefined || fromRadarr
                  ? undefined
                  : ((t.work.titleTypes as Record<string, string>)[type] ?? alt.titleType)
              const origin = titleOrigin(alt.language, locale, fromRadarr ? 'language' : 'place')
              const note = [origin, kind].filter(Boolean).join(' · ')

              return (
                <li key={alt.id} className="flex items-baseline justify-between gap-3 border-b border-rule py-1.5">
                  <span className="min-w-0 text-sm text-bone-dim">{alt.title}</span>
                  {note ? (
                    <span className="max-w-[45%] shrink-0 truncate text-right text-xs text-bone-faint" title={note}>
                      {note}
                    </span>
                  ) : null}
                </li>
              )
            })}
          </ul>
          {titles.length > shown.length ? (
            <Button size="sm" variant="quiet" className="mt-3" onClick={() => setAll(true)}>
              {t.work.allTitles(titles.length)}
            </Button>
          ) : null}
        </>
      ) : null}

      {keywords.length ? (
        <div className={titles.length ? 'mt-6' : undefined}>
          {titles.length ? <h3 className="label mb-2">{t.work.keywords}</h3> : null}
          <ul className="flex flex-wrap gap-x-1.5 gap-y-3">
            {keywords.map((keyword) => (
              <li key={keyword}>
                <Link
                  to={`/browse?keyword=${encodeURIComponent(keyword)}`}
                  className="hit inline-flex min-h-8 items-center rounded-full border border-rule-bright px-2.5 text-xs text-bone-dim transition-colors duration-150 hover:border-bone-faint hover:text-bone"
                >
                  # {keyword}
                </Link>
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </section>
  )
}

/* ── The record ───────────────────────────────────────────────────────────── */

function Record({ item }: { item: MediaItem }) {
  const { t, locale } = useI18n()

  const rows: [string, string | undefined][] = [
    [t.work.status, statusLabel(item.status, t)],
    [
      item.kind === 'series' ? t.work.firstAired : t.work.released,
      fmt.longDate(item.firstAired ?? item.inCinemas, locale),
    ],
    [t.work.lastAired, item.kind === 'series' ? fmt.longDate(item.lastAired, locale) : undefined],
    [t.work.runtime, fmt.runtime(item.runtime, locale)],
    [t.work.network, item.network],
    [t.work.studio, item.studio],
    [t.work.certification, item.contentRating],
    [t.work.originalLanguage, languageName(item.originalLanguage, locale)],
  ]

  const present = rows.filter((row): row is [string, string] => Boolean(row[1]))

  return (
    <Panel>
      <PanelHead title={t.work.details} />
      <dl className="divide-y divide-rule">
        {present.map(([label, value]) => (
          <Field key={label} label={label}>
            {value}
          </Field>
        ))}
      </dl>

      {item.genres.length ? (
        <div className="flex flex-wrap gap-1.5 border-t border-rule p-4">
          {item.genres.map((genre) => (
            <Genre key={genre} name={genre} to={`/browse?genre=${encodeURIComponent(genre)}`} />
          ))}
        </div>
      ) : null}
    </Panel>
  )
}

/**
 * Every mark out of ten a source gave, the one on the plate first.
 *
 * One figure hides how far apart the audiences are: an anime can sit a point
 * higher on MyAnimeList than on TMDB, from a hundred times the votes. Left out
 * when the plate already shows the only one there is.
 */
function Ratings({ item }: { item: MediaItem }) {
  const { t, locale } = useI18n()
  const ratings = ratingsOf(item.ratings)

  if (ratings.length < 2) return null

  return (
    <Panel>
      <PanelHead title={t.work.ratings} />
      <ul className="divide-y divide-rule">
        {ratings.map((rating) => {
          const value = rating.value ?? 0

          return (
            <li key={rating.source} className="px-5 py-3">
              <div className="flex items-baseline justify-between gap-3">
                <span className="label">{providerName(rating.source)}</span>
                <span className="font-display text-lg leading-none text-bone tabular-nums">
                  {fmt.score(value, locale)}
                  <span className="sr-only"> {t.work.outOfTen}</span>
                </span>
              </div>
              <div aria-hidden className="mt-2 h-1 overflow-hidden rounded-full bg-rule">
                <div
                  className="h-full rounded-full bg-gradient-to-r from-vermillion to-brass"
                  style={{ width: `${value * 10}%` }}
                />
              </div>
              {rating.votes ? (
                <p className="mt-1.5 font-mono text-[0.6875rem] text-bone-faint tabular-nums">
                  {t.work.votes(fmt.count(rating.votes, locale), rating.votes)}
                </p>
              ) : null}
            </li>
          )
        })}
      </ul>
    </Panel>
  )
}

/**
 * Every identifier, each a way to the work on the site that issued it.
 *
 * MyAnimeList and AniList give one id per season or cour, so a long series
 * carries a dozen: the first — the entry that stands for the whole — is shown,
 * and the rest wait behind a count rather than filling the panel.
 */
function Identifiers({ item }: { item: MediaItem }) {
  const { t } = useI18n()

  const entries = Object.entries(item.externalIds).filter(([, value]) =>
    Array.isArray(value) ? value.length : value !== undefined && value !== null,
  )

  if (!entries.length) return null

  return (
    <Panel>
      <PanelHead title={t.work.identifiers} />
      <dl className="divide-y divide-rule">
        {entries.map(([source, value]) => (
          <Field key={source} label={providerName(source)}>
            <IdentifierValues
              source={source}
              values={Array.isArray(value) ? value : [value as string | number]}
              kind={item.kind}
            />
          </Field>
        ))}
      </dl>
    </Panel>
  )
}

function IdentifierValues({
  source,
  values,
  kind,
}: {
  source: string
  values: (string | number)[]
  kind: MediaItem['kind']
}) {
  const { t } = useI18n()
  const [all, setAll] = useState(false)
  const shown = all ? values : values.slice(0, 1)
  const hidden = values.length - shown.length

  return (
    <span className="inline-flex flex-wrap items-baseline justify-end gap-x-2 gap-y-1 font-mono text-[0.8125rem] tabular-nums">
      {shown.map((value) => {
        const href = identifierLink(source, value, kind)
        return href ? (
          <ExternalLink key={value} href={href}>
            {value}
          </ExternalLink>
        ) : (
          <span key={value}>{value}</span>
        )
      })}
      {values.length > 1 ? (
        <button
          type="button"
          aria-expanded={all}
          onClick={() => setAll(!all)}
          className="min-h-6 cursor-pointer text-xs text-bone-faint underline decoration-dotted underline-offset-2 hover:text-bone"
        >
          {all ? t.work.fewerIds : t.work.moreIds(hidden)}
        </button>
      ) : null}
    </span>
  )
}

/* ── Waiting ──────────────────────────────────────────────────────────────── */

function WorkSkeleton() {
  return (
    <div className="pb-8">
      <div className="mx-[calc(50%-50vw)] h-56 w-screen sm:h-72 lg:h-[22rem]">
        <Skeleton className="size-full rounded-none" />
      </div>
      <div className="-mt-24 flex gap-6 sm:-mt-28">
        <Skeleton className="aspect-2/3 w-32 sm:w-40 lg:w-48" />
        <div className="flex-1 space-y-3 pt-24 sm:pt-28">
          <Skeleton className="h-10 w-2/3" />
          <Skeleton className="h-4 w-1/3" />
        </div>
      </div>
      <div className="mt-12 space-y-4">
        <Skeleton className="h-7 w-40" />
        <Skeleton className="h-24 w-full" />
      </div>
    </div>
  )
}

/* ── Where to watch ───────────────────────────────────────────────────────── */

const REGION_KEY = 'ams.watchRegion'

/**
 * The services carrying the work in a country, as TMDB lists them from
 * JustWatch — named beside the data, as TMDB's terms ask. The country is
 * the reader's own setting, then the language's; any other TMDB knows of
 * can be picked. Absent altogether where there is no TMDB key, or nothing.
 */
function WhereToWatch({ item }: { item: MediaItem }) {
  const { t, locale } = useI18n()
  // The country picked last time, kept in this browser alone: a reader abroad
  // picks it once, not on every page.
  const [region, setRegion] = useState<string | undefined>(() => {
    try {
      const stored = localStorage.getItem(REGION_KEY)
      return stored && /^[A-Z]{2}$/.test(stored) ? stored : undefined
    } catch {
      return undefined
    }
  })
  const pick = (code: string) => {
    setRegion(code)
    try {
      localStorage.setItem(REGION_KEY, code)
    } catch {
      // A browser that keeps nothing still shows the country picked.
    }
  }

  const watch = useQuery({
    queryKey: ['watch', item.id, region ?? ''],
    queryFn: () => api.get<WhereToWatch>(`/items/${item.id}/watch${query({ region })}`),
    retry: false,
    staleTime: 60 * 60_000,
  })
  // A remembered country the server no longer takes is forgotten, and the
  // panel asked for again without it, rather than staying away for good.
  useEffect(() => {
    if (region && watch.error instanceof ApiError && watch.error.status === 400) {
      try {
        localStorage.removeItem(REGION_KEY)
      } catch {
        // Nothing kept, nothing to forget.
      }
      setRegion(undefined)
    }
  }, [region, watch.error])
  if (!watch.data) return null
  const data = watch.data
  const groups = (['flatrate', 'free', 'ads', 'rent', 'buy'] as const).filter((group) => data[group].length)
  if (!groups.length && data.regions.length === 0) return null

  // A country's name where the engine knows them; its code where it does not.
  const names = (() => {
    try {
      return new Intl.DisplayNames(locale, { type: 'region' })
    } catch {
      return undefined
    }
  })()
  const place = (code: string) => {
    try {
      return names?.of(code) ?? code
    } catch {
      return code
    }
  }

  return (
    <Panel>
      <PanelHead
        title={t.work.watch}
        action={
          data.regions.length > 1 ? (
            <Select
              aria-label={t.work.watchRegion}
              value={data.region}
              onChange={(event) => pick(event.target.value)}
              className="min-h-9 w-auto py-0 text-xs"
            >
              {data.regions.map((code) => (
                <option key={code} value={code}>
                  {place(code)}
                </option>
              ))}
            </Select>
          ) : null
        }
      />
      <div className="space-y-4 p-5">
        {groups.length ? (
          groups.map((group) => (
            <div key={group}>
              <span className="label">{t.work.watchGroups[group]}</span>
              <ul className="mt-2 flex flex-wrap gap-2">
                {data[group].map((provider) => (
                  <li
                    key={provider.id}
                    className="flex items-center gap-2 rounded-full border border-rule py-1 pr-3 pl-1 text-sm text-bone"
                  >
                    {provider.logo ? (
                      <img
                        src={provider.logo}
                        alt=""
                        width={24}
                        height={24}
                        loading="lazy"
                        className="size-6 rounded-full"
                      />
                    ) : (
                      <span className="ml-2" />
                    )}
                    {provider.name}
                  </li>
                ))}
              </ul>
            </div>
          ))
        ) : (
          <p className="text-sm text-bone-dim">{t.work.watchNone(place(data.region))}</p>
        )}
        <p className="text-xs text-bone-faint">
          {place(data.region)} · {t.work.watchCredit}
          {data.link ? (
            <>
              {' · '}
              <ExternalLink href={data.link} className="text-bone-dim">
                {t.work.watchLink}
              </ExternalLink>
            </>
          ) : null}
        </p>
      </div>
    </Panel>
  )
}
