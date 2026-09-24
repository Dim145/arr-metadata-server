/**
 * One work, in full.
 *
 * The page is a catalogue entry: a plate at the top, then the record. Seasons
 * are stacked scrollers rather than tabs, so a whole series is on one page and
 * nothing is hidden behind a state the URL does not carry.
 */

import { useQuery } from '@tanstack/react-query'
import { useRef, useState } from 'react'
import { Link, useParams } from 'react-router'

import { ExternalLink } from '../components/elsewhere'
import { Artwork, PosterCard, PosterGrid, Score } from '../components/media'
import { Lightbox, Trailer, useLightbox } from '../components/Theatre'
import {
  Button,
  Chip,
  Genre,
  Field,
  Glyph,
  Panel,
  PanelHead,
  Provenance,
  SectionTitle,
  Skeleton,
} from '../components/ui'
import { ApiError, api, query } from '../lib/api'
import { useMe, useTitle, useWork } from '../lib/hooks'
import { identifierLink } from '../lib/links'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { languageName, providerName, statusLabel, titleOrigin } from '../lib/labels'
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
  headlineRating,
  imagesOf,
  latestEpisode,
  nextEpisode,
  poster,
  ratingsOf,
  seasonName,
  seasonNumbers,
} from '../lib/media'
import type { Credit, Episode, ItemPage, MediaItem } from '../lib/types'
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

      <div className="mt-12 grid gap-12 lg:grid-cols-[minmax(0,1fr)_20rem]">
        <div className="min-w-0 space-y-14">
          <Synopsis item={item} />
          {item.kind === 'series' ? <OnAir item={item} /> : null}
          <CastList credits={cast(item.credits)} />
          {item.kind === 'series' ? <Seasons item={item} /> : null}
          <Gallery item={item} />
          {item.kind === 'movie' && item.collectionTmdbId ? <Collection item={item} /> : null}
          <AlsoKnownAs item={item} />
        </div>

        <aside className="space-y-6">
          <Record item={item} />
          <Ratings item={item} />
          <Identifiers item={item} />
        </aside>
      </div>
    </article>
  )
}

/* ── The plate ────────────────────────────────────────────────────────────── */

