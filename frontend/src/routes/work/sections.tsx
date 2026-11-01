/**
 * The sections of a work's page under its plate: the synopsis and who made
 * it, the cast, the seasons, the pictures, the works around it, and the other
 * names it goes by.
 *
 * Each section a page's index can lead to carries its id — the anchor the
 * row of sections on a phone points at — and stops short of the bars that
 * stay at the top when it is scrolled to.
 */

import { useQuery } from '@tanstack/react-query'
import { Fragment, useEffect, useRef, useState } from 'react'
import { Link } from 'react-router'

import { Artwork, PosterCard, PosterGrid, PosterShelf, VEILED, Veil, useVeiled } from '../../components/media'
import { Lightbox, useLightbox } from '../../components/Theatre'
import { Button, Chip, Glyph, SectionTitle, Shelf } from '../../components/ui'
import { keywordPath } from '../../lib/facets'
import { api, query } from '../../lib/api'
import { cn } from '../../lib/cn'
import * as fmt from '../../lib/format'
import { useI18n } from '../../lib/i18n'
import { jobLabel, languageName, providerName, titleOrigin } from '../../lib/labels'
import {
  airTime,
  airValue,
  crew,
  crewByDepartment,
  episodeCode,
  episodesOf,
  hasAired,
  imagesOf,
  latestEpisode,
  nextEpisode,
  seasonName,
  seasonNumbers,
  seasonPoster,
  videoPage,
  videosOf,
  type Department,
} from '../../lib/media'
import { airsWhenOn, releaseDay, releasedNotAired } from '../../lib/platforms'
import { unveil } from '../../lib/prefs'
import type { Credit, Episode, ItemPage, MediaItem, Relation, Similar as SimilarData, Video } from '../../lib/types'
import { Fold } from './fold'
import { SECTION_MARGIN } from './record'

/**
 * One person in a line of names: a way to the rest of their work here — their
 * own page where the caller gives it, else the catalogue narrowed to them
 * where TMDB knows them.
 */
interface Named {
  name: string
  tmdbId?: number
  to?: string
}

export function Synopsis({ item }: { item: MediaItem }) {
  const { t, lang } = useI18n()
  // One line a job, its people together: "Production" once, not six times.
  const jobs: { label: string; people: Named[] }[] = []
  for (const person of crew(item.credits, 24)) {
    const label = jobLabel(person.characterName ?? person.creditType, lang)
    const job = jobs.find((j) => j.label === label)
    const named = { name: person.personName, tmdbId: person.tmdbPersonId ?? undefined }
    if (!job) jobs.push({ label, people: [named] })
    else if (!job.people.some((p) => p.name === person.personName)) job.people.push(named)
  }

  return (
    <section id="overview" className={SECTION_MARGIN}>
      <SectionTitle>{t.work.overview}</SectionTitle>
      {item.overview ? (
        <p className="max-w-[68ch] text-lead leading-relaxed text-bone-dim">{item.overview}</p>
      ) : (
        <p className="text-sm text-bone-faint italic">{t.work.noOverview}</p>
      )}

      {jobs.length ? (
        <dl className="mt-8 grid gap-x-8 gap-y-4 sm:grid-cols-2 lg:grid-cols-3">
          {jobs.slice(0, 6).map((job) => (
            <div key={job.label}>
              <dt className="label">{job.label}</dt>
              <dd className="mt-0.5 text-sm text-bone">
                <People people={job.people} />
              </dd>
            </div>
          ))}
        </dl>
      ) : null}
    </section>
  )
}

/**
 * Names in a line, as the reader's language joins them — "A, B and C" — each
 * a way to everything the catalogue holds with that person in it.
 */
