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
  Politics: 'Politique',
  'War & Politics': 'Guerre & politique',
  Western: 'Western',
}

export function genreLabel(name: string, lang: Lang): string {
  return lang === 'fr' ? (GENRES_FR[name] ?? name) : name
}

/**
 * TMDB's three combined series genres, in the two languages this interface
 * speaks, and the film genres each stands for. The server lists works the same
 * way (`genre_parts`); a genre typed by hand stays whole.
 */
const COMBINED: Record<string, string[]> = {
  'Action & Adventure': ['Action', 'Adventure'],
  'Sci-Fi & Fantasy': ['Science Fiction', 'Fantasy'],
  'War & Politics': ['War', 'Politics'],
  'Action & Aventure': ['Action', 'Aventure'],
  'Science-Fiction & Fantastique': ['Science-Fiction', 'Fantastique'],
  'Guerre & Politique': ['Guerre', 'Politique'],
}

/**
 * The genres one of a work's stands for, as the server lists them: a series'
 * "Action & Adventure" is the "Action" and "Adventure" films are filed under,
 * so that one filter finds both.
 */
export function genreParts(genre: string): string[] {
  const name = genre.trim()
  return name ? (COMBINED[name] ?? [name]) : []
}

/** A work's genres, each once, as a list files them. */
export function listedGenres(genres: string[]): string[] {
  return [...new Set(genres.flatMap(genreParts))]
}

/**
 * The jobs TMDB credits most, in French: what a film's credits open with.
 *
 * TMDB names a job in English whatever language it is asked in, so a French
 * page said "Director" above Coppola. Named as French credits name them — the
 * work done, "Réalisation", rather than the person, "Réalisateur", which
 * would need a gender TMDB does not give.
 */
const JOBS_FR: Record<string, string> = {
  Director: 'Réalisation',
  'Co-Director': 'Coréalisation',
  Creator: 'Création',
  Writer: 'Scénario',
  Screenplay: 'Scénario',
  Teleplay: 'Scénario',
  Story: 'Histoire',
  Novel: 'Roman',
  Characters: 'Personnages',
  Author: 'Œuvre originale',
  Producer: 'Production',
  'Executive Producer': 'Production exécutive',
  'Co-Producer': 'Coproduction',
  'Original Music Composer': 'Musique',
  Music: 'Musique',
  'Director of Photography': 'Photographie',
  Editor: 'Montage',
  Casting: 'Distribution des rôles',
  'Production Design': 'Décors',
  'Costume Design': 'Costumes',
  'Art Direction': 'Direction artistique',
  'Visual Effects Supervisor': 'Effets visuels',
  Showrunner: 'Showrunner',
  // The kind of credit, where a hand-added one has no job of its own.
  director: 'Réalisation',
  writer: 'Scénario',
  producer: 'Production',
}

export function jobLabel(job: string, lang: Lang): string {
  return lang === 'fr' ? (JOBS_FR[job] ?? job) : job
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

/**
 * The countries the server files alternative titles under, by the code it
 * stores for each — the inverse of `iso_3166_2_to_3` in src/providers/lang.rs.
 * A country outside that table is stored in the two letters TMDB sent.
 */
const COUNTRIES = new Map(
  (
    'are:AE arg:AR aut:AT aus:AU bel:BE bgr:BG bra:BR can:CA che:CH chl:CL chn:CN col:CO ' +
    'cze:CZ deu:DE dnk:DK est:EE egy:EG esp:ES fin:FI fra:FR gbr:GB grc:GR hkg:HK hrv:HR ' +
    'hun:HU idn:ID irl:IE isr:IL ind:IN irn:IR isl:IS ita:IT jpn:JP kor:KR ltu:LT lux:LU ' +
    'lva:LV mex:MX mys:MY nld:NL nor:NO nzl:NZ per:PE phl:PH pol:PL prt:PT rou:RO srb:RS ' +
    'rus:RU sau:SA swe:SE sgp:SG svn:SI svk:SK tha:TH tur:TR twn:TW ukr:UA usa:US vnm:VN ' +
    'zaf:ZA'
  )
    .split(' ')
    .map((pair) => pair.split(':') as [string, string]),
)

/**
 * Where an alternative title is from, in words.
 *
 * TMDB, AniList and MyAnimeList file a title under a country and TheTVDB under
 * a language, in the same field — so a code is read as a country where it is
 * one this server stores, and as a language otherwise. Read the other way
 * round, Brazil's title was Braj's and China's was Chinook Jargon's. A code
 * nothing recognises is left out, rather than shown to a reader as a code.
 *
 * Radarr's own titles are the exception, filed under a language outright; the
 * caller says so with `reading`.
 */
export function titleOrigin(
  code: string | undefined,
  locale: string,
  reading: 'place' | 'language' = 'place',
): string | undefined {
  if (!code) {
    return undefined
  }

  const lower = code.trim().toLowerCase()
  const named = (type: 'region' | 'language', value: string) => {
    try {
      const name = new Intl.DisplayNames([locale], { type, fallback: 'none' }).of(value)
      return name ? name.charAt(0).toLocaleUpperCase(locale) + name.slice(1) : undefined
    } catch {
      return undefined
    }
  }

  if (reading === 'language') {
    return named('language', lower)
  }

  // The same three letters for a country TMDB names and a language TheTVDB
  // does, meaning different things: Belgium or Belarusian, India or
  // Indonesian. Which was meant is not stored, so neither is said.
  if (AMBIGUOUS.has(lower)) {
    return undefined
  }

  const region = COUNTRIES.get(lower) ?? (/^[a-z]{2}$/.test(lower) ? lower.toUpperCase() : undefined)
  return (region && named('region', region)) || (/^[a-z]{3}$/.test(lower) ? named('language', lower) : undefined)
}

/**
 * Countries in `COUNTRIES` whose code is also a language's that TheTVDB
 * could send: Argentina / Aragonese, Belgium / Belarusian, Switzerland /
 * Chechen, Egypt / Egyptian, India / Indonesian, Peru / Persian.
 */
const AMBIGUOUS = new Set(['arg', 'bel', 'che', 'egy', 'ind', 'per'])

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
  fankai: 'Fankai',
  fankaiwiki: 'Wiki Fankai',
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
