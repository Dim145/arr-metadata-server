//! Webhooks and notifications: what happened, told to the destinations an
//! operator names.
//!
//! Every entry the audit trail takes is offered here, and so are the facts
//! nobody did — a work's refresh failing, a medium given up on, a newer
//! release out. Each destination names the events it wants, and is posted
//! them in its own way: signed as Standard Webhooks says, as the JSON the
//! single address of earlier versions was sent, as a line for Discord or
//! Slack, as a notification for ntfy, Gotify or Apprise (see [`kinds`]).
//!
//! Nothing is sent from where it happened. A delivery — one event owed to
//! one destination — is written to the database first, and a worker of its
//! own sends it: a destination that is slow or down costs the action
//! nothing, and a restart loses nothing owed. One that is not taken is tried
//! again five seconds later, then five minutes, half an hour, two hours,
//! five, ten and ten, and given up on after its eighth try; an address that
//! answers 410 Gone is switched off, and says why. An address that does not
//! answer at all — it times out, or no connection is made — is asked once for
//! everything due to it, and the rest is put off as that try was, so that it
//! holds up nobody else's. What was told is kept thirty days — the newest
//! thousand of each destination — for the Notifications page to show and
//! replay.
//!
//! Two things the trail holds are not passed on. The name typed into a
//! failed sign-in was typed by whoever came to the door, with no credential
//! at all, and is not something to relay to a chat channel. And a
//! destination's address is, for a Discord or Slack webhook or an ntfy
//! topic, the secret that lets anyone post there: like a signing secret or a
//! token, it is kept out of the log, and the API shows only its host.

mod digest;
mod kinds;

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use futures::StreamExt as _;
use tokio::sync::Notify;

pub use kinds::{Kind, Secret, Told};

use crate::{
    db::{
        self, Db,
        repo::{
            self,
            audit::Action,
            webhook::{Delivery, Destination, NewDestination, Settled, Status},
        },
    },
    domain::MediaItem,
    outbound,
    settings::Scope,
    state::AppState,
};

/// The actions a destination is told of when nobody chose: what enters and
/// leaves the catalogue, not every sign-in and refresh.
pub const DEFAULT_EVENTS: &str = "item.imported,item.created,item.deleted,dataset.imported";

/// Every event, those the server grows later included.
pub const EVERY: &str = "*";

/// What happens without anybody doing it, told as the trail's actions are:
/// by name, with `system` for the actor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fact {
    /// A work's refresh failed, where its refreshes had not been failing.
    RefreshFailed,
    /// A medium was given up on, its tries spent.
    MediaFailed,
    /// A release newer than the one running is out.
    UpdateAvailable,
    /// TheTVDB has not answered for a series long enough that TMDB numbers
    /// it in its place (`service::gather`).
    NumberingFallback,
    /// An archive of the server was made: by the schedule, by somebody, or
    /// before a restore.
    BackupCompleted,
    /// An archive could not be made.
    BackupFailed,
}

impl Fact {
    /// Every fact, in the order the Notifications page offers them.
    pub const ALL: &'static [Fact] = &[
        Self::RefreshFailed,
        Self::MediaFailed,
        Self::UpdateAvailable,
        Self::NumberingFallback,
        Self::BackupCompleted,
        Self::BackupFailed,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RefreshFailed => "refresh.failed",
            Self::MediaFailed => "media.failed",
            Self::UpdateAvailable => "update.available",
            Self::NumberingFallback => "numbering.fallback",
            Self::BackupCompleted => "backup.completed",
            Self::BackupFailed => "backup.failed",
        }
    }
}

/// What a test is posted as. Never one to ask for: it goes to the
/// destination whatever it is told of, and is no entry of the trail.
pub const TEST_EVENT: &str = "webhook.test";

/// What a test says.
const TEST_DETAIL: &str = "a test, sent from the Notifications page: this destination works";

/// Who a fact is told as the doing of.
const SYSTEM: &str = "system";

/// The most of an error a fact carries: enough to say what went wrong,
/// short enough for a line in a chat.
const ERROR_CHARS: usize = 300;

/// How long an address is given to answer: within the fifteen to thirty
/// seconds Standard Webhooks advises.
const TIMEOUT: Duration = Duration::from_secs(15);

/// How long it is given to be reached at all.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The pauses between two tries of a delivery that was not taken: five
/// seconds after the first, then five minutes, half an hour, two hours,
/// five, ten and ten — a day and more in all, as Standard Webhooks advises
/// for a receiver that is down for a night.
const RETRIES: [Duration; 7] = [
    Duration::from_secs(5),
    Duration::from_secs(5 * 60),
    Duration::from_secs(30 * 60),
    Duration::from_secs(2 * 3600),
    Duration::from_secs(5 * 3600),
    Duration::from_secs(10 * 3600),
    Duration::from_secs(10 * 3600),
];

/// The tries a delivery gets: the first, and one after each pause.
pub const MOST_ATTEMPTS: i32 = RETRIES.len() as i32 + 1;

/// The longest an address's own `Retry-After` puts a try off: the longest
/// pause of the schedule.
const LONGEST_PAUSE: Duration = Duration::from_secs(10 * 3600);

/// A claim older than this is a worker that stopped mid-send: well past the
/// timeout, so a slow answer is never taken for one.
const CLAIM_STALE: Duration = Duration::from_secs(120);

/// How many deliveries are taken from the line at a time.
const BATCH: i64 = 16;

/// How many destinations are sent to at once: to each, one delivery at a
/// time, in the order they are owed.
const AT_ONCE: usize = 4;

/// How long the worker sleeps when nothing rings and nothing falls due:
/// what another instance owed is not left waiting longer than this.
const LOOK_EVERY: Duration = Duration::from_secs(30);

/// The shortest it sleeps, so that a line it could not take is not asked
/// for again at once, and again.
const SHORTEST_PAUSE: Duration = Duration::from_secs(1);

/// How many deliveries may wait for one destination. Past that, an event is
/// not owed to it — never dropped from the trail, which is written first —
/// rather than queued without end behind an address that is not answering.
const MOST_PENDING: i64 = 1000;

/// How long the log is kept.
const KEPT_DAYS: i64 = 30;

/// How much of each destination's log is kept, at the most.
const MOST_KEPT: i64 = 1000;

/// How often the log is pruned.
const PRUNE_EVERY: Duration = Duration::from_secs(3600);

/// What a destination switched off after a 410 says.
pub const GONE: &str = "the address answered 410 Gone: the destination was switched off";

/// What the destination made of the single address in the settings is
/// called.
pub const ADOPTED_NAME: &str = "Webhook";

/// The two settings the single address was kept in until this version.
const LEGACY_URL: &str = "webhooks.url";
const LEGACY_EVENTS: &str = "webhooks.events";

/// What the webhooks keep from one event to the next: the client they are
/// posted with, the bell that wakes the worker, and which events the
/// destinations switched on want — asked before anything is looked up to
/// tell a fact with.
pub struct Hooks {
    client: reqwest::Client,
    wake: Notify,
    /// The events of each destination switched on, as last read.
    wanted: parking_lot::RwLock<Vec<Vec<String>>>,
    /// The failed refreshes waiting to be told together, for each
    /// destination that groups them.
    digest: digest::Digest,
}

impl Hooks {
    pub fn new() -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "arr-metadata-server/",
                env!("CARGO_PKG_VERSION"),
                " (+",
                env!("CARGO_PKG_REPOSITORY"),
                ")"
            ))
            // Loopback, link-local and the clouds' metadata services are
            // refused where each connection is made; a home network's own
            // addresses — an ntfy or a Gotify beside this server — are not.
            .dns_resolver(Arc::new(outbound::GuardedResolver::default()))
            // A proxy would resolve the name out of the resolver's sight.
            .no_proxy()
            // An address that moved is told to the operator, as Standard
            // Webhooks asks, rather than followed: a 3xx is a failure, and
            // the log says where to look.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .context("failed to build the webhooks' HTTP client")?;
        Ok(Self {
            client,
            wake: Notify::new(),
            wanted: parking_lot::RwLock::new(Vec::new()),
            digest: digest::Digest::default(),
        })
    }

    /// Whether a destination switched on wants this event, as last read.
    fn wants(&self, event: &str) -> bool {
        self.wanted
            .read()
            .iter()
            .any(|events| wanted(events, event))
    }

    /// Read the destinations again: which events are wanted, and what of
    /// each to keep out of the log.
    pub async fn reload(&self, db: &Db) -> anyhow::Result<()> {
        let destinations = repo::webhook::all(db).await?;
        for destination in &destinations {
            hide(destination);
        }
        *self.wanted.write() = destinations
            .into_iter()
            .filter(|d| d.enabled)
            .map(|d| d.events)
            .collect();
        Ok(())
    }

    /// Wake the worker: something is owed.
    pub fn wake(&self) {
        self.wake.notify_one();
    }
}

/// One thing that happened, owned, so it can outlive the request it came in.
pub struct Happening {
    pub action: Action,
    pub actor: Option<String>,
    pub target: Option<String>,
    pub detail: Option<String>,
}

/// Owe this to every destination that wants it. Returns once the
/// deliveries are written: the sending is the worker's.
pub async fn notify(state: &AppState, happening: Happening) {
    let happening = discreet(happening);
    let told = Told {
        event: happening.action.as_str().to_string(),
        actor: happening.actor,
        target: happening.target,
        detail: happening.detail,
        at: at_now(),
        count: None,
    };
    owe(state, &told).await;
}

/// Owe a fact to every destination that wants it. Returns at once; the
/// owing is a task of its own.
pub fn tell(state: &AppState, fact: Fact, target: Option<String>, detail: Option<String>) {
    let told = Told {
        event: fact.as_str().to_string(),
        actor: Some(SYSTEM.to_string()),
        target,
        detail,
        at: at_now(),
        count: None,
    };
    let state = state.clone();
    tokio::spawn(async move { owe(&state, &told).await });
}

/// Whether a fact would be told now: a destination switched on wants it.
/// Asked before anything is looked up to tell it with.
pub fn wants(state: &AppState, fact: Fact) -> bool {
    state.webhooks.wants(fact.as_str())
}

/// A work's refresh failed where its refreshes had not been failing:
/// `refresh.failed`, with the work and the start of what went wrong. Told
/// when a work starts failing and not at each try while it goes on failing,
/// which an outage would turn into a post per work every few hours.
pub fn refresh_failed(state: &AppState, id: &str, work: Option<&MediaItem>, error: &str) {
    if !wants(state, Fact::RefreshFailed) || !first_telling(&TOLD_FAILING, id, Instant::now()) {
        return;
    }
    let named = match work {
        Some(work) => named_work(id, &work.title, work.kind),
        None => id.to_string(),
    };
    let told = Told {
        event: Fact::RefreshFailed.as_str().to_string(),
        actor: Some(SYSTEM.to_string()),
        target: Some(id.to_string()),
        detail: Some(format!(
            "{named}: {}",
            crate::providers::clip(error, ERROR_CHARS)
        )),
        at: at_now(),
        count: None,
    };
    // Held a few minutes for a destination that groups failures, and told
    // with the others then (see `digest`); owed at once to the rest.
    let failure = digest::Failure {
        cause: digest::cause(error),
        named,
    };
    let state = state.clone();
    tokio::spawn(async move { owe_holding(&state, &told, Some(&failure)).await });
}

