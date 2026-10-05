//! The second level's server: Valkey, Redis, or anything that speaks their
//! protocol, reached over one multiplexed connection that reconnects on its
//! own.
//!
//! Nothing here is allowed to slow an answer down or to fail one: every
//! command runs under a short deadline, an error marks the server down and
//! is said once, and a caller is told "nothing" rather than made to wait.
//! A ping every few seconds says when it is back. Every key this server
//! writes carries its prefix, and the one thing it deletes is what carries
//! it: a server may be shared with somebody else's data.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use bytes::Bytes;
use redis::{AsyncCommands as _, aio::ConnectionManager};

/// How long one command may take before it is given up on.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(150);
/// How often a server that answered nothing is asked whether it is back.
const HEARTBEAT: Duration = Duration::from_secs(5);
/// How long one hop of a walk over the keys may take.
const WALK_TIMEOUT: Duration = Duration::from_secs(2);
/// Keys asked for at once when a prefix is walked or counted.
const SCAN_BATCH: usize = 500;
/// The most keys a prefix is walked over, so a shared server with millions
/// of keys does not turn a count into a stall.
const SCAN_LIMIT: u64 = 2_000_000;

/// The longest message read off the channel: past what any instance sends —
/// a medium's origin and key are the longest — and short of anything that
/// would cost to look at.
const MOST_MESSAGE_BYTES: usize = 64 * 1024;

/// Push a key's end back, if the key is still the caller's: what a lease
/// is renewed with, so a lease that ran out and went to another instance
/// is not taken back from them.
const KEEP_IF_HELD: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
    return redis.call('PEXPIRE', KEYS[1], ARGV[2])
end
return 0
"#;

/// Take a key that is free, or the caller's already: a holder that lost
/// track of its own key — a moment the server did not answer — takes it
/// back rather than waiting out its own TTL.
const TAKE_IF_FREE_OR_HELD: &str = r#"
local held = redis.call('GET', KEYS[1])
if held == false or held == ARGV[1] then
    redis.call('SET', KEYS[1], ARGV[1], 'PX', ARGV[2])
    return 1
end
return 0
"#;
/// Remove a key, if it is still the caller's.
const RELEASE_IF_HELD: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
    return redis.call('DEL', KEYS[1])
end
return 0
"#;

pub struct Redis {
    client: redis::Client,
    manager: ConnectionManager,
    /// What every key this server writes begins with, `ams:` unless set.
    pub prefix: String,
    timeout: Duration,
    up: AtomicBool,
    last_error: Mutex<Option<String>>,
    /// The last ping, in microseconds.
    latency_micros: AtomicU64,
    /// Commands that ended in an error, since the start.
    pub errors: AtomicU64,
}

/// What the server says of itself, the parts an operator reads.
#[derive(Clone, Debug, Default, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    /// `valkey` or `redis`, as it names itself.
    pub server: String,
    pub version: String,
    pub used_memory: u64,
    /// Zero when no ceiling is set.
    pub maxmemory: u64,
    pub eviction_policy: String,
    pub evicted_keys: u64,
    pub expired_keys: u64,
    pub connected_clients: u64,
    pub uptime_seconds: u64,
    /// The server's own tally, over every client it has.
    pub keyspace_hits: u64,
    pub keyspace_misses: u64,
}

impl Redis {
    /// Connect, and say so; a server that is down at start is not an error,
    /// the first level carries on alone until it answers.
    pub async fn connect(url: &str, prefix: String, timeout: Duration) -> Result<Self> {
        let client = redis::Client::open(url).context("AMS_REDIS_URL is not a valid address")?;
        let config = redis::aio::ConnectionManagerConfig::new()
            .set_connection_timeout(Some(Duration::from_secs(2)))
            .set_response_timeout(Some(timeout.max(DEFAULT_TIMEOUT)))
            .set_number_of_retries(1);
        let manager = ConnectionManager::new_with_config(client.clone(), config)
            .await
            .context("could not open the connection to the cache server")?;
        Ok(Self {
            client,
            manager,
            prefix,
            timeout,
            up: AtomicBool::new(true),
            last_error: Mutex::new(None),
            latency_micros: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        })
    }