function People({ people }: { people: Named[] }) {
  const { t, locale } = useI18n()
  // The names as the language joins them, one part a name and one a joint,
  // the names in the order they were given.
  const parts = new Intl.ListFormat(locale, { type: 'conjunction' }).formatToParts(people.map((p) => p.name))
  const names = parts.flatMap((part, index) => (part.type === 'element' ? [index] : []))

  return (
    <>
      {parts.map((part, index) => {
        if (part.type !== 'element') return <Fragment key={index}>{part.value}</Fragment>
        const person = people[names.indexOf(index)]
        const to = person?.to ?? (person?.tmdbId ? `/browse?person=${person.tmdbId}` : undefined)
        return person && to ? (
          <Link
            key={index}
            to={to}
            aria-label={t.work.withPerson(person.name)}
            title={t.work.withPerson(person.name)}
            className="underline decoration-rule-bright underline-offset-4 transition-colors duration-150 hover:text-vermillion hover:decoration-vermillion"
          >
            {part.value}
          </Link>
        ) : (
          <Fragment key={index}>{part.value}</Fragment>
        )
      })}
    </>
  )
}

/**
 * How much of the cast shows before the rest is folded: two rows at the
 * widest, three on a tablet, six names on a phone, where each has a row of
 * its own.
 */
const SHOWN_CAST = 6

/**
 * Cast as a list of rows with circular portraits.
 *
 * Circles rather than rectangles: a rectangular head-crop beside a page full of
 * posters reads as a poster that failed to load, and a row list reflows to one
 * column on a phone without a second layout.
 *
 * The top of the bill, then the rest folded — out of the page's height, still
 * found by the browser's search (see `Fold`).
 */