/// A medium given up on, its tries spent: `media.failed`, with the address
/// it was fetched from — without its query, which may carry a token — the
/// work that wanted it, by its title while it is held, and what went wrong
/// the last time.
pub async fn media_failed(state: &AppState, asset: &repo::asset::Asset, error: &str) {
    if !wants(state, Fact::MediaFailed) {
        return;
    }
    let address = without_query(&asset.origin);
    let wanted = match &asset.wanted_by {
        Some(id) => {
            let held = repo::item::titles(&state.db, std::slice::from_ref(id))
                .await
                .ok()
                .and_then(|mut titles| titles.remove(id));
            Some(match held {
                Some(work) => named_work(id, &work.title, work.kind),
                None => id.clone(),
            })
        }
        None => None,
    };
    let what = match wanted {
        Some(work) => format!("the {} of {work}", asset.kind.as_str()),
        None => format!("an {}", asset.kind.as_str()),
    };
    tell(
        state,
        Fact::MediaFailed,
        Some(address.clone()),
        Some(format!(
            "{what} was given up on: {} — {address}",
            crate::providers::clip(error, ERROR_CHARS)
        )),
    );
}

/// A newer release: `update.available`, with its version and its page.
pub fn update_available(state: &AppState, release: &crate::jobs::update_check::Release) {
    tell(
        state,
        Fact::UpdateAvailable,
        Some(release.version.clone()),
        Some(format!(
            "{} is out, and this server runs {}: {}",
            release.version,
            env!("CARGO_PKG_VERSION"),
            release.url
        )),
    );
}

/// An archive made: `backup.completed`, with its name, what it weighs and
/// why it was made.
pub fn backup_completed(state: &AppState, name: &str, detail: &str) {
    tell(
        state,
        Fact::BackupCompleted,
        Some(name.to_string()),
        Some(detail.to_string()),
    );
}

/// An archive that could not be made: `backup.failed`, with why it was
/// asked for and the start of what went wrong. Told, because a schedule
/// that stopped making archives is otherwise found out on the day one is
/// needed.
pub fn backup_failed(state: &AppState, why: &str, error: &str) {
    tell(
        state,
        Fact::BackupFailed,
        Some(why.to_string()),
        Some(crate::providers::clip(error, ERROR_CHARS)),
    );
}

/// The works a failed refresh was told of lately, and when. One failure can
/// be seen twice — by the fetch that fell back on the copy held, and then by
/// the sweep that asked for it, each reading the work as it was before — and
/// is told once.
static TOLD_FAILING: LazyLock<parking_lot::Mutex<HashMap<String, Instant>>> =
    LazyLock::new(Default::default);

/// How long a work told of is not told of again.
const TOLD_FOR: Duration = Duration::from_secs(3600);

/// The most works remembered as told of: past that, one not remembered is
/// told, rather than the memory growing with an outage.
const MOST_TOLD: usize = 10_000;

/// Whether `id` is told of now: not told of within [`TOLD_FOR`].
fn first_telling(
    told: &parking_lot::Mutex<HashMap<String, Instant>>,
    id: &str,
    now: Instant,
) -> bool {
    let mut told = told.lock();
    if told
        .get(id)
        .is_some_and(|at| now.duration_since(*at) < TOLD_FOR)
    {
        return false;
    }
    if told.len() >= MOST_TOLD {
        told.retain(|_, at| now.duration_since(*at) < TOLD_FOR);
    }
    if told.len() < MOST_TOLD {
        told.insert(id.to_string(), now);
    }
    true
}

/// A work as a fact names it: `“Title” (series {id})`.
fn named_work(id: &str, title: &str, kind: crate::domain::MediaKind) -> String {
    format!("“{title}” ({} {id})", kind.as_str())
}

/// An address without its query or its fragment.
fn without_query(address: &str) -> String {
    address
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Now, as what is told says when it happened.
fn at_now() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Owe what is told to every destination switched on that wants it: a
/// delivery each, under one message id, and the worker woken.
async fn owe(state: &AppState, told: &Told) {
    owe_holding(state, told, None).await;
}

/// Owe a fact to every destination that wants it — or, a failed refresh
/// (`failure`) for a destination that groups them, hold it to be told with
/// the others.
async fn owe_holding(state: &AppState, told: &Told, failure: Option<&digest::Failure>) {
    // Alone, the destinations as last read are the destinations: every
    // change to them is made here, and read again at once. Among several,
    // one may have been added elsewhere a moment ago, and the table is asked.
    if !state.coord.is_multi() && !state.webhooks.wants(&told.event) {
        return;
    }
    let destinations = match repo::webhook::enabled(&state.db).await {
        Ok(destinations) => destinations,
        Err(e) => {
            tracing::warn!(error = %e, event = %told.event, "the destinations could not be read; the event is not told");
            return;
        }
    };
    let wanting: Vec<&Destination> = destinations
        .iter()
        .filter(|d| wanted(&d.events, &told.event))
        .collect();
    if wanting.is_empty() {
        return;
    }
    let payload = match serde_json::to_string(told) {
        Ok(payload) => payload,
        Err(e) => {
            tracing::warn!(error = %e, event = %told.event, "what was to be told could not be written");
            return;
        }
    };

    let message_id = kinds::message_id();
    let mut owed = false;
    for destination in wanting {
        if destination.grouped
            && let Some(failure) = failure
        {
            state
                .webhooks
                .digest
                .hold(&destination.id, told, failure.clone());
            continue;
        }
        match repo::webhook::enqueue(
            &state.db,
            &destination.id,
            &message_id,
            &told.event,
            &payload,
            MOST_PENDING,
        )
        .await
        {
            Ok(true) => owed = true,
            Ok(false) => tracing::warn!(
                destination = %destination.name,
                event = %told.event,
                "the event is not owed to this destination: {MOST_PENDING} deliveries already wait for it"
            ),
            Err(e) => tracing::warn!(
                destination = %destination.name,
                event = %told.event,
                error = %e,
                "the delivery could not be written"
            ),
        }
    }
    if owed {
        state.webhooks.wake();
    }
}

/// Whether an event is among those named — `*` for every one.
pub fn wanted(events: &[String], event: &str) -> bool {
    events.iter().any(|e| e == EVERY || e == event)
}

/// The names a list kept as text holds: separated by commas, trimmed, the
/// empty ones dropped.
pub fn names_in(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// The happening with what is not to be told left out: the name typed into
/// a failed sign-in.
fn discreet(mut happening: Happening) -> Happening {
    if happening.action == Action::SignInFailed {
        happening.target = None;
        happening.detail = None;
    }
    happening
}

// ─── the start ───────────────────────────────────────────────────────────────

/// Start the webhooks: the single address the settings held made a
/// destination, if none exists yet, and the destinations read. Run under
/// the start's lock, so that several instances starting together make it
/// once.
pub async fn start(state: &AppState) -> anyhow::Result<()> {
    adopt_the_settings(state).await?;
    state.webhooks.reload(&state.db).await
}

/// Until this version one address was posted to, kept in `webhooks.url`
/// with its events in `webhooks.events`. Set, and with no destination yet,
/// it becomes the `json` destination “Webhook”, told of the same events in
/// the same shape. Either way the two settings are then removed: nothing
/// reads them any more, and the address — a secret, for most services — is
/// kept in one place.
async fn adopt_the_settings(state: &AppState) -> anyhow::Result<()> {
    let url = state.settings.at(Scope::Server, "", LEGACY_URL);
    let events = state.settings.at(Scope::Server, "", LEGACY_EVENTS);
    if url.is_none() && events.is_none() {
        return Ok(());
    }

    if let Some(url) = url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
        if repo::webhook::count(&state.db).await? == 0 {
            let names = names_in(events.as_deref().unwrap_or(DEFAULT_EVENTS));
            repo::webhook::create(
                &state.db,
                &NewDestination {
                    name: ADOPTED_NAME,
                    kind: Kind::Json.as_str(),
                    url,
                    secret: None,
                    events: &names,
                    enabled: true,
                    // A program reads it: an event per work.
                    grouped: false,
                },
            )
            .await?;
            if let Err(e) = repo::audit::record(
                &state.db,
                repo::audit::Record {
                    actor: Some(SYSTEM),
                    action: Action::NotificationCreated,
                    target: Some(ADOPTED_NAME),
                    detail: Some("json, from the settings webhooks.url and webhooks.events"),
                    ip: None,
                },
            )
            .await
            {
                tracing::warn!(error = %e, "could not write the audit entry");
            }
            tracing::info!(
                events = names.len(),
                "the webhook address the settings held is now the destination “{ADOPTED_NAME}”, \
                 on the Notifications page; webhooks.url and webhooks.events are no longer read"
            );
        } else {
            tracing::warn!(
                "webhooks.url was set beside destinations that already exist: it is no longer \
                 read, and is removed — add it on the Notifications page if it is still wanted"
            );
        }
    }

    state.settings.clear(Scope::Server, "", LEGACY_URL).await?;
    state
        .settings
        .clear(Scope::Server, "", LEGACY_EVENTS)
        .await?;
    Ok(())
}

// ─── the worker ──────────────────────────────────────────────────────────────

/// The worker: from the start, for as long as the server runs. Woken when
/// a delivery is owed, and otherwise when the next one falls due — never
/// later than [`LOOK_EVERY`], so that what another instance owed, or what
/// waited through a restart, is not left long.
pub async fn run(state: AppState) {
    let mut pruned: Option<Instant> = None;
    loop {
        // Among several instances, a destination changed elsewhere is
        // known here by the next look.
        if let Err(e) = state.webhooks.reload(&state.db).await {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "the destinations could not be read"
            );
        }

        match repo::webhook::give_up_spent(&state.db, MOST_ATTEMPTS, &claim_cutoff()).await {
            Ok(0) => {}
            Ok(given_up) => tracing::warn!(
                given_up,
                "deliveries whose last try was cut short by a stop were given up on"
            ),
            Err(e) => tracing::warn!(error = %e, "could not give up on spent deliveries"),
        }

        // What waited its window to be told together, owed now: the batch
        // just below sends it.
        tell_grouped(&state).await;

        loop {
            match batch(&state).await {
                Ok(taken) if taken < BATCH as usize => break,
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!(
                        error = format_args!("{e:#}"),
                        "the webhook worker could not read its line"
                    );
                    break;
                }
            }
        }

        // The leader's, alone always this instance: the log is one.
        if state.coord.leads() && pruned.is_none_or(|at| at.elapsed() >= PRUNE_EVERY) {
            prune(&state).await;
            pruned = Some(Instant::now());
        }

        let pause = until_next(&state)
            .await
            .min(state.webhooks.digest.until_due().unwrap_or(LOOK_EVERY))
            .max(SHORTEST_PAUSE);
        let _ = tokio::time::timeout(pause, state.webhooks.wake.notified()).await;
    }
}

/// Owe each destination that groups failed refreshes the message its
/// window made, once the window is past.
async fn tell_grouped(state: &AppState) {
    owe_grouped(state, state.webhooks.digest.due()).await;
}

