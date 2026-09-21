/**
 * Reading a work the way a catalogue page needs it.
 *
 * The canonical model carries everything every provider said; a page wants one
 * poster, one backdrop, the best score and a runtime that reads as a duration.
 * Choosing among them is a judgement, so it lives here rather than being made
 * three different ways in three components.
 */

import type { Credit, Image, MediaItem, Rating } from './types'

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
