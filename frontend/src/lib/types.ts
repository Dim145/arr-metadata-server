/**
 * The shapes the native API sends, as the interface reads them.
 *
 * Derived from the server's own OpenAPI document rather than copied from it:
 * `api.gen.ts` is written from `frontend/openapi.json` by `npm run types`,
 * and `cargo test` fails while that copy differs from what the server builds.
 * So a field added on the server reaches these types without anybody copying
 * it, and one taken away is a type error wherever a page still reads it. The
 * document's `required` is what says a field is always sent: a default only
 * says what a field left out stands for, and the generator is told so
 * (`--default-non-nullable false`).
 *
 * What is still said here is said more exactly than the document can: a
 * string the server only ever fills from a fixed set is given that set, and
 * a type holding one is given the narrower type.
 */

import type { components } from './api.gen'

type Schemas = components['schemas']

/**
 * A schema as the server sends it.
 *
 * utoipa documents a Rust `Option` as optional and nullable both, while the
 * server leaves a `None` out (`skip_serializing_if`): an optional field is
 * absent rather than `null`. So the `null` of an optional field is dropped, at
 * every depth. A field the server always sends is documented as required —
 * `cargo test` fails on one that is not — and keeps its `null` where it can
 * be one.
 */
type Sent<T> = T extends readonly (infer U)[]
  ? Sent<U>[]
  : T extends object
    ? { [K in keyof T]: undefined extends T[K] ? Sent<Exclude<T[K], null>> : Sent<T[K]> }
    : T

/** One of the document's schemas, as sent. */
type Schema<N extends keyof Schemas> = Sent<Schemas[N]>

/** A schema with the fields in `R` said here instead, and the rest as the document has them. */
type Refined<N extends keyof Schemas, R> = Omit<Schema<N>, keyof R> & R

export type MediaKind = Schema<'MediaKind'>

export type ExternalIds = Schema<'ExternalIds'>

export type Rating = Schema<'Rating'>

export type Image = Schema<'Image'>

export type Credit = Schema<'Credit'>

/**
 * Another work a provider files beside one — its sequel, prequel, side story —
 * as AniList names, dates and pictures it, with the work here that it is when
 * the catalogue holds it.
 */
export type Relation = Schema<'Relation'>

/** Where one episode stands in one of the other orders TheTVDB keeps. */
export type PlacedEpisode = Schema<'PlacedEpisode'>

/** One of the other orders a series' episodes come in: dvd, absolute, alternate, regional, altdvd. */
export type EpisodeOrder = Schema<'EpisodeOrder'>

export type Orders = Schema<'Orders'>

export type AlternativeTitle = Schema<'AlternativeTitle'>

export type Season = Schema<'Season'>

export type Episode = Schema<'Episode'>

export type Translation = Schema<'Translation'>

/** How a work came out in a country, as TMDB tells its releases apart. */
export type ReleaseKind = Schema<'ReleaseKind'>

/** What a work was given in one country: a day it came out there and how, its rating there, or both. */
export type CountryRelease = Schema<'CountryRelease'>

/** A work. */
export type MediaItem = Schema<'MediaItem'>

/** What a person hid of what the sources gave a work. */
export type HiddenChildren = Schema<'Hidden'>

export type FieldType = Schema<'FieldType'>

export type FieldDef = Schema<'FieldDef'>

export type FieldRegistry = Schema<'FieldRegistry'>

/** One stored manual edit. Its value is always sent: `null` where the field was cleared and locked so. */
export type Override = Schema<'Override'>

export type ApiClient = Schema<'ApiClient'>

/** What an account may do, least first. */
export type AccountRole = Schema<'AccountRole'>

export type AccountStatus = Schema<'AccountStatus'>

/** The signed-in person, as every page needs them. */
export type SessionUser = Schema<'MeUser'>

export type User = Schema<'User'>

export type ListedUser = Schema<'Listed'>

export type UsersPage = Schema<'UsersPage'>

export type AccountSession = Schema<'Session'>

export type UserDetail = Schema<'UserDetail'>