/// Owe each destination its grouped message.
async fn owe_grouped(state: &AppState, due: Vec<(String, Told)>) {
    for (destination, told) in due {
        let payload = match serde_json::to_string(&told) {
            Ok(payload) => payload,
            Err(e) => {
                tracing::warn!(error = %e, "the grouped failures could not be written");
                continue;
            }
        };
        match repo::webhook::enqueue(
            &state.db,
            &destination,
            &kinds::message_id(),
            &told.event,
            &payload,
            MOST_PENDING,
        )
        .await
        {
            Ok(_) => {}
            // Removed while its failures waited, as like as not.
            Err(e) => tracing::warn!(
                error = %e,
                "the grouped failures could not be owed to their destination"
            ),
        }
    }
}

/// When a claim taken before now is stale.
fn claim_cutoff() -> String {
    let stale = chrono::Duration::from_std(CLAIM_STALE).unwrap_or_default();
    db::to_rfc3339(Utc::now() - stale)
}

/// How long to sleep before the next delivery falls due.
async fn until_next(state: &AppState) -> Duration {
    match repo::webhook::next_due(&state.db, MOST_ATTEMPTS, &claim_cutoff()).await {
        Ok(Some(at)) => db::parse_rfc3339(&at)
            .map(|at| (at - Utc::now()).to_std().unwrap_or_default())
            .unwrap_or(LOOK_EVERY)
            .clamp(SHORTEST_PAUSE, LOOK_EVERY),
        Ok(None) => LOOK_EVERY,
        Err(e) => {
            tracing::warn!(error = %e, "could not read when the next delivery is due");
            LOOK_EVERY
        }
    }
}

/// What posts: the client, and in a test something that never leaves the
/// machine.
trait Poster: Sync {
    fn post(&self, post: &kinds::Post) -> impl std::future::Future<Output = Outcome> + Send;
}

impl Poster for reqwest::Client {
    async fn post(&self, post: &kinds::Post) -> Outcome {
        send(self, post).await
    }
}

/// What a delivery put off says of itself: the address it is owed to had
/// just failed to answer.
const SET_ASIDE: &str = "not sent: the address had just failed to answer another delivery, and is \
                         left alone until this one's next try";

/// Send the next few due: a destination's deliveries one after another, a few
/// destinations at a time. How many were taken.
async fn batch(state: &AppState) -> anyhow::Result<usize> {
    batch_with(state, &state.webhooks.client).await
}

async fn batch_with(state: &AppState, poster: &impl Poster) -> anyhow::Result<usize> {
    let cutoff = claim_cutoff();
    let due = repo::webhook::due(&state.db, MOST_ATTEMPTS, &cutoff, BATCH).await?;
    if due.is_empty() {
        return Ok(0);
    }
    let destinations: HashMap<String, Destination> = repo::webhook::enabled(&state.db)
        .await?
        .into_iter()
        .map(|d| (d.id.clone(), d))
        .collect();

    // Each destination's deliveries in a line of their own, in the order they
    // are owed; those of a destination switched off meanwhile are not sent.
    let mut lines: Vec<(Destination, Vec<Delivery>)> = Vec::new();
    for delivery in due {
        let Some(destination) = destinations.get(&delivery.destination_id) else {
            continue;
        };
        match lines.iter_mut().find(|(held, _)| held.id == destination.id) {
            Some((_, owed)) => owed.push(delivery),
            None => lines.push((destination.clone(), vec![delivery])),
        }
    }

    let taken = futures::stream::iter(lines)
        .map(|(destination, owed)| line(state, poster, &cutoff, destination, owed))
        .buffer_unordered(AT_ONCE)
        .fold(0, |taken, line| std::future::ready(taken + line))
        .await;
    Ok(taken)
}

/// One destination's deliveries, one after another in the order they are
/// owed. How many were taken.
///
/// An address that does not answer — it times out, or no connection can be
/// made — is asked once. What else is due to it is then put off as that try
/// was, rather than sent in its turn to wait out the same timeout: the worker
/// is not held by an address that is down, nor are the destinations beside it
/// kept waiting.
async fn line(
    state: &AppState,
    poster: &impl Poster,
    cutoff: &str,
    destination: Destination,
    owed: Vec<Delivery>,
) -> usize {
    let mut taken = 0;
    for delivery in owed {
        match deliver(state, poster, cutoff, delivery, &destination, None).await {
            Tried::Another => {}
            Tried::Done => taken += 1,
            Tried::Silent => {
                taken += 1 + set_aside(state, poster, cutoff, &destination).await;
                break;
            }
        }
    }
    taken
}

/// Put off every delivery of a destination that is due, as a try that got no
/// answer is: each one's try is counted and each waits the pause its own
/// schedule gives it. An address that is down so falls back through the
/// pauses with all it is owed at once, and costs the worker one timeout a
/// pause, not one a delivery. How many were put off.
async fn set_aside(
    state: &AppState,
    poster: &impl Poster,
    cutoff: &str,
    destination: &Destination,
) -> usize {
    let owed = match repo::webhook::due_of(
        &state.db,
        &destination.id,
        MOST_ATTEMPTS,
        cutoff,
        MOST_PENDING,
    )
    .await
    {
        Ok(owed) => owed,
        Err(e) => {
            tracing::warn!(error = %e, "could not read what else is owed to an address that did not answer");
            return 0;
        }
    };
    let held = Outcome::Unsent {
        why: SET_ASIDE.to_string(),
    };
    let mut put_off = 0;
    for delivery in owed {
        let tried = deliver(state, poster, cutoff, delivery, destination, Some(&held)).await;
        if matches!(tried, Tried::Done) {
            put_off += 1;
        }
    }
    if put_off > 0 {
        let host = acceptable(&destination.url).unwrap_or_default();
        tracing::warn!(
            destination = %destination.name, host, put_off,
            "the address did not answer: what else is owed to it is put off, as that try was"
        );
    }
    put_off
}

/// How taking a delivery went.
enum Tried {
    /// Another worker's to take.
    Another,
    /// Taken, and how it went is written.
    Done,
    /// Taken, and the address did not answer: it timed out, or no connection
    /// could be made.
    Silent,
}

/// Take one delivery, try it, and write how it went. With `held`, it is not
/// sent: the try is counted and written as that outcome says, for a delivery
/// put off after another did not get an answer.
async fn deliver(
    state: &AppState,
    poster: &impl Poster,
    cutoff: &str,
    delivery: Delivery,
    destination: &Destination,
    held: Option<&Outcome>,
) -> Tried {
    let attempt = delivery.attempts + 1;
    let now = Utc::now();
    // Due again then, should the server stop before the try comes back.
    let fallback = retry_in(attempt)
        .and_then(|pause| chrono::Duration::from_std(pause).ok())
        .map_or(now, |pause| now + pause);
    match repo::webhook::claim(
        &state.db,
        &delivery.id,
        &state.instance,
        MOST_ATTEMPTS,
        cutoff,
        &db::to_rfc3339(fallback),
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => return Tried::Another,
        Err(e) => {
            tracing::warn!(error = %e, "could not take a delivery");
            return Tried::Another;
        }
    }

    let outcome = match held {
        Some(held) => held.clone(),
        None => match prepare(destination, &delivery) {
            Ok(post) => poster.post(&post).await,
            Err(why) => Outcome::Unsent { why },
        },
    };
    let silent = matches!(outcome, Outcome::Unreached { .. });
    let verdict = verdict(attempt, &outcome, Utc::now());
    match repo::webhook::settle(&state.db, &delivery.id, &state.instance, &verdict.settled).await {
        Ok(true) => {}
        Ok(false) => tracing::warn!(
            destination = %destination.name,
            "a delivery was taken over by another worker before its try came back"
        ),
        Err(e) => tracing::warn!(error = %e, "could not write how a delivery went"),
    }
    // A delivery put off is told of once, with the rest, by whoever put it off.
    if held.is_none() {
        report(destination, &delivery, attempt, &verdict);
    }

    if verdict.gone {
        if let Err(e) = repo::webhook::switch_off(&state.db, &destination.id, GONE).await {
            tracing::warn!(error = %e, "could not switch the destination off");
        }
        if let Err(e) = state.webhooks.reload(&state.db).await {
            tracing::warn!(error = %e, "the destinations could not be read");
        }
    }
    if silent { Tried::Silent } else { Tried::Done }
}

/// The post a delivery is sent as, now: its destination's kind, address and
/// secret, and what it tells.
fn prepare(destination: &Destination, delivery: &Delivery) -> Result<kinds::Post, String> {
    let kind = Kind::parse(&destination.kind)
        .ok_or_else(|| "this server does not know this destination's kind".to_string())?;
    let told: Told = serde_json::from_str(&delivery.payload)
        .map_err(|_| "what was to be told could not be read".to_string())?;
    kinds::post(
        kind,
        &destination.url,
        destination.secret.as_deref(),
        &delivery.message_id,
        &told,
        Utc::now().timestamp(),
    )
}

/// How a try went, in the log — by the destination's name and host, never
/// its address.
fn report(destination: &Destination, delivery: &Delivery, attempt: i32, verdict: &Verdict) {
    let host = acceptable(&destination.url).unwrap_or_default();
    let settled = &verdict.settled;
    let status = settled.last_status;
    let error = settled.last_error.as_deref().unwrap_or_default();
    match settled.status {
        Status::Delivered => tracing::debug!(
            destination = %destination.name, host, event = %delivery.event, attempt,
            "a delivery was taken"
        ),
        Status::Pending => tracing::info!(
            destination = %destination.name, host, event = %delivery.event, attempt, status, error,
            again_at = settled.next_attempt_at.as_deref().unwrap_or_default(),
            "a delivery was not taken; it is tried again later"
        ),
        Status::Failed if verdict.gone => tracing::warn!(
            destination = %destination.name, host, event = %delivery.event,
            "the address answered 410 Gone: the destination is switched off"
        ),
        Status::Failed => tracing::warn!(
            destination = %destination.name, host, event = %delivery.event, attempt, status, error,
            "a delivery was given up on, its tries spent"
        ),
    }
}

/// Forget the log past its thirty days, and past the newest thousand of
/// each destination.
async fn prune(state: &AppState) {
    let Some(cutoff) = chrono::TimeDelta::try_days(KEPT_DAYS)
        .and_then(|days| Utc::now().checked_sub_signed(days))
        .map(db::to_rfc3339)
    else {
        return;
    };
    match repo::webhook::prune(&state.db, &cutoff, MOST_KEPT).await {
        Ok(0) => {}
        Ok(forgotten) => tracing::info!(forgotten, "forgot deliveries past the log's keeping"),
        Err(e) => tracing::warn!(error = %e, "could not prune the deliveries' log"),
    }
}

/// The pause before the next try, once `attempts` were made and the last
/// was not taken: none once they are spent.
fn retry_in(attempts: i32) -> Option<Duration> {
    usize::try_from(attempts - 1)
        .ok()
        .and_then(|after| RETRIES.get(after).copied())
}

/// What a try came to, and whether the destination is gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub settled: Settled,
    /// The address answered 410 Gone: the destination is to be switched off.
    pub gone: bool,
}