    pub fn is_up(&self) -> bool {
        self.up.load(Relaxed)
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The last ping's round trip.
    pub fn latency(&self) -> Duration {
        Duration::from_micros(self.latency_micros.load(Relaxed))
    }

    /// The full key: prefix, space, and the key within it.
    pub fn key(&self, space: &str, key: &str) -> String {
        format!("{}{space}:{key}", self.prefix)
    }

    /// The channel the instances tell each other what to forget on.
    pub fn channel(&self) -> String {
        format!("{}events", self.prefix)
    }

    /// Run one command under the deadline; an error or a timeout marks the
    /// server down and answers nothing.
    async fn run<T, F>(&self, what: &'static str, future: F) -> Option<T>
    where
        F: std::future::Future<Output = redis::RedisResult<T>>,
    {
        match tokio::time::timeout(self.timeout, future).await {
            Ok(Ok(value)) => {
                if !self.up.swap(true, Relaxed) {
                    tracing::info!("the cache server answers again");
                }
                Some(value)
            }
            Ok(Err(e)) => {
                self.down(what, &e.to_string());
                None
            }
            Err(_) => {
                self.down(what, "timed out");
                None
            }
        }
    }

    /// One hop of a walk over the keyspace: given longer than a read, and
    /// not held against the server — a slow SCAN on a large keyspace says
    /// nothing about how it answers a GET.
    async fn walk<T, F>(&self, future: F) -> Option<T>
    where
        F: std::future::Future<Output = redis::RedisResult<T>>,
    {
        match tokio::time::timeout(WALK_TIMEOUT, future).await {
            Ok(Ok(value)) => Some(value),
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "a walk over the cache server's keys stopped");
                None
            }
            Err(_) => {
                tracing::debug!("a walk over the cache server's keys timed out");
                None
            }
        }
    }

