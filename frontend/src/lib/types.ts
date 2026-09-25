/** Mirrors the canonical model the native API serializes. */

export type MediaKind = 'series' | 'movie'

export interface ExternalIds {
  tmdb?: number
  tvdb?: number
  imdb?: string
  tvmaze?: number
  tvrage?: number
  mal?: number[]
  anilist?: number[]
  trakt?: number
}

export interface Rating {
  source: string
  value?: number
  votes?: number
  ratingType?: string
}

export interface Image {
  id: string
  seasonNumber?: number
  coverType: string
  url: string
  sortOrder: number
  source?: string
  isManual: boolean
}

export interface Credit {
  id: string
  creditType: string
  personName: string
  characterName?: string
  image?: string
  /** TMDB's id for the person, which is what a person's page is found by. */
  tmdbPersonId?: number
  sortOrder: number
  isManual: boolean
}

/**
 * Another work a provider files beside one — its sequel, prequel, side story —
 * as AniList names, dates and pictures it, with the work here that it is when
 * the catalogue holds it.
 */
export interface Relation {
  id: string
  /** SEQUEL, PREQUEL, PARENT, SIDE_STORY, SPIN_OFF, ALTERNATIVE, SUMMARY, COMPILATION, CONTAINS, SOURCE, ADAPTATION, CHARACTER or OTHER. */
  relationType: string
  /** Where the other work is filed — `anilist` — and its id there. */
  source: string
  externalId: number
  malId?: number
  title: string
  /** `anime` or `manga`. */
  medium: string
  /** TV, MOVIE, OVA, ONA, SPECIAL, MANGA, … */
  format?: string
  year?: number
  image?: string
  /** For adults, as AniList flags it or as the catalogue holds it; a reader kept from such works is not sent it. */
  isAdult: boolean
  workId?: string
  sortOrder: number
}

export interface AlternativeTitle {
  id: string
  title: string
  titleType?: string
  language?: string
  isManual: boolean
}

export interface Season {
  id: string
  seasonNumber: number
  title?: string
  overview?: string
  airDate?: string
  tmdbId?: number
  tvdbId?: number
  isManual: boolean
  images?: Image[]
}

export interface Episode {
  id: string
  seasonNumber: number
  episodeNumber: number
  absoluteEpisodeNumber?: number
  /** Where a special belongs among the regular episodes, as TheTVDB files it. */
  airedAfterSeasonNumber?: number
  airedBeforeSeasonNumber?: number
  airedBeforeEpisodeNumber?: number
  title: string
  overview?: string
  airDate?: string
  airDateUtc?: string
  runtime?: number
  finaleType?: string
  image?: string
  tvdbId?: number
  tmdbId?: number
  rating?: { value: number; votes: number }
  isManual: boolean
}

export interface Translation {
  language: string
  title?: string
  overview?: string
  isManual: boolean
}

export interface MediaItem {
  id: string
  kind: MediaKind
  slug: string
  title: string
  sortTitle?: string
  originalTitle?: string
  overview?: string
  status?: string
  originalLanguage?: string
  originalCountry?: string
  runtime?: number
  year?: number
  firstAired?: string
  lastAired?: string
  inCinemas?: string
  physicalRelease?: string
  digitalRelease?: string
  airTime?: string
  network?: string
  studio?: string
  contentRating?: string
  homepage?: string
  trailerYoutubeId?: string
  popularity?: number
  collectionTmdbId?: number
  isAdult?: boolean
  genres: string[]
  keywords: string[]
  externalIds: ExternalIds
  isManual: boolean
  isEnabled: boolean
  createdAt: string
  updatedAt: string
  refreshedAt?: string
  refreshAfter?: string
  refreshError?: string
  seasons?: Season[]
  episodes?: Episode[]
  images?: Image[]
  credits?: Credit[]
  alternativeTitles?: AlternativeTitle[]
  ratings?: Rating[]
  translations?: Translation[]
  relations?: Relation[]
  lockedFields?: string[]
}

export type FieldType =
  | 'text'
  | 'longText'
  | 'integer'
  | 'float'
  | 'boolean'
  | 'date'
  | 'dateTime'
  | 'timeOfDay'
  | 'textList'

export interface FieldDef {
  name: string
  fieldType: FieldType
  label: string
}

export interface FieldRegistry {
  item: FieldDef[]
  season: FieldDef[]
  episode: FieldDef[]
}

export interface Override {
  scope: string
  field: string
  value: unknown | null
  updatedAt: string
  updatedBy?: string
}

export interface ApiClient {
  id: string
  name: string
  keyPrefix: string
  scopes: string[]
  isEnabled: boolean
  expiresAt?: string
  createdAt: string
  lastUsedAt?: string
  lastUsedIp?: string
  note?: string
}

