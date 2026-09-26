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
  /** Fankai's id, for a Fan-Kai production. */
  fankai?: number
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
  /**
   * SEQUEL, PREQUEL, PARENT, SIDE_STORY, SPIN_OFF, ALTERNATIVE, SUMMARY, COMPILATION, CONTAINS, SOURCE,
   * ADAPTATION, CHARACTER or OTHER, as AniList names them; ORIGINAL, the anime a Fan-Kai was cut from;
   * RECUT, a Fan-Kai cut from this work.
   */
  relationType: string
  /** Where the other work is filed — `anilist`, `mal` or `fankai` — and its id there. */
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

/** Where one episode stands in one of the other orders TheTVDB keeps. */
export interface PlacedEpisode {
  tvdbId: number
  seasonNumber: number
  episodeNumber: number
  absoluteNumber?: number
}

/** One of the other orders a series' episodes come in: dvd, absolute, alternate, regional, altdvd. */
export interface EpisodeOrder {
  kind: string
  episodes: PlacedEpisode[]
}

export interface Orders {
  orders: EpisodeOrder[]
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
  /** Where the work's theme music is: Fankai keeps one for every Fan-Kai. */
  themeMusic?: string
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
  /** The account it belongs to; absent for one of the server's own keys. */
  ownerId?: string
  ownerName?: string
}

/** What an account may do, least first. */
export type AccountRole = 'member' | 'editor' | 'admin'
export type AccountStatus = 'active' | 'pending' | 'disabled'

/** The signed-in person, as every page needs them. */
export interface SessionUser {
  id: string
  username: string
  /** What to call them: the name they gave, or their username. */
  name: string
  role: AccountRole
  locale?: string
  hasPassword: boolean
}

export interface User {
  id: string
  username: string
  displayName?: string
  email?: string
  role: AccountRole
  status: AccountStatus
  locale?: string
  oidcLinked: boolean
  hasPassword: boolean
  invitedBy?: string
  createdAt: string
  updatedAt?: string
  lastLoginAt?: string
}

export interface ListedUser extends User {
  keys: number
  sessions: number
}

export interface UsersPage {
  users: ListedUser[]
  total: number
  counts: { total: number; pending: number; admins: number; oidc: number }
}

export interface AccountSession {
  id: string
  createdAt: string
  expiresAt: string
  lastSeenAt?: string
  userAgent?: string
  ip?: string
  current: boolean
}

export interface UserDetail {
  user: User
  keys: ApiClient[]
  sessions: AccountSession[]
}

export interface AccountKeys {
  keys: ApiClient[]
  /** How many may be held; absent for an administrator. */
  limit?: number
  /** The rights this person's role lets a key carry. */
  scopes: string[]
  /** Whether these keys also open the TMDB relay. */
  relay: boolean
}

export interface IssuedKey {
  key: ApiClient
  /** Shown once. */
  secret: string
}

/** Who may open an account for themselves. */
export type Registration = 'closed' | 'invite' | 'approval' | 'open'

/** What the sign-in page offers, readable before anyone signs in. */
export interface AuthOptions {
  site: 'public' | 'private'
  registration: Registration
  /** Whether the password form is offered to everyone. */
  passwordLogin: boolean
  /** The identity provider's button, when signing in through one is on. */
  oidc?: { label?: string }
}

/** How people sign in through the identity provider, as an administrator sets it. */
export interface OidcConfiguration {
  enabled: boolean
  issuer: string
  clientId: string
  /** A secret is stored; it is never sent back. */
  secretSet: boolean
  /** AMS_OIDC_CLIENT_SECRET holds it, and wins. */
  secretFromEnv: boolean
  scopes: string
  buttonLabel: string
  autoRegister: boolean
  roleClaim: string
  adminValues: string
  editorValues: string
  passwordLogin: boolean
  /** AMS_FORCE_PASSWORD_LOGIN keeps passwords on whatever is set. */
  passwordForced: boolean
  /** To register at the provider; absent without AMS_PUBLIC_URL. */
  redirectUri?: string
  ready: boolean
  /** The account AMS_ADMIN_USERNAME names, which keeps its password. */
  breakGlass?: string
}

/** What the test button found, or why it found nothing. */
export interface OidcTest {
  ok: boolean
  error?: string
  discovery?: OidcDiscovery
}

/** What the provider's discovery document said. */
export interface OidcDiscovery {
  issuer: string
  authorizationEndpoint: string
  tokenEndpoint?: string
  userinfoEndpoint?: string
  keys: number
  signingAlgorithms: string[]
  scopes: string[]
}

export interface InvitationOffer {
  role: AccountRole
  expiresAt?: string
}

export interface Invitation {
  id: string
  /** The code's first group: `K7QM`. */
  codePrefix: string
  role: AccountRole
  maxUses: number
  uses: number
  expiresAt?: string
  note?: string
  createdBy?: string
  createdByName?: string
  createdAt: string
  revokedAt?: string
  /** Whether its maker is still an active administrator: it is void otherwise. */
  creatorActive: boolean
}

export interface Invitations {
  invitations: Invitation[]
  usable: number
  /** Accounts the usable ones may still open. */
  places: number
}

export interface IssuedInvitation {
  invitation: Invitation
  /** `K7QM-2XRP-9DHT-4WCN`. Shown once. */
  code: string
  /** The link to send, at the server's public address when it has one. */
  link?: string
}

