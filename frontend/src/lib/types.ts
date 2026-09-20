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
  sortOrder: number
  isManual: boolean
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
  isManual: boolean
  images?: Image[]
}

export interface Episode {
  id: string
  seasonNumber: number
  episodeNumber: number
  title: string
  overview?: string
  airDate?: string
  airDateUtc?: string
  runtime?: number
  finaleType?: string
  image?: string
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
  lockedFields?: string[]
}

export type FieldType =
  | 'text'
  | 'longText'
  | 'integer'
  | 'float'
  | 'boolean'
  | 'date'
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
  nativePolicy: string
  tmdbPolicy: string
  arrPolicy: string
}

export interface Me {
  identity: string
  canWrite: boolean
  isAdmin: boolean
}

export interface Snapshot {
  provider: string
  fetchedAt: string
  etag?: string
  payload: unknown
}