export type AccountKeys = Schema<'AccountKeys'>

export type IssuedKey = Schema<'IssuedKey'>

/** Who may open an account for themselves. */
export type Registration = Schema<'Registration'>

/** What the sign-in page offers, readable before anyone signs in. */
export type AuthOptions = Refined<'AuthOptions', { site: 'public' | 'private' }>

/** How people sign in through the identity provider, as an administrator sets it. */
export type OidcConfiguration = Schema<'OidcConfiguration'>

/** What the test button found, or why it found nothing. */
export type OidcTest = Schema<'TestOutcome'>

/** The operator's own Sonarr or Radarr, which this server reaches the other way. */
export type ArrApp = Schema<'App'>

/** One of the two, as set: the key is never sent back, only whether one is kept. */
export type ArrConnection = Schema<'ArrConnection'>

/** One work told to Sonarr or Radarr, and how it went. */
export type ArrPush = Refined<'ArrPush', { app: ArrApp; outcome: 'sent' | 'absent' | 'failed' }>

/** The Sonarr & Radarr page: both connections, the switch, what was told lately. */
export type ArrConfiguration = Refined<'ArrConfiguration', { pushes: ArrPush[] }>

/** What the test button found: who answered, or why nobody did. */
export type ArrTest = Schema<'ArrTestOutcome'>

/** What the provider's discovery document said. */
export type OidcDiscovery = Schema<'Discovery'>

export type InvitationOffer = Schema<'InvitationOffer'>

export type Invitation = Schema<'Invitation'>

export type Invitations = Schema<'Invitations'>

export type IssuedInvitation = Schema<'IssuedInvitation'>

export type ApiName = Schema<'Api'>

/** What an API asks of a caller: a string in the document, always one of these. */
export type SurfacePolicyName = 'apikey' | 'allowlist' | 'open'

export type ApiState = Refined<'ApiState', { policy: SurfacePolicyName }>

/** The way in, on one page. */
export type AccessReport = Refined<
  'Access',
  { site: 'public' | 'private'; apis: ApiState[]; relayCredentials: RelayCredentialState[] }
>

/** Whose key a relay asks its service with (`tmdb.relayCredential`, `tvdb.relayCredential`). */
export type RelayCredentialState = Refined<
  'RelayCredentialState',
  { api: 'tmdb' | 'tvdb'; credential: 'server' | 'client'; configuredPolicy: SurfacePolicyName }
>

export type AuditEntry = Schema<'AuditEntry'>

export type AuditResponse = Schema<'AuditListResponse'>

export type Job = Refined<'Job', { status: 'running' | 'succeeded' | 'failed' | 'stopped' }>

/** The background tasks, by the id the server knows each by: a string in the document. */
export type TaskId =
  | 'refresh.sweep'
  | 'refresh.all'
  | 'import.anime'
  | 'import.imdb'
  | 'export.nfo'
  | 'media.store'
  | 'media.sweep'
  | 'tls.renew'
  | 'update.check'
  | 'arr.seed'

/** A background task as it stands: when it runs, how it last went. */
export type Task = Refined<
  'TaskState',
  {
    id: TaskId
    last?: Job
    running?: Job
    /** Why it cannot run now. */
    blocked?: 'source_off' | 'not_configured' | 'running' | 'busy' | 'media_off' | 'no_authority' | 'updates_off' | 'arr_off'
  }
>

/** Every event a destination can be told of, besides `*`. */
export type WebhookEvents = Refined<'WebhookEvents', { kinds: WebhookKind[] }>

/** How a destination is posted to: a string in the document, always one of these. */
export type WebhookKind = 'standard' | 'json' | 'discord' | 'slack' | 'ntfy' | 'gotify' | 'apprise'

/** What a test post to a destination came to. */
export type WebhookTest = Refined<'WebhookTest', { refused?: 'not_web' | 'internal' | 'unsent' }>

/** Where what happens is told: never its secret, its address masked past the host. */
export type WebhookDestination = Refined<'WebhookDestination', { kind: WebhookKind }>

