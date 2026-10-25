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
 * (`--default-non-nullable false`). A string the server fills from a fixed
 * set is an enum in the document, so the set reaches these types too.
 *
 * What is still said here is what the document cannot say: how the server
 * sends what it documents — `Sent`, below, drops the `null` utoipa gives
 * every optional field, since the server leaves one out instead — and the
 * names the pages know these shapes by, where the document's are its Rust
 * types' (`SessionUser` for `MeUser`). Nothing else: a set written out here
 * by hand would be one the server could change without the compiler
 * noticing.
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

/** One of the other orders a series' episodes come in: dvd, absolute, alternate, regional, altdvd, alttwo. */
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
export type AuthOptions = Schema<'AuthOptions'>

/** How people sign in through the identity provider, as an administrator sets it. */
export type OidcConfiguration = Schema<'OidcConfiguration'>

/** What the test button found, or why it found nothing. */
export type OidcTest = Schema<'TestOutcome'>

/** The operator's own Sonarr or Radarr, which this server reaches the other way. */
export type ArrApp = Schema<'App'>

/** One of the two, as set: the key is never sent back, only whether one is kept. */
export type ArrConnection = Schema<'ArrConnection'>

/** One work told to Sonarr or Radarr, and how it went. */
export type ArrPush = Schema<'ArrPush'>

/** The Sonarr & Radarr page: both connections, the switch, what was told lately. */
export type ArrConfiguration = Schema<'ArrConfiguration'>

/** What the test button found: who answered, or why nobody did. */
export type ArrTest = Schema<'ArrTestOutcome'>

/** What the provider's discovery document said. */
export type OidcDiscovery = Schema<'Discovery'>

export type InvitationOffer = Schema<'InvitationOffer'>

export type Invitation = Schema<'Invitation'>

export type Invitations = Schema<'Invitations'>

export type IssuedInvitation = Schema<'IssuedInvitation'>

export type ApiName = Schema<'Api'>

export type ApiState = Schema<'ApiState'>

/** The way in, on one page. */
export type AccessReport = Schema<'Access'>

/** Whose key a relay asks its service with (`tmdb.relayCredential`, `tvdb.relayCredential`). */
export type RelayCredentialState = Schema<'RelayCredentialState'>

export type AuditEntry = Schema<'AuditEntry'>

export type AuditResponse = Schema<'AuditListResponse'>

/** A line of the application log, and the log as the administration reads it. */
export type LogEntry = Schema<'LogEntry'>

export type LogLevel = Schema<'LogLevel'>

export type LogResponse = Schema<'LogResponse'>

export type Job = Schema<'Job'>

/** The background tasks, by the name the history files their runs by. */
export type TaskId = Schema<'TaskId'>

/** A background task as it stands: when it runs, how it last went. */
export type Task = Schema<'TaskState'>

/** Every task and the newest run, as the tasks' stream sends them. */
export type TaskBoard = Schema<'TaskBoard'>

/** When a periodic task runs, where somebody said: a window of the day, or a cron expression, in UTC. */
export type TaskSchedule = Schema<'TaskSchedule'>

/** The next moments a schedule being written names. */
export type ScheduleMoments = Schema<'ScheduleMoments'>

/** A line of a batch to import, and the batch set beside the catalogue. */
export type ImportLine = Schema<'ImportLine'>

export type ImportPreview = Schema<'ImportPreview'>

export type ImportPreviewLine = Schema<'ImportPreviewLine'>

/** What a batch started as: its run, and how many works it takes. */
export type BatchStarted = Schema<'BatchStarted'>

/** What is imported on a schedule, and into which selection. */
export type ImportRule = Schema<'ImportRule'>

export type ImportRuleRequest = Schema<'ImportRuleRequest'>

/** The works by what their media come to. */
export type MediaByWork = Schema<'MediaByWork'>

/** Every event a destination can be told of, besides `*`. */
export type WebhookEvents = Schema<'WebhookEvents'>