export interface AuditEntry {
  id: string
  at: string
  actor?: string
  action: string
  target?: string
  detail?: string
  ip?: string
  /** The work the target names, while it is held. */
  work?: { id: string; title: string; kind: MediaKind }
}

export interface AuditResponse {
  entries: AuditEntry[]
  total: number
  actions: string[]
}

export interface Job {
  id: string
  kind: string
  target?: string
  status: 'running' | 'succeeded' | 'failed'
  startedAt?: string
  finishedAt?: string
  error?: string
  detail?: string
  createdAt: string
  /** The work it acted on, while it is held. */
  work?: { id: string; title: string; kind: MediaKind }
}

export interface JobsResponse {
  jobs: Job[]
  total: number
}

export interface ExportSummary {
  root: string
  works: number
  episodes: number
  failed: number
}

export interface Stats {
  series: number
  movies: number
  total: number
  overrides: number
  clients: number
  auditEntries: number
  jobs: number
  cachedItems: number
  cachedSearches: number
}

export interface Settings {
  version: string
  publicUrl?: string
  database: string
  tmdbConfigured: boolean
  tmdbLanguage: string
  skyhookFallback: boolean
  refreshEnabled: boolean
  authDisabled: boolean
  publicBrowse: boolean
  nativePolicy: string
  tmdbPolicy: string
  arrPolicy: string
  furtherSources: FurtherSources
}

/** A list the server downloads whole: when it last landed, and what it kept. */
export interface ListImport {
  importedAt: string
  rows: number
}

export interface FurtherSources {
  malVia: 'official' | 'jikan'
  animeList?: ListImport
  imdbRatings?: ListImport
}

/** Where the data comes from, for the credits: only what is switched on. */
export interface Sources {
  sources: string[]
}

export interface Me {
  identity: string
  canWrite: boolean
  isAdmin: boolean
  /** Whether a reader with no credential may browse: the feeds answer a calendar app only then. */
  publicBrowse: boolean
}

export interface Snapshot {
  provider: string
  fetchedAt: string
  etag?: string
  /** Absent when the caller asked for `payload=false`. */
  payload?: unknown
}

export interface ItemPage {
  items: MediaItem[]
  total: number
}

/** A service that carries a work in a country, as TMDB lists it from JustWatch. */
export interface WatchProvider {
  id: number
  name: string
  logo?: string
}

export interface WhereToWatch {
  region: string
  link?: string
  flatrate: WatchProvider[]
  rent: WatchProvider[]
  buy: WatchProvider[]
  free: WatchProvider[]
  ads: WatchProvider[]
  /** Every country TMDB lists anything for. */
  regions: string[]
  /** Who the data comes from; shown beside it. */
  attribution: string
}

/** The catalogue's own works in the same vein as one. */
export interface Similar {
  items: MediaItem[]
}

/** What TMDB recommends beside a work, for whoever maintains the catalogue. */
export interface Suggestion {
  tmdbId: number
  kind: MediaKind
  title: string
  year?: number
  overview?: string
  poster?: string
  score?: number
  /** The catalogue's own id, where the work is already in it. */
  held?: string
}

export interface Suggestions {
  suggestions: Suggestion[]
}

export interface CollectionCard {
  tmdbId: number
  name?: string
  poster?: string
  /** How many of its films the catalogue holds. */
  count: number
}

export interface Collections {
  collections: CollectionCard[]
}

export interface CollectionPart {
  tmdbId: number
  title: string
  year?: number
  poster?: string
  held?: string
}

export interface CollectionPage {
  tmdbId: number
  name?: string
  overview?: string
  poster?: string
  backdrop?: string
  items: MediaItem[]
  parts: CollectionPart[]
}

export interface Count {
  name: string
  count: number
}

/** The catalogue in numbers, as a reader is shown it. */
export interface Figures {
  total: number
  series: number
  movies: number
  episodes: number
  addedRecently: number
  decades: Count[]
  genres: Count[]
  networks: Count[]
  languages: Count[]
  scores: Count[]
  statuses: Count[]
}

export interface CacheFigures {
  entries: number
  bytes: number
}

export interface Health {
  version: string
  uptimeSeconds: number
  database: string
  works: number
  episodes: number
  refreshFailed: number
  rules: number
  itemsCache: CacheFigures
  searchesCache: CacheFigures
  listsCache: CacheFigures
  sources: { name: string; on: boolean }[]
  jobs: Job[]
}

export type ListKind = 'series' | 'movie' | 'mixed'
export type ListMode = 'manual' | 'filter'

/** A filter kept with a list, in the list query's own vocabulary. */
export interface ListFilter {
  term?: string
  genres?: string[]
  keyword?: string
  yearFrom?: number
  yearTo?: number
  status?: string
  originalLanguage?: string
  network?: string
  collection?: number
  minRating?: number
  sort?: string
  descending?: boolean
  limit?: number
}

