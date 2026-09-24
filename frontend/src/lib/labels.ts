/**
 * Words the providers hand over in English, said in the reader's language.
 *
 * The server stores what its providers answered in whatever language it asks
 * them in, and overlays the caller's language on titles and synopses. Genres,
 * statuses and language codes are not overlaid — they are values, not prose —
 * so a French page said "Drama", "ended" and "ENG" beside a French synopsis.
 * These are small, closed vocabularies, which is what makes translating them
 * here honest rather than a guess.
 */

import type { Dict, Lang } from './i18n'

/**
 * TMDB's movie and television genres and TheTVDB's, in French.
 *
 * Anything not listed is shown as it came: a server set to answer in French
 * already stores French genres, and an unknown one is better shown than lost.
 */
const GENRES_FR: Record<string, string> = {
  Action: 'Action',
  'Action & Adventure': 'Action & aventure',
  Adventure: 'Aventure',
  Animation: 'Animation',
  Anime: 'Anime',
  Children: 'Jeunesse',
  Comedy: 'Comédie',
  Crime: 'Crime',
  Documentary: 'Documentaire',
  Drama: 'Drame',
  Family: 'Familial',
  Fantasy: 'Fantastique',
  Food: 'Cuisine',
  'Game Show': 'Jeu télévisé',
  History: 'Histoire',
  'Home and Garden': 'Maison et jardin',
  Horror: 'Horreur',
  Indie: 'Indépendant',
  Kids: 'Jeunesse',
  'Martial Arts': 'Arts martiaux',
  'Mini-Series': 'Mini-série',
  Music: 'Musique',
  Musical: 'Comédie musicale',
  Mystery: 'Mystère',
  News: 'Actualités',
  Podcast: 'Podcast',
  Reality: 'Téléréalité',
  Romance: 'Romance',
  'Sci-Fi & Fantasy': 'Science-fiction & fantastique',
  'Science Fiction': 'Science-fiction',
  'Science-Fiction': 'Science-fiction',
  Soap: 'Feuilleton',
  Sport: 'Sport',
  Suspense: 'Suspense',
  Talk: 'Talk-show',
  'Talk Show': 'Talk-show',
  Thriller: 'Thriller',
  Travel: 'Voyage',
  'TV Movie': 'Téléfilm',
  War: 'Guerre',
  'War & Politics': 'Guerre & politique',
  Western: 'Western',
}

export function genreLabel(name: string, lang: Lang): string {
  return lang === 'fr' ? (GENRES_FR[name] ?? name) : name
}

/**
 * A language's name from the code the server stores (`eng`, `fra`, `ja`).
 *
 * `Intl.DisplayNames` knows two- and three-letter codes alike. French names
 * come back lower-case, which reads as a typo at the start of a table cell.
 */
export function languageName(code: string | undefined, locale: string): string | undefined {
  if (!code) {
    return undefined
  }

  try {
    const name = new Intl.DisplayNames([locale], { type: 'language' }).of(code)
    if (!name || name === code) {
      return code.toUpperCase()
    }
    return name.charAt(0).toLocaleUpperCase(locale) + name.slice(1)
  } catch {
    return code.toUpperCase()
  }
}

/** A work's status — `ended`, `inCinemas` — as a word. */
export function statusLabel(status: string | undefined, t: Dict): string | undefined {
  if (!status) {
    return undefined
  }
  const known = t.labels.status as Record<string, string>
  return known[status] ?? status
}

/**
 * A provider or rating source by its own name.
 *
 * The server files everything under a short key — `mal`, `tvdb`,
 * `rottenTomatoes` — and those are identifiers, not names. These are proper
 * nouns, the same in every language, so they live here rather than in the
 * dictionary.
 */
const PROVIDERS: Record<string, string> = {
  anilist: 'AniList',
  fanart: 'Fanart.tv',
  imdb: 'IMDb',
  mal: 'MyAnimeList',
  metacritic: 'Metacritic',
  radarr: 'Radarr',
  rottenTomatoes: 'Rotten Tomatoes',
  skyhook: 'Skyhook',
  tmdb: 'TMDB',
  trakt: 'Trakt',
  tvdb: 'TheTVDB',
  tvmaze: 'TVmaze',
  tvrage: 'TVRage',
}

export function providerName(key: string): string {
  return PROVIDERS[key] ?? key
}

/** How a surface authenticates — `apikey`, `allowlist` — as a word. */
export function policyLabel(policy: string, t: Dict): string {
  const known = t.labels.policy as Record<string, string>
  return known[policy] ?? policy
}

/**
 * Who is signed in, from the server's label for them (`admin:alice`,
 * `client:jellyseerr`, `anonymous`).
 *
 * Shown raw, an administrator named admin read as `admin:admin` — which is
 * exactly what a default username and password look like written down.
 */
export function describeIdentity(identity: string, t: Dict): { name: string; role: string } {
  const at = identity.indexOf(':')
  const kind = at === -1 ? identity : identity.slice(0, at)
  const name = at === -1 ? '' : identity.slice(at + 1)

  switch (kind) {
    case 'admin':
      return { name, role: t.labels.roles.admin }
    case 'client':
      return { name, role: t.labels.roles.client }
    case 'anonymous':
      return { name: t.labels.roles.anonymous, role: '' }
    default:
      return { name: name || identity, role: '' }
  }
}