/** A destination to make, or what to change of one: each field left out stays. */
export type WebhookDestinationInput = Refined<'WebhookDestinationInput', { kind?: WebhookKind }>

/** One thing told, as a delivery keeps it: who, on what and how, each sent as `null` when there is none. */
export type WebhookMessage = Schema<'WebhookMessage'>

/** One event owed to a destination, and how its tries went. */
export type WebhookDelivery = Schema<'WebhookDelivery'>

export type WebhookDeliveries = Schema<'WebhookDeliveries'>

export type JobsResponse = Refined<'JobsResponse', { jobs: Job[] }>

/** What a run did to one work: the detail behind its summary. */
export type JobEntry = Refined<'JobEntry', { kind?: MediaKind; outcome: 'ok' | 'failed' | 'skipped' | 'imported' }>

/** One run, with what it did to each work — the first five hundred of them. */
export type JobDetail = Refined<'JobDetail', { job: Job; entries: JobEntry[] }>

/** An export under way, written in the background and recorded as a job. */
export type ExportStarted = Schema<'ExportStarted'>

/** What the catalogue holds; how the server is run, only for whoever maintains it. */
export type Stats = Schema<'Stats'>

export type Settings = Refined<'Settings', { furtherSources: FurtherSources }>

/** A list the server downloads whole: when it last landed, and what it kept. */
export type ListImport = Schema<'ListImport'>

export type FurtherSources = Refined<'FurtherSources', { malVia: 'official' | 'jikan' }>

/** Where the data comes from, for the credits: only what is switched on. */
export type Sources = Schema<'Sources'>

export type Me = Schema<'MeResponse'>

export type Snapshot = Schema<'SnapshotSummary'>

/** Who gave one of a work's values, as its last merge worked out. */
export type ValueSource = Schema<'ValueSource'>

/** Where a work's values came from. */
export type WorkProvenance = Schema<'Provenance'>

/** One provider a work could be synced from. */
export type SyncSource = Schema<'SyncSource'>

export type ProvenanceReport = Schema<'ProvenanceReport'>

export type SyncOutcome = Schema<'SyncOutcome'>

/** What each source gives a work, and which wins. */
export type SourceRules = Schema<'SourceRules'>

/** One of MyAnimeList's ways in. */
export type MalBase = Schema<'Base'>

/** How MyAnimeList is reached, as the server last found it. */
export type MalReach = Schema<'Reach'>

export type SourceRule = Schema<'Laid'>

/** How the media store stands. */
export type MediaStatus = Refined<'MediaStatus', { backend: 'off' | 'filesystem' | 's3' }>

/** One medium kept, or in line to be. */
export type MediaAsset = Schema<'Asset'>

/** One address a work points at, and what is kept of it. */
export type WorkMedium = Refined<'WorkMedium', { status: 'stored' | 'pending' | 'failed' | 'absent' }>

/** A work's media, and whether a store is configured at all. */
export type WorkMedia = Refined<'WorkMedia', { media: WorkMedium[] }>

export type Uploaded = Schema<'Uploaded'>

export type ItemPage = Schema<'ItemListResponse'>

/** A service that carries a work in a country, as TMDB lists it from JustWatch. */
export type WatchProvider = Schema<'WatchProvider'>

export type WhereToWatch = Schema<'WhereToWatch'>

/** The catalogue's own works in the same vein as one. */
export type Similar = Schema<'Similar'>

/** What TMDB recommends beside a work, for whoever maintains the catalogue. */
export type Suggestion = Schema<'Suggestion'>

export type Suggestions = Schema<'Suggestions'>

export type CollectionCard = Schema<'CollectionCard'>

export type Collections = Schema<'Collections'>

export type CollectionPart = Schema<'CollectionPart'>

export type CollectionPage = Schema<'CollectionPage'>

export type Count = Schema<'Count'>

/** The catalogue in numbers, as a reader is shown it. */
export type Figures = Schema<'Figures'>

export type CacheFigures = Schema<'CacheFigures'>