/// What becomes of a delivery whose try number `attempts` came to this, at
/// `now`: taken; tried again after its pause — or after what the address
/// asked for, when that is longer; given up on once its tries are spent;
/// and its destination switched off when the address is gone.
pub fn verdict(attempts: i32, outcome: &Outcome, now: DateTime<Utc>) -> Verdict {
    let millis = |took: &Duration| i64::try_from(took.as_millis()).unwrap_or(i64::MAX);
    let (status, error, took, asked) = match outcome {
        Outcome::Answered {
            status,
            took,
            retry_after,
            said,
        } => {
            let error = if status.is_redirection() {
                Some(
                    "redirected: the address has moved, and redirects are not followed — set \
                     the new one"
                        .to_string(),
                )
            } else {
                said.clone()
            };
            (Some(*status), error, Some(millis(took)), *retry_after)
        }
        Outcome::Unreached { why, took } => (None, Some(why.clone()), Some(millis(took)), None),
        Outcome::Refused(refusal) => (None, Some(refusal.says().to_string()), None, None),
        Outcome::Unsent { why } => (None, Some(why.clone()), None, None),
    };
    let last_status = status.map(|s| i32::from(s.as_u16()));

    if status.is_some_and(|s| s.is_success()) {
        return Verdict {
            settled: Settled {
                status: Status::Delivered,
                next_attempt_at: None,
                last_status,
                last_error: None,
                duration_ms: took,
            },
            gone: false,
        };
    }
    if status == Some(reqwest::StatusCode::GONE) {
        return Verdict {
            settled: Settled {
                status: Status::Failed,
                next_attempt_at: None,
                last_status,
                last_error: Some(GONE.to_string()),
                duration_ms: took,
            },
            gone: true,
        };
    }

    let settled = match retry_in(attempts) {
        Some(pause) => {
            let pause = pause.max(asked.unwrap_or_default().min(LONGEST_PAUSE));
            let at = chrono::Duration::from_std(pause)
                .ok()
                .and_then(|pause| now.checked_add_signed(pause))
                .unwrap_or(now);
            Settled {
                status: Status::Pending,
                next_attempt_at: Some(db::to_rfc3339(at)),
                last_status,
                last_error: error,
                duration_ms: took,
            }
        }
        None => Settled {
            status: Status::Failed,
            next_attempt_at: None,
            last_status,
            last_error: error,
            duration_ms: took,
        },
    };
    Verdict {
        settled,
        gone: false,
    }
}

// ─── sending ─────────────────────────────────────────────────────────────────

/// How a post went.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// Answered, with this status, after this long; how long it asked to be
    /// left alone, when it said; and what it said, when it was not a success
    /// — the start of it, with whatever names a secret masked.
    Answered {
        status: reqwest::StatusCode,
        took: Duration,
        retry_after: Option<Duration>,
        said: Option<String>,
    },
    /// Not reached, for this reason — in words that name no address —
    /// after this long.
    Unreached { why: String, took: Duration },
    /// Not sent: the address is not one this server posts to.
    Refused(Refusal),
    /// Not sent: the post could not be made — a standard destination
    /// without its secret, a payload that does not read.
    Unsent { why: String },
}

/// Why an address is not posted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not an http or https address, or one that names this machine.
    NotWeb,
    /// A name that resolves to an address only reachable from here.
    Internal,
    /// A token, or an address that holds one, over plain http to a host
    /// beyond the home network: anybody on the way could read it.
    Cleartext,
}

impl Refusal {
    /// What the log of a delivery says of it.
    const fn says(self) -> &'static str {
        match self {
            Self::NotWeb => {
                "not sent: this server posts only to an http or https address that names no \
                 machine of its own"
            }
            Self::Internal => {
                "not sent: the address's name resolves to an address only reachable from this server"
            }
            Self::Cleartext => {
                "not sent: a destination's token goes out only over https, or to an address of the \
                 home network"
            }
        }
    }
}

/// Why a post is not to be sent, when it is not.
async fn refusal(post: &kinds::Post) -> Option<Refusal> {
    // An address to post to, and not one that only means something from in
    // here: the operator names it, but a destination is a thing that can be
    // set by whoever holds the administration.
    if acceptable(&post.url).is_none() {
        return Some(Refusal::NotWeb);
    }
    // Asked again, of the resolver, just before the post — and by the
    // client's own resolver as it connects: a name that meant a public
    // address when it was set can mean this machine now.
    if outbound::resolves_internally(&post.url).await {
        return Some(Refusal::Internal);
    }
    // What lets anybody post as this server crosses no network in clear but
    // the home one, where a Gotify or an ntfy beside the server is often
    // reached over plain http.
    if post.carries_credential && cleartext_beyond_home(&post.url) {
        return Some(Refusal::Cleartext);
    }
    None
}

/// Post one, and say how it went.
async fn send(client: &reqwest::Client, post: &kinds::Post) -> Outcome {
    if let Some(refusal) = refusal(post).await {
        return Outcome::Refused(refusal);
    }
    let started = Instant::now();
    let mut request = client.post(&post.url).body(post.body.clone());
    for (name, value) in &post.headers {
        request = request.header(name.clone(), value.clone());
    }
    match request.send().await {
        Ok(response) => {
            let status = response.status();
            let retry_after = pause_asked(response.headers());
            let said = if status.is_success() || status.is_redirection() {
                None
            } else {
                // What it said, which may quote what it was sent: the
                // destination's secrets are masked in it as in the log.
                Some(crate::telemetry::redact(
                    &crate::providers::error_text(response).await,
                ))
                .filter(|said| !said.is_empty())
            };
            Outcome::Answered {
                status,
                took: started.elapsed(),
                retry_after,
                said,
            }
        }
        // Never `%e`: reqwest's own words end with the address, and the
        // address is the destination's secret.
        Err(e) => Outcome::Unreached {
            why: crate::providers::describe_request_error(&e),
            took: started.elapsed(),
        },
    }
}

/// How long an answer asks to be left alone: its `Retry-After`, in seconds
/// or as a date.
fn pause_asked(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    match value.parse::<u64>() {
        Ok(seconds) => Some(Duration::from_secs(seconds)),
        Err(_) => {
            let at = DateTime::parse_from_rfc2822(value).ok()?;
            (at.with_timezone(&Utc) - Utc::now()).to_std().ok()
        }
    }
}

/// Post `webhook.test` to a destination, whatever it is told of, and say
/// how it went. Nothing is written: neither in the trail nor in its log.
/// `by` is who asked, and the actor the test is posted as.
pub async fn test(state: &AppState, destination: &Destination, by: &str) -> Outcome {
    let Some(kind) = Kind::parse(&destination.kind) else {
        return Outcome::Unsent {
            why: "this server does not know this destination's kind".to_string(),
        };
    };
    let told = Told {
        event: TEST_EVENT.to_string(),
        actor: Some(by.to_string()),
        target: None,
        detail: Some(TEST_DETAIL.to_string()),
        at: at_now(),
        count: None,
    };
    match kinds::post(
        kind,
        &destination.url,
        destination.secret.as_deref(),
        &kinds::message_id(),
        &told,
        Utc::now().timestamp(),
    ) {
        Ok(post) => send(&state.webhooks.client, &post).await,
        Err(why) => Outcome::Unsent { why },
    }
}

/// The host of an address this server will post to: http or https, and not
/// an address that only means something from where the server stands.
/// Whether an address is plain http to a host beyond the home network: one
/// that is no private address (RFC 1918, the shared space of carrier and
/// mesh networks, an IPv6 unique local one) nor a name only a home network
/// knows (`.lan`, `.local`, `.home.arpa`, `.internal`, a name without a
/// dot).
fn cleartext_beyond_home(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return true;
    };
    if parsed.scheme() != "http" {
        return false;
    }
    let home = match parsed.host() {
        Some(url::Host::Ipv4(v4)) => {
            v4.is_private() || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
        }
        Some(url::Host::Ipv6(v6)) => (v6.segments()[0] & 0xfe00) == 0xfc00,
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            !name.contains('.')
                || [".lan", ".local", ".home.arpa", ".internal"]
                    .iter()
                    .any(|end| name.ends_with(end))
        }
        None => false,
    };
    !home
}

fn acceptable(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") || outbound::names_internal_host(url) {
        return None;
    }
    parsed.host_str().map(str::to_string)
}

// ─── what a destination may be ───────────────────────────────────────────────

/// The longest name a destination may be given.
const NAME_CHARS: usize = 100;

/// The longest address.
const ADDRESS_BYTES: usize = 2000;

/// The longest token.
const TOKEN_BYTES: usize = 500;

/// The longest an event's name may be, and how many a destination may name.
const EVENT_BYTES: usize = 64;
const MOST_EVENTS: usize = 200;

/// A destination's name, trimmed, or why it cannot be one.
pub fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        Err("a destination needs a name".to_string())
    } else if name.chars().count() > NAME_CHARS {
        Err(format!("a name is {NAME_CHARS} characters at the most"))
    } else {
        Ok(name.to_string())
    }
}

/// An address to post to, trimmed, or why it is not one: an http or https
/// address that names a host, and not this machine or an address only it
/// can reach — what a name resolves to is asked at each post. ntfy is
/// posted to a topic, which is the address's path.
pub fn check_address(kind: Kind, address: &str) -> Result<String, String> {
    let address = address.trim();
    if address.is_empty() {
        return Err("a destination needs an address".to_string());
    }
    if address.len() > ADDRESS_BYTES {
        return Err("the address is too long".to_string());
    }
    let parsed = url::Url::parse(address)
        .map_err(|_| "the address is not one: it is written https://host/path".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("the address is an http or https one".to_string());
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("the address names no host".to_string());
    }
    if outbound::names_internal_host(address) {
        return Err(
            "the address names this machine, or an address only it can reach — loopback, \
             link-local, a cloud's metadata service"
                .to_string(),
        );
    }
    // These always carry what lets anybody post as this server: never in
    // clear beyond the home network. An ntfy topic's token, when it has one,
    // is held to the same when it is sent.
    if matches!(kind, Kind::Gotify | Kind::Discord | Kind::Slack) && cleartext_beyond_home(address)
    {
        return Err(
            "this destination's token would cross the internet in clear: give it an https \
             address, or one of the home network"
                .to_string(),
        );
    }
    if kind == Kind::Ntfy && parsed.path().trim_matches('/').is_empty() {
        return Err(
            "ntfy is posted to its topic's address: https://ntfy.sh/your-topic".to_string(),
        );
    }
    Ok(address.to_string())
}

/// A token as it travels in a header, or why it cannot.
pub fn check_token(token: &str) -> Result<String, String> {
    let token = token.trim();
    if token.len() > TOKEN_BYTES {
        Err("the token is too long".to_string())
    } else if !token.bytes().all(|b| b.is_ascii_graphic()) {
        Err("a token is written in visible characters, without spaces".to_string())
    } else {
        Ok(token.to_string())
    }
}

/// The events a destination names, as kept: each once, in the order given,
/// and `*` alone when it is among them — every event says the rest. Any
/// name of the right shape is kept, a name this server does not know too,
/// as the single address's setting kept it.
pub fn check_events(names: &[String]) -> Result<Vec<String>, String> {
    let mut kept: Vec<String> = Vec::new();
    for name in names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let shaped = name == EVERY
            || (name.len() <= EVENT_BYTES
                && name.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_'
                }));
        if !shaped {
            return Err(format!(
                "{:?} is not the name of an event",
                crate::providers::clip(name, EVENT_BYTES)
            ));
        }
        if !kept.iter().any(|k| k == name) {
            kept.push(name.to_string());
        }
    }
    if kept.len() > MOST_EVENTS {
        return Err(format!(
            "a destination names {MOST_EVENTS} events at the most"
        ));
    }
    if kept.iter().any(|k| k == EVERY) {
        kept = vec![EVERY.to_string()];
    }
    Ok(kept)
}

