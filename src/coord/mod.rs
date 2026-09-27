//! Several instances of this server, as one.
//!
//! With `AMS_MODE=multi` a request may land on any instance, so what one
//! instance keeps to itself has to be shared or told: which of them runs the
//! schedules — one, the *leader*, chosen by a lease on the cache server and
//! replaced within seconds when it goes — which holds which job, so no two
//! run the same at once, and what each must tell the others: the settings
//! moved, a medium was kept, a run is to stop. All of it goes through the
//! cache server every instance reaches; with `AMS_MODE=single` there is
//! nothing to coordinate, and everything here answers at once.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    cache::{Redis, RedisSlot},
    config::{Config, Mode},
    state::AppState,
};

/// How often the lease is kept, the instance announced, the counters told.
pub const TICK: Duration = Duration::from_secs(5);
/// How long the lead is held without being kept: an instance that stops
/// answering is replaced within this.
const LEAD_TTL: Duration = Duration::from_secs(15);
/// How long before the lease's end this instance stops believing it leads,
/// in case its keeping is late: two instances must never both schedule.
const LEAD_MARGIN: Duration = Duration::from_secs(3);
/// How long an instance's announcement stands without being renewed.
const ANNOUNCE_TTL: Duration = Duration::from_secs(20);
/// How long a job is held without being kept, and how often it is kept.
const HOLD_TTL: Duration = Duration::from_secs(60);
const HOLD_EVERY: Duration = Duration::from_secs(20);
/// A run another instance started, whose instance has not been heard of
/// for this long, is one a crash left behind.
const GONE_AFTER: Duration = Duration::from_secs(180);
/// How long a place that is taken for a moment — the accounts' — is
/// waited for.
const WAIT_FOR: Duration = Duration::from_secs(5);

/// This instance, as it introduces itself to the others.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    /// This process's own id, new at every start.
    pub id: String,
    /// `AMS_INSTANCE_NAME`, or the hostname: stable across restarts.
    pub name: String,
    pub version: String,
    pub started_at: String,
}

/// An instance as the others last heard of it.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Announced {
    #[serde(flatten)]
    pub instance: Instance,
    /// Whether it led when it last announced itself.
    pub leads: bool,
    pub seen_at: String,
}

pub struct Coordination {
    mode: Mode,
    pub instance: Instance,
    leads: AtomicBool,
    /// Until when the lead is good, in milliseconds of this process's own
    /// clock: past it, this instance does not lead whatever it last heard.
    lead_until: AtomicU64,
    started: Instant,
    /// Who leads, as last seen on the server.
    leader: Mutex<Option<String>>,
    /// The instances as last listed, this one included.
    announced: Mutex<Vec<Announced>>,
    /// When each instance, by name, was last heard of on this clock.
    seen: Mutex<HashMap<String, Instant>>,
    twin_said: AtomicBool,
    slot: RedisSlot,
    prefix: String,
}