    fn down(&self, what: &str, why: &str) {
        self.errors.fetch_add(1, Relaxed);
        let first = self.up.swap(false, Relaxed);
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("{what}: {why}"));
        if first {
            tracing::warn!(
                command = what,
                error = why,
                "the cache server does not answer; the first level carries on alone"
            );
        }
    }

    pub async fn get(&self, key: &str) -> Option<Bytes> {
        if !self.is_up() {
            return None;
        }
        let mut conn = self.manager.clone();
        self.run(
            "GET",
            async move { conn.get::<_, Option<Vec<u8>>>(key).await },
        )
        .await
        .flatten()
        .map(Bytes::from)
    }

    pub async fn set_ex(&self, key: &str, value: Bytes, ttl: Duration) {
        if !self.is_up() {
            return;
        }
        let mut conn = self.manager.clone();
        let secs = ttl.as_secs().max(1);
        self.run("SET", async move {
            conn.set_ex::<_, _, ()>(key, value.as_ref(), secs).await
        })
        .await;
    }

    /// Tried whatever the server's state: a forgetting that is lost leaves a
    /// stale copy behind for as long as it lives, which is worse than a
    /// deadline spent.
    pub async fn unlink(&self, key: &str) {
        let mut conn = self.manager.clone();
        self.run("UNLINK", async move { conn.unlink::<_, ()>(key).await })
            .await;
    }

    /// Every key under a prefix, walked with SCAN and removed with UNLINK in
    /// batches: never KEYS, never FLUSHDB. How many went.
    pub async fn unlink_prefix(&self, prefix: &str) -> u64 {
        let mut removed = 0u64;
        let mut cursor = 0u64;
        let pattern = format!("{}*", glob_escape(prefix));
        loop {
            let mut conn = self.manager.clone();
            let pattern = pattern.clone();
            let Some((next, keys)) = self
                .walk(async move {
                    redis::cmd("SCAN")
                        .arg(cursor)
                        .arg("MATCH")
                        .arg(pattern)
                        .arg("COUNT")
                        .arg(SCAN_BATCH)
                        .query_async::<(u64, Vec<String>)>(&mut conn)
                        .await
                })
                .await
            else {
                break;
            };
            if !keys.is_empty() {
                let mut conn = self.manager.clone();
                let n = keys.len() as u64;
                if self
                    .run("UNLINK", async move { conn.unlink::<_, ()>(keys).await })
                    .await
                    .is_some()
                {
                    removed += n;
                }
            }
            if next == 0 || removed >= SCAN_LIMIT {
                break;
            }
            cursor = next;
        }
        removed
    }

    /// How many keys a prefix holds, walked the same way; `None` when the
    /// server did not answer.
    #[cfg(test)]
    pub async fn count_prefix(&self, prefix: &str) -> Option<u64> {
        let mut total = 0u64;
        let mut cursor = 0u64;
        let pattern = format!("{}*", glob_escape(prefix));
        loop {
            let mut conn = self.manager.clone();
            let pattern = pattern.clone();
            let (next, keys) = self
                .walk(async move {
                    redis::cmd("SCAN")
                        .arg(cursor)
                        .arg("MATCH")
                        .arg(pattern)
                        .arg("COUNT")
                        .arg(SCAN_BATCH)
                        .query_async::<(u64, Vec<String>)>(&mut conn)
                        .await
                })
                .await?;
            total += keys.len() as u64;
            if next == 0 || total >= SCAN_LIMIT {
                return Some(total);
            }
            cursor = next;
        }
    }

    /// How many keys sit under each of several prefixes, in one walk of the
    /// keyspace — and how many under the common prefix altogether. `None`
    /// when the server did not answer.
    pub async fn count_by_prefixes(
        &self,
        common: &str,
        prefixes: &[String],
    ) -> Option<(u64, Vec<u64>)> {
        let mut total = 0u64;
        let mut counts = vec![0u64; prefixes.len()];
        let mut cursor = 0u64;
        let pattern = format!("{}*", glob_escape(common));
        loop {
            let mut conn = self.manager.clone();
            let pattern = pattern.clone();
            let (next, keys) = self
                .walk(async move {
                    redis::cmd("SCAN")
                        .arg(cursor)
                        .arg("MATCH")
                        .arg(pattern)
                        .arg("COUNT")
                        .arg(SCAN_BATCH)
                        .query_async::<(u64, Vec<String>)>(&mut conn)
                        .await
                })
                .await?;
            for key in &keys {
                total += 1;
                if let Some(i) = prefixes.iter().position(|p| key.starts_with(p.as_str())) {
                    counts[i] += 1;
                }
            }
            if next == 0 || total >= SCAN_LIMIT {
                return Some((total, counts));
            }
            cursor = next;
        }
    }

    pub async fn incr(&self, key: &str) -> Option<u64> {
        if !self.is_up() {
            return None;
        }
        let mut conn = self.manager.clone();
        self.run(
            "INCR",
            async move { conn.incr::<_, _, u64>(key, 1u64).await },
        )
        .await
    }

    pub async fn set_u64(&self, key: &str, value: u64) {
        let mut conn = self.manager.clone();
        self.run("SET", async move { conn.set::<_, _, ()>(key, value).await })
            .await;
    }

    pub async fn get_u64(&self, key: &str) -> Option<u64> {
        if !self.is_up() {
            return None;
        }
        let mut conn = self.manager.clone();
        self.run("GET", async move { conn.get::<_, Option<u64>>(key).await })
            .await
            .flatten()
    }

    /// Add `n` to a counter that lives until somebody removes it.
    pub async fn incr_by(&self, key: &str, n: u64) -> Option<u64> {
        if !self.is_up() {
            return None;
        }
        let mut conn = self.manager.clone();
        self.run(
            "INCRBY",
            async move { conn.incr::<_, _, u64>(key, n).await },
        )
        .await
    }

    /// One more in a window that closes `ttl` after it opened: the count
    /// after this one. INCR and EXPIRE in one round trip; the window's key
    /// names its own period, so an EXPIRE repeated only ever pushes the end
    /// of a key nobody will ask for again.
    pub async fn incr_window(&self, key: &str, ttl: Duration) -> Option<u64> {
        if !self.is_up() {
            return None;
        }
        let mut conn = self.manager.clone();
        let secs = ttl.as_secs().max(1);
        self.run("INCR", async move {
            let (count, _): (u64, i64) = redis::pipe()
                .atomic()
                .incr(key, 1u64)
                .expire(key, secs as i64)
                .query_async(&mut conn)
                .await?;
            Ok(count)
        })
        .await
    }

    /// One fewer, for a place given back.
    pub async fn decr(&self, key: &str) {
        let mut conn = self.manager.clone();
        self.run(
            "DECR",
            async move { conn.decr::<_, _, i64>(key, 1i64).await },
        )
        .await;
    }

    /// Several counters at once, a missing one read as zero.
    pub async fn mget_u64(&self, keys: &[String]) -> Option<Vec<u64>> {
        if !self.is_up() || keys.is_empty() {
            return None;
        }
        let mut conn = self.manager.clone();
        let keys = keys.to_vec();
        self.run("MGET", async move {
            conn.mget::<_, Vec<Option<u64>>>(keys).await
        })
        .await
        .map(|values| values.into_iter().map(Option::unwrap_or_default).collect())
    }

    /// Write a text where there is none, for good: whether it was written.
    pub async fn set_nx_text(&self, key: &str, value: &str) -> Option<bool> {
        if !self.is_up() {
            return None;
        }
        let mut conn = self.manager.clone();
        let value = value.to_string();
        self.run("SET NX", async move {
            let answer: Option<String> = redis::cmd("SET")
                .arg(key)
                .arg(value)
                .arg("NX")
                .query_async(&mut conn)
                .await?;
            Ok(answer.is_some())
        })
        .await
    }

    /// A text, or nothing where there is none: `None` only when the server
    /// did not answer.
    pub async fn get_text(&self, key: &str) -> Option<Option<String>> {
        if !self.is_up() {
            return None;
        }
        let mut conn = self.manager.clone();
        self.run(
            "GET",
            async move { conn.get::<_, Option<String>>(key).await },
        )
        .await
    }

    /// Take a key that nobody holds — or that this holder holds still —
    /// for `ttl`: whether it is held. The way one instance among several
    /// becomes the one that schedules, and the way a job is held so no
    /// other instance starts the same. Tried whatever the server's state:
    /// a lease is what says whether the server answers at all.
    pub async fn take(&self, key: &str, holder: &str, ttl: Duration) -> Option<bool> {
        let mut conn = self.manager.clone();
        let millis = ttl.as_millis().max(1) as u64;
        let script = redis::Script::new(TAKE_IF_FREE_OR_HELD);
        let key = key.to_string();
        let holder = holder.to_string();
        self.run("SET NX", async move {
            script
                .key(key)
                .arg(holder)
                .arg(millis)
                .invoke_async::<i64>(&mut conn)
                .await
                .map(|taken| taken == 1)
        })
        .await
    }

    /// Keep a key taken for `ttl` more — only while it is still the
    /// holder's, so a key that expired and was taken by another is not
    /// taken back from under them. Whether it is still held.
    pub async fn keep(&self, key: &str, holder: &str, ttl: Duration) -> Option<bool> {
        let mut conn = self.manager.clone();
        let millis = ttl.as_millis().max(1) as u64;
        let script = redis::Script::new(KEEP_IF_HELD);
        let key = key.to_string();
        let holder = holder.to_string();
        self.run("PEXPIRE", async move {
            script
                .key(key)
                .arg(holder)
                .arg(millis)
                .invoke_async::<i64>(&mut conn)
                .await
                .map(|kept| kept == 1)
        })
        .await
    }

    /// Let a key go — only while it is the holder's. Tried whatever the
    /// server's state: a key that is not let go of holds everybody else for
    /// its whole TTL.
    pub async fn release(&self, key: &str, holder: &str) -> Option<bool> {
        let mut conn = self.manager.clone();
        let script = redis::Script::new(RELEASE_IF_HELD);
        let key = key.to_string();
        let holder = holder.to_string();
        self.run("DEL", async move {
            script
                .key(key)
                .arg(holder)
                .invoke_async::<i64>(&mut conn)
                .await
                .map(|released| released == 1)
        })
        .await
    }

    /// A field of a hash, written: what the instances announce themselves
    /// in, one field each.
    pub async fn hset_text(&self, key: &str, field: &str, value: &str) {
        if !self.is_up() {
            return;
        }
        let mut conn = self.manager.clone();
        let (field, value) = (field.to_string(), value.to_string());
        self.run("HSET", async move {
            conn.hset::<_, _, _, ()>(key, field, value).await
        })
        .await;
    }

    /// Every field of a hash with its text. `None` when the server did not
    /// answer.
    pub async fn hgetall_text(&self, key: &str) -> Option<Vec<(String, String)>> {
        if !self.is_up() {
            return None;
        }
        let mut conn = self.manager.clone();
        self.run("HGETALL", async move {
            conn.hgetall::<_, Vec<(String, String)>>(key).await
        })
        .await
    }

    /// Fields of a hash, removed.
    pub async fn hdel(&self, key: &str, fields: &[String]) {
        if !self.is_up() || fields.is_empty() {
            return;
        }
        let mut conn = self.manager.clone();
        let fields = fields.to_vec();
        self.run(
            "HDEL",
            async move { conn.hdel::<_, _, ()>(key, fields).await },
        )
        .await;
    }
    pub async fn publish(&self, message: &str) {
        let mut conn = self.manager.clone();
        let channel = self.channel();
        let message = message.to_string();
        self.run("PUBLISH", async move {
            conn.publish::<_, _, ()>(channel, message).await
        })
        .await;
    }

    /// A round trip, timed; the server's state follows from it.
    pub async fn ping(&self) -> Option<Duration> {
        let mut conn = self.manager.clone();
        let started = Instant::now();
        let answered = self
            .run("PING", async move {
                redis::cmd("PING").query_async::<String>(&mut conn).await
            })
            .await
            .is_some();
        let elapsed = started.elapsed();
        if answered {
            self.latency_micros.store(
                elapsed.as_micros().min(u128::from(u64::MAX)) as u64,
                Relaxed,
            );
            Some(elapsed)
        } else {
            None
        }
    }

    /// What the server says of itself.
    pub async fn info(&self) -> Option<Info> {
        if !self.is_up() {
            return None;
        }
        let mut conn = self.manager.clone();
        let text = self
            .run("INFO", async move {
                redis::cmd("INFO")
                    .arg("server")
                    .arg("memory")
                    .arg("stats")
                    .arg("clients")
                    .query_async::<String>(&mut conn)
                    .await
            })
            .await?;
        Some(parse_info(&text))
    }

    /// Ask every few seconds whether a server that went quiet is back, and
    /// keep the latency figure fresh while it is up.
    pub async fn heartbeat(self: Arc<Self>) {
        loop {
            tokio::time::sleep(HEARTBEAT).await;
            self.ping().await;
        }
    }

    /// Listen for what other instances say to forget, and hand each message
    /// to `apply`. The subscription is its own connection, reopened whenever
    /// it drops.
    pub async fn subscribe(self: Arc<Self>, apply: impl Fn(String) + Send + Sync + 'static) {
        use futures::StreamExt as _;
        let channel = self.channel();
        loop {
            match self.client.get_async_pubsub().await {
                Ok(mut pubsub) => {
                    if let Err(e) = pubsub.subscribe(&channel).await {
                        tracing::warn!(error = %e, "could not subscribe to the cache server's channel");
                    } else {
                        let mut stream = pubsub.on_message();
                        while let Some(message) = stream.next().await {
                            // Anyone who can publish here can say anything:
                            // what no instance of this server says — a
                            // message longer than any it sends — is dropped
                            // unread. What is read is checked by whoever acts
                            // on it.
                            if let Ok(text) = message.get_payload::<String>()
                                && text.len() <= MOST_MESSAGE_BYTES
                            {
                                apply(text);
                            }
                        }
                        tracing::info!("the cache server's channel closed; subscribing again");
                    }
                }
                Err(e) => {
                    tracing::debug!(error = %e, "the cache server's channel is not reachable");
                }
            }
            tokio::time::sleep(HEARTBEAT).await;
        }
    }
}

