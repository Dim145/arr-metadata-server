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

import type { Episode, MediaItem, MediaKind, Relation, Season } from './types'

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
    // The id is Fankai's metadata service's, which numbers its productions
    // its own way; the website numbers them another, behind a sign-in, and
    // nothing says which of its pages a production is. No link beats the
    // wrong one — the work's homepage is its page on the Fankai wiki.
    case 'fankai':
      return undefined
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

/**
 * Whether an address is one a browser may be sent to: `http` or `https`,
 * nothing else. A provider's field and a locked one are both text, and
 * `javascript:` is text too — rendered as a link, it would run in the page
 * of whoever clicked it.
 */
export function isWebAddress(href: string): boolean {
  try {
    const { protocol } = new URL(href)
    return protocol === 'http:' || protocol === 'https:'
  } catch {
    return false
  }
}

/** Where a related work the catalogue does not hold is found, by where it is filed. */
export function relationLink(relation: Pick<Relation, 'source' | 'externalId' | 'medium'>): string {
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