export function CastList({
  credits,
  people,
}: {
  credits: Credit[]
  /** Whom each credit is filed under, by the credit's id. */
  people?: NonNullable<MediaItem['people']>['byCredit']
}) {
  const { t } = useI18n()

  if (!credits.length) return null

  const grid = 'grid gap-x-8 gap-y-5 sm:grid-cols-2 xl:grid-cols-3'
  const row = (person: Credit) => {
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
        {person.characterName ? <p className="truncate text-sm text-bone-faint">{person.characterName}</p> : null}
      </div>
    )

    // Everybody credited has a page: everything else they are in, as far as
    // this catalogue holds it — by TMDB's id, as it always was, or by their
    // record where TMDB knows them not.
    const record = people?.[person.id]?.personId
    const page = person.tmdbPersonId
      ? `/person/${person.tmdbPersonId}`
      : record
        ? `/people/${encodeURIComponent(record)}`
        : undefined

    return (
      <li key={person.id} tabIndex={-1} className="rounded-card">
        {page ? (
          <Link to={page} className="group flex items-center gap-3.5 rounded-card">
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
  }
  const rest = credits.slice(SHOWN_CAST)

  return (
    <section id="cast" className={SECTION_MARGIN}>
      <SectionTitle>{t.work.cast}</SectionTitle>
      <ul className={cn('stagger', grid)}>{credits.slice(0, SHOWN_CAST).map(row)}</ul>
      {rest.length ? (
        <Fold label={t.work.wholeCast(credits.length)} className="mt-6">
          <ul className={cn('stagger mt-5', grid)}>{rest.map(row)}</ul>
        </Fold>
      ) : null}
    </section>
  )
}

/** How many departments of the crew stand open before the rest are folded. */
const SHOWN_DEPARTMENTS = 3

/**
 * Everybody behind the camera, a department at a time — directing, writing,
 * production, then music, camera, editing and the rest, as TMDB's crew is
 * kept, the most telling jobs first — each job once with everybody who did
 * it, each name the way to that person's page. The first departments stand
 * open; the rest are folded, and found by the browser's search all the same.
 *
 * Only where it says more than the synopsis does: a director and a writer or
 * two are named there already.
 */
export function CrewList({ item }: { item: MediaItem }) {
  const { t, lang } = useI18n()
  const departments = crewByDepartment(item.credits)
  if (!crewShown(item, departments)) return null

  const byCredit = item.people?.byCredit
  // Everybody once, however many jobs they did.
  const everybody = new Set(departments.flatMap((d) => d.jobs.flatMap((j) => j.people.map((p) => p.personName)))).size
  const named = (credit: Credit): Named => {
    const record = byCredit?.[credit.id]?.personId
    return {
      name: credit.personName,
      tmdbId: credit.tmdbPersonId ?? undefined,
      to: credit.tmdbPersonId
        ? `/person/${credit.tmdbPersonId}`
        : record
          ? `/people/${encodeURIComponent(record)}`
          : undefined,
    }
  }
  const block = (department: Department) => (
    <div key={department.department} className="min-w-0">
      <h3 className="label">{t.work.departments[department.department] ?? department.department}</h3>
      <dl className="mt-2 space-y-2.5">
        {department.jobs.map((line) => (
          <div key={line.job}>
            <dt className="text-xs text-bone-faint">{jobLabel(line.job, lang)}</dt>
            <dd className="mt-0.5 text-sm text-bone">
              <People people={line.people.map(named)} />
            </dd>
          </div>
        ))}
      </dl>
    </div>
  )
  const grid = 'grid gap-x-8 gap-y-7 sm:grid-cols-2 xl:grid-cols-3'
  const rest = departments.slice(SHOWN_DEPARTMENTS)

  return (
    <section id="crew" className={SECTION_MARGIN}>
      <SectionTitle>{t.work.crew}</SectionTitle>
      <p className="-mt-3 mb-5 text-xs text-bone-faint">{t.work.crewLead}</p>
      <div className={grid}>{departments.slice(0, SHOWN_DEPARTMENTS).map(block)}</div>
      {rest.length ? (
        <Fold label={t.work.wholeCrew(everybody)} className="mt-6">
          <div className={cn('mt-5', grid)}>{rest.map(block)}</div>
        </Fold>
      ) : null}
    </section>
  )
}

/**
 * Whether the crew has a section of its own: somebody beyond the directors,
 * writers and producers, or more jobs than the synopsis names.
 */
export function crewShown(item: MediaItem, departments: Department[] = crewByDepartment(item.credits)): boolean {
  const jobs = departments.reduce((n, department) => n + department.jobs.length, 0)
  return departments.length > 0 && ((item.credits ?? []).some((c) => c.creditType === 'crew') || jobs > 6)
}

/** How many videos stand open before the rest are folded. */
const SHOWN_VIDEOS = 6

/**
 * Every video the sources list for the work — trailers, teasers, clips — the
 * trailer chosen for it first. Nothing is fetched from the sites that keep
 * them until one is asked for, not even a still: a YouTube video plays here,
 * in the trailer's own player, through YouTube's cookie-free address, which
 * is the one this server's page policy lets frame; the others open on their
 * own site.
 */
export function Videos({ item, onPlay }: { item: MediaItem; onPlay: (key: string, name: string) => void }) {
  const { t, locale } = useI18n()
  const videos = videosOf(item)
  if (!videos.length) return null

  const row = (video: Video) => {
    const kind = video.videoType ? (t.work.videoTypes[video.videoType] ?? video.videoType) : undefined
    const name = video.name?.trim() || kind || t.work.trailer
    const facts = [
      kind,
      languageName(video.language ?? undefined, locale),
      video.publishedAt ? fmt.shortDate(video.publishedAt.slice(0, 10), locale) : undefined,
      video.size ? `${video.size}p` : undefined,
    ]
      .filter(Boolean)
      .join(' · ')
    const youtube = video.site.toLowerCase() === 'youtube'
    const page = videoPage(video)
    const face = (
      <>
        <span
          aria-hidden
          className="grid size-10 shrink-0 place-items-center rounded-full border border-rule-bright text-bone-dim transition-colors duration-150 group-hover:border-vermillion group-hover:text-vermillion"
        >
          <Glyph name={youtube ? 'play' : 'external'} className="size-4" />
        </span>
        {/* Named by what it shows, the spaces spelt out: a checker reads the
            text of side-by-side elements run together, and a name that does
            not hold the text a reader sees fails WCAG's label in name. */}
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm font-medium text-bone transition-colors duration-150 group-hover:text-vermillion">
            {name}
          </span>{' '}
          <span className="block truncate text-xs text-bone-faint">{youtube ? facts : [facts, video.site].filter(Boolean).join(' · ')}</span>
        </span>{' '}
        {video.official ? <Chip className="shrink-0">{t.work.official}</Chip> : null}
      </>
    )
    const line = 'group flex min-h-14 w-full items-center gap-3 rounded-card px-2 py-1.5 text-left transition-colors duration-150 hover:bg-ink-high'

    return (
      <li key={video.id || `${video.site}:${video.key}`}>
        {youtube ? (
          <button type="button" onClick={() => onPlay(video.key, name)} className={cn(line, 'cursor-pointer')}>
            <span className="sr-only">{t.work.playVideo}</span>
            {face}
          </button>
        ) : page ? (
          <a href={page} target="_blank" rel="noreferrer noopener" className={line}>
            {face}
          </a>
        ) : (
          <div className={line}>{face}</div>
        )}
      </li>
    )
  }
  const list = 'grid gap-x-6 gap-y-1 sm:grid-cols-2'
  const rest = videos.slice(SHOWN_VIDEOS)

  return (
    <section id="videos" className={SECTION_MARGIN}>
      <SectionTitle>{t.work.videos}</SectionTitle>
      <p className="-mt-3 mb-4 max-w-prose text-xs text-bone-faint">{t.work.videosLead}</p>
      <ul className={cn('-mx-2', list)}>{videos.slice(0, SHOWN_VIDEOS).map(row)}</ul>
      {rest.length ? (
        <Fold label={t.work.allVideos(videos.length)} className="mt-3">
          <ul className={cn('-mx-2 mt-2', list)}>{rest.map(row)}</ul>
        </Fold>
      ) : null}
    </section>
  )
}

/**
 * Every season, as a shelf of its posters, each the way to its page.
 *
 * A season's episodes are its own page's. Laid out here, season after season,
 * a series of twenty-four seasons was twelve hundred cards — ten thousand
 * nodes and a page ten thousand pixels tall — most of it never looked at.
 * What the page is opened to choose is a season; the episode that aired last
 * and the one airing next are already above.
 */
export function Seasons({ item }: { item: MediaItem }) {
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
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once, as the page opens: a refresh must not move the shelf under the reader
  }, [])

  if (!numbers.length) return null

  return (
    <section id="seasons" className={SECTION_MARGIN}>
      <SectionTitle>{t.work.seasons}</SectionTitle>

      <p className="mb-6 flex items-start gap-2 text-xs leading-relaxed text-bone-faint">
        <Glyph name="cloud" className="mt-0.5 size-3.5 shrink-0 text-slate" />
        {/* A Fan-Kai is numbered by Fankai alone: TheTVDB has never seen it. */}
        {item.externalIds.fankai && !item.externalIds.tvdb ? t.work.numberingFankai : t.work.numbering}
      </p>

      {/* Bleeds past the container on the right, where the shelf says it goes
          on — its edge fading, its arrows — and each card is a link, so a
          keyboard walks the shelf by tabbing and the browser brings each one
          into view. Room above and to the left for a card's focus ring, which
          a scroller would otherwise cut off. */}
      <Shelf
        ref={shelf}
        label={t.work.seasons}
        className="-mt-2 -mr-4 -ml-2 sm:-mr-6"
        listClassName="gap-4 pt-2 pr-4 pb-3 pl-2 sm:pr-6"
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
      </Shelf>
    </section>
  )
}