impl Coordination {
    pub fn new(config: &Config, slot: RedisSlot) -> Self {
        Self {
            mode: config.mode,
            instance: Instance {
                id: crate::db::new_id(),
                name: config.instance_name.clone(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                started_at: crate::db::now(),
            },
            // Alone, this instance leads from the start; among several it
            // has to take the lease first.
            leads: AtomicBool::new(config.mode == Mode::Single),
            lead_until: AtomicU64::new(0),
            started: Instant::now(),
            leader: Mutex::new(None),
            announced: Mutex::new(Vec::new()),
            seen: Mutex::new(HashMap::new()),
            twin_said: AtomicBool::new(false),
            slot,
            prefix: config.cache.redis_prefix.clone(),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn is_multi(&self) -> bool {
        self.mode == Mode::Multi
    }

    fn clock(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Whether this instance is the one that runs the schedules: always,
    /// alone; among several, while it holds the lease and the lease has
    /// time left on this clock.
    pub fn leads(&self) -> bool {
        match self.mode {
            Mode::Single => true,
            Mode::Multi => {
                self.leads.load(Ordering::SeqCst)
                    && self.clock() < self.lead_until.load(Ordering::SeqCst)
            }
        }
    }

    /// Who leads, by name, as last seen.
    pub fn leader(&self) -> Option<String> {
        match self.mode {
            Mode::Single => Some(self.instance.name.clone()),
            Mode::Multi => self
                .leader
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }

    /// The instances as last listed: this one alone, alone.
    pub fn instances(&self) -> Vec<Announced> {
        match self.mode {
            Mode::Single => vec![Announced {
                instance: self.instance.clone(),
                leads: true,
                seen_at: crate::db::now(),
            }],
            Mode::Multi => self
                .announced
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }

    /// Whether an instance of that name announced itself lately. This one,
    /// always.
    pub fn is_alive(&self, name: &str) -> bool {
        name == self.instance.name || self.instances().iter().any(|a| a.instance.name == name)
    }

    /// Whether an instance of that name has been gone long enough for its
    /// runs to be somebody else's to close: not heard of for a while, or
    /// never since this instance started, which is a while ago.
    fn is_gone(&self, name: &str) -> bool {
        if name == self.instance.name {
            return false;
        }
        let seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        match seen.get(name) {
            Some(at) => at.elapsed() >= GONE_AFTER,
            None => self.started.elapsed() >= GONE_AFTER,
        }
    }

    fn redis(&self) -> Option<Arc<Redis>> {
        match self.mode {
            Mode::Single => None,
            Mode::Multi => self.slot.load_full(),
        }
    }

    fn key(&self, what: &str) -> String {
        format!("{}{what}", self.prefix)
    }

    /// Hold a name — a job's — so that no other instance runs the same
    /// meanwhile: held until the answer is dropped, kept alive in between.
    /// Alone, always held at once. Among several, `None` while another
    /// holds it, or while the cache server does not answer: with nobody to
    /// ask, nothing that must run once is started.
    pub async fn hold(&self, name: &str) -> Option<Held> {
        let Some(redis) = self.redis() else {
            return match self.mode {
                Mode::Single => Some(Held::alone()),
                Mode::Multi => None,
            };
        };
        let key = self.key(&format!("hold:{name}"));
        let holder = self.instance.id.clone();
        if !redis.take(&key, &holder, HOLD_TTL).await? {
            return None;
        }
        let keeper = tokio::spawn({
            let (redis, key, holder) = (redis.clone(), key.clone(), holder.clone());
            async move {
                loop {
                    tokio::time::sleep(HOLD_EVERY).await;
                    if redis.keep(&key, &holder, HOLD_TTL).await == Some(false) {
                        tracing::warn!(hold = %key, "the hold on a job was lost to another instance");
                        return;
                    }
                }
            }
        });
        Some(Held {
            redis: Some(redis),
            key,
            holder,
            keeper: Some(keeper),
        })
    }

    /// Hold a name for something somebody asked for, or say why not: held
    /// by another instance, or nobody to ask.
    pub async fn hold_for(&self, name: &str, what: &str) -> Result<Held, crate::error::AppError> {
        if let Some(held) = self.hold(name).await {
            return Ok(held);
        }
        Err(crate::error::AppError::Conflict(
            if self.is_held(name).await {
                format!("{what} on another instance; wait for it to end")
            } else {
                "the cache server does not answer, and nothing that must run once starts without it"
                    .to_string()
            },
        ))
    }

    /// Hold a name that is only ever held for a moment — the accounts',
    /// across a check and a write — waiting a little for whoever has it.
    /// `None` when nobody answers in time, in which case the caller goes
    /// on with its own process's lock alone.
    pub async fn hold_wait(&self, name: &str) -> Option<Held> {
        let until = Instant::now() + WAIT_FOR;
        loop {
            if let Some(held) = self.hold(name).await {
                return Some(held);
            }
            if self.redis().is_none() || Instant::now() >= until {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Whether another instance holds the name right now.
    pub async fn is_held(&self, name: &str) -> bool {
        let Some(redis) = self.redis() else {
            return false;
        };
        matches!(
            redis.get_text(&self.key(&format!("hold:{name}"))).await,
            Some(Some(holder)) if holder != self.instance.id
        )
    }

    /// Tell the other instances something. Fire and forget: what they are
    /// told is always something they would find out anyway, later.
    pub fn tell(&self, message: Message) {
        let Some(redis) = self.redis() else {
            return;
        };
        let text = message.encode(&self.instance.id);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { redis.publish(&text).await });
        }
    }

    /// Tell the other instances something, and wait until it is told: for
    /// what must reach them before what follows.
    pub async fn tell_now(&self, message: Message) {
        if let Some(redis) = self.redis() {
            redis.publish(&message.encode(&self.instance.id)).await;
        }
    }

    /// Keep the lead, or take it: the one thing that must happen on time,
    /// so it shares its task with nothing else. Kept while the server says
    /// the key is this instance's; taken while it is free — or this
    /// instance's still, after a moment the server did not answer.
    async fn keep_lead(&self) {
        let Some(redis) = self.redis() else {
            return;
        };
        let key = self.key("leader");
        let me = &self.instance.id;
        let led = self.leads.load(Ordering::SeqCst);
        let answer = if led {
            redis.keep(&key, me, LEAD_TTL).await
        } else {
            redis.take(&key, me, LEAD_TTL).await
        };
        match answer {
            Some(true) => {
                let until = self.clock() + (LEAD_TTL - LEAD_MARGIN).as_millis() as u64;
                self.lead_until.store(until, Ordering::SeqCst);
                if !led {
                    self.leads.store(true, Ordering::SeqCst);
                    tracing::info!(
                        instance = %self.instance.name,
                        mode = self.mode.as_str(),
                        "this instance leads: it runs the schedules"
                    );
                }
            }
            // Another has it, or it ran out: not this instance's.
            Some(false) if led => {
                self.leads.store(false, Ordering::SeqCst);
                tracing::warn!(instance = %self.instance.name, "this instance no longer leads");
            }
            Some(false) => {}
            // No answer: what was kept stands until its time runs out on
            // this clock, and is asked about again at the next round.
            None => {}
        }
    }

    /// Announce this instance, list the others, and tell the server what
    /// this instance counted meanwhile.
    async fn announce(&self, state: &AppState) {
        let Some(redis) = self.redis() else {
            return;
        };
        let me = &self.instance.id;
        let leads = self.leads();
        let announced = Announced {
            instance: self.instance.clone(),
            leads,
            seen_at: crate::db::now(),
        };
        let hash = self.key("instances");
        if let Ok(text) = serde_json::to_string(&announced) {
            redis.hset_text(&hash, me, &text).await;
        }
        let holder = redis.get_text(&self.key("leader")).await;
        // With no answer, nobody is known to lead: better no leader shown
        // than one that may have gone.
        if holder.is_none() {
            *self.leader.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        let holder = holder.flatten();
        if let Some(listed) = redis.hgetall_text(&hash).await {
            let now = chrono::Utc::now();
            let mut all = Vec::new();
            let mut stale = Vec::new();
            for (id, text) in listed {
                let Ok(one) = serde_json::from_str::<Announced>(&text) else {
                    stale.push(id);
                    continue;
                };
                // Its own word of when it spoke, on its own clock: a few
                // seconds of skew between two machines are within the
                // window, a machine that stopped is not.
                let fresh = crate::db::parse_rfc3339(&one.seen_at).is_some_and(|at| {
                    now - at < chrono::Duration::from_std(ANNOUNCE_TTL).unwrap_or_default()
                });
                if fresh {
                    // Who leads is the lease's word, not each instance's
                    // own last one: a leader that went keeps saying so
                    // until its announcement runs out.
                    all.push(Announced {
                        leads: holder.as_deref() == Some(one.instance.id.as_str()),
                        ..one
                    });
                } else {
                    stale.push(id);
                }
            }
            all.sort_by(|a, b| a.instance.name.cmp(&b.instance.name));
            {
                let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
                for one in &all {
                    seen.insert(one.instance.name.clone(), Instant::now());
                }
            }
            if all
                .iter()
                .any(|a| a.instance.name == self.instance.name && a.instance.id != *me)
                && !self.twin_said.swap(true, Ordering::SeqCst)
            {
                tracing::warn!(
                    name = %self.instance.name,
                    "another instance announces itself under this instance's name: give each \
                     its own AMS_INSTANCE_NAME, or their runs cannot be told apart"
                );
            }
            let leader = holder
                .as_deref()
                .and_then(|id| all.iter().find(|a| a.instance.id == id))
                .map(|a| a.instance.name.clone());
            *self.leader.lock().unwrap_or_else(|e| e.into_inner()) = leader;
            *self.announced.lock().unwrap_or_else(|e| e.into_inner()) = all;
            // What stopped announcing itself is forgotten by the leader.
            if leads && !stale.is_empty() {
                redis.hdel(&hash, &stale).await;
            }
        }

        state.calls.flush_to(&redis, &self.prefix).await;
    }

    /// Whether another instance announces itself under this one's name.
    pub fn has_twin(&self) -> bool {
        self.instances()
            .iter()
            .any(|a| a.instance.name == self.instance.name && a.instance.id != self.instance.id)
    }
}

/// A name held; let go of when dropped.
pub struct Held {
    redis: Option<Arc<Redis>>,
    key: String,
    holder: String,
    keeper: Option<tokio::task::JoinHandle<()>>,
}

impl Held {
    fn alone() -> Self {
        Self {
            redis: None,
            key: String::new(),
            holder: String::new(),
            keeper: None,
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(keeper) = self.keeper.take() {
            keeper.abort();
        }
        if let Some(redis) = self.redis.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let (key, holder) = (
                std::mem::take(&mut self.key),
                std::mem::take(&mut self.holder),
            );
            runtime.spawn(async move { redis.release(&key, &holder).await });
        }
    }
}

/// What one instance tells the others, besides what the caches forget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// The settings table changed: read it again.
    Settings,
    /// The network rules changed: read them again.
    Allowlist,
    /// A medium was kept: point at it from now on.
    MediaKept {
        origin: String,
        key: String,
        thumb: i64,
        content_type: String,
    },
    /// A medium was forgotten: its address is the provider's again.
    MediaForgotten(String),
    /// Media were put in line: whoever is idle may fetch them.
    MediaWake,
    /// The clients' certificate was issued anew: load it.
    TlsReload,
    /// A run is to stop, whichever instance runs it.
    Stop(String),
}

impl Message {
    /// `kind@sender:payload`. The caches' own messages (`flush:…`, `gen:…`)
    /// have a colon before any `@`, so the two never read as each other.
    pub fn encode(&self, sender: &str) -> String {
        let (kind, payload) = match self {
            Self::Settings => ("settings", String::new()),
            Self::Allowlist => ("allowlist", String::new()),
            Self::MediaKept {
                origin,
                key,
                thumb,
                content_type,
            } => (
                "media+",
                serde_json::json!({"o": origin, "k": key, "t": thumb, "c": content_type})
                    .to_string(),
            ),
            Self::MediaForgotten(origin) => ("media-", origin.clone()),
            Self::MediaWake => ("wake", String::new()),
            Self::TlsReload => ("tls", String::new()),
            Self::Stop(id) => ("stop", id.clone()),
        };
        format!("{kind}@{sender}:{payload}")
    }

    /// The message and who sent it; `None` for what is not one.
    fn decode(text: &str) -> Option<(Self, String)> {
        let (kind, rest) = text.split_once('@')?;
        if kind.contains(':') {
            return None;
        }
        let (sender, payload) = rest.split_once(':')?;
        let message = match kind {
            "settings" => Self::Settings,
            "allowlist" => Self::Allowlist,
            "media+" => {
                let value: serde_json::Value = serde_json::from_str(payload).ok()?;
                Self::MediaKept {
                    origin: value.get("o")?.as_str()?.to_string(),
                    key: value.get("k")?.as_str()?.to_string(),
                    thumb: value.get("t")?.as_i64()?,
                    content_type: value.get("c")?.as_str()?.to_string(),
                }
            }
            "media-" => Self::MediaForgotten(payload.to_string()),
            "wake" => Self::MediaWake,
            "tls" => Self::TlsReload,
            "stop" => Self::Stop(payload.to_string()),
            _ => return None,
        };
        Some((message, sender.to_string()))
    }
}

/// Keep the lease, the announcement and the counters current for as long
/// as the process runs — the lease on a task of its own, since it must be
/// kept on time whatever else takes long; the leader also closes the runs
/// of instances that are gone, and every instance reads the media index
/// again now and then. Nothing to do alone.
pub async fn run(state: AppState) {
    if !state.coord.is_multi() {
        return;
    }
    tokio::spawn({
        let state = state.clone();
        async move {
            loop {
                state.coord.keep_lead().await;
                tokio::time::sleep(TICK).await;
            }
        }
    });
    let mut rounds = 0u32;
    loop {
        state.coord.announce(&state).await;
        rounds = rounds.wrapping_add(1);
        if rounds.is_multiple_of(12) && state.coord.leads() {
            close_orphans(&state).await;
        }
        // A word of a medium kept or forgotten is lost while the cache
        // server is away: the index is read from the database again every
        // ten minutes, so what was missed is known within that.
        if rounds.is_multiple_of(120)
            && state.media.is_on()
            && let Err(e) = state.media.load_index(&state.db).await
        {
            tracing::warn!(
                error = format_args!("{e:#}"),
                "could not read the media index again"
            );
        }
        tokio::time::sleep(TICK).await;
    }
}

/// Runs still marked running by an instance nobody has heard of for a
/// while — and whose job nobody holds — closed, as a run this instance's
/// own crash left behind is closed at its start.
async fn close_orphans(state: &AppState) {
    let running = match crate::db::repo::job::running_elsewhere(
        &state.db,
        &state.coord.instance.name,
    )
    .await
    {
        Ok(running) => running,
        Err(e) => {
            tracing::warn!(error = %e, "could not read the runs of the other instances");
            return;
        }
    };
    let mut closed = 0;
    for run in running {
        let Some(instance) = run.instance.as_deref() else {
            continue;
        };
        if !state.coord.is_gone(instance) {
            continue;
        }
        // Somebody holds this job: it is being run, whatever the name says.
        if let Some(hold) = crate::jobs::tasks::hold_of(&run.kind)
            && state.coord.is_held(hold).await
        {
            continue;
        }
        match crate::db::repo::job::fail_gone(&state.db, &run.id).await {
            Ok(n) => closed += n,
            Err(e) => tracing::warn!(error = %e, run = %run.id, "could not close an orphaned run"),
        }
    }
    if closed > 0 {
        tracing::warn!(
            closed,
            "closed job runs left open by instances that are gone"
        );
    }
}

/// Act on a message from the channel: the caches' own, or another
/// instance's. What this instance told itself is passed over.
pub async fn apply_message(state: &AppState, text: &str) {
    let Some((message, sender)) = Message::decode(text) else {
        state.caches.apply_message(text).await;
        return;
    };
    if sender == state.coord.instance.id {
        return;
    }
    match message {
        Message::Settings => {
            if let Err(e) = state.settings.reload().await {
                tracing::warn!(error = %e, "could not read the settings another instance changed");
            }
            state.adopt_settings().await;
            // The identity provider may be another now.
            crate::auth::oidc::forget().await;
        }
        Message::Allowlist => {
            if let Err(e) = state.reload_allowlist_quietly().await {
                tracing::warn!(error = %e, "could not read the rules another instance changed");
            }
        }
        Message::MediaKept {
            origin,
            key,
            thumb,
            content_type,
        } => {
            state.media.remember_quietly(
                &origin,
                &key,
                crate::db::repo::asset::Thumb::from_i64(thumb),
                &content_type,
            );
        }
        Message::MediaForgotten(origin) => state.media.forget_quietly(&origin),
        Message::MediaWake => state.media.notify.notify_one(),
        Message::TlsReload => {
            if let Err(e) = crate::tls::reload(state).await {
                tracing::warn!(
                    error = format_args!("{e:#}"),
                    "could not load the certificate another instance issued"
                );
            }
        }
        Message::Stop(id) => {
            crate::jobs::cancel::cancel(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_goes_round_and_the_caches_own_do_not_read_as_one() {
        for message in [
            Message::Settings,
            Message::Allowlist,
            Message::MediaKept {
                origin: "https://p/a.jpg?x=1".into(),
                key: "ab.jpg".into(),
                thumb: 1,
                content_type: "image/jpeg".into(),
            },
            Message::MediaForgotten("https://p/a:b@c.jpg".into()),
            Message::MediaWake,
            Message::TlsReload,
            Message::Stop("job-1".into()),
        ] {
            let text = message.encode("i-1");
            assert_eq!(Message::decode(&text), Some((message, "i-1".to_string())));
        }
        assert_eq!(Message::decode("flush:items"), None);
        assert_eq!(Message::decode("drop:items:item:a@b"), None);
        assert_eq!(Message::decode("gen:4"), None);
        assert_eq!(Message::decode("nonsense@x:y"), None);
    }

    /// Among several, the lead is only ever believed for as long as the
    /// lease was kept for; alone, it is held from the start.
    #[test]
    fn the_lead_runs_out_on_this_clock() {
        let config = |mode: &str| {
            let mut config = crate::config::Config::from_env().unwrap();
            config.mode = mode.parse().unwrap();
            config
        };
        let alone = Coordination::new(&config("single"), RedisSlot::default());
        assert!(alone.leads());
        assert_eq!(
            alone.leader().as_deref(),
            Some(alone.instance.name.as_str())
        );

        let several = Coordination::new(&config("multi"), RedisSlot::default());
        assert!(!several.leads());
        several.leads.store(true, Ordering::SeqCst);
        assert!(!several.leads(), "believed, but never kept: not leading");
        several
            .lead_until
            .store(several.clock() + 1_000, Ordering::SeqCst);
        assert!(several.leads());
        several.lead_until.store(0, Ordering::SeqCst);
        assert!(!several.leads(), "the lease ran out");
        assert!(!several.is_gone(&several.instance.name));
        assert!(
            !several.is_gone("other"),
            "not gone until a while after the start"
        );
    }
}