/** How a destination is posted to. */
export type WebhookKind = Schema<'WebhookKind'>

/** What a test post to a destination came to. */
export type WebhookTest = Schema<'WebhookTest'>

/** Where what happens is told: never its secret, its address masked past the host. */
export type WebhookDestination = Schema<'WebhookDestination'>

/** A destination to make, or what to change of one: each field left out stays. */
export type WebhookDestinationInput = Schema<'WebhookDestinationInput'>

/** One thing told, as a delivery keeps it: who, on what and how, each sent as `null` when there is none. */
export type WebhookMessage = Schema<'WebhookMessage'>

/** One event owed to a destination, and how its tries went. */
export type WebhookDelivery = Schema<'WebhookDelivery'>

export type WebhookDeliveries = Schema<'WebhookDeliveries'>

export type JobsResponse = Schema<'JobsResponse'>

/** What a run did to one work: the detail behind its summary. */
export type JobEntry = Schema<'JobEntry'>

/** One run, with what it did to each work — the first five hundred of them. */
export type JobDetail = Schema<'JobDetail'>

/** An export under way, written in the background and recorded as a job. */
export type ExportStarted = Schema<'ExportStarted'>

/** What the catalogue holds; how the server is run, only for whoever maintains it. */
export type Stats = Schema<'Stats'>

export type Settings = Schema<'Settings'>

/** A list the server downloads whole: when it last landed, and what it kept. */
export type ListImport = Schema<'ListImport'>

export type FurtherSources = Schema<'FurtherSources'>

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

/** A lock whose sources say something else now, beside what they say. */
export type LockDivergence = Schema<'LockDivergence'>

/** What a refresh or a sync would write over, from a dry run of it. */
export type RefreshPreview = Schema<'RefreshPreview'>

/** One value a write changed, or would. */
export type FieldChange = Schema<'FieldChange'>

/** One change in a work's journal: what the value held and took, who and why. */
export type ChangeEntry = Schema<'ChangeEntry'>

/** A page of a work's journal, and what its filters can offer. */
export type ChangeList = Schema<'ChangeList'>

export type ChangeCause = Schema<'ChangeCause'>

/** What each source gives a work, and which wins. */
export type SourceRules = Schema<'SourceRules'>

/** One of MyAnimeList's ways in. */
export type MalBase = Schema<'Base'>

/** How MyAnimeList is reached, as the server last found it. */
export type MalReach = Schema<'Reach'>

export type SourceRule = Schema<'Laid'>

/** How the media store stands. */
export type MediaStatus = Schema<'MediaStatus'>

/** One medium kept, or in line to be. */
export type MediaAsset = Schema<'Asset'>

/** One address a work points at, and what is kept of it. */
export type WorkMedium = Schema<'WorkMedium'>

/** A work's media, and whether a store is configured at all. */
export type WorkMedia = Schema<'WorkMedia'>

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
export type Health = Schema<'Health'>

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

/** The list's facets, and the countries and ratings its further filters choose among. */
export type CatalogueFacets = Schema<'CatalogueFacets'>

/** A genre's, a network's, a year's or a decade's page: its name, its count, its five best, its note. */
export type FacetPage = Schema<'FacetPage'>

/** The programmer's note on one thing, in one language. */
export type EditorialNote = Schema<'EditorialNote'>

/** What a programmer's note is about: a work, a list, a genre, a network, a year, a decade. */
export type EditorialScope = Schema<'EditorialScope'>

export type EditorialNotes = Schema<'EditorialNotes'>

/** Two works side by side, and the people they share. */
export type Comparison = Schema<'Comparison'>

export type SharedPerson = Schema<'SharedPerson'>

/** What a search across the catalogue found: its works, people and episodes, each ranked. */
export type SearchHits = Schema<'SearchHits'>

export type WorkHit = Schema<'WorkHit'>

export type PersonHit = Schema<'PersonHit'>

export type EpisodeHit = Schema<'EpisodeHit'>

export type Airing = Schema<'Airing'>