/** A lock, as it travels: the work by every id it can be found by, the field and its value. */
export type Lock = Schema<'Lock'>

/** Every lock, as a document. */
export type Locks = Schema<'Locks'>

/** What an import did. */
export type LocksImported = Schema<'LocksImported'>

/** How the server is. */
export type Health = Refined<'Health', { jobs: Job[] }>

/** A release newer than the version running, and its page on GitHub. */
export type UpdateAvailable = Schema<'UpdateAvailable'>

/**
 * What there is to do, counted: the dashboard's list of errands. Each of the
 * works' figures is what the admin catalogue lists under the matching filter.
 */
export type Errands = Schema<'Todo'>

export type ListKind = Schema<'ListKind'>

export type ListMode = Schema<'ListMode'>

/** A filter kept with a list, in the list query's own vocabulary. */
export type ListFilter = Schema<'ListFilter'>

/** A curated list: a selection composed by hand, or by a filter kept current. */
export type CuratedList = Schema<'CuratedList'>

export type CuratedLists = Schema<'CuratedLists'>

export type CuratedListPage = Schema<'CuratedListPage'>

export type CuratedListRequest = Schema<'CuratedListRequest'>

/** One value a list can be narrowed to, and how many works it would leave. */
export type Facet = Schema<'Facet'>

export type Facets = Schema<'Facets'>

export type Airing = Schema<'Airing'>

/** How a film reaches the public: the three dates Radarr keeps for one. */
export type ReleaseType = Schema<'ReleaseType'>

/** A film coming out on one day of the window. */
export type Release = Schema<'Release'>

/** One thing the schedule lists: an episode airing, or a film coming out. */
export type CalendarEntry = Schema<'CalendarEntry'>

/**
 * The schedule. The server still sends `episodes`, the episodes alone, for
 * whoever read it before it held films; it is deprecated, so the generated
 * types leave it out (`--exclude-deprecated`) and this site reads `entries`.
 */
export type Calendar = Schema<'Calendar'>

/** One part a person played, or one job they did, on one work. */
export type Role = Schema<'PersonRole'>

/** Somebody as TMDB knows them, beside what this catalogue holds of theirs. */
export type PersonDetails = Schema<'PersonDetails'>

export type Person = Schema<'Person'>

export type NetworkRule = Schema<'NetworkRule'>

export type NetworkCaller = Schema<'Caller'>

/**
 * A setting, as the server describes it.
 *
 * The interface renders controls from `kind` rather than from a list of its
 * own, so a setting added to the server appears here without this code being
 * touched — which is the only reason the registry is served at all.
 */
export type SettingKind = Schema<'SettingKind'>

export type SettingScope = Schema<'Scope'>

export type SettingDef = Schema<'Definition'>

export type EffectiveSetting = Refined<'Effective', { source: SettingScope }>

/** A work a provider has, before this server holds it. */
export type Found = Schema<'Found'>

/** What an entry of a season chart is. */
export type SeasonEntryKind = Schema<'EntryKind'>

export type SeasonEntry = Schema<'SeasonEntry'>

export type SeasonChart = Schema<'Chart'>

/** A work premiering in a season that the catalogue does not hold, by TMDB. */
export type SeasonCandidate = Schema<'Candidate'>

/** The server's doors: the interface's, and the clients' — `null` when there is none. */
export type Listeners = Refined<'Listeners', { clients: ClientsListener | null }>

export type ClientsListener = Refined<'Door', { mode: 'authority' | 'own' }>

/** The caches, as the Cache page reads them. */
export type CacheReport = Refined<'Report', { spaces: CacheSpace[] }>

/** One instance of the server, as it introduces itself to the others. */
export type Instance = Schema<'Instance'>

/** The instances: one, or several coordinating through the cache server. */
export type Instances = Schema<'Instances'>

export type CacheServer = Schema<'Server'>

export type CacheSpace = Refined<'SpaceReport', { id: 'items' | 'searches' | 'lists' | 'relay' | 'sessions' }>

export type CacheFlushed = Schema<'Flushed'>
