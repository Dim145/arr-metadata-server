/**
 * One work, in full.
 *
 * The page is a catalogue entry: a plate at the top, then the record. Seasons
 * are stacked scrollers rather than tabs, so a whole series is on one page and
 * nothing is hidden behind a state the URL does not carry.
 */

import { useQuery } from '@tanstack/react-query'
import { Link, useParams } from 'react-router'

import { Score } from '../components/media'
import {
  Chip,
  Genre,
  EmptyState,
  Field,
  Glyph,
  Panel,
  PanelHead,
  Provenance,
  SectionTitle,
  Skeleton,
} from '../components/ui'
import { api, query } from '../lib/api'
import { cn } from '../lib/cn'
import * as fmt from '../lib/format'
import { useI18n } from '../lib/i18n'
import { backdrop, cast, crew, episodesOf, headlineRating, poster, seasonNumbers } from '../lib/media'
import type { Credit, Episode, MediaItem } from '../lib/types'

export function Work() {
  const { id = '' } = useParams()
  const { t, lang } = useI18n()

  const work = useQuery({
    queryKey: ['work', id, lang],
    queryFn: () => api.get<MediaItem>(`/items/${id}${query({ language: lang })}`),
  })

  if (work.isPending) {
    return <WorkSkeleton />
  }

  if (work.isError || !work.data) {
    return (
      <div className="pt-16">
        <EmptyState
          title={t.common.error}
          action={
            <Link
              to="/"
              className="mt-2 inline-flex min-h-11 items-center gap-2 rounded-card border border-rule-bright px-4 text-sm text-bone"
            >
              <Glyph name="arrowLeft" className="size-4" />
              {t.work.back}
            </Link>
          }
        />
      </div>
    )
  }

  const item = work.data

  return (
    <article className="pb-8">
      <Plate item={item} />

      <div className="mt-12 grid gap-12 lg:grid-cols-[minmax(0,1fr)_20rem]">
        <div className="min-w-0 space-y-14">
          <Synopsis item={item} />
          <CastList credits={cast(item.credits)} />
          {item.kind === 'series' ? <Seasons item={item} /> : null}
        </div>

        <aside className="space-y-6">
          <Record item={item} />
          <Identifiers item={item} />
        </aside>
      </div>
    </article>
  )
}

/* ── The plate ────────────────────────────────────────────────────────────── */