export type ApiName = 'sonarr' | 'radarr' | 'tmdb' | 'native'
export type SurfacePolicyName = 'apikey' | 'allowlist' | 'open'

export interface ApiState {
  api: ApiName
  enabled: boolean
  policy: SurfacePolicyName
  served: number
  refused: number
}

/** The way in, on one page. */
export interface AccessReport {
  site: 'public' | 'private'
  /** AMS_PUBLIC_BROWSE=false keeps the site private whatever is chosen. */
  siteLocked: boolean
  relayForMembers: boolean
  registration: Registration
  registrationRole: AccountRole
  keysPerUser: number
  authDisabled: boolean
  uptimeSeconds: number
  apis: ApiState[]
  pending: number
  invitations: number
  places: number
  allowlistRules: number
  serverKeys: number
  personalKeys: number
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
  status: 'running' | 'succeeded' | 'failed' | 'stopped'
  startedAt?: string
  finishedAt?: string
  error?: string
  detail?: string
  createdAt: string
  /** Who started it, as the journal names them; absent for the schedule. */
  triggeredBy?: string
  /** The work it acted on, while it is held. */
  work?: { id: string; title: string; kind: MediaKind }
}

export type TaskId = 'refresh.sweep' | 'refresh.all' | 'import.anime' | 'import.imdb' | 'export.nfo'

/** A background task as it stands: when it runs, how it last went. */
export interface Task {
  id: TaskId
  mode: 'scheduled' | 'manual' | 'off'
  everySeconds?: number
  last?: Job
  lastSuccessAt?: string
  nextAt?: string
  running?: Job
  cancelable: boolean
  /** Why it cannot run now: `source_off`, `not_configured`, `running`. */
  blocked?: 'source_off' | 'not_configured' | 'running' | 'busy'
}

export interface JobsResponse {
  jobs: Job[]
  total: number
}

/** An export under way, written in the background and recorded as a job. */
export interface ExportStarted {
  root: string
  /** The `export.nfo` run in the jobs list; its detail reads `N works, N episodes, N failed`. */
  jobId?: string
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
  /** The signed-in person, when this is a session rather than a key. */
  user?: SessionUser
}

export interface Snapshot {
  provider: string
  fetchedAt: string
  etag?: string
  /** Absent when the caller asked for `payload=false`. */
  payload?: unknown
}

/** Who gave one of a work's values, as its last merge worked out. */
export interface ValueSource {
  /** The provider whose value was kept. */
  from: string
  /** Others that gave the same value. Skyhook is listed beside TheTVDB, whose answer it republishes. */
  agreed?: string[]
  /** Others that gave another value, and lost to it. */
  differed?: string[]
  /** Those ahead of it that gave none: why a lower source's value stands. */
  passed?: string[]
}

/** Where a work's values came from. */
export interface WorkProvenance {
  /** By field name, as the registry names fields; `credits` and `relations` too. */
  fields?: Record<string, ValueSource>
  /** The images kept, counted by the provider they came from. */
  images?: Record<string, number>
  /** The provider of the episode list and its numbering. */
  episodes?: string
  /** Each translation kept, by language, to the provider it came from. */
  translations?: Record<string, string>
  /** Each rating kept, by the agency it is filed under, to the provider that reported it. */
  ratings?: Record<string, string>
}

/** One provider a work could be synced from. */
export interface SyncSource {
  provider: string
  /** When it last answered for this work, if it ever has. */
  fetchedAt?: string
  /** Why it cannot be asked now. */
  unavailable?: 'off' | 'noId' | 'manual'
  /** Asked along with it, because part of what it gives is theirs. */
  brings?: string[]
}

export interface ProvenanceReport {
  /** Absent for a work entered by hand, or not refreshed since provenance began to be kept. */
  provenance?: WorkProvenance
  sources: SyncSource[]
}

export interface SyncOutcome {
  item: MediaItem
  answered: string[]
  silent: string[]
}

/** What each source gives a work, and which wins. */
export interface SourceRules {
  /** Every provider, in the order the merge consults them. */
  providers: { id: string; rank: number; on: boolean }[]
  rows: SourceRule[]
}

export interface SourceRule {
  group: 'identity' | 'release' | 'classification' | 'people' | 'episodes'
  /** A key of the page's own row names. */
  field: string
  /** `first` the first with a value; `whole` a list taken whole from the first that has one; `union` every source adds its own; `spine` whoever numbers the episodes first; `either` true if anyone says so. */
  rule: 'first' | 'whole' | 'union' | 'spine' | 'either'
  /** Who supplies it, in the order their values are taken. */
  suppliers: string[]
  /** Whose value replaces every other's. */
  authorities?: string[]
  /** When it applies to one kind of work only. */
  only?: 'series' | 'movie'
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

/** A lock, as it travels: the work by every id it can be found by, the field and its value. */
export interface Lock {
  work: { id?: string; kind: MediaKind; title: string; year?: number; tmdb?: number; tvdb?: number; imdb?: string }
  scope: string
  field: string
  value?: unknown
}

export interface Locks {
  version: number
  exportedAt: string
  locks: Lock[]
}

/** What an import did. */
export interface LocksImported {
  applied: number
  /** Locks already set to the same value. */
  unchanged: number
  /** The works written to. */
  works: number
  unmatched: string[]
  refused: string[]
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
  /** Written, never read back: shown masked, and only on its own page. */
  | { type: 'secret' }

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
  /** Fankai's id, for a Fan-Kai production. */
  fankaiId?: number
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