function SeasonCard({ item, season }: { item: MediaItem; season: number }) {
  const { t, locale } = useI18n()
  const episodes = episodesOf(item, season)
  const meta = item.seasons?.find((s) => s.seasonNumber === season)
  const art = seasonPoster(item, season)
  const veiled = useVeiled(item)

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
          <Artwork url={art} role="card" sizes="160px" alt="" className={cn('size-full object-cover', veiled && VEILED)} />
        ) : (
          <div className="grid size-full place-items-center">
            <Glyph name="tv" className="size-6 text-bone-faint" />
          </div>
        )}
        {veiled && art ? <Veil id={item.id} shows={false} /> : null}
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
      <p className="mt-0.5 font-mono text-caption text-bone-faint tabular-nums">
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
export function OnAir({ item }: { item: MediaItem }) {
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
  // A platform's midnight is a release day, not an hour anybody was up for.
  const released = when !== undefined && releasedNotAired(item.network, when)
  const date = fmt.longDate(when && released ? releaseDay(when) : airValue(when), locale)
  const time = !released && when?.moment ? fmt.clock(when.moment.toISOString(), locale) : undefined
  const veiled = useVeiled(item)

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
          <Artwork url={episode.image} role="still" alt="" className={cn('size-full object-cover', veiled && VEILED)} />
        ) : (
          <div className="grid size-full place-items-center">
            <Glyph name={upcoming ? 'clock' : 'film'} className="size-4 text-bone-faint" />
          </div>
        )}
        {veiled && episode.image ? <Veil id={item.id} shows={false} /> : null}
      </div>
      <div className="min-w-0">
        <p className={cn('label', upcoming && 'text-vermillion')}>{label}</p>
        <p className="mt-1 truncate text-sm font-medium text-bone transition-colors duration-150 group-hover:text-vermillion">
          <span className="font-mono text-xs text-bone-faint">{episodeCode(episode)}</span>{' '}
          {episode.title || t.episode.untitled(episode.episodeNumber)}
        </p>
        <p className="mt-0.5 text-xs text-bone-faint">
          {released && date ? t.episode.availableOn(date) : [date, time].filter(Boolean).join(' · ')}
          {upcoming && when ? ` · ${airsWhenOn(item.network, when, locale)}` : ''}
        </p>
      </div>
    </Link>
  )
}