/// What a destination keeps as its secret after a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NextSecret {
    /// This, nothing included.
    Store(Option<String>),
    /// A signing secret, to make now and show once.
    Make,
}

/// What a destination of `kind` keeps as its secret, given what it was —
/// its kind and secret, for one that exists — whether its address `moved` to
/// another origin (another scheme, host or port than the one the secret was
/// kept for), and the token asked for: left out, the one kept stays while the
/// destination stays the same kind at the same origin; empty, it is removed.
/// A token is its service's, given for one server: one kept for an ntfy topic
/// is not handed to a Gotify server, nor to another ntfy, unless it is given
/// again. A signing secret is made here, once, and kept while the kind stays
/// `standard` at the same origin: at another, the receiver there is given a
/// new one, and the old receiver, which holds the old, signs for nobody. Why
/// not, when the kind needs a token and has none.
pub fn next_secret(
    before: Option<(Kind, Option<&str>)>,
    kind: Kind,
    moved: bool,
    token: Option<&str>,
) -> Result<NextSecret, String> {
    match kind.secret() {
        Secret::None => Ok(NextSecret::Store(None)),
        Secret::Signing => match before {
            Some((Kind::Standard, Some(secret))) if !moved => {
                Ok(NextSecret::Store(Some(secret.to_string())))
            }
            _ => Ok(NextSecret::Make),
        },
        Secret::Token { required } => {
            // A token given for another service — another kind, another
            // origin — is left behind with it.
            let left_behind = match before {
                Some((was, Some(_))) => {
                    matches!(was.secret(), Secret::Token { .. }) && (was != kind || moved)
                }
                _ => false,
            };
            let kept = match token.map(str::trim) {
                Some("") => None,
                Some(token) => Some(check_token(token)?),
                None => match before {
                    Some((was, secret)) if was == kind && !moved => secret.map(str::to_string),
                    _ => None,
                },
            };
            if required && kept.is_none() {
                return Err(if left_behind {
                    "the token kept was given for another server or another kind, and goes to \
                     no other: give the Gotify application's token again"
                        .to_string()
                } else {
                    "a Gotify destination needs the token its server gave the application"
                        .to_string()
                });
            }
            Ok(NextSecret::Store(kept))
        }
    }
}

/// A new signing secret: see [`kinds::new_secret`].
pub fn new_secret() -> anyhow::Result<String> {
    kinds::new_secret()
}

/// An address as the administration is shown it: its scheme, host and port,
/// and the rest masked — for most services the token is in the path.
pub fn masked(address: &str) -> String {
    let mask = crate::settings::registry::MASK;
    let Ok(parsed) = url::Url::parse(address) else {
        return mask.to_string();
    };
    let host = parsed.host_str().unwrap_or_default();
    let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    let hidden = !parsed.path().trim_matches('/').is_empty() || parsed.query().is_some();
    format!(
        "{}://{host}{port}{}",
        parsed.scheme(),
        if hidden {
            format!("/{mask}")
        } else {
            String::new()
        }
    )
}

