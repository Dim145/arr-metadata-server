/**
 * Reading a work the way a catalogue page needs it.
 *
 * The canonical model carries everything every provider said; a page wants one
 * poster, one backdrop, the best score and a runtime that reads as a duration.
 * Choosing among them is a judgement, so it lives here rather than being made
 * three different ways in three components.
 */

import * as fmt from './format'
import type { Credit, Episode, Image, MediaItem, Rating } from './types'

/**
 * What an image is on the page, which decides how big a file it needs.
 *
 * `card` is a poster in a grid, `poster` the one on a work's own page,
 * `backdrop` the wide strip behind a header, `headshot` a cast member and
 * `still` an episode; `frame` an episode's still drawn wide on its own page,
 * `portrait` a person's photograph on theirs, and `full` whatever the provider
 * has, for the lightbox.
 */
export type ImageRole = 'card' | 'poster' | 'backdrop' | 'headshot' | 'still' | 'frame' | 'portrait' | 'full'

/**
 * TMDB's own size ladder for each role, in the widths it actually serves.
 *
 * Every stored TMDB URL points at `original` — two to three thousand pixels and
 * over half a megabyte for a poster shown a hundred and ninety wide. A front
 * page of twenty-four posters was tens of megabytes. These are the published
 * sizes (`w342` of the same poster is 48 KB); the browser picks among them by
 * layout width and screen density through `srcSet`.
 */
const TMDB_LADDER: Record<ImageRole, readonly number[]> = {
  card: [185, 342, 500],
  poster: [342, 500, 780],
  backdrop: [780, 1280],
  headshot: [185],
  still: [300],
  // TMDB cuts stills at 92, 185 and 300 and nothing between that and the
  // original, so a wide still is the original past the first step.
  frame: [300],
  portrait: [185],
  full: [],
}

/**
 * The step past the ladder, where TMDB has one: the original for a still,
 * `h632` for a photograph. Widths are what the step is at least.
 */
const TMDB_BEYOND: Partial<Record<ImageRole, { size: string; width: number }>> = {
  frame: { size: 'original', width: 1920 },
  portrait: { size: 'h632', width: 421 },
}

/** The layout width each role is drawn at, for `sizes`. */
const SIZES: Record<ImageRole, string> = {
  card: '(min-width: 1280px) 200px, (min-width: 768px) 22vw, 45vw',
  poster: '(min-width: 1024px) 192px, 160px',
  backdrop: '100vw',
  headshot: '56px',
  still: '240px',
  frame: '(min-width: 1024px) 880px, 100vw',
  portrait: '(min-width: 640px) 224px, 160px',
  full: '100vw',
}

