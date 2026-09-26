/**
 * Where each identifier leads: the work's own page on the site that gave it.
 *
 * Built from ids alone, in the forms each site keeps working: TheTVDB's
 * `dereferrer` redirects an id to whatever the page is called this year, TMDB
 * and TVmaze accept an id without the slug they would add, IMDb, MyAnimeList
 * and AniList are addressed by id outright.
 *
 * Left unlinked on purpose: TVRage, closed since 2015, and Trakt, whose pages
 * are addressed by a slug this server never learns.
 */

import type { Episode, MediaItem, MediaKind, Season } from './types'

export interface Destination {
  /** The source's key: `tmdb`, `tvdb`, `imdb`… */
  source: string
  href: string
}

/** The page one identifier stands for, when there is one to open. */
export function identifierLink(
  source: string,
  value: string | number,
  kind: MediaKind,
): string | undefined {
  const id = encodeURIComponent(String(value))

  switch (source) {
    case 'tmdb':
      return `https://www.themoviedb.org/${kind === 'series' ? 'tv' : 'movie'}/${id}`
    case 'tvdb':
      // TheTVDB's redirect for films does not answer; its film pages are only
      // reachable by slug.
      return kind === 'series' ? `https://thetvdb.com/dereferrer/series/${id}` : undefined
    case 'imdb':
      return `https://www.imdb.com/title/${id}/`
    case 'tvmaze':
      return `https://www.tvmaze.com/shows/${id}`
    case 'mal':
      return `https://myanimelist.net/anime/${id}`
    case 'anilist':
      return `https://anilist.co/anime/${id}`
    case 'fankai':
      return `https://fankai.fr/productions/${id}`
    default:
      return undefined
  }
}

/** A work's pages elsewhere, in the order the identifiers panel lists them. */
export function workLinks(item: MediaItem): Destination[] {
  const out: Destination[] = []
  const ids = item.externalIds

  for (const source of ['tmdb', 'tvdb', 'imdb', 'tvmaze', 'fankai'] as const) {
    const value = ids[source]
    const href = value === undefined || value === null ? undefined : identifierLink(source, value, item.kind)
    if (href) out.push({ source, href })
  }

  // The first of several entries is the one that stands for the whole work.
  for (const source of ['mal', 'anilist'] as const) {
    const first = ids[source]?.[0]
    const href = first === undefined ? undefined : identifierLink(source, first, item.kind)
    if (href) out.push({ source, href })
  }

  return out
}

/**
 * A season's page on TMDB, by TMDB's own season.
 *
 * Only where TMDB's season was matched to this one: TMDB numbers some series'
 * seasons differently, and the season number shown here is TheTVDB's.
 */
export function seasonLinks(item: MediaItem, season: Season | undefined): Destination[] {
  const tmdb = item.externalIds.tmdb
  if (!season || !tmdb || !season.tmdbId) return []
  return [
    {
      source: 'tmdb',
      href: `https://www.themoviedb.org/tv/${tmdb}/season/${season.seasonNumber}`,
    },
  ]
}

/** An episode's pages elsewhere. */
export function episodeLinks(item: MediaItem, episode: Episode): Destination[] {
  const out: Destination[] = []

  if (episode.tvdbId) {
    out.push({ source: 'tvdb', href: `https://thetvdb.com/dereferrer/episode/${episode.tvdbId}` })
  }

  // TMDB addresses an episode by its numbers, not its id — and only where it
  // matched this one does it have the same numbers.
  const tmdb = item.externalIds.tmdb
  if (tmdb && episode.tmdbId) {
    out.push({
      source: 'tmdb',
      href: `https://www.themoviedb.org/tv/${tmdb}/season/${episode.seasonNumber}/episode/${episode.episodeNumber}`,
    })
  }

  return out
}

/** A person's page on TMDB. */
export function personLink(tmdbId: number): string {
  return `https://www.themoviedb.org/person/${tmdbId}`
}

/** A trailer, played from YouTube's no-cookie domain. */
export function trailerEmbed(youtubeId: string): string {
  return `https://www.youtube-nocookie.com/embed/${encodeURIComponent(youtubeId)}?autoplay=1&rel=0`
}

/** The same trailer on YouTube itself, for somebody who would rather go there. */
export function trailerPage(youtubeId: string): string {
  return `https://www.youtube.com/watch?v=${encodeURIComponent(youtubeId)}`
}