/** How a film reaches the public: the three dates Radarr keeps for one. */
export type ReleaseType = Schema<'ReleaseType'>

/** A film coming out on one day of the window. */
export type Release = Schema<'Release'>

/** One thing the schedule lists: an episode airing, or a film coming out. */
export type CalendarEntry = Schema<'CalendarEntry'>

/** The schedule: the episodes and the films' releases of a window, and the works they belong to. */
export type Calendar = Schema<'Calendar'>

/** One part a person played, or one job they did, on one work — or on one episode of it. */
export type Role = Schema<'PersonRole'>

/** Somebody as TMDB knows them, beside what this catalogue holds of theirs. */
export type PersonDetails = Schema<'PersonDetails'>

export type Person = Schema<'Person'>

/** What somebody did on a work, as their page and the index put works together: acting, or one of three jobs behind the camera. */
export type PersonJob = Schema<'PersonJob'>

/** Somebody, as the index of people lists them; for those who keep the catalogue, with their ids and the other names they go by. */
export type PersonCard = Schema<'PersonCard'>

/** A page of the index of people, and the letters somebody is filed under. */
export type PeoplePage = Schema<'PeoplePage'>

/** Somebody as they are entered, or put right, by hand: every field, a field left out cleared. */
export type EnteredPerson = Schema<'EnteredPerson'>

/** What became of a record of somebody: its id. */
export type PersonId = Schema<'PersonId'>

/** Whom one of a work's credits is filed under, with the ids they are known by. */
export type PersonRef = Schema<'PersonRef'>

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

export type EffectiveSetting = Schema<'Effective'>

/** A work a provider has, before this server holds it. */
export type Found = Schema<'Found'>

/** What an entry of a season chart is. */
export type SeasonEntryKind = Schema<'EntryKind'>

export type SeasonEntry = Schema<'SeasonEntry'>

export type SeasonChart = Schema<'Chart'>

/** A work premiering in a season that the catalogue does not hold, by TMDB. */
export type SeasonCandidate = Schema<'Candidate'>

/** The server's doors: the interface's, and the clients' — `null` when there is none. */
export type Listeners = Schema<'Listeners'>

export type ClientsListener = Schema<'Door'>

/** The caches, as the Cache page reads them. */
export type CacheReport = Schema<'Report'>

/** One instance of the server, as it introduces itself to the others. */
export type Instance = Schema<'Instance'>

/** The instances: one, or several coordinating through the cache server. */
export type Instances = Schema<'Instances'>

export type CacheServer = Schema<'Server'>

export type CacheSpace = Schema<'SpaceReport'>

export type CacheFlushed = Schema<'Flushed'>

/** The Backups page: the archives, where they are kept, and how the server makes them. */
export type BackupList = Schema<'BackupList'>

/** One archive, as the Backups page lists it. */
export type Backup = Schema<'Backup'>

/** Where the last restore stands. */
export type BackupRestore = Schema<'BackupRestore'>

/** Two works that look like one, as the catalogue shows them: each work, why, how sure, and the one the rule keeps. */
export type DuplicatePair = Schema<'DuplicateView'>

export type DuplicateList = Schema<'DuplicateList'>

/** A work as a merge names it: enough to tell two apart, and to choose which stays. */
export type MergeWork = Schema<'MergeWork'>

/** What merging one work into another would do: the two, the fields that differ, what moves, the names kept. */
export type MergePreview = Schema<'MergePreview'>

export type MergeField = Schema<'MergeField'>

/** Which of two works a field's value is kept from. */
export type MergeSide = Schema<'MergeSide'>

export type MergeOutcome = Schema<'MergeOutcome'>

/** The merges into a work that can still be undone, and the names it answers for. */
export type MergeList = Schema<'MergeList'>

/** How many works the register lists under each of its gaps (`?missing=`). */
export type Gaps = Schema<'Gaps'>

/** What a person keeps of their own way of working, by name, with their account. */
export type Preferences = Schema<'Preferences'>