/** A curated list: a selection composed by hand, or by a filter kept current. */
export interface CuratedList {
  id: string
  slug: string
  name: string
  description?: string
  kind: ListKind
  mode: ListMode
  filter?: ListFilter
  isPublic: boolean
  /** The members of a hand-made list; nought for one composed by a filter. */
  itemCount: number
  createdAt: string
  updatedAt: string
}

export interface CuratedLists {
  lists: CuratedList[]
}

export interface CuratedListPage {
  list: CuratedList
  items: MediaItem[]
  total: number
}

export interface CuratedListRequest {
  name: string
  description?: string
  kind: ListKind
  mode: ListMode
  filter?: ListFilter
  isPublic: boolean
  items?: string[]
}

/** One value a list can be narrowed to, and how many works it would leave. */
export interface Facet {
  value: string
  count: number
}

export interface Facets {
  total: number
  genres: Facet[]
  networks: Facet[]
  languages: Facet[]
  statuses: Facet[]
  yearMin?: number
  yearMax?: number
}

export interface Airing {
  workId: string
  episode: Episode
}

export interface Calendar {
  episodes: Airing[]
  works: MediaItem[]
  /** More aired in the window than one answer carries; the latest are missing. */
  truncated: boolean
}

export interface Role {
  workId: string
  creditType: string
  character?: string
}

/** Somebody as TMDB knows them, beside what this catalogue holds of theirs. */
export interface PersonDetails {
  biography?: string
  /** YYYY-MM-DD. */
  birthday?: string
  deathday?: string
  placeOfBirth?: string
  /** What TMDB files them under: Acting, Directing, Writing, … */
  knownFor?: string
  alsoKnownAs?: string[]
  homepage?: string
  imdbId?: string
  /** A few portraits, in TMDB's order. */
  photos?: string[]
}

export interface Person {
  tmdbId: number
  name: string
  image?: string
  roles: Role[]
  works: MediaItem[]
  /** What TMDB says of them, when it is configured and answered. */
  details?: PersonDetails
}


export interface NetworkRule {
  id: string
  cidr: string
  /** What the client behind the address is called. Its settings hang off this. */
  name?: string
  note?: string
  createdAt: string
  createdBy?: string
}

export interface NetworkCaller {
  ip: string
  /** From the hosts file or the system resolver; often a container name. */
  hostname?: string
  userAgent?: string
  lastSurface: string
  lastPath?: string
  lastAllowed: boolean
  /** Whether a rule covers this address now — the server's own answer, not the
   *  last call's outcome, so a rule added since is reflected. */
  covered: boolean
  hits: number
  refusals: number
  firstSeen: string
  lastSeen: string
}

/**
 * A setting, as the server describes it.
 *
 * The interface renders controls from `kind` rather than from a list of its
 * own, so a setting added to the server appears here without this code being
 * touched — which is the only reason the registry is served at all.
 */
export type SettingKind =
  | { type: 'bool' }
  | { type: 'int'; min: number; max: number }
  | { type: 'text' }
  | { type: 'choice'; options: string[] }

export type SettingScope = 'server' | 'client' | 'peer'

export interface SettingDef {
  key: string
  kind: SettingKind
  /** Where it may be set. Anything narrower than the first entry overrides. */
  scopes: SettingScope[]
}

export interface EffectiveSetting {
  key: string
  value: string
  /** The scope the value came from, which is `server` unless this one set it. */
  source: SettingScope
  overridden: boolean
}

/** A work a provider has, before this server holds it. */
export interface Found {
  kind: MediaKind
  title: string
  year?: number
  overview?: string
  poster?: string
  tmdbId?: number
  tvdbId?: number
  imdbId?: string
  /** Whether this server already holds it, so the screen can say so. */
  stored: boolean
  isAdult: boolean
}

/** What an entry of a season chart is. */
export type SeasonEntryKind = 'newSeries' | 'newSeason' | 'continuing' | 'film'

export interface SeasonEntry {
  workId: string
  kind: SeasonEntryKind
  /** The season, for a series whose episodes are known. */
  seasonNumber?: number
  /** The day it premiered, or will; for a season carrying on, the day it began. */
  starts: string
  ends?: string
  episodes?: number
  aired?: number
  nextEpisode?: { episodeNumber: number; airDate: string }
  /** What it is the sequel of, where a provider filed one; the work here that it is, when held. */
  sequelOf?: { title: string; workId?: string }
}

export interface SeasonChart {
  year: number
  season: 'winter' | 'spring' | 'summer' | 'autumn'
  from: string
  to: string
  entries: SeasonEntry[]
  works: MediaItem[]
}

/** A work premiering in a season that the catalogue does not hold, by TMDB. */
export interface SeasonCandidate {
  kind: MediaKind
  tmdbId: number
  title: string
  originalTitle?: string
  premiere?: string
  overview?: string
  poster?: string
  originalLanguage?: string
  score?: number
  votes?: number
}
