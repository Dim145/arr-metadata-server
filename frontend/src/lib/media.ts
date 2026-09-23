/**
 * Reading a work the way a catalogue page needs it.
 *
 * The canonical model carries everything every provider said; a page wants one
 * poster, one backdrop, the best score and a runtime that reads as a duration.
 * Choosing among them is a judgement, so it lives here rather than being made
 * three different ways in three components.
 */

import type { Credit, Image, MediaItem, Rating } from './types'

/**
 * What an image is on the page, which decides how big a file it needs.
 *
 * `card` is a poster in a grid, `poster` the one on a work's own page,
 * `backdrop` the wide strip behind a header, `headshot` a cast member and
 * `still` an episode.
 */
export type ImageRole = 'card' | 'poster' | 'backdrop' | 'headshot' | 'still'

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
}

/** The layout width each role is drawn at, for `sizes`. */
const SIZES: Record<ImageRole, string> = {
  card: '(min-width: 1280px) 200px, (min-width: 768px) 22vw, 45vw',
  poster: '(min-width: 1024px) 192px, 160px',
  backdrop: '100vw',
  headshot: '56px',
  still: '240px',
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
    return {
      src: `${base}w${ladder[Math.min(1, ladder.length - 1)]}${path}`,
      srcSet: ladder.map((w) => `${base}w${w}${path} ${w}w`).join(', '),
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
 * The score to lead with.
 *
 * Providers disagree and count votes on different scales. TMDB is preferred
 * because it is the one with a vote count most works actually have; anything
 * with no value at all is not a rating, whatever the provider called it.
 */
export function headlineRating(ratings: Rating[] | undefined): Rating | undefined {
  const real = (ratings ?? []).filter((r) => typeof r.value === 'number' && r.value > 0)

  if (!real.length) return undefined

  const byVotes = [...real].sort((a, b) => (b.votes ?? 0) - (a.votes ?? 0))

  return real.find((r) => r.source === 'tmdb') ?? byVotes[0]
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