/// A prefix, safe inside a glob: a key with `*`, `?` or `[` in it is only
/// matched literally.
fn glob_escape(prefix: &str) -> String {
    let mut out = String::with_capacity(prefix.len());
    for c in prefix.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The fields this server reads out of `INFO`.
fn parse_info(text: &str) -> Info {
    let mut info = Info::default();
    let mut redis_version = String::new();
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        let number = || value.trim().parse::<u64>().unwrap_or(0);
        match key {
            "server_name" => info.server = value.trim().to_string(),
            "valkey_version" => info.version = value.trim().to_string(),
            "redis_version" => redis_version = value.trim().to_string(),
            "used_memory" => info.used_memory = number(),
            "maxmemory" => info.maxmemory = number(),
            "maxmemory_policy" => info.eviction_policy = value.trim().to_string(),
            "evicted_keys" => info.evicted_keys = number(),
            "expired_keys" => info.expired_keys = number(),
            "connected_clients" => info.connected_clients = number(),
            "uptime_in_seconds" => info.uptime_seconds = number(),
            "keyspace_hits" => info.keyspace_hits = number(),
            "keyspace_misses" => info.keyspace_misses = number(),
            _ => {}
        }
    }
    if info.server.is_empty() {
        info.server = "redis".to_string();
    }
    if info.version.is_empty() {
        info.version = redis_version;
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against a server somebody started for the test: `AMS_TEST_REDIS_URL`.
    /// Everything it writes is under a prefix of its own, and removed.
    #[tokio::test]
    async fn against_a_live_server() {
        let Ok(url) = std::env::var("AMS_TEST_REDIS_URL") else {
            return;
        };
        let prefix = format!("ams-test-{}:", crate::db::new_id());
        let redis = Redis::connect(&url, prefix.clone(), DEFAULT_TIMEOUT)
            .await
            .expect("the test server answers");
        assert!(redis.ping().await.is_some());
        assert!(redis.is_up());

        let key = redis.key("items", "a");
        assert_eq!(redis.get(&key).await, None);
        redis
            .set_ex(&key, Bytes::from_static(b"one"), Duration::from_secs(60))
            .await;
        assert_eq!(redis.get(&key).await.as_deref(), Some(&b"one"[..]));
        assert_eq!(redis.incr(&format!("{prefix}gen")).await, Some(1));
        assert_eq!(redis.incr(&format!("{prefix}gen")).await, Some(2));
        assert_eq!(redis.get_u64(&format!("{prefix}gen")).await, Some(2));
        assert_eq!(redis.count_prefix(&redis.key("items", "")).await, Some(1));
        assert_eq!(redis.count_prefix(&prefix).await, Some(2));

        // A lease: taken once, kept by its holder alone, let go by them alone.
        let lease = format!("{prefix}leader");
        let ttl = Duration::from_secs(30);
        assert_eq!(redis.take(&lease, "a", ttl).await, Some(true));
        assert_eq!(redis.take(&lease, "b", ttl).await, Some(false));
        assert_eq!(redis.keep(&lease, "a", ttl).await, Some(true));
        assert_eq!(redis.keep(&lease, "b", ttl).await, Some(false));
        assert_eq!(redis.release(&lease, "b").await, Some(false));
        assert_eq!(redis.get_text(&lease).await, Some(Some("a".into())));
        assert_eq!(redis.release(&lease, "a").await, Some(true));
        assert_eq!(redis.get_text(&lease).await, Some(None));
        assert_eq!(redis.take(&lease, "b", ttl).await, Some(true));

        // A window counts, and the counters are read together.
        let window = format!("{prefix}rl:x");
        assert_eq!(redis.incr_window(&window, ttl).await, Some(1));
        assert_eq!(redis.incr_window(&window, ttl).await, Some(2));
        assert_eq!(redis.incr_by(&format!("{prefix}calls:a"), 5).await, Some(5));
        assert_eq!(
            redis
                .mget_u64(&[
                    format!("{prefix}calls:a"),
                    format!("{prefix}calls:none"),
                    window.clone()
                ])
                .await,
            Some(vec![5, 0, 2])
        );
        redis.decr(&window).await;
        assert_eq!(redis.get_u64(&window).await, Some(1));
        let hash = format!("{prefix}instances");
        redis.hset_text(&hash, "i1", "{}").await;
        redis.hset_text(&hash, "i2", "[]").await;
        let mut announced = redis.hgetall_text(&hash).await.unwrap();
        announced.sort();
        assert_eq!(
            announced,
            vec![
                ("i1".to_string(), "{}".to_string()),
                ("i2".to_string(), "[]".to_string())
            ]
        );
        redis.hdel(&hash, &["i1".to_string()]).await;
        assert_eq!(redis.hgetall_text(&hash).await.unwrap().len(), 1);
        // Taken back by its own holder after a moment nobody answered.
        assert_eq!(redis.take(&lease, "b", ttl).await, Some(true));
        assert_eq!(redis.take(&lease, "a", ttl).await, Some(false));

        let info = redis.info().await.expect("INFO answers");
        assert!(!info.version.is_empty());
        assert!(info.uptime_seconds > 0 || info.used_memory > 0);

        // Everything under the prefix goes, and nothing else does.
        let other = format!("{prefix}other:b");
        redis
            .set_ex(&other, Bytes::from_static(b"two"), Duration::from_secs(60))
            .await;
        assert_eq!(redis.unlink_prefix(&redis.key("items", "")).await, 1);
        assert_eq!(redis.get(&other).await.as_deref(), Some(&b"two"[..]));
        assert!(redis.unlink_prefix(&prefix).await >= 2);
        assert_eq!(redis.count_prefix(&prefix).await, Some(0));
        assert_eq!(redis.errors.load(Relaxed), 0);
    }

    #[test]
    fn a_prefix_is_matched_literally() {
        assert_eq!(glob_escape("ams:items:"), "ams:items:");
        assert_eq!(glob_escape("a*b?[c]"), "a\\*b\\?\\[c\\]");
    }

    /// Valkey names itself and its version; a Redis names only its own.
    #[test]
    fn the_info_read_is_the_operators() {
        let valkey = "# Server\r\nredis_version:7.2.4\r\nserver_name:valkey\r\nvalkey_version:8.1.10\r\nuptime_in_seconds:42\r\n# Memory\r\nused_memory:1000\r\nmaxmemory:2000\r\nmaxmemory_policy:allkeys-lru\r\n# Stats\r\nevicted_keys:3\r\nexpired_keys:4\r\nkeyspace_hits:10\r\nkeyspace_misses:5\r\n# Clients\r\nconnected_clients:2\r\n";
        let info = parse_info(valkey);
        assert_eq!(info.server, "valkey");
        assert_eq!(info.version, "8.1.10");
        assert_eq!((info.used_memory, info.maxmemory), (1000, 2000));
        assert_eq!(info.eviction_policy, "allkeys-lru");
        assert_eq!((info.evicted_keys, info.expired_keys), (3, 4));
        assert_eq!((info.keyspace_hits, info.keyspace_misses), (10, 5));
        assert_eq!(info.connected_clients, 2);
        assert_eq!(info.uptime_seconds, 42);

        let redis = "redis_version:8.0.1\r\nused_memory:7\r\n";
        let info = parse_info(redis);
        assert_eq!(info.server, "redis");
        assert_eq!(info.version, "8.0.1");
    }
}