/** What each kind of artwork is called, in the order the gallery offers them. */
const GALLERY_KINDS = ['poster', 'fanart', 'landscape', 'banner', 'clearlogo', 'clearart', 'characterart'] as const

type GalleryKind = (typeof GALLERY_KINDS)[number]

/** Whether the work has any picture the gallery would show. */
export function hasGallery(item: MediaItem): boolean {
  return GALLERY_KINDS.some((kind) => imagesOf(item, kind).length)
}

/**
 * Every picture the providers filed for the work, by kind — the light table.
 *
 * A grid of the set and a lightbox for one at a time. The first twelve of a
 * kind are drawn; a work with sixty posters asks before drawing the rest.
 *
 * The kinds are tabs in the full sense: one stop for the Tab key, the arrow
 * keys between them, and the panel named after the tab that shows it.
 */
export function Gallery({ item }: { item: MediaItem }) {
  const { t } = useI18n()
  const kinds = GALLERY_KINDS.filter((kind) => imagesOf(item, kind).length)
  const [kind, setKind] = useState<GalleryKind | undefined>(undefined)
  const [all, setAll] = useState(false)
  const lightbox = useLightbox()
  const tabs = useRef<(HTMLButtonElement | null)[]>([])
  const veiled = useVeiled(item)

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
    <section id="gallery" className={SECTION_MARGIN}>
      <SectionTitle>{t.gallery.title}</SectionTitle>

      {/* eslint-disable-next-line jsx-a11y/interactive-supports-focus -- the tabs take the focus, one at a time; the list hears their arrows */}
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
                onClick={() => {
                  // A work for adults shows its pictures once asked to: the
                  // first press uncovers them, as on its poster.
                  if (veiled) unveil(item.id)
                  else lightbox.open(index)
                }}
                aria-label={t.gallery.open(index + 1)}
                className={cn(
                  'relative block w-full cursor-zoom-in overflow-hidden rounded-card border border-rule bg-ink-high',
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
                    veiled && VEILED,
                  )}
                />
                {veiled ? <Veil id={item.id} shows={current === 'poster' ? 'poster' : 'picture'} /> : null}
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
export function Collection({ item }: { item: MediaItem }) {
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
 * The works AniList files beside this one, nearest first: what it follows and
 * what follows it, then the stories beside it, then what it was drawn from.
 * A work the catalogue holds leads to its page; the rest lead to AniList.
 */
export function Related({ item }: { item: MediaItem }) {
  const { t } = useI18n()
  const related = item.relations ?? []
  if (!related.length) return null

  // A Fan-Kai's come from the Fankai wiki; an anime's from AniList, with the
  // Fan-Kai cut from it first.
  const recuts = related.filter((relation) => relation.relationType === 'RECUT').length
  const hint = item.externalIds.fankai
    ? t.work.relatedHintFankai
    : recuts === related.length
      ? t.work.relatedHintRecutOnly
      : recuts
        ? t.work.relatedHintRecut
        : t.work.relatedHint

  return (
    <section>
      <SectionTitle>{t.work.related}</SectionTitle>
      <p className="-mt-3 mb-4 text-xs text-bone-faint">{hint}</p>
      <PosterShelf label={t.work.related}>
        {related.map((relation) => (
          <RelationCard key={relation.id} relation={relation} />
        ))}
      </PosterShelf>
    </section>
  )
}

/** Where an entry the catalogue does not hold is found, by where it is filed. */
function relationHome(relation: Relation): string {
  const id = encodeURIComponent(String(relation.externalId))
  const medium = relation.medium === 'manga' ? 'manga' : 'anime'
  switch (relation.source) {
    case 'fankai':
      return `https://fankai.fr/productions/${id}`
    case 'mal':
      return `https://myanimelist.net/${medium}/${id}`
    default:
      return `https://anilist.co/${medium}/${id}`
  }
}

function RelationCard({ relation }: { relation: Relation }) {
  const { t } = useI18n()
  const frame = useRef<HTMLDivElement>(null)
  // Uncovered with the work itself where the catalogue holds it.
  const id = relation.workId ?? relation.id
  const veiled = useVeiled({ id, isAdult: relation.isAdult })
  const kind = t.work.relation[relation.relationType] ?? t.work.relation.OTHER
  const format = relation.format ? (t.work.format[relation.format] ?? relation.format) : undefined
  const caption = [relation.year, format].filter(Boolean).join(' · ')
  const label = `${relation.title} · ${kind}${relation.workId ? '' : ` · ${t.work.notHeld}`}`
  const className = 'group block focus-visible:outline-offset-4'
  // A veiled picture is uncovered by the press that lands on it, as a
  // poster's card has it.
  const onClick = (event: React.MouseEvent) => {
    if (veiled && relation.image && frame.current?.contains(event.target as Node)) {
      event.preventDefault()
      unveil(id)
    }
  }
  const face = (
    <>
      <div
        ref={frame}
        className={cn(
          'relative aspect-2/3 overflow-hidden rounded-panel border border-rule bg-ink-high',
          'shadow-[var(--shadow-lift)] transition-colors duration-150 group-hover:border-vermillion/45',
          !relation.workId && 'opacity-80 group-hover:opacity-100',
        )}
      >
        {relation.image ? (
          <Artwork url={relation.image} role="card" alt="" className={cn('size-full object-cover', veiled && VEILED)} />
        ) : (
          <div className="flex size-full items-center justify-center font-display text-3xl text-bone-faint">
            {relation.title.slice(0, 1)}
          </div>
        )}
        {veiled && relation.image ? <Veil id={id} /> : null}
        <div className="absolute right-0 bottom-0 left-0 bg-gradient-to-t from-ink/85 to-transparent px-2 pt-6 pb-2">
          <span className="label text-bone">{kind}</span>
        </div>
      </div>
      <div className="mt-2.5 space-y-0.5">
        <h3 className="line-clamp-2 text-sm leading-snug font-medium text-bone transition-colors duration-200 group-hover:text-vermillion">
          {relation.title}
        </h3>
        <p className="font-mono text-caption text-bone-faint tabular-nums">
          {relation.workId
            ? caption
            : [caption, t.work.onSite(providerName(relation.source))].filter(Boolean).join(' · ')}
        </p>
      </div>
    </>
  )

  return relation.workId ? (
    <Link to={`/work/${relation.workId}`} aria-label={label} className={className} onClick={onClick}>
      {face}
    </Link>
  ) : (
    <a
      href={relationHome(relation)}
      target="_blank"
      rel="noreferrer noopener"
      aria-label={label}
      className={className}
      onClick={onClick}
    >
      {face}
    </a>
  )
}

/**
 * The catalogue's own works in the same vein — its kind, sharing its genres,
 * keywords, network or decade — read once for the section and for the page's
 * index, which offers the way there only where there is something.
 */
export function useSimilar(id: string | undefined) {
  const { lang } = useI18n()

  return useQuery({
    queryKey: ['similar', id, lang],
    queryFn: () => api.get<SimilarData>(`/items/${id}/similar${query({ language: lang })}`),
    enabled: Boolean(id),
    staleTime: 10 * 60_000,
    retry: false,
  })
}

/**
 * The catalogue's own works in the same vein, for a reader who liked this
 * one. Nothing where nothing is close enough.
 */
export function Similar({ item }: { item: MediaItem }) {
  const { t } = useI18n()
  const alike = useSimilar(item.id)

  const works = alike.data?.items ?? []
  if (!works.length) return null

  return (
    <section id="similar" className={SECTION_MARGIN}>
      <SectionTitle>{t.work.similar}</SectionTitle>
      <PosterShelf label={t.work.similar}>
        {works.map((work) => (
          <PosterCard key={work.id} item={work} to={`/work/${work.id}`} />
        ))}
      </PosterShelf>
    </section>
  )
}

/** The sources api.radarr.video files a title under, where others file its kind. */
const RADARR_SOURCES = new Set(['tmdb', 'mappings', 'user', 'indexer'])

/** How many other names, and how many keywords, stand open before the rest are folded. */
const SHOWN_TITLES = 6
const SHOWN_KEYWORDS = 12

/** The other names a work goes by, told apart from its own: those the section lists. */
export function otherTitles(item: MediaItem) {
  // The work's own names are on its plate already, whatever case a provider
  // wrote them in: "BREAKING BAD" is not another name for Breaking Bad.
  const own = new Set([item.title, item.originalTitle].filter(Boolean).map((n) => n!.trim().toLocaleLowerCase()))
  return [
    ...new Map((item.alternativeTitles ?? []).map((a) => [a.title.trim().toLocaleLowerCase(), a])).values(),
  ].filter((a) => a.title.trim() && !own.has(a.title.trim().toLocaleLowerCase()))
}

/**
 * The other names a work goes by, and what it is about.
 *
 * Alternative titles are how a work is found from a release name or a
 * translated poster; keywords are how TMDB and AniList say what it is about.
 * Each is a way into the catalogue: a title to search it for — another work
 * may go by it too — a keyword to the rest that share it. The first few of
 * each stand open; the rest are folded, and found by the browser's search.
 */
export function AlsoKnownAs({ item }: { item: MediaItem }) {
  const { t, locale } = useI18n()

  const titles = otherTitles(item)
  const keywords = item.keywords.filter((k) => k.trim())
  // TMDB's keywords are things of their own, with a page each; the rest —
  // AniList's tags — narrow the catalogue by their words.
  const entities = new Map((item.keywordEntities ?? []).filter((k) => k.id).map((k) => [k.name.trim().toLocaleLowerCase(), k]))

  if (!titles.length && !keywords.length) return null

  const title = (alt: (typeof titles)[number]) => {
    // Where the title is used, and what kind of title it is: the second is a
    // fixed word from AniList or MyAnimeList, or TMDB's own note, which is
    // free text in whatever language it came in. Radarr's titles say where
    // Radarr found them instead, which is no use to a reader, and a language
    // rather than a place.
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
        <Link
          to={`/browse?q=${encodeURIComponent(alt.title.trim())}`}
          aria-label={t.work.searchTitle(alt.title.trim())}
          title={t.work.searchTitle(alt.title.trim())}
          className="min-w-0 text-sm text-bone-dim underline decoration-rule-bright underline-offset-4 transition-colors duration-150 hover:text-bone hover:decoration-bone-faint"
        >
          {alt.title}
        </Link>
        {note ? (
          <span className="max-w-[45%] shrink-0 truncate text-right text-xs text-bone-faint" title={note}>
            {note}
          </span>
        ) : null}
      </li>
    )
  }

  const keyword = (word: string) => {
    const entity = entities.get(word.trim().toLocaleLowerCase())
    return (
      <li key={word}>
        {/* Worn as a genre wears its chip, filled and edged in its colour —
            the site's chip that leads somewhere — in the neutral a word has
            where a genre has a tint; an outline alone is the chip that only
            labels. Under the pointer its edge takes the colour of its text, as
            a genre's does. */}
        <Link
          to={entity?.id ? keywordPath(entity.id) : `/browse?keyword=${encodeURIComponent(word)}`}
          title={entity?.id ? t.work.keywordPage(word) : undefined}
          className="hit group inline-flex min-h-8 items-center gap-1 rounded-full border border-bone-faint/35 bg-bone-faint/10 px-2.5 text-xs font-medium text-bone-dim transition-colors duration-150 hover:border-current hover:text-bone focus-visible:outline-offset-2"
        >
          <span
            aria-hidden
            className="font-mono text-body-sm leading-none text-bone-faint transition-colors duration-150 group-hover:text-vermillion"
          >
            #
          </span>
          {word}
        </Link>
      </li>
    )
  }

  const titleList = 'grid gap-x-8 gap-y-2 sm:grid-cols-2'
  const keywordList = 'flex flex-wrap gap-x-1.5 gap-y-3'

  return (
    <section id="titles" className={SECTION_MARGIN}>
      {/* Named for what it holds: a work with keywords and no other title
          had them under "Also known as". */}
      <SectionTitle>{titles.length ? t.work.alsoKnownAs : t.work.keywords}</SectionTitle>

      {titles.length ? (
        <>
          <ul className={titleList}>{titles.slice(0, SHOWN_TITLES).map(title)}</ul>
          {titles.length > SHOWN_TITLES ? (
            <Fold label={t.work.allTitles(titles.length)} className="mt-3">
              <ul className={cn('mt-2', titleList)}>{titles.slice(SHOWN_TITLES).map(title)}</ul>
            </Fold>
          ) : null}
        </>
      ) : null}

      {keywords.length ? (
        <div className={titles.length ? 'mt-6' : undefined}>
          {titles.length ? <h3 className="label mb-2">{t.work.keywords}</h3> : null}
          <ul className={keywordList}>{keywords.slice(0, SHOWN_KEYWORDS).map(keyword)}</ul>
          {keywords.length > SHOWN_KEYWORDS ? (
            <Fold label={t.work.allKeywords(keywords.length)} className="mt-4">
              <ul className={cn('mt-3', keywordList)}>{keywords.slice(SHOWN_KEYWORDS).map(keyword)}</ul>
            </Fold>
          ) : null}
        </div>
      ) : null}
    </section>
  )
}