/// What of a destination is kept out of the log wherever it turns up: its
/// address, the path and query of it — where a webhook's token is — and a
/// password in it, and its secret.
fn hide(destination: &Destination) {
    crate::telemetry::redact_also(&destination.url);
    if let Ok(url) = url::Url::parse(destination.url.trim()) {
        let mut rest = url.path().to_string();
        if let Some(query) = url.query() {
            rest.push('?');
            rest.push_str(query);
        }
        // `/` alone, or a short path, is no secret: the redaction leaves
        // values too short to be told from ordinary text alone.
        if rest.len() > 1 {
            crate::telemetry::redact_also(&rest);
        }
        if let Some(password) = url.password() {
            crate::telemetry::redact_also(password);
        }
    }
    if let Some(secret) = &destination.secret {
        crate::telemetry::redact_also(secret);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn happening(action: Action, target: Option<&str>, detail: Option<&str>) -> Happening {
        Happening {
            action,
            actor: Some("admin".into()),
            target: target.map(String::from),
            detail: detail.map(String::from),
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn answered(code: u16) -> Outcome {
        Outcome::Answered {
            status: reqwest::StatusCode::from_u16(code).unwrap(),
            took: Duration::from_millis(120),
            retry_after: None,
            said: None,
        }
    }

    fn at(text: &str) -> DateTime<Utc> {
        db::parse_rfc3339(text).expect("an instant")
    }

    /// A destination straight into the table, as the API would make it.
    async fn destination(state: &AppState, kind: Kind, events: &[&str]) -> Destination {
        repo::webhook::create(
            &state.db,
            &NewDestination {
                name: kind.as_str(),
                kind: kind.as_str(),
                url: "https://hooks.example.com/in",
                secret: (kind == Kind::Standard)
                    .then_some("whsec_C2FVsBQIhrscChlQIMV+b5sSYspob7oD"),
                events: &names(events),
                enabled: true,
                grouped: false,
            },
        )
        .await
        .expect("a destination")
    }

    #[tokio::test]
    async fn failed_refreshes_are_held_together_for_who_groups_them_and_owed_at_once_to_the_rest() {
        let state = crate::api::native::testing::server().await;
        let people = repo::webhook::create(
            &state.db,
            &NewDestination {
                name: "phone",
                kind: Kind::Ntfy.as_str(),
                url: "https://ntfy.example.com/ams",
                secret: None,
                events: &names(&["refresh.failed"]),
                enabled: true,
                grouped: true,
            },
        )
        .await
        .expect("a destination");
        let program = destination(&state, Kind::Json, &["refresh.failed"]).await;
        state.webhooks.reload(&state.db).await.expect("read");

        for (id, error) in [
            (
                "w1",
                "refreshed in part; kept what these gave before — tvdb: timed out",
            ),
            (
                "w2",
                "refreshed in part; kept what these gave before — tvdb: timed out",
            ),
            (
                "w3",
                "refreshed in part; kept what these gave before — tmdb: TMDB returned 404 Not Found for /3/tv/1: {}",
            ),
        ] {
            let named = format!("“Work {id}” (series {id})");
            let told = Told {
                event: Fact::RefreshFailed.as_str().to_string(),
                actor: Some(SYSTEM.to_string()),
                target: Some(id.to_string()),
                detail: Some(format!("{named}: {error}")),
                at: at_now(),
                count: None,
            };
            let failure = digest::Failure {
                cause: digest::cause(error),
                named,
            };
            owe_holding(&state, &told, Some(&failure)).await;
        }

        // A program wants an event per work, and is owed each at once.
        let (owed, total) = repo::webhook::log(&state.db, &program.id, 10, 0)
            .await
            .expect("log");
        assert_eq!(total, 3);
        let told: Told = serde_json::from_str(&owed[0].payload).expect("what was told");
        assert_eq!(told.count, None);
        // The phone waits for the window.
        let (_, total) = repo::webhook::log(&state.db, &people.id, 10, 0)
            .await
            .expect("log");
        assert_eq!(total, 0);

        // Its window past, one message, counting the three by cause.
        owe_grouped(
            &state,
            state
                .webhooks
                .digest
                .due_at(Instant::now() + digest::WINDOW),
        )
        .await;
        let (owed, total) = repo::webhook::log(&state.db, &people.id, 10, 0)
            .await
            .expect("log");
        assert_eq!(total, 1);
        let told: Told = serde_json::from_str(&owed[0].payload).expect("what was told");
        assert_eq!(told.event, "refresh.failed");
        assert_eq!(told.count, Some(3));
        assert_eq!(told.target, None);
        let detail = told.detail.unwrap_or_default();
        assert!(detail.starts_with("3 refreshes failed since "), "{detail}");
        assert!(
            detail.contains("\nTheTVDB — timed out: 2\nTMDB — 404 Not Found: 1\n"),
            "{detail}"
        );
        assert!(
            detail.ends_with("“Work w1” (series w1), “Work w2” (series w2), “Work w3” (series w3)"),
            "{detail}"
        );
    }

    #[test]
    fn only_the_events_asked_for_are_told() {
        let defaults = names_in(DEFAULT_EVENTS);
        assert!(wanted(&names(&["*"]), Action::SignedIn.as_str()));
        assert!(wanted(&defaults, Action::ItemImported.as_str()));
        assert!(!wanted(&defaults, Action::SignedIn.as_str()));
        assert!(!wanted(&[], Action::ItemCreated.as_str()));
        // The facts are asked for by name, as an action is, and not by default.
        assert!(wanted(
            &names(&["item.created", "refresh.failed"]),
            "refresh.failed"
        ));
        assert!(wanted(&names(&["*"]), Fact::UpdateAvailable.as_str()));
        for fact in Fact::ALL {
            assert!(!wanted(&defaults, fact.as_str()), "{}", fact.as_str());
        }
        assert_eq!(
            names_in(" item.deleted , list.created ,,"),
            ["item.deleted", "list.created"]
        );
    }

    #[test]
    fn a_fact_never_shares_a_name_with_an_action_or_the_test() {
        let mut names: Vec<&str> = Action::ALL
            .iter()
            .map(|a| a.as_str())
            .chain(Fact::ALL.iter().map(|f| f.as_str()))
            .chain([TEST_EVENT])
            .collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "two events share a name");
        // Every one is a name a destination can be told of.
        for name in names {
            assert!(check_events(&[name.to_string()]).is_ok(), "{name}");
        }
    }

    #[test]
    fn the_name_typed_into_a_failed_sign_in_is_not_relayed() {
        let told = discreet(happening(
            Action::SignInFailed,
            Some("'; DROP TABLE users; -- @everyone"),
            Some("wrong password"),
        ));
        assert_eq!(told.target, None);
        assert_eq!(told.detail, None);
        assert_eq!(told.actor.as_deref(), Some("admin"));

        // A sign-in that succeeded names an account that exists.
        let told = discreet(happening(Action::SignedIn, Some("admin"), None));
        assert_eq!(told.target.as_deref(), Some("admin"));
    }

    #[test]
    fn a_try_that_is_not_taken_is_tried_again_on_the_schedule_then_given_up_on() {
        let seconds: Vec<Option<u64>> = (1..=MOST_ATTEMPTS + 1)
            .map(|n| retry_in(n).map(|pause| pause.as_secs()))
            .collect();
        assert_eq!(
            seconds,
            [
                Some(5),
                Some(5 * 60),
                Some(30 * 60),
                Some(2 * 3600),
                Some(5 * 3600),
                Some(10 * 3600),
                Some(10 * 3600),
                None,
                None,
            ]
        );
        assert_eq!(MOST_ATTEMPTS, 8);

        let now = at("2026-10-11T08:00:00.000Z");
        // The first try failed: again in five seconds, with what it said.
        let first = verdict(1, &answered(503), now);
        assert!(!first.gone);
        assert_eq!(first.settled.status, Status::Pending);
        assert_eq!(
            first.settled.next_attempt_at.as_deref(),
            Some("2026-10-11T08:00:05.000Z")
        );
        assert_eq!(first.settled.last_status, Some(503));
        assert_eq!(first.settled.duration_ms, Some(120));
        // The fourth: two hours.
        assert_eq!(
            verdict(4, &answered(500), now)
                .settled
                .next_attempt_at
                .as_deref(),
            Some("2026-10-11T10:00:00.000Z")
        );
        // Not reached at all is a failure like any other.
        let unreached = verdict(
            2,
            &Outcome::Unreached {
                why: "could not connect".into(),
                took: Duration::from_secs(10),
            },
            now,
        );
        assert_eq!(unreached.settled.status, Status::Pending);
        assert_eq!(unreached.settled.last_status, None);
        assert_eq!(
            unreached.settled.last_error.as_deref(),
            Some("could not connect")
        );
        assert_eq!(
            unreached.settled.next_attempt_at.as_deref(),
            Some("2026-10-11T08:05:00.000Z")
        );
        // The eighth and last: given up on.
        let last = verdict(MOST_ATTEMPTS, &answered(500), now);
        assert_eq!(last.settled.status, Status::Failed);
        assert_eq!(last.settled.next_attempt_at, None);

        // Taken: delivered, whatever the try.
        let taken = verdict(3, &answered(204), now);
        assert_eq!(taken.settled.status, Status::Delivered);
        assert_eq!(taken.settled.last_status, Some(204));
        assert_eq!(taken.settled.last_error, None);
    }

    #[test]
    fn an_address_that_asks_to_be_left_alone_longer_is() {
        let now = at("2026-10-11T08:00:00.000Z");
        let throttled = |seconds: u64| Outcome::Answered {
            status: reqwest::StatusCode::TOO_MANY_REQUESTS,
            took: Duration::from_millis(5),
            retry_after: Some(Duration::from_secs(seconds)),
            said: Some("slow down".into()),
        };
        // Longer than the schedule's pause: what it asked.
        assert_eq!(
            verdict(1, &throttled(60), now)
                .settled
                .next_attempt_at
                .as_deref(),
            Some("2026-10-11T08:01:00.000Z")
        );
        // Shorter: the schedule's.
        assert_eq!(
            verdict(2, &throttled(60), now)
                .settled
                .next_attempt_at
                .as_deref(),
            Some("2026-10-11T08:05:00.000Z")
        );
        // Never longer than the longest pause.
        assert_eq!(
            verdict(1, &throttled(7 * 24 * 3600), now)
                .settled
                .next_attempt_at
                .as_deref(),
            Some("2026-10-11T18:00:00.000Z")
        );
        assert_eq!(
            verdict(1, &throttled(60), now)
                .settled
                .last_error
                .as_deref(),
            Some("slow down")
        );

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "120".parse().unwrap());
        assert_eq!(pause_asked(&headers), Some(Duration::from_secs(120)));
        headers.insert(reqwest::header::RETRY_AFTER, "soon".parse().unwrap());
        assert_eq!(pause_asked(&headers), None);
    }

    #[test]
    fn a_redirect_is_a_failure_that_says_the_address_moved() {
        let moved = verdict(1, &answered(301), at("2026-10-11T08:00:00.000Z"));
        assert_eq!(moved.settled.status, Status::Pending);
        assert_eq!(moved.settled.last_status, Some(301));
        assert!(moved.settled.last_error.unwrap().contains("moved"));
    }

    #[test]
    fn an_address_gone_switches_its_destination_off() {
        let gone = verdict(1, &answered(410), at("2026-10-11T08:00:00.000Z"));
        assert!(gone.gone);
        assert_eq!(gone.settled.status, Status::Failed);
        assert_eq!(gone.settled.last_status, Some(410));
        assert_eq!(gone.settled.last_error.as_deref(), Some(GONE));
        // Any other refusal is tried again.
        assert!(!verdict(1, &answered(404), at("2026-10-11T08:00:00.000Z")).gone);
    }

    #[tokio::test]
    async fn a_410_switches_the_destination_off_and_fails_what_waited_for_it() {
        let state = crate::api::native::testing::server().await;
        let hook = destination(&state, Kind::Standard, &["*"]).await;
        state.webhooks.reload(&state.db).await.expect("read");
        notify(&state, happening(Action::ItemCreated, Some("a"), None)).await;
        notify(&state, happening(Action::ItemDeleted, Some("b"), None)).await;

        let cutoff = claim_cutoff();
        let due = repo::webhook::due(&state.db, MOST_ATTEMPTS, &cutoff, 10)
            .await
            .expect("due");
        assert_eq!(due.len(), 2);
        let first = &due[0];
        assert!(
            repo::webhook::claim(
                &state.db,
                &first.id,
                "me",
                MOST_ATTEMPTS,
                &cutoff,
                &db::now()
            )
            .await
            .expect("claim")
        );
        let verdict = verdict(1, &answered(410), Utc::now());
        assert!(
            repo::webhook::settle(&state.db, &first.id, "me", &verdict.settled)
                .await
                .expect("settled")
        );
        repo::webhook::switch_off(&state.db, &hook.id, GONE)
            .await
            .expect("off");

        let off = repo::webhook::find(&state.db, &hook.id)
            .await
            .unwrap()
            .expect("kept");
        assert!(!off.enabled);
        assert_eq!(off.disabled_reason.as_deref(), Some(GONE));
        assert!(off.disabled_at.is_some());

        let (log, total) = repo::webhook::log(&state.db, &hook.id, 10, 0)
            .await
            .expect("log");
        assert_eq!(total, 2);
        let gone = log.iter().find(|d| d.id == first.id).expect("the first");
        assert_eq!(gone.status, Status::Failed);
        assert_eq!(gone.last_status, Some(410));
        let other = log.iter().find(|d| d.id != first.id).expect("the other");
        assert_eq!(other.status, Status::Failed);
        assert_eq!(
            other.last_error.as_deref(),
            Some(repo::webhook::SWITCHED_OFF)
        );

        // Switched off, it is owed nothing more.
        state.webhooks.reload(&state.db).await.expect("read");
        notify(&state, happening(Action::ItemCreated, Some("c"), None)).await;
        let (_, total) = repo::webhook::log(&state.db, &hook.id, 10, 0)
            .await
            .expect("log");
        assert_eq!(total, 2);
    }

    #[tokio::test]
    async fn a_try_under_way_when_its_destination_is_switched_off_is_failed_on_its_return() {
        let state = crate::api::native::testing::server().await;
        let hook = destination(&state, Kind::Json, &["*"]).await;
        state.webhooks.reload(&state.db).await.expect("read");
        notify(&state, happening(Action::ItemCreated, Some("a"), None)).await;

        let cutoff = claim_cutoff();
        let due = repo::webhook::due(&state.db, MOST_ATTEMPTS, &cutoff, 10)
            .await
            .expect("due");
        assert!(
            repo::webhook::claim(
                &state.db,
                &due[0].id,
                "me",
                MOST_ATTEMPTS,
                &cutoff,
                &db::now()
            )
            .await
            .expect("claim")
        );
        // Switched off while the try is under way: the delivery is held, and
        // left to its worker.
        repo::webhook::save(
            &state.db,
            &Destination {
                enabled: false,
                ..hook.clone()
            },
        )
        .await
        .expect("off");
        let failed = verdict(1, &answered(503), Utc::now());
        assert_eq!(failed.settled.status, Status::Pending);
        assert!(
            repo::webhook::settle(&state.db, &due[0].id, "me", &failed.settled)
                .await
                .expect("settled")
        );

        let (log, _) = repo::webhook::log(&state.db, &hook.id, 10, 0)
            .await
            .expect("log");
        assert_eq!(log[0].status, Status::Failed);
        assert_eq!(
            log[0].last_error.as_deref(),
            Some(repo::webhook::SWITCHED_OFF)
        );
        assert_eq!(log[0].next_attempt_at, None);
    }

    #[tokio::test]
    async fn an_event_is_owed_to_each_destination_that_wants_it_and_no_other() {
        let state = crate::api::native::testing::server().await;
        let every = destination(&state, Kind::Discord, &["*"]).await;
        let facts = destination(&state, Kind::Ntfy, &["refresh.failed"]).await;
        let off = destination(&state, Kind::Slack, &["*"]).await;
        repo::webhook::save(
            &state.db,
            &Destination {
                enabled: false,
                ..off.clone()
            },
        )
        .await
        .expect("off");
        state.webhooks.reload(&state.db).await.expect("read");

        notify(
            &state,
            happening(
                Action::SignInFailed,
                Some("@everyone"),
                Some("wrong password"),
            ),
        )
        .await;

        let (owed, _) = repo::webhook::log(&state.db, &every.id, 10, 0)
            .await
            .unwrap();
        assert_eq!(owed.len(), 1);
        let told: Told = serde_json::from_str(&owed[0].payload).expect("what was told");
        assert_eq!(told.event, "auth.sign_in_failed");
        // What a failed sign-in typed is not passed on.
        assert_eq!(told.target, None);
        assert_eq!(told.detail, None);
        assert!(owed[0].message_id.starts_with("msg_"));
        assert_eq!(owed[0].status, Status::Pending);

        assert_eq!(
            repo::webhook::log(&state.db, &facts.id, 10, 0)
                .await
                .unwrap()
                .1,
            0
        );
        assert_eq!(
            repo::webhook::log(&state.db, &off.id, 10, 0)
                .await
                .unwrap()
                .1,
            0
        );

        // A fact is asked about before anything is looked up to tell it.
        assert!(wants(&state, Fact::RefreshFailed));
        assert!(wants(&state, Fact::UpdateAvailable));
    }

    #[tokio::test]
    async fn a_destination_behind_on_its_deliveries_is_owed_no_more() {
        let state = crate::api::native::testing::server().await;
        let hook = destination(&state, Kind::Json, &["*"]).await;
        for n in 0..3 {
            assert!(
                repo::webhook::enqueue(&state.db, &hook.id, &format!("msg_{n}"), "x.y", "{}", 3)
                    .await
                    .unwrap()
            );
        }
        assert!(
            !repo::webhook::enqueue(&state.db, &hook.id, "msg_4", "x.y", "{}", 3)
                .await
                .unwrap()
        );
    }

    /// Posts nothing: each address answers as `answer` says of it, and what
    /// was asked of each is kept.
    struct Canned {
        answer: fn(&str) -> Outcome,
        asked: parking_lot::Mutex<Vec<String>>,
    }

    impl Canned {
        fn new(answer: fn(&str) -> Outcome) -> Self {
            Self {
                answer,
                asked: parking_lot::Mutex::new(Vec::new()),
            }
        }

        /// How many posts went to the addresses that name this host.
        fn asked_of(&self, host: &str) -> usize {
            self.asked
                .lock()
                .iter()
                .filter(|url| url.contains(host))
                .count()
        }
    }

    impl Poster for Canned {
        async fn post(&self, post: &kinds::Post) -> Outcome {
            self.asked.lock().push(post.url.clone());
            (self.answer)(&post.url)
        }
    }

    /// An address that is down: nothing answers, and the try runs out of time.
    fn down_or_taken(url: &str) -> Outcome {
        if url.contains("down.example.com") {
            Outcome::Unreached {
                why: "timed out".into(),
                took: TIMEOUT,
            }
        } else {
            answered(204)
        }
    }

    /// A destination at an address of its own, told of everything.
    async fn reached_at(state: &AppState, name: &str, url: &str) -> Destination {
        repo::webhook::create(
            &state.db,
            &NewDestination {
                name,
                kind: "json",
                url,
                secret: None,
                events: &names(&["*"]),
                enabled: true,
                grouped: false,
            },
        )
        .await
        .expect("a destination")
    }

    /// `n` things that happen, each owed to every destination there is.
    async fn happen(state: &AppState, n: usize) {
        state.webhooks.reload(&state.db).await.expect("read");
        for event in 0..n {
            let target = format!("work-{event}");
            notify(state, happening(Action::ItemCreated, Some(&target), None)).await;
        }
    }

    #[tokio::test]
    async fn an_address_that_does_not_answer_is_asked_once_and_the_rest_owed_to_it_is_put_off() {
        let state = crate::api::native::testing::server().await;
        let down = reached_at(&state, "down", "https://down.example.com/in").await;
        let up = reached_at(&state, "up", "https://up.example.com/in").await;
        happen(&state, 5).await;

        let poster = Canned::new(down_or_taken);
        let taken = batch_with(&state, &poster).await.expect("a batch");

        // The address that does not answer was posted to once, not for each
        // of the five; the other, for each of its.
        assert_eq!(poster.asked_of("down.example.com"), 1);
        assert_eq!(poster.asked_of("up.example.com"), 5);
        assert_eq!(taken, 10);

        // All five of the first wait for their next try, each with its try
        // counted: one with what went wrong, the others with why they were
        // not sent.
        let (owed, _) = repo::webhook::log(&state.db, &down.id, 10, 0)
            .await
            .unwrap();
        assert_eq!(owed.len(), 5);
        for delivery in &owed {
            assert_eq!(delivery.status, Status::Pending, "{delivery:?}");
            assert_eq!(delivery.attempts, 1, "{delivery:?}");
            assert!(
                delivery
                    .next_attempt_at
                    .as_deref()
                    .is_some_and(|at| at > db::now().as_str()),
                "{delivery:?}"
            );
        }
        let said: Vec<&str> = owed
            .iter()
            .filter_map(|d| d.last_error.as_deref())
            .collect();
        assert_eq!(said.iter().filter(|s| **s == "timed out").count(), 1);
        assert_eq!(said.iter().filter(|s| **s == SET_ASIDE).count(), 4);

        // The other was told everything.
        let (told, _) = repo::webhook::log(&state.db, &up.id, 10, 0).await.unwrap();
        assert_eq!(told.len(), 5);
        assert!(told.iter().all(|d| d.status == Status::Delivered));
    }

    #[tokio::test]
    async fn all_an_address_that_does_not_answer_is_owed_is_put_off_at_once_and_the_others_come_next()
     {
        let state = crate::api::native::testing::server().await;
        let down = reached_at(&state, "down", "https://down.example.com/in").await;
        // More than a batch is owed to it, before the other destination is
        // there at all, and then one thing is owed to both.
        happen(&state, BATCH as usize + 4).await;
        let up = reached_at(&state, "up", "https://up.example.com/in").await;
        happen(&state, 1).await;
        // What it is owed has waited longest, whatever the clock's grain.
        sqlx::query(state.db.sql(
            "UPDATE webhook_delivery SET next_attempt_at = '2000-01-01T00:00:00.000Z'
             WHERE destination_id = ?",
        ))
        .bind(&down.id)
        .execute(state.db.pool())
        .await
        .unwrap();

        let poster = Canned::new(down_or_taken);
        // A batch is the oldest few that are due, which are all the first's:
        // it is asked once, and everything it is owed — what the batch did
        // not take too — is put off.
        let taken = batch_with(&state, &poster).await.expect("a batch");
        assert_eq!(taken, BATCH as usize + 5);
        assert_eq!(poster.asked_of("down.example.com"), 1);
        assert_eq!(poster.asked_of("up.example.com"), 0);
        let (owed, total) = repo::webhook::log(&state.db, &down.id, 200, 0)
            .await
            .unwrap();
        assert_eq!(total, BATCH + 5);
        assert!(
            owed.iter()
                .all(|d| d.status == Status::Pending && d.attempts == 1),
            "{owed:?}"
        );

        // So the next batch is the other's, and the first is not asked again.
        let taken = batch_with(&state, &poster).await.expect("a batch");
        assert_eq!(taken, 1);
        assert_eq!(poster.asked_of("down.example.com"), 1);
        assert_eq!(poster.asked_of("up.example.com"), 1);
        let (told, _) = repo::webhook::log(&state.db, &up.id, 10, 0).await.unwrap();
        assert_eq!(told[0].status, Status::Delivered);
    }

    #[tokio::test]
    async fn an_address_that_answers_no_is_asked_for_each_of_its_deliveries() {
        let state = crate::api::native::testing::server().await;
        let refusing = reached_at(&state, "refusing", "https://refusing.example.com/in").await;
        happen(&state, 3).await;

        // Answered, and refused, in no time: not an address to leave alone.
        // Each delivery is tried on its own account and waits its own pause.
        let poster = Canned::new(|_| answered(503));
        let taken = batch_with(&state, &poster).await.expect("a batch");
        assert_eq!(taken, 3);
        assert_eq!(poster.asked_of("refusing.example.com"), 3);
        let (owed, _) = repo::webhook::log(&state.db, &refusing.id, 10, 0)
            .await
            .unwrap();
        assert!(
            owed.iter()
                .all(|d| d.status == Status::Pending && d.last_status == Some(503)),
            "{owed:?}"
        );
    }

    #[tokio::test]
    async fn a_replay_is_the_same_event_owed_again() {
        let state = crate::api::native::testing::server().await;
        let hook = destination(&state, Kind::Standard, &["*"]).await;
        state.webhooks.reload(&state.db).await.expect("read");
        notify(&state, happening(Action::ListCreated, Some("Noir"), None)).await;
        let (log, _) = repo::webhook::log(&state.db, &hook.id, 10, 0)
            .await
            .unwrap();
        let original = &log[0];

        let again = repo::webhook::replay(&state.db, original)
            .await
            .expect("replayed");
        assert_eq!(again.message_id, original.message_id);
        assert_eq!(again.payload, original.payload);
        assert_eq!(again.replay_of.as_deref(), Some(original.id.as_str()));
        assert_eq!(again.status, Status::Pending);
        assert_eq!(
            repo::webhook::log(&state.db, &hook.id, 10, 0)
                .await
                .unwrap()
                .1,
            2
        );
    }

    /// What waits in the table is still owed after a restart: a delivery due,
    /// and one a worker took and never came back from, once its claim is
    /// stale — and one whose every try was taken that way is given up on.
    #[tokio::test]
    async fn what_is_owed_outlives_a_restart() {
        let path = crate::testing::scratch_path("webhooks").with_extension("db");
        let config = crate::config::Database {
            url: format!("sqlite://{}?mode=rwc", path.display()),
            max_connections: 1,
            acquire_timeout: Duration::from_secs(5),
        };

        let (hook, waiting, taken) = {
            let db = Db::connect(&config).await.expect("a database");
            db.migrate().await.expect("migrated");
            let hook = repo::webhook::create(
                &db,
                &NewDestination {
                    name: "n",
                    kind: "json",
                    url: "https://hooks.example.com/in",
                    secret: None,
                    events: &names(&["*"]),
                    enabled: true,
                    grouped: false,
                },
            )
            .await
            .unwrap();
            for n in 0..3 {
                repo::webhook::enqueue(&db, &hook.id, &format!("msg_{n}"), "x.y", "{}", 10)
                    .await
                    .unwrap();
            }
            let due = repo::webhook::due(&db, MOST_ATTEMPTS, &claim_cutoff(), 10)
                .await
                .unwrap();
            // One taken and never settled: the server stops mid-send.
            let taken = due[1].id.clone();
            assert!(
                repo::webhook::claim(
                    &db,
                    &taken,
                    "gone",
                    MOST_ATTEMPTS,
                    &claim_cutoff(),
                    &db::now()
                )
                .await
                .unwrap()
            );
            // One with every try spent the same way.
            sqlx::query(db.sql("UPDATE webhook_delivery SET attempts = ?, claimed_by = 'gone', claimed_at = ? WHERE id = ?"))
                .bind(MOST_ATTEMPTS)
                .bind(db::now())
                .bind(&due[2].id)
                .execute(db.pool())
                .await
                .unwrap();
            db.close().await;
            (hook, due[0].id.clone(), taken)
        };

        let db = Db::connect(&config).await.expect("the database again");
        db.migrate().await.expect("migrated");
        let due = repo::webhook::due(&db, MOST_ATTEMPTS, &claim_cutoff(), 10)
            .await
            .unwrap();
        assert_eq!(
            due.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            [waiting.as_str()]
        );

        // Once the claims are stale, the one cut short is anybody's again,
        // its try counted; the one with none left is given up on.
        let later = db::to_rfc3339(Utc::now() + chrono::Duration::minutes(5));
        assert_eq!(
            repo::webhook::give_up_spent(&db, MOST_ATTEMPTS, &later)
                .await
                .unwrap(),
            1
        );
        let due = repo::webhook::due(&db, MOST_ATTEMPTS, &later, 10)
            .await
            .unwrap();
        let again = due
            .iter()
            .find(|d| d.id == taken)
            .expect("cut short, owed again");
        assert_eq!(again.attempts, 1);
        let (log, _) = repo::webhook::log(&db, &hook.id, 10, 0).await.unwrap();
        let spent = log
            .iter()
            .find(|d| d.status == Status::Failed)
            .expect("given up on");
        assert_eq!(spent.last_error.as_deref(), Some(repo::webhook::CUT_SHORT));

        db.close().await;
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn the_single_address_becomes_a_destination_once() {
        let state = crate::api::native::testing::server().await;
        // As a server of an earlier version left them.
        for (key, value) in [
            (LEGACY_URL, "https://discord.com/api/webhooks/1/secret"),
            (LEGACY_EVENTS, "item.imported,refresh.failed,future.event"),
        ] {
            sqlx::query(state.db.sql(
                "INSERT INTO setting (scope, scope_id, key, value, updated_at) VALUES ('server', '', ?, ?, ?)",
            ))
            .bind(key)
            .bind(value)
            .bind(db::now())
            .execute(state.db.pool())
            .await
            .expect("an old setting");
        }
        state.settings.reload().await.expect("read");

        start(&state).await.expect("started");
        let made = repo::webhook::all(&state.db).await.unwrap();
        assert_eq!(made.len(), 1);
        let hook = &made[0];
        assert_eq!(hook.name, ADOPTED_NAME);
        assert_eq!(hook.kind, "json");
        assert_eq!(hook.url, "https://discord.com/api/webhooks/1/secret");
        assert_eq!(
            hook.events,
            ["item.imported", "refresh.failed", "future.event"]
        );
        assert!(hook.enabled);
        // The settings are gone: read nowhere, and not read again.
        assert_eq!(state.settings.at(Scope::Server, "", LEGACY_URL), None);
        assert_eq!(state.settings.at(Scope::Server, "", LEGACY_EVENTS), None);
        assert!(wants(&state, Fact::RefreshFailed));

        start(&state).await.expect("started again");
        assert_eq!(repo::webhook::count(&state.db).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn an_address_without_its_events_is_told_the_defaults_and_none_is_left_alone() {
        let state = crate::api::native::testing::server().await;
        sqlx::query(state.db.sql(
            "INSERT INTO setting (scope, scope_id, key, value, updated_at) VALUES ('server', '', ?, ?, ?)",
        ))
        .bind(LEGACY_URL)
        .bind("https://hooks.example.com/x")
        .bind(db::now())
        .execute(state.db.pool())
        .await
        .unwrap();
        state.settings.reload().await.unwrap();
        start(&state).await.expect("started");
        assert_eq!(
            repo::webhook::all(&state.db).await.unwrap()[0].events,
            names_in(DEFAULT_EVENTS)
        );

        // Destinations already there: the address is not made one more.
        let state = crate::api::native::testing::server().await;
        destination(&state, Kind::Ntfy, &["*"]).await;
        sqlx::query(state.db.sql(
            "INSERT INTO setting (scope, scope_id, key, value, updated_at) VALUES ('server', '', ?, ?, ?)",
        ))
        .bind(LEGACY_URL)
        .bind("https://hooks.example.com/x")
        .bind(db::now())
        .execute(state.db.pool())
        .await
        .unwrap();
        state.settings.reload().await.unwrap();
        start(&state).await.expect("started");
        assert_eq!(repo::webhook::count(&state.db).await.unwrap(), 1);
        assert_eq!(state.settings.at(Scope::Server, "", LEGACY_URL), None);

        // Nothing set: nothing made.
        let state = crate::api::native::testing::server().await;
        start(&state).await.expect("started");
        assert_eq!(repo::webhook::count(&state.db).await.unwrap(), 0);
    }

    #[test]
    fn only_an_ordinary_web_address_is_posted_to() {
        assert_eq!(
            acceptable("https://discord.com/api/webhooks/1/abc").as_deref(),
            Some("discord.com")
        );
        assert!(acceptable("ftp://example.com/x").is_none());
        assert!(acceptable("not a url").is_none());
        assert!(acceptable("http://127.0.0.1:8080/hook").is_none());
        assert!(acceptable("http://169.254.169.254/latest").is_none());
        assert!(acceptable("http://localhost:8479/hook").is_none());
        // A home network's own: an ntfy beside the server.
        assert!(acceptable("http://192.168.1.20/alerts").is_some());

        assert!(check_address(Kind::Json, "http://127.0.0.1:9/hook").is_err());
        assert!(check_address(Kind::Json, "http://[::1]/hook").is_err());
        assert!(check_address(Kind::Json, "ftp://example.com/x").is_err());
        assert!(check_address(Kind::Json, "   ").is_err());
        assert_eq!(
            check_address(Kind::Gotify, " http://192.168.1.20:8080 ").as_deref(),
            Ok("http://192.168.1.20:8080")
        );
        // ntfy needs its topic.
        assert!(check_address(Kind::Ntfy, "https://ntfy.sh/").is_err());
        assert!(check_address(Kind::Ntfy, "https://ntfy.sh/ams-alerts").is_ok());

        // A token crosses no network in clear but the home one.
        assert!(check_address(Kind::Gotify, "http://gotify.example.com").is_err());
        assert!(check_address(Kind::Discord, "http://discord.com/api/webhooks/1/abc").is_err());
        assert!(check_address(Kind::Gotify, "https://gotify.example.com").is_ok());
        assert!(check_address(Kind::Gotify, "http://gotify.lan").is_ok());
        assert!(check_address(Kind::Json, "http://example.com/hook").is_ok());
    }

    #[test]
    fn plain_http_is_told_from_the_home_network() {
        for beyond in [
            "http://8.8.8.8/hook",
            "http://203.0.113.5/topic",
            "http://ntfy.example.com/topic",
            "http://[2001:db8::1]/hook",
        ] {
            assert!(cleartext_beyond_home(beyond), "{beyond}");
        }
        for home in [
            "https://ntfy.example.com/topic",
            "http://10.0.0.5/hook",
            "http://172.16.4.2/hook",
            "http://192.168.1.20:8080",
            "http://100.100.1.1/hook",
            "http://[fd00::1]/hook",
            "http://nas:8080/hook",
            "http://gotify.lan",
            "http://gotify.home.arpa",
            "http://printer.local/x",
        ] {
            assert!(!cleartext_beyond_home(home), "{home}");
        }
    }

    #[tokio::test]
    async fn a_token_is_not_sent_in_clear_beyond_the_home_network() {
        let told = Told {
            event: "webhook.test".to_string(),
            actor: None,
            target: None,
            detail: None,
            at: at_now(),
            count: None,
        };
        let post = kinds::post(
            Kind::Ntfy,
            "http://203.0.113.5/ams-alerts",
            Some("tk_secret"),
            &kinds::message_id(),
            &told,
            0,
        )
        .expect("a post");
        assert!(matches!(refusal(&post).await, Some(Refusal::Cleartext)));
    }

    #[tokio::test]
    async fn a_test_to_an_address_not_posted_to_says_why_and_sends_nothing() {
        let state = crate::api::native::testing::server().await;
        for refused in ["ftp://example.com/x", "http://127.0.0.1:8080/hook"] {
            let destination = Destination {
                url: refused.to_string(),
                ..destination(&state, Kind::Json, &["*"]).await
            };
            assert!(
                matches!(
                    test(&state, &destination, "admin").await,
                    Outcome::Refused(Refusal::NotWeb)
                ),
                "{refused}"
            );
        }
        // A standard destination without its secret is not sent unsigned.
        let unsigned = Destination {
            secret: None,
            ..destination(&state, Kind::Standard, &["*"]).await
        };
        assert!(matches!(
            test(&state, &unsigned, "admin").await,
            Outcome::Unsent { .. }
        ));
    }

    #[test]
    fn an_address_is_shown_without_what_follows_its_host() {
        assert_eq!(
            masked("https://discord.com/api/webhooks/1/secret"),
            "https://discord.com/••••••••"
        );
        assert_eq!(
            masked("https://user:pass@ntfy.example.org:8443/topic?auth=x"),
            "https://ntfy.example.org:8443/••••••••"
        );
        assert_eq!(
            masked("http://192.168.1.20:8080/"),
            "http://192.168.1.20:8080"
        );
        assert_eq!(masked("https://push.example.de"), "https://push.example.de");
        assert_eq!(masked("not an address"), "••••••••");
    }

    #[test]
    fn the_events_named_are_kept_once_and_every_one_alone() {
        assert_eq!(
            check_events(&names(&[
                " item.created ",
                "item.created",
                "",
                "refresh.failed"
            ])),
            Ok(names(&["item.created", "refresh.failed"]))
        );
        assert_eq!(
            check_events(&names(&["item.created", "*"])),
            Ok(names(&["*"]))
        );
        assert!(check_events(&names(&["Item Created"])).is_err());
        assert!(check_events(&names(&[&"x".repeat(65)])).is_err());
        let many: Vec<String> = (0..=MOST_EVENTS).map(|n| format!("e.{n}")).collect();
        assert!(check_events(&many).is_err());
    }

    #[test]
    fn a_destination_keeps_the_secret_its_kind_takes() {
        // A standard one is given a signing secret, once.
        assert_eq!(
            next_secret(None, Kind::Standard, false, None),
            Ok(NextSecret::Make)
        );
        assert_eq!(
            next_secret(
                Some((Kind::Standard, Some("whsec_x"))),
                Kind::Standard,
                false,
                Some("ignored")
            ),
            Ok(NextSecret::Store(Some("whsec_x".into())))
        );
        // Made standard: a secret of its own, not the token it had.
        assert_eq!(
            next_secret(
                Some((Kind::Ntfy, Some("tk_1"))),
                Kind::Standard,
                false,
                None
            ),
            Ok(NextSecret::Make)
        );
        // ntfy: a token if given, kept if left out, removed if emptied.
        assert_eq!(
            next_secret(None, Kind::Ntfy, false, Some(" tk_1 ")),
            Ok(NextSecret::Store(Some("tk_1".into())))
        );
        assert_eq!(
            next_secret(Some((Kind::Ntfy, Some("tk_1"))), Kind::Ntfy, false, None),
            Ok(NextSecret::Store(Some("tk_1".into())))
        );
        assert_eq!(
            next_secret(
                Some((Kind::Ntfy, Some("tk_1"))),
                Kind::Ntfy,
                false,
                Some("")
            ),
            Ok(NextSecret::Store(None))
        );
        // A signing secret is no token.
        assert_eq!(
            next_secret(
                Some((Kind::Standard, Some("whsec_x"))),
                Kind::Ntfy,
                false,
                None
            ),
            Ok(NextSecret::Store(None))
        );
        // Gotify needs one.
        assert!(next_secret(None, Kind::Gotify, false, None).is_err());
        assert!(
            next_secret(
                Some((Kind::Gotify, Some("A1"))),
                Kind::Gotify,
                false,
                Some("")
            )
            .is_err()
        );
        assert!(next_secret(None, Kind::Gotify, false, Some("two words")).is_err());
        // Discord keeps nothing: its address is its key.
        assert_eq!(
            next_secret(
                Some((Kind::Standard, Some("whsec_x"))),
                Kind::Discord,
                false,
                Some("t")
            ),
            Ok(NextSecret::Store(None))
        );
    }

    #[test]
    fn a_secret_kept_goes_to_no_other_service() {
        // An ntfy topic's token is not a Gotify server's: made Gotify, the
        // destination is given its own, and refused without it, saying why.
        let refused = next_secret(Some((Kind::Ntfy, Some("tk_1"))), Kind::Gotify, false, None);
        assert!(
            refused
                .as_ref()
                .is_err_and(|why| why.contains("another server or another kind")),
            "kept, or refused for another reason"
        );
        assert_eq!(
            next_secret(
                Some((Kind::Ntfy, Some("tk_1"))),
                Kind::Gotify,
                false,
                Some("A2")
            ),
            Ok(NextSecret::Store(Some("A2".into())))
        );
        // Moved to another origin, a token kept stays behind — unless given
        // again with the address.
        assert_eq!(
            next_secret(Some((Kind::Ntfy, Some("tk_1"))), Kind::Ntfy, true, None),
            Ok(NextSecret::Store(None))
        );
        assert!(next_secret(Some((Kind::Gotify, Some("A1"))), Kind::Gotify, true, None).is_err());
        assert_eq!(
            next_secret(
                Some((Kind::Gotify, Some("A1"))),
                Kind::Gotify,
                true,
                Some("A1")
            ),
            Ok(NextSecret::Store(Some("A1".into())))
        );
        // A signing secret is made anew for the receiver at the new origin.
        assert_eq!(
            next_secret(
                Some((Kind::Standard, Some("whsec_x"))),
                Kind::Standard,
                true,
                None
            ),
            Ok(NextSecret::Make)
        );
    }

    #[test]
    fn a_failing_work_is_told_of_once_an_hour_at_most() {
        let told = parking_lot::Mutex::new(HashMap::new());
        let now = Instant::now();
        assert!(first_telling(&told, "a", now));
        // The same failure, seen by the sweep after the fetch.
        assert!(!first_telling(&told, "a", now + Duration::from_secs(1)));
        assert!(first_telling(&told, "b", now));
        // An hour on, a work failing again is news again.
        assert!(first_telling(&told, "a", now + TOLD_FOR));
    }

    #[test]
    fn a_medium_is_named_without_what_its_query_carries() {
        assert_eq!(
            without_query("https://img.example/p/a.jpg?token=secret#x"),
            "https://img.example/p/a.jpg"
        );
        assert_eq!(
            without_query("https://img.example/p/a.jpg"),
            "https://img.example/p/a.jpg"
        );
    }
}