function Plate({ item }: { item: MediaItem }) {
  const { t, locale } = useI18n()
  const art = backdrop(item)
  const sheet = poster(item)
  const rating = headlineRating(item.ratings)

  return (
    <header className="relative mx-[calc(50%-50vw)] w-screen">
      <div className="relative h-56 overflow-hidden sm:h-72 lg:h-[22rem]">
        {art ? (
          <img src={art} alt="" aria-hidden className="fade-in size-full object-cover object-top" />
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
              <img
                src={sheet}
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
                <span className="ml-3 font-normal text-bone-dim opacity-80">{item.year}</span>
              ) : null}
            </h1>

            {item.originalTitle && item.originalTitle !== item.title ? (
              <p className="mt-1.5 text-sm text-bone-faint italic">{item.originalTitle}</p>
            ) : null}

            <div className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-3">
              {rating?.value ? <Score value={rating.value} votes={rating.votes} size="md" /> : null}

              <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-sm text-bone-dim">
                {fmt.runtime(item.runtime, locale) ? (
                  <span>{fmt.runtime(item.runtime, locale)}</span>
                ) : null}
                {item.network ?? item.studio ? <span>{item.network ?? item.studio}</span> : null}
                {item.genres.slice(0, 4).map((genre) => (
                  <Genre key={genre} name={genre} />
                ))}
              </div>
            </div>
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
        {credits.map((person) => (
          <li key={person.id} className="flex items-center gap-3.5">
            {person.image ? (
              <img
                src={person.image}
                alt={t.a11y.headshot(person.personName)}
                loading="lazy"
                decoding="async"
                className="size-14 shrink-0 rounded-full border border-rule object-cover"
              />
            ) : (
              <div className="grid size-14 shrink-0 place-items-center rounded-full border border-rule bg-ink-high">
                <Glyph name="user" className="size-5 text-bone-faint" />
              </div>
            )}
            <div className="min-w-0">
              <p className="truncate text-sm font-medium text-bone">{person.personName}</p>
              {person.characterName ? (
                <p className="truncate text-sm text-bone-faint">{person.characterName}</p>
              ) : null}
            </div>
          </li>
        ))}
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

  // Providers name most seasons "Season 3", which is the label this interface
  // already has in the reader's language. Only a real name — "Specials", or a
  // part title an anime uses — is worth showing instead.
  const generic = /^(season|saison|series|specials?|hors-s\u00e9rie)\s*\d*$/i
  const given = meta?.title?.trim()
  const name = given && !generic.test(given) ? given : t.work.season(season)

  return (
    <div>
      <div className="mb-3 flex items-baseline gap-3">
        <h3 className="font-display text-lg font-medium text-bone">{name}</h3>
        <span className="font-mono text-xs text-bone-faint tabular-nums">
          {t.work.episodeCount(episodes.length)}
          {first ? ` · ${fmt.year(first)}` : ''}
        </span>
      </div>

      {/* Bleeds past the container on the right so a clipped card shows there is
          more; the padding puts it back for the last one. */}
      <div className="-mr-4 flex snap-x snap-mandatory gap-3 overflow-x-auto pr-4 pb-2 sm:-mr-6 sm:pr-6">
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
    <article
      className={cn(
        'w-60 shrink-0 snap-start overflow-hidden rounded-panel border border-rule bg-ink-raised',
        'transition-colors duration-150 hover:border-rule-bright',
      )}
    >
      <div className="relative aspect-video bg-ink-high">
        {episode.image ? (
          <img
            src={episode.image}
            alt={t.a11y.still(episode.title)}
            loading="lazy"
            decoding="async"
            className="size-full object-cover"
          />
        ) : (
          <div className="grid size-full place-items-center">
            <Glyph name="film" className="size-5 text-bone-faint" />
          </div>
        )}
        <span className="absolute bottom-1.5 left-1.5 rounded-card bg-ink/85 px-1.5 py-0.5 font-mono text-[0.625rem] font-medium text-bone tabular-nums">
          S{String(episode.seasonNumber).padStart(2, '0')}E
          {String(episode.episodeNumber).padStart(2, '0')}
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
        <h4 className="line-clamp-2 text-sm leading-snug font-medium text-bone">
          {episode.title || '—'}
        </h4>
        <p className="font-mono text-[0.6875rem] text-bone-faint tabular-nums">
          {[fmt.shortDate(episode.airDate, locale), fmt.runtime(episode.runtime, locale)]
            .filter(Boolean)
            .join(' · ') || '—'}
        </p>
      </div>
      <span className="sr-only">{item.title}</span>
    </article>
  )
}

/* ── The record ───────────────────────────────────────────────────────────── */

function Record({ item }: { item: MediaItem }) {
  const { t, locale } = useI18n()

  const rows: [string, string | undefined][] = [
    [t.work.status, item.status],
    [
      item.kind === 'series' ? t.work.firstAired : t.work.released,
      fmt.longDate(item.firstAired ?? item.inCinemas, locale),
    ],
    [t.work.lastAired, item.kind === 'series' ? fmt.longDate(item.lastAired, locale) : undefined],
    [t.work.runtime, fmt.runtime(item.runtime, locale)],
    [t.work.network, item.network],
    [t.work.studio, item.studio],
    [t.work.certification, item.contentRating],
    [t.work.originalLanguage, item.originalLanguage?.toUpperCase()],
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
            <Genre key={genre} name={genre} />
          ))}
        </div>
      ) : null}
    </Panel>
  )
}

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
          <Field key={source} label={source}>
            <span className="font-mono text-[0.8125rem] tabular-nums">
              {Array.isArray(value) ? value.join(', ') : String(value)}
            </span>
          </Field>
        ))}
      </dl>
    </Panel>
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