const TMDB = /^(https:\/\/image\.tmdb\.org\/t\/p\/)original(\/[^?#]+\.(?:jpe?g|png|webp))$/i
const TVDB = /^(https:\/\/artworks\.thetvdb\.com\/banners\/.+?)(\.(?:jpe?g|png))$/i

/** The attributes an `<img>` needs to fetch no more than it will show. */
export interface Sourced {
  src: string
  srcSet?: string
  sizes?: string
  /** The full-size original, to fall back on if a thumbnail is missing. */
  original: string
}

/**
 * The right file for an image in a role.
 *
 * TMDB gets its ladder. TheTVDB publishes a thumbnail beside each artwork —
 * the same path with `_t` before the extension — which is used for the small
 * roles, with the original kept as a fallback because an old upload is not
 * guaranteed to have one. Anything else, including a URL somebody typed by
 * hand, is used as it is.
 */
export function sized(url: string, role: ImageRole): Sourced {
  const tmdb = TMDB.exec(url)
  const [, base, path] = tmdb ?? []
  if (base && path) {
    const ladder = TMDB_LADDER[role]
    if (!ladder.length) {
      return { src: url, original: url }
    }

    const beyond = TMDB_BEYOND[role]
    const steps = ladder.map((w) => `${base}w${w}${path} ${w}w`)
    if (beyond) {
      steps.push(`${base}${beyond.size}${path} ${beyond.width}w`)
    }

    return {
      src: `${base}w${ladder[Math.min(1, ladder.length - 1)]}${path}`,
      srcSet: steps.join(', '),
      sizes: SIZES[role],
      original: url,
    }
  }

  const [, stem, extension] = TVDB.exec(url) ?? []
  const small = role === 'card' || role === 'headshot' || role === 'still'
  if (stem && extension && small && !stem.endsWith('_t')) {
    return { src: `${stem}_t${extension}`, original: url }
  }

  return { src: url, original: url }
}

/**
 * Swap a thumbnail that failed for the original it was made from.
 *
 * Wired to `onError`, and only once: the original failing too is a broken
 * image, not a reason to loop.
 */
export function fallBackToOriginal(original: string) {
  return (event: { currentTarget: HTMLImageElement }) => {
    const img = event.currentTarget
    if (img.dataset.fellBack) {
      return
    }
    img.dataset.fellBack = 'true'
    img.removeAttribute('srcset')
    img.src = original
  }
}

/** Cover types as the server spells them. */
const COVER = {
  poster: 'poster',
  fanart: 'fanart',
  banner: 'banner',
  clearlogo: 'clearlogo',
  landscape: 'landscape',
} as const

/**
 * The image to show for a kind, preferring one a person chose.
 *
 * `images` arrives sorted by the merge engine — most-wanted first — so the
 * first match of a kind is already the best one a provider offered. A manual
 * image outranks it: somebody picked that on purpose.
 */
export function pick(images: Image[] | undefined, kind: string, season?: number): Image | undefined {
  if (!images?.length) return undefined

  const matches = images.filter(
    (image) =>
      image.coverType === kind &&
      (season === undefined ? image.seasonNumber === undefined || image.seasonNumber === null : image.seasonNumber === season),
  )

  return matches.find((image) => image.isManual) ?? matches[0]
}

export function poster(item: Pick<MediaItem, 'images'>, season?: number) {
  return pick(item.images, COVER.poster, season)?.url
}

export function backdrop(item: Pick<MediaItem, 'images'>) {
  return pick(item.images, COVER.fanart)?.url ?? pick(item.images, COVER.landscape)?.url
}

export function logo(item: Pick<MediaItem, 'images'>) {
  return pick(item.images, COVER.clearlogo)?.url
}

/**
 * The ratings worth showing, the one to lead with first.
 *
 * Only marks out of ten: anything with no value is not a rating, whatever the
 * provider called it, and anything above ten is TheTVDB's popularity figure,
 * which older entries still carry until their next refresh.
 */
export function ratingsOf(ratings: Rating[] | undefined): Rating[] {
  const real = (ratings ?? []).filter(
    (r) => typeof r.value === 'number' && r.value > 0 && r.value <= 10,
  )

  const byVotes = [...real].sort((a, b) => (b.votes ?? 0) - (a.votes ?? 0))
  const imdb = byVotes.find((r) => r.source === 'imdb')

  return imdb ? [imdb, ...byVotes.filter((r) => r !== imdb)] : byVotes
}

/**
 * The score to lead with: the same one Sonarr is given.
 *
 * IMDb's when there is one — it is what Skyhook serves, so it is the figure
 * Sonarr has always shown — and otherwise the one with the most votes behind
 * it. The server picks by the same rule; see `MediaItem::headline_rating`.
 */
export function headlineRating(ratings: Rating[] | undefined): Rating | undefined {
  return ratingsOf(ratings)[0]
}

/** Cast, in billing order, capped so a page is not a phone book. */
export function cast(credits: Credit[] | undefined, limit = 18): Credit[] {
  return (credits ?? [])
    .filter((c) => c.creditType === 'actor')
    .sort((a, b) => a.sortOrder - b.sortOrder)
    .slice(0, limit)
}

/** Everyone who is not on screen, with their job. */
export function crew(credits: Credit[] | undefined, limit = 12): Credit[] {
  return (credits ?? [])
    .filter((c) => c.creditType !== 'actor')
    .sort((a, b) => a.sortOrder - b.sortOrder)
    .slice(0, limit)
}

/** Season numbers a work actually has episodes for, specials last. */
export function seasonNumbers(item: MediaItem): number[] {
  const numbers = new Set<number>()

  for (const episode of item.episodes ?? []) {
    numbers.add(episode.seasonNumber)
  }
  for (const season of item.seasons ?? []) {
    numbers.add(season.seasonNumber)
  }

  // Specials are season 0 and belong at the end, where a reader expects them,
  // not before the first season.
  return [...numbers].sort((a, b) => (a === 0 ? 1 : b === 0 ? -1 : a - b))
}

export function episodesOf(item: MediaItem, season: number) {
  return (item.episodes ?? [])
    .filter((e) => e.seasonNumber === season)
    .sort((a, b) => a.episodeNumber - b.episodeNumber)
}

/** `S01E02`, the way every release name and every client writes it. */
export function episodeCode(episode: Pick<Episode, 'seasonNumber' | 'episodeNumber'>): string {
  const two = (n: number) => String(n).padStart(2, '0')
  return `S${two(episode.seasonNumber)}E${two(episode.episodeNumber)}`
}

/**
 * When an episode airs, for sorting: the moment a provider knew, otherwise
 * midnight UTC on its date — the same instant Sonarr is given. Not for saying
 * whether it has aired, or when: see `airTime`.
 */
export function airsAt(episode: Pick<Episode, 'airDate' | 'airDateUtc'>): number | undefined {
  const raw = episode.airDateUtc ?? (episode.airDate ? `${episode.airDate}T00:00:00Z` : undefined)
  const time = raw ? Date.parse(raw) : Number.NaN
  return Number.isNaN(time) ? undefined : time
}

/** When an episode airs, as precisely as anybody knows it. */
export interface AirTime {
  /** The reader's own calendar date of it: `2026-09-22`. */
  day: string
  /** The moment, where a provider knew one. */
  moment?: Date
}

/**
 * A moment where a provider gave a full timestamp, and otherwise a day — the
 * date the network gave, which is the same day for every reader. Read as the
 * midnight UTC it is stored at, it moved to the evening before for anybody
 * west of Greenwich, and aired "in 8 hours" there. A value an override made
 * something else — a bare date, a typing slip — says a day at most, and
 * never breaks the page it is on.
 */
export function airTime(episode: Pick<Episode, 'airDate' | 'airDateUtc'>): AirTime | undefined {
  const utc = episode.airDateUtc?.trim()
  if (utc && /^\d{4}-\d{2}-\d{2}T/.test(utc)) {
    const moment = new Date(utc)
    if (!Number.isNaN(moment.getTime())) return { day: fmt.localDay(moment), moment }
  }

  const day = [utc, episode.airDate?.trim()].find((v): v is string => !!v && /^\d{4}-\d{2}-\d{2}$/.test(v))
  return day ? { day } : undefined
}

/**
 * The value to hand a date formatter for it: the moment, so the day comes out
 * the reader's own, or the day as it was given.
 */
export function airValue(time: AirTime | undefined): string | undefined {
  return time?.moment?.toISOString() ?? time?.day
}

/** Whether it has aired by `now`: a moment once it is past, a day once it is over. */
export function hasAired(time: AirTime, now = new Date()): boolean {
  return time.moment ? time.moment.getTime() <= now.getTime() : time.day < fmt.localDay(now)
}

/**
 * How far off it is: in hours or minutes for a moment later the same day, and
 * otherwise in the reader's calendar days — never an hour made up for an
 * episode nobody gave a time.
 */
export function airsWhen(time: AirTime, locale: string, now = new Date()): string {
  if (time.moment && time.day === fmt.localDay(now)) {
    return fmt.relative(time.moment.toISOString(), locale) ?? fmt.dayDistance(time.day, locale, now)
  }
  return fmt.dayDistance(time.day, locale, now)
}

/** The regular episodes in airing order, specials left out. */
function regular(item: MediaItem): Episode[] {
  return (item.episodes ?? [])
    .filter((e) => e.seasonNumber > 0 && airTime(e) !== undefined)
    .sort((a, b) => (airsAt(a) ?? 0) - (airsAt(b) ?? 0))
}

/** The next regular episode to air, if one is known: tonight's until tonight is over. */
export function nextEpisode(item: MediaItem, now = new Date()): Episode | undefined {
  return regular(item).find((e) => !hasAired(airTime(e)!, now))
}

/** The last regular episode to have aired by `now`. */
export function latestEpisode(item: MediaItem, now = new Date()): Episode | undefined {
  return regular(item)
    .filter((e) => hasAired(airTime(e)!, now))
    .at(-1)
}

/** The seasons either side of `season` in reading order, specials last. */
export function adjacentSeasons(item: MediaItem, season: number): { before?: number; after?: number } {
  const numbers = seasonNumbers(item)
  const at = numbers.indexOf(season)
  return {
    before: at > 0 ? numbers[at - 1] : undefined,
    after: at >= 0 ? numbers[at + 1] : undefined,
  }
}

/** The episodes either side of one, in reading order across seasons. */
export function adjacentEpisodes(
  item: MediaItem,
  episode: Pick<Episode, 'seasonNumber' | 'episodeNumber'>,
): { before?: Episode; after?: Episode } {
  const order = seasonNumbers(item).flatMap((n) => episodesOf(item, n))
  const at = order.findIndex(
    (e) => e.seasonNumber === episode.seasonNumber && e.episodeNumber === episode.episodeNumber,
  )
  return at < 0 ? {} : { before: order[at - 1], after: order[at + 1] }
}

/** Every image of one kind, a person's choice first, then in the order given. */
export function imagesOf(item: Pick<MediaItem, 'images'>, kind: string): Image[] {
  return (item.images ?? [])
    .filter(
      (image) =>
        image.coverType === kind && (image.seasonNumber === undefined || image.seasonNumber === null),
    )
    .sort((a, b) => Number(b.isManual) - Number(a.isManual) || a.sortOrder - b.sortOrder)
}

/** Whether a person has claimed this field, so the interface can say so. */
export function isLocked(item: Pick<MediaItem, 'lockedFields'>, path: string): boolean {
  return (item.lockedFields ?? []).includes(path)
}

/**
 * What to call a season: its own title if it has a real one, otherwise its
 * number in the reader's language.
 *
 * Providers name most seasons "Season 1" or "Specials" in English, which is a
 * placeholder rather than a title; a real one — the part title an anime uses —
 * is worth showing instead.
 */
export function seasonName(
  title: string | undefined,
  number: number,
  byNumber: (n: number) => string,
): string {
  const given = title?.trim()
  const generic = /^(season|saison|series|specials?|hors-s\u00e9rie)\s*\d*$/i
  return given && !generic.test(given) ? given : byNumber(number)
}