function Plate({ item }: { item: MediaItem }) {
  const { t, locale } = useI18n()
  const me = useMe()
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

            {item.trailerYoutubeId || item.homepage || me.data?.canWrite ? (
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
  const { t } = useI18n()

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
              <dt className="label">{person.characterName ?? person.creditType}</dt>
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

  if (!credits.length) return null

  return (
    <section>
      <SectionTitle>{t.work.cast}</SectionTitle>
      <ul className="stagger grid gap-x-8 gap-y-5 sm:grid-cols-2 xl:grid-cols-3">
        {credits.map((person) => {
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
            <li key={person.id}>
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
    </section>
  )
}

/**
 * Every season, each as its own scroller.
 *
 * Tabs would hide five sixths of a series behind a click and put its state
 * nowhere. Stacked scrollers keep the whole thing on one page, and the card
 * clipped by the right edge is the only affordance a reader needs.
 */
function Seasons({ item }: { item: MediaItem }) {
  const { t } = useI18n()
  const numbers = seasonNumbers(item)

  if (!numbers.length) return null

  return (
    <section>
      <SectionTitle>{t.work.seasons}</SectionTitle>

      <p className="mb-6 flex items-start gap-2 text-xs leading-relaxed text-bone-faint">
        <Glyph name="cloud" className="mt-0.5 size-3.5 shrink-0 text-slate" />
        {t.work.numbering}
      </p>

      <div className="space-y-10">
        {numbers.map((number) => (
          <SeasonRow key={number} item={item} season={number} />
        ))}
      </div>
    </section>
  )
}

function SeasonRow({ item, season }: { item: MediaItem; season: number }) {
  const { t, locale } = useI18n()
  const episodes = episodesOf(item, season)
  const meta = item.seasons?.find((s) => s.seasonNumber === season)

  if (!episodes.length) return null

  const first = episodes.find((e) => e.airDate)?.airDate

  // "Season 3" and "Specials" are placeholders this interface already has in
  // the reader's language; see `seasonName`.
  const name = seasonName(meta?.title, season, t.work.season)

  return (
    <div>
      <div className="mb-3 flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h3 className="font-display text-lg font-medium text-bone">
          <Link
            to={`/work/${item.id}/season/${season}`}
            className="transition-colors duration-150 hover:text-vermillion"
          >
            {name}
          </Link>
        </h3>
        <span className="font-mono text-xs text-bone-faint tabular-nums">
          {t.work.episodeCount(episodes.length)}
          {first ? ` · ${fmt.year(first)}` : ''}
        </span>
        <Link
          to={`/work/${item.id}/season/${season}`}
          className="label ml-auto inline-flex min-h-11 items-center gap-1 transition-colors duration-150 hover:text-vermillion"
        >
          {/* Named by what it says, then which season: someone who speaks
              "All episodes" to their computer has to find it. */}
          {t.work.allEpisodes}
          <span className="sr-only"> — {name}</span>
          <Glyph name="chevronRight" className="size-3" />
        </Link>
      </div>

      {/* Bleeds past the container on the right so a clipped card shows there is
          more; the padding puts it back for the last one. Each card is a link
          now, so a keyboard walks the strip by tabbing and the browser brings
          each one into view; the region is named, and no longer a tab stop of
          its own. */}
      <div
        role="region"
        aria-label={`${name} — ${t.work.episodeCount(episodes.length)}`}
        className="-mr-4 flex snap-x snap-mandatory gap-3 overflow-x-auto pr-4 pb-2 sm:-mr-6 sm:pr-6"
      >
        {episodes.map((episode) => (
          <EpisodeCard key={episode.id} item={item} episode={episode} locale={locale} />
        ))}
      </div>
    </div>
  )
}

function EpisodeCard({
  item,
  episode,
  locale,
}: {
  item: MediaItem
  episode: Episode
  locale: string
}) {
  const { t } = useI18n()

  return (
    <Link
      to={`/work/${item.id}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`}
      className={cn(
        'group w-60 shrink-0 snap-start overflow-hidden rounded-panel border border-rule bg-ink-raised',
        'transition-colors duration-150 hover:border-rule-bright',
      )}
    >
      <div className="relative aspect-video bg-ink-high">
        {episode.image ? (
          <Artwork
            url={episode.image}
            role="still"
            alt={t.a11y.still(episode.title)}
            className="size-full object-cover"
          />
        ) : (
          <div className="grid size-full place-items-center">
            <Glyph name="film" className="size-5 text-bone-faint" />
          </div>
        )}
        <span className="absolute bottom-1.5 left-1.5 rounded-card bg-ink/85 px-1.5 py-0.5 font-mono text-[0.6875rem] font-medium text-bone tabular-nums">
          {episodeCode(episode)}
        </span>
        {episode.isManual ? (
          <span className="absolute top-1.5 right-1.5">
            <Chip tone="manual">
              <Glyph name="lock" className="size-3" />
            </Chip>
          </span>
        ) : null}
      </div>

      <div className="space-y-1 p-3">
        <h4 className="line-clamp-2 text-sm leading-snug font-medium text-bone transition-colors duration-150 group-hover:text-vermillion">
          {episode.title || '—'}
        </h4>
        <p className="font-mono text-[0.6875rem] text-bone-faint tabular-nums">
          {[fmt.shortDate(airValue(airTime(episode)), locale), fmt.runtime(episode.runtime, locale)]
            .filter(Boolean)
            .join(' · ') || '—'}
        </p>
      </div>
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
          <ul className="flex flex-wrap gap-1.5">
            {keywords.map((keyword) => (
              <li key={keyword}>
                <Link
                  to={`/browse?keyword=${encodeURIComponent(keyword)}`}
                  className="inline-flex min-h-8 items-center rounded-full border border-rule-bright px-2.5 text-xs text-bone-dim transition-colors duration-150 hover:border-bone-faint hover:text-bone"
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
