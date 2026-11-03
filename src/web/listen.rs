//! The doors' listeners: how many connections each holds at once, and how
//! long a connection may take to say what it wants.
//!
//! axum's own `serve` builds hyper's connection builder without a clock, and
//! hyper drops its header-read timeout without one: a connection that sent
//! nothing, or a byte a minute, was held for ever, and with nothing counting
//! them either, a thousand of those took every descriptor the process had —
//! the database's and the providers' sockets with them. Every door is served
//! here instead:
//!
//! * a semaphore in the accept loop holds each door to `AMS_MAX_CONNECTIONS`:
//!   past it, a connection waits in the system's queue, not accepted, until
//!   another one closes;
//! * no one network holds more than [`PER_PEER`] of them: a connection past
//!   that is closed as it is accepted, so that one address cannot take every
//!   place of the door — a trusted proxy's, and this machine's own, are not
//!   counted, since everybody behind a proxy has its address;
//! * hyper is given a clock, and with it `AMS_HEADER_READ_TIMEOUT` for a
//!   request's head — the first, and each one a connection kept alive waits
//!   for — and a head no larger than a head needs to be;
//! * the bytes before that, which tell HTTP/2 from HTTP/1 and which hyper
//!   does not time, are given the same;
//! * an idle HTTP/2 connection is pinged, and closed when it stops answering —
//!   and closed when it has had no request in flight for [`IDLE`] whatever it
//!   answers, since a ping keeps a connection alive however little it is used;
//! * a response that cannot be written because its peer does not read it is
//!   given up on after [`WRITE_STALL`], and the connection with it.

use std::{
    collections::HashMap,
    convert::Infallible,
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{
        Arc, LazyLock, Mutex, PoisonError,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use axum::{
    Router,
    extract::ConnectInfo,
    http::{Request, Response},
};
use axum_server::{
    AddrListener, Address, Handle, Server,
    accept::{Accept, DefaultAcceptor},
    tls_rustls::{RustlsAcceptor, RustlsConfig},
};
use bytes::Bytes;
use hyper::body::{Body as HttpBody, Frame, Incoming, SizeHint};
use ipnet::IpNet;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore},
};
use tower::Service;

use crate::auth::ip;

/// What every door is held to.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Connections at once.
    pub connections: usize,
    /// How long a connection has to say what it wants.
    pub head: Duration,
    /// How long a connection may have no request in flight.
    pub idle: Duration,
    /// How long a response may wait to be written to a peer that does not
    /// read it.
    pub write: Duration,
}

impl Limits {
    /// The limits of a door holding `connections` at once, whose connections
    /// have `head` to say what they want: idle no longer than that, nor than
    /// [`IDLE`] — a head's time is what a connection kept alive was given to
    /// sit idle in, and a longer one is the operator's word for it.
    pub fn new(connections: usize, head: Duration) -> Self {
        Self {
            connections,
            head,
            idle: IDLE.max(head),
            write: WRITE_STALL,
        }
    }
}

/// Connections a door bound by address alone holds; the configured doors
/// are given theirs.
const DEFAULT_CONNECTIONS: usize = 1024;

/// The most connections one network holds at once — an address, or an IPv6
/// /64, as the rate limit counts a caller. A browser holds a handful, a
/// Sonarr or a Plex a few; a thousand and twenty-four is sixteen times this.
const PER_PEER: usize = 64;

/// The most a request's head may take in a connection's buffer. A head is
/// a few kilobytes; hyper's default lets each connection hold four hundred
/// before refusing one, which a door full of slow connections turns into
/// memory the container does not have.
const HEAD_BUFFER: usize = 64 * 1024;

/// How often an idle HTTP/2 connection is asked whether it is still there,
/// and how long it has to answer.
const KEEP_ALIVE: Duration = Duration::from_secs(30);
const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a connection may have no request in flight before it is closed.
/// Longer than the idle time most clients keep a connection for — a browser's
/// minute or two, a proxy's — so that a client reuses a connection that is
/// still open, not one being closed as it sends.
const IDLE: Duration = Duration::from_secs(120);

/// How long a response may wait to be written to a peer that does not read
/// it — the time a send buffer may stay full, as a web server's own send
/// timeout is — before it is given up on.
const WRITE_STALL: Duration = Duration::from_secs(60);

/// How long bytes must have stopped coming before an idle connection is
/// closed, so that a request on its way is not met by the connection closing:
/// the bytes of a request read are not yet a request in flight.
const SETTLE: Duration = Duration::from_secs(1);

/// The address a connection came from, as the doors hand it on, and what the
/// connection is doing.
#[derive(Clone, Debug)]
pub struct Peer {
    addr: SocketAddr,
    life: Arc<Life>,
}

impl Address for Peer {
    type Stream = Held;
    type Listener = Door;
}

/// How many connections each network holds at once, and the networks nobody
/// counts.
pub struct Crowd {
    most: usize,
    exempt: Vec<IpNet>,
    holding: Mutex<HashMap<IpAddr, usize>>,
}

impl Crowd {
    /// Every network held to [`PER_PEER`] connections but the `proxies` —
    /// which hold those of everybody behind them — and this machine.
    pub fn new(proxies: &[IpNet]) -> Self {
        let machine = ["127.0.0.0/8", "::1/128"]
            .into_iter()
            .filter_map(|network| network.parse::<IpNet>().ok());
        Self::of(PER_PEER, proxies.iter().copied().chain(machine).collect())
    }

    fn of(most: usize, exempt: Vec<IpNet>) -> Self {
        Self {
            most: most.max(1),
            exempt,
            holding: Mutex::new(HashMap::new()),
        }
    }

    /// A share for a connection from `peer` — or none, when its network
    /// holds as many as it may.
    fn admit(self: &Arc<Self>, peer: SocketAddr) -> Option<Share> {
        if ip::is_trusted_peer(Some(peer), &self.exempt) {
            return Some(Share {
                crowd: self.clone(),
                network: None,
            });
        }
        let network = ip::bucket(peer.ip());
        let mut holding = self.holding.lock().unwrap_or_else(PoisonError::into_inner);
        let held = holding.entry(network).or_default();
        if *held >= self.most {
            return None;
        }
        *held += 1;
        Some(Share {
            crowd: self.clone(),
            network: Some(network),
        })
    }

    /// How many networks hold a connection now.
    #[cfg(test)]
    fn networks(&self) -> usize {
        self.holding
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// A connection's place in its network's count, given back when it closes.
struct Share {
    crowd: Arc<Crowd>,
    /// The network it is counted under; none for one nobody counts.
    network: Option<IpAddr>,
}

impl Drop for Share {
    fn drop(&mut self) {
        let Some(network) = self.network else {
            return;
        };
        let mut holding = self
            .crowd
            .holding
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = holding.get_mut(&network) {
            *held = held.saturating_sub(1);
            if *held == 0 {
                holding.remove(&network);
            }
        }
    }
}

/// What a connection is doing: shared by the stream it is read from and
/// written to, and the service its requests are given to.
#[derive(Debug)]
struct Life {
    /// The requests begun and not yet done with: a response whose last byte
    /// was written, or whose caller has gone.
    open: AtomicUsize,
    /// When the connection last had none, or began: milliseconds after
    /// [`EPOCH`].
    idle_since: AtomicU64,
}

impl Life {
    fn new() -> Self {
        Self {
            open: AtomicUsize::new(0),
            idle_since: AtomicU64::new(now_ms()),
        }
    }

    /// A request begins.
    fn begin(self: &Arc<Self>) -> Open {
        self.open.fetch_add(1, Ordering::SeqCst);
        Open(self.clone())
    }

    fn open(&self) -> usize {
        self.open.load(Ordering::SeqCst)
    }

    fn idle_since(&self) -> u64 {
        self.idle_since.load(Ordering::SeqCst)
    }
}

/// A request in flight, until it is dropped.
struct Open(Arc<Life>);

impl Drop for Open {
    fn drop(&mut self) {
        if self.0.open.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.idle_since.store(now_ms(), Ordering::SeqCst);
        }
    }
}

/// A listener that holds no more connections than its share.
pub struct Door {
    listener: TcpListener,
    connections: usize,
    places: Arc<Semaphore>,
    crowd: Arc<Crowd>,
    /// When the door last said it was full, in seconds after [`EPOCH`] plus
    /// one; nought for never.
    said_full: AtomicU64,
    /// The same, for a network that holds as many as it may.
    said_crowded: AtomicU64,
}

static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Milliseconds since [`EPOCH`].
fn now_ms() -> u64 {
    u64::try_from(EPOCH.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn millis(time: Duration) -> u64 {
    u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
}

impl Door {
    pub fn new(listener: TcpListener, connections: usize, crowd: Crowd) -> Self {
        let connections = connections.max(1);
        Self {
            listener,
            connections,
            places: Arc::new(Semaphore::new(connections)),
            crowd: Arc::new(crowd),
            said_full: AtomicU64::new(0),
            said_crowded: AtomicU64::new(0),
        }
    }
}

/// Whether a thing that stays so a while — a door full, a network holding its
/// share — is to be said now: once a minute at most.
fn say(last: &AtomicU64) -> bool {
    let now = EPOCH.elapsed().as_secs() + 1;
    let before = last.load(Ordering::Relaxed);
    (before == 0 || now.saturating_sub(before) >= 60)
        && last
            .compare_exchange(before, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

impl Door {
    fn say_full(&self) {
        if say(&self.said_full) {
            tracing::warn!(
                connections = self.connections,
                "a door holds as many connections as AMS_MAX_CONNECTIONS allows; new ones wait \
                 until one closes"
            );
        }
    }

    fn say_crowded(&self, peer: SocketAddr) {
        if say(&self.said_crowded) {
            tracing::warn!(
                %peer,
                most = self.crowd.most,
                "a network holds as many connections as one may; its new ones are closed"
            );
        }
    }
}

impl AddrListener<Held, Peer> for Door {
    async fn bind_to(addr: Peer) -> io::Result<Self> {
        Ok(Self::new(
            TcpListener::bind(addr.addr).await?,
            DEFAULT_CONNECTIONS,
            Crowd::new(&[]),
        ))
    }

    /// A place first, then the connection: past the limit nothing more is
    /// accepted, so nothing more holds a descriptor. A connection whose
    /// network holds its share already is closed as it is accepted, and the
    /// place goes to the next.
    async fn accept_stream(&self) -> io::Result<(Held, Peer)> {
        let place = match self.places.clone().try_acquire_owned() {
            Ok(place) => place,
            Err(_) => {
                self.say_full();
                self.places
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(io::Error::other)?
            }
        };
        loop {
            let (stream, addr) = self.listener.accept().await?;
            let Some(share) = self.crowd.admit(addr) else {
                self.say_crowded(addr);
                drop(stream);
                continue;
            };
            let life = Arc::new(Life::new());
            return Ok((
                Held {
                    stream,
                    life: life.clone(),
                    _place: place,
                    _share: share,
                },
                Peer { addr, life },
            ));
        }
    }

    fn get_local_addr(&self) -> io::Result<Peer> {
        self.listener.local_addr().map(|addr| Peer {
            addr,
            life: Arc::new(Life::new()),
        })
    }
}

/// An accepted connection, holding its place at the door and in its network's
/// count until it closes.
pub struct Held {
    stream: TcpStream,
    life: Arc<Life>,
    _place: OwnedSemaphorePermit,
    _share: Share,
}

impl AsyncRead for Held {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for Held {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

/// How long a connection may be idle, and how long a write may stall.
#[derive(Clone, Copy, Debug)]
struct Tempo {
    idle: Duration,
    write: Duration,
}

/// Gives a connection's first bytes — after TLS, where there is any — the
/// time a request's head is given, and watches it from then on
/// ([`Watched`]).
#[derive(Clone)]
pub struct Prompt<A> {
    inner: A,
    within: Duration,
    tempo: Tempo,
}

impl<A, S> Accept<Held, S> for Prompt<A>
where
    A: Accept<Held, S>,
    A::Future: Send + 'static,
    A::Stream: Send + 'static,
    A::Service: Send + 'static,
{
    type Stream = Watched<Timed<A::Stream>>;
    type Service = A::Service;
    type Future = Pin<Box<dyn Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send>>;

    fn accept(&self, stream: Held, service: S) -> Self::Future {
        let life = stream.life.clone();
        let accepted = self.inner.accept(stream, service);
        let (within, tempo) = (self.within, self.tempo);
        Box::pin(async move {
            let (stream, service) = accepted.await?;
            Ok((
                Watched::new(Timed::new(stream, within), life, tempo),
                service,
            ))
        })
    }
}

/// What a connection opens with to speak HTTP/2.
const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// A connection whose protocol has to be told by a deadline.
///
/// hyper reads up to HTTP/2's preface to choose between the two protocols,
/// and times nothing while it does: a connection that sends nothing, or a
/// few bytes of the preface and then nothing, waited there for ever. Once
/// the bytes are not the preface, or are all of it, the protocol is told and
/// hyper's own timers take over.
pub struct Timed<S> {
    inner: S,
    /// The moment the protocol must be told by, until it is.
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    /// How much of the preface the bytes so far have been.
    matched: usize,
}

impl<S> Timed<S> {
    fn new(inner: S, within: Duration) -> Self {
        Self {
            inner,
            deadline: Some(Box::pin(tokio::time::sleep(within))),
            matched: 0,
        }
    }

    fn saw(&mut self, fresh: &[u8]) {
        for byte in fresh {
            if PREFACE.get(self.matched) != Some(byte) {
                self.deadline = None;
                return;
            }
            self.matched += 1;
            if self.matched == PREFACE.len() {
                self.deadline = None;
                return;
            }
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Timed<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if this.deadline.is_some() {
                    this.saw(&buf.filled()[before..]);
                }
                Poll::Ready(Ok(()))
            }
            Poll::Pending => {
                if let Some(deadline) = this.deadline.as_mut()
                    && deadline.as_mut().poll(cx).is_ready()
                {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the connection did not say what it wants in time",
                    )));
                }
                Poll::Pending
            }
            failed => failed,
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Timed<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// A connection watched for what hyper's own timers do not see.
///
/// *Idle.* An HTTP/2 connection is pinged while it has nothing to say, and a
/// peer that answers the pings is a peer that is there: kept for ever,
/// whatever it asks, and so are those of a client that asks for something
/// once a minute. One with no request in flight for [`Tempo::idle`] is
/// closed — read to its end, as if its peer had gone — unless bytes are still
/// coming in, which it is given a moment to finish.
///
/// *Stalled writes.* A response a peer does not read fills the send buffer,
/// and what hyper writes waits for room for ever. A write that makes no
/// headway for [`Tempo::write`] fails, and the connection with it.
pub struct Watched<S> {
    inner: S,
    life: Arc<Life>,
    tempo: Tempo,
    /// Wakes a read that has nothing to read when the connection may be due
    /// to close.
    idle: Pin<Box<tokio::time::Sleep>>,
    /// The moment `idle` is set to wake, in milliseconds after [`EPOCH`].
    armed_for: u64,
    /// When bytes were last read, in milliseconds after [`EPOCH`].
    read_at: u64,
    /// Armed while a write cannot go through.
    stalled: Option<Pin<Box<tokio::time::Sleep>>>,
}

/// When an idle connection is to be closed, in milliseconds after
/// [`EPOCH`]: once it has had no request in flight for `idle` — from `since`
/// — and bytes have stopped coming for [`SETTLE`] (a quarter of `idle`, where
/// that is less) since `read_at`; but not later than twice `idle` from
/// `since`, whatever arrives, since a peer that sends something every second
/// is no peer that wants an answer.
fn closes_at(since: u64, read_at: u64, idle: Duration) -> u64 {
    let idle = millis(idle);
    let settle = millis(SETTLE).min(idle / 4);
    let earliest = since.saturating_add(idle);
    let latest = since.saturating_add(idle.saturating_mul(2));
    earliest.max(read_at.saturating_add(settle).min(latest))
}

impl<S> Watched<S> {
    fn new(inner: S, life: Arc<Life>, tempo: Tempo) -> Self {
        Self {
            inner,
            life,
            tempo,
            idle: Box::pin(tokio::time::sleep(tempo.idle)),
            armed_for: now_ms().saturating_add(millis(tempo.idle)),
            read_at: now_ms(),
            stalled: None,
        }
    }

    /// Whether the connection is to be closed now; where it is not, the timer
    /// that wakes the read when it may be is set, and has the task's waker.
    fn closes_idle(&mut self, cx: &mut Context<'_>) -> bool {
        loop {
            let now = now_ms();
            let busy = self.life.open() > 0;
            let at = if busy {
                // Looked at again after an idle time: what is asked of it
                // ends, and the connection is idle from then.
                now.saturating_add(millis(self.tempo.idle))
            } else {
                closes_at(self.life.idle_since(), self.read_at, self.tempo.idle)
            };
            if !busy && now >= at {
                return true;
            }
            // Set again only where it would wake the read too late, or has
            // gone off: a busy connection finds nothing to read after every
            // batch of bytes, and a timer moved each time is work for nothing.
            // One that wakes it too soon only has it look again.
            if self.armed_for <= now || self.armed_for > at {
                let wake = tokio::time::Instant::now() + Duration::from_millis(at - now);
                self.idle.as_mut().reset(wake);
                self.armed_for = at;
            }
            if self.idle.as_mut().poll(cx).is_pending() {
                return false;
            }
            // Gone off: looked at again, and set for what is then due.
            self.armed_for = 0;
        }
    }

    /// A write's answer, given up on if it has been pending for as long as
    /// writes may stall.
    fn watch<T>(
        &mut self,
        cx: &mut Context<'_>,
        polled: Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        match polled {
            Poll::Pending => {
                let within = self.tempo.write;
                let stalled = self
                    .stalled
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(within)));
                if stalled.as_mut().poll(cx).is_ready() {
                    self.stalled = None;
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the peer did not read what it was sent in time",
                    )));
                }
                Poll::Pending
            }
            ready => {
                self.stalled = None;
                ready
            }
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Watched<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                if buf.filled().len() > before {
                    this.read_at = now_ms();
                }
                Poll::Ready(Ok(()))
            }
            Poll::Pending => {
                if this.closes_idle(cx) {
                    // The end of what the peer sent: hyper takes the peer
                    // for gone, and closes the connection.
                    return Poll::Ready(Ok(()));
                }
                Poll::Pending
            }
            failed => failed,
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Watched<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.inner).poll_write(cx, buf);
        this.watch(cx, polled)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.inner).poll_write_vectored(cx, bufs);
        this.watch(cx, polled)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.inner).poll_flush(cx);
        this.watch(cx, polled)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.inner).poll_shutdown(cx);
        this.watch(cx, polled)
    }
}

/// What the requests of the connections of a door are served by: made for
/// each connection, with the address it came from and what it is doing.
#[derive(Clone)]
struct Connections {
    router: Router,
}

impl Service<Peer> for Connections {
    type Response = Tracked;
    type Error = Infallible;
    type Future = std::future::Ready<Result<Tracked, Infallible>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, peer: Peer) -> Self::Future {
        std::future::ready(Ok(Tracked {
            router: self.router.clone(),
            addr: peer.addr,
            life: peer.life,
        }))
    }
}

/// The router, serving one connection's requests: each told the address the
/// connection came from — as `ConnectInfo<SocketAddr>`, which is what the
/// handlers read — and counted among those in flight until its response's
/// last byte is written.
#[derive(Clone)]
struct Tracked {
    router: Router,
    addr: SocketAddr,
    life: Arc<Life>,
}

impl Service<Request<Incoming>> for Tracked {
    type Response = Response<Counted>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut request: Request<Incoming>) -> Self::Future {
        request.extensions_mut().insert(ConnectInfo(self.addr));
        // Counted from here, and given to the response's body below: a
        // request whose future is dropped — its caller gone — is no longer
        // in flight either.
        let open = self.life.begin();
        let mut router = self.router.clone();
        Box::pin(async move {
            let response = match router.call(request).await {
                Ok(response) => response,
                Err(never) => match never {},
            };
            Ok(response.map(|body| Counted { body, _open: open }))
        })
    }
}

/// A response's body, keeping its request among those in flight until it is
/// dropped — written to its end, or its caller gone. Everything else of the
/// body is the body's: its frames, whether it is at its end, how large it is
/// (hyper writes a `Content-Length` of it).
struct Counted {
    body: axum::body::Body,
    _open: Open,
}

impl HttpBody for Counted {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        Pin::new(&mut self.body).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

/// hyper's clock, kept by tokio: what lets hyper time a connection out.
#[derive(Clone, Copy, Debug)]
struct Clock;

impl hyper::rt::Timer for Clock {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn hyper::rt::Sleep>> {
        Box::pin(Alarm(Box::pin(tokio::time::sleep(duration))))
    }

    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn hyper::rt::Sleep>> {
        Box::pin(Alarm(Box::pin(tokio::time::sleep_until(deadline.into()))))
    }
}

struct Alarm(Pin<Box<tokio::time::Sleep>>);

impl Future for Alarm {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.0.as_mut().poll(cx)
    }
}

impl hyper::rt::Sleep for Alarm {}

/// Serve `router` at `door` — in TLS when `tls` is given — until `handle`
/// is told to stop.
pub async fn serve(
    door: Door,
    tls: Option<RustlsConfig>,
    router: Router,
    handle: Handle<Peer>,
    limits: Limits,
) -> io::Result<()> {
    let service = Connections { router };
    let tempo = Tempo {
        idle: limits.idle,
        write: limits.write,
    };
    match tls {
        Some(config) => {
            let mut server = Server::<Peer>::from_listener(door)
                .acceptor(Prompt {
                    inner: RustlsAcceptor::new(config),
                    within: limits.head,
                    tempo,
                })
                .handle(handle);
            tune(&mut server, limits);
            server.serve(service).await
        }
        None => {
            let mut server = Server::<Peer>::from_listener(door)
                .acceptor(Prompt {
                    inner: DefaultAcceptor::new(),
                    within: limits.head,
                    tempo,
                })
                .handle(handle);
            tune(&mut server, limits);
            server.serve(service).await
        }
    }
}

/// hyper's own limits: a clock, the time a head may take, the room it may
/// take, and the pings that find an HTTP/2 connection gone.
fn tune<A>(server: &mut Server<Peer, A>, limits: Limits) {
    let builder = server.http_builder();
    builder
        .http1()
        .timer(Clock)
        .header_read_timeout(limits.head)
        .max_buf_size(HEAD_BUFFER);
    builder
        .http2()
        .timer(Clock)
        .keep_alive_interval(KEEP_ALIVE)
        .keep_alive_timeout(KEEP_ALIVE_TIMEOUT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    impl Crowd {
        /// Every network counted, this machine's too: what a test of the
        /// count runs on, over loopback.
        fn counting_all(most: usize) -> Self {
            Self::of(most, Vec::new())
        }
    }

    /// The limits a door of these tests has: generous, but for what a test
    /// narrows.
    fn limits() -> Limits {
        Limits {
            connections: 8,
            head: Duration::from_secs(30),
            idle: Duration::from_secs(60),
            write: Duration::from_secs(60),
        }
    }

    /// A door serving a few routes, with the limits given, its network
    /// counted as the crowd says; its address.
    async fn door_with(limits: Limits, crowd: Crowd) -> (SocketAddr, Handle<Peer>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/", axum::routing::get(|| async { "here" }))
            .route(
                "/slow",
                axum::routing::get(|| async {
                    tokio::time::sleep(Duration::from_millis(900)).await;
                    "slowly"
                }),
            )
            .route(
                "/endless",
                axum::routing::get(|| async {
                    let chunks = futures::stream::repeat_with(|| {
                        Ok::<_, io::Error>(Bytes::from(vec![0u8; 64 * 1024]))
                    });
                    axum::body::Body::from_stream(chunks)
                }),
            );
        let handle = Handle::new();
        tokio::spawn(serve(
            Door::new(listener, limits.connections, crowd),
            None,
            router,
            handle.clone(),
            limits,
        ));
        (addr, handle)
    }

    async fn door(limits: Limits) -> (SocketAddr, Handle<Peer>) {
        door_with(limits, Crowd::new(&[])).await
    }

    async fn ask(addr: SocketAddr) -> String {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        answer
    }

    /// An answer's status line, and its body.
    fn parts(answer: &str) -> (&str, &str) {
        let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((answer, ""));
        (head.lines().next().unwrap_or_default(), body)
    }

    /// Whether the door has closed a connection: its end is read within
    /// `within`, and nothing else is.
    async fn closed_within(stream: &mut TcpStream, within: Duration) -> bool {
        let mut rest = Vec::new();
        matches!(
            tokio::time::timeout(within, stream.read_to_end(&mut rest)).await,
            Ok(Ok(_) | Err(_))
        )
    }

    /// Whether the door leaves a connection open for `for_`: nothing is read
    /// of it, and it is not closed.
    async fn open_for(stream: &mut TcpStream, for_: Duration) -> bool {
        let mut byte = [0u8; 1];
        tokio::time::timeout(for_, stream.read(&mut byte))
            .await
            .is_err()
    }

    #[tokio::test]
    async fn a_door_answers_through_its_limits() {
        let (addr, handle) = door(Limits {
            connections: 4,
            head: Duration::from_secs(5),
            ..limits()
        })
        .await;
        let answer = ask(addr).await;
        assert_eq!(parts(&answer), ("HTTP/1.1 200 OK", "here"));
        handle.shutdown();
    }

    #[tokio::test]
    async fn a_connection_that_says_nothing_is_closed() {
        let (addr, handle) = door(Limits {
            connections: 4,
            head: Duration::from_millis(200),
            ..limits()
        })
        .await;

        // Nothing at all: the protocol is never told.
        let mut silent = TcpStream::connect(addr).await.unwrap();
        let mut byte = [0u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(5), silent.read(&mut byte))
            .await
            .expect("the door should have closed the connection");
        assert!(matches!(closed, Ok(0) | Err(_)));

        // Half of HTTP/2's preface, and then nothing.
        let mut stalled = TcpStream::connect(addr).await.unwrap();
        stalled.write_all(b"PRI * HTTP").await.unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(5), stalled.read(&mut byte))
            .await
            .expect("the door should have closed the connection");
        assert!(matches!(closed, Ok(0) | Err(_)));

        // Half a request's head, and then nothing: hyper's own timer.
        let mut slow = TcpStream::connect(addr).await.unwrap();
        slow.write_all(b"GET / HTTP/1.1\r\nHost: ").await.unwrap();
        let mut answer = Vec::new();
        let closed = tokio::time::timeout(Duration::from_secs(5), slow.read_to_end(&mut answer))
            .await
            .expect("the door should have closed the connection");
        // Closed, with at most a refusal said first; never a 200.
        if closed.is_ok() {
            assert!(
                answer.is_empty() || answer.starts_with(b"HTTP/1.1 4"),
                "{}",
                String::from_utf8_lossy(&answer)
            );
        }

        handle.shutdown();
    }

    #[tokio::test]
    async fn a_full_door_keeps_the_next_connection_waiting() {
        let (addr, handle) = door(Limits {
            connections: 1,
            head: Duration::from_secs(30),
            ..limits()
        })
        .await;

        // The one place, taken by a connection that is still deciding.
        let held = TcpStream::connect(addr).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The next is not accepted while it is taken…
        let waiting = tokio::spawn(ask(addr));
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!waiting.is_finished());

        // …and is answered as soon as the place is given back.
        drop(held);
        let answer = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the waiting connection should be answered")
            .unwrap();
        assert_eq!(parts(&answer).0, "HTTP/1.1 200 OK");
        handle.shutdown();
    }

    #[tokio::test]
    async fn the_protocol_is_told_once_the_bytes_say_which() {
        // HTTP/1: its first byte is not the preface's.
        let mut h1 = Timed::new((), Duration::from_secs(1));
        h1.saw(b"G");
        assert!(h1.deadline.is_none());

        // HTTP/2: told only once the whole preface is there.
        let mut h2 = Timed::new((), Duration::from_secs(1));
        h2.saw(&PREFACE[..10]);
        assert!(h2.deadline.is_some());
        h2.saw(&PREFACE[10..]);
        assert!(h2.deadline.is_none());

        // Nothing read — the stream's end — tells nothing.
        let mut nothing = Timed::new((), Duration::from_secs(1));
        nothing.saw(b"");
        assert!(nothing.deadline.is_some());
    }

    // ── the count of a network's connections ────────────────────────────

    fn peer(address: &str) -> SocketAddr {
        format!("{address}:40000").parse().unwrap()
    }

    #[test]
    fn a_network_holds_so_many_connections_and_gives_a_share_back() {
        let crowd = Arc::new(Crowd::counting_all(2));
        let first = crowd.admit(peer("203.0.113.5")).expect("a share");
        let _second = crowd.admit(peer("203.0.113.5")).expect("a share");
        assert!(
            crowd.admit(peer("203.0.113.5")).is_none(),
            "a third is more than one network may hold"
        );
        // Another address has its own.
        let _other = crowd.admit(peer("203.0.113.6")).expect("a share");
        assert_eq!(crowd.networks(), 2);

        // A share given back is the next connection's.
        drop(first);
        let _third = crowd
            .admit(peer("203.0.113.5"))
            .expect("the share given back");
        assert!(crowd.admit(peer("203.0.113.5")).is_none());
    }

    #[test]
    fn a_count_does_not_outlive_its_connections() {
        let crowd = Arc::new(Crowd::counting_all(3));
        let shares: Vec<_> = (0..3)
            .map(|_| crowd.admit(peer("198.51.100.7")).expect("a share"))
            .collect();
        assert_eq!(crowd.networks(), 1);
        drop(shares);
        assert_eq!(crowd.networks(), 0, "nothing is kept of a network gone");
    }

    /// One machine has a whole /64 to pick its addresses from: they are one
    /// network, as the rate limit counts them.
    #[test]
    fn an_ipv6_network_holds_one_count_between_its_addresses() {
        let crowd = Arc::new(Crowd::counting_all(2));
        let _a = crowd.admit(peer("[2001:db8:1:2::1]")).expect("a share");
        let _b = crowd
            .admit(peer("[2001:db8:1:2:ffff::9]"))
            .expect("a share");
        assert!(crowd.admit(peer("[2001:db8:1:2::3]")).is_none());
        assert!(crowd.admit(peer("[2001:db8:1:3::1]")).is_some());

        // An IPv4 client of a dual-stack socket is its IPv4 address.
        let _c = crowd.admit(peer("[::ffff:192.0.2.1]")).expect("a share");
        let _d = crowd.admit(peer("192.0.2.1")).expect("a share");
        assert!(crowd.admit(peer("[::ffff:192.0.2.1]")).is_none());
    }

    /// Everybody behind a proxy has its address, and this machine's own are
    /// the operator's: neither is counted.
    #[test]
    fn a_trusted_proxy_and_this_machine_are_not_counted() {
        let proxies: Vec<IpNet> = vec!["10.0.0.0/8".parse().unwrap(), "fd00::/16".parse().unwrap()];
        let mut crowd = Crowd::new(&proxies);
        crowd.most = 1;
        let crowd = Arc::new(crowd);
        let mut held = Vec::new();
        for address in [
            "10.1.2.3",
            "[fd00::7]",
            "127.0.0.1",
            "[::1]",
            "[::ffff:127.0.0.1]",
        ] {
            for _ in 0..5 {
                held.push(
                    crowd
                        .admit(peer(address))
                        .unwrap_or_else(|| panic!("{address} is counted")),
                );
            }
        }
        assert_eq!(crowd.networks(), 0);

        // Any other is, and holds the one.
        let _only = crowd.admit(peer("203.0.113.5")).expect("a share");
        assert!(crowd.admit(peer("203.0.113.5")).is_none());
        // The standard crowd leaves this machine out and counts the rest.
        let standard = Arc::new(Crowd::new(&[]));
        let counted: Vec<_> = (0..PER_PEER)
            .map(|_| standard.admit(peer("203.0.113.5")).expect("a share"))
            .collect();
        assert!(standard.admit(peer("203.0.113.5")).is_none());
        drop(counted);
    }

    #[tokio::test]
    async fn a_network_past_its_share_is_closed_and_the_others_are_answered() {
        let (addr, handle) = door_with(limits(), Crowd::counting_all(2)).await;

        // Two connections, as many as this network may hold: idle, open.
        let mut first = TcpStream::connect(addr).await.unwrap();
        let mut second = TcpStream::connect(addr).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The third is accepted, and closed with no word.
        let mut third = TcpStream::connect(addr).await.unwrap();
        assert!(
            closed_within(&mut third, Duration::from_secs(5)).await,
            "the door should have closed a third connection of one network"
        );
        // The two it holds are left as they were.
        assert!(open_for(&mut first, Duration::from_millis(200)).await);
        assert!(open_for(&mut second, Duration::from_millis(200)).await);

        // One given back, the next is answered.
        drop(first);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let answer = tokio::time::timeout(Duration::from_secs(5), ask(addr))
            .await
            .expect("answered");
        assert_eq!(parts(&answer).0, "HTTP/1.1 200 OK");
        handle.shutdown();
    }

    // ── a connection with nothing in flight ─────────────────────────────

    #[test]
    fn an_idle_connection_is_closed_once_it_is_quiet_and_never_later_than_twice_its_time() {
        let idle = Duration::from_secs(120);
        // Quiet: closed when its idle time is up.
        assert_eq!(closes_at(0, 0, idle), 120_000);
        // Bytes read just before: a moment more, for what is on its way.
        assert_eq!(closes_at(0, 119_900, idle), 120_900);
        // Bytes read all the time: not later than twice the time.
        assert_eq!(closes_at(0, 239_999, idle), 240_000);
        assert_eq!(closes_at(1_000, 10_000_000, idle), 241_000);
        // A short time settles in a quarter of it.
        assert_eq!(closes_at(0, 100, Duration::from_millis(400)), 400);
        assert_eq!(closes_at(0, 350, Duration::from_millis(400)), 450);
    }

    /// What HTTP/2 opens with and says next, as an idle client sends it: the
    /// preface and an empty SETTINGS frame, and a PING.
    fn h2_hello() -> Vec<u8> {
        let mut bytes = PREFACE.to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 4, 0, 0, 0, 0, 0]);
        bytes
    }

    fn h2_ping() -> [u8; 17] {
        let mut ping = [0u8; 17];
        ping[2] = 8;
        ping[3] = 6;
        ping
    }

    /// An idle HTTP/2 connection is closed whatever it answers — here, a
    /// client that pings every fifty milliseconds — within twice the idle
    /// time.
    #[tokio::test]
    async fn an_h2_connection_with_no_request_is_closed_though_it_pings() {
        let (addr, handle) = door(Limits {
            idle: Duration::from_millis(300),
            ..limits()
        })
        .await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(&h2_hello()).await.unwrap();

        let started = Instant::now();
        let mut buf = [0u8; 4096];
        let mut closed_after = None;
        while started.elapsed() < Duration::from_secs(10) {
            if stream.write_all(&h2_ping()).await.is_err() {
                closed_after = Some(started.elapsed());
                break;
            }
            if let Ok(Ok(0) | Err(_)) =
                tokio::time::timeout(Duration::from_millis(50), stream.read(&mut buf)).await
            {
                closed_after = Some(started.elapsed());
                break;
            }
        }
        let closed_after = closed_after.expect("the door should have closed the connection");
        assert!(
            closed_after >= Duration::from_millis(250),
            "closed after {closed_after:?}, before its idle time"
        );
        assert!(
            closed_after <= Duration::from_secs(5),
            "closed after {closed_after:?}"
        );
        handle.shutdown();
    }

    /// An HTTP/1 connection kept alive between two requests is closed once
    /// it has had none in flight for the idle time — here, shorter than the
    /// head's.
    #[tokio::test]
    async fn an_h1_connection_with_no_request_is_closed_after_its_idle_time() {
        let (addr, handle) = door(Limits {
            idle: Duration::from_millis(800),
            ..limits()
        })
        .await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut buf = [0u8; 1024];
        let read = stream.read(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..read]).starts_with("HTTP/1.1 200 OK"));

        // Kept alive, and then closed.
        assert!(open_for(&mut stream, Duration::from_millis(150)).await);
        assert!(
            closed_within(&mut stream, Duration::from_secs(3)).await,
            "the door should have closed the idle connection"
        );
        handle.shutdown();
    }

    /// A connection with a request in flight is not idle, however long the
    /// request takes: the answer comes, and the connection is closed after
    /// it, once the idle time has passed.
    #[tokio::test]
    async fn a_request_in_flight_keeps_its_connection_open() {
        let (addr, handle) = door(Limits {
            idle: Duration::from_millis(300),
            ..limits()
        })
        .await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /slow HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();

        // The handler takes three times the idle time to answer.
        let mut answer = Vec::new();
        let mut buf = [0u8; 1024];
        while !String::from_utf8_lossy(&answer).contains("slowly") {
            let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
                .await
                .expect("an answer")
                .expect("read");
            assert!(read > 0, "closed before it was answered");
            answer.extend_from_slice(&buf[..read]);
        }
        assert!(String::from_utf8_lossy(&answer).starts_with("HTTP/1.1 200 OK"));

        // Nothing in flight now: closed once it has been idle.
        assert!(
            closed_within(&mut stream, Duration::from_secs(3)).await,
            "the door should have closed the connection after its answer"
        );
        handle.shutdown();
    }

    // ── a response nobody reads ─────────────────────────────────────────

    /// A response a peer does not read is given up on once a write has made
    /// no headway for the write time, and the connection with it.
    #[tokio::test]
    async fn a_response_nobody_reads_is_given_up_on() {
        let (addr, handle) = door(Limits {
            write: Duration::from_millis(300),
            ..limits()
        })
        .await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /endless HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();

        // Nothing is read while the send buffers fill and the write stalls.
        tokio::time::sleep(Duration::from_millis(1200)).await;

        // What was written before is there; and then the end, long before
        // an endless body's.
        let mut got = 0usize;
        let mut buf = vec![0u8; 256 * 1024];
        let ended = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match stream.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(read) => got += read,
                }
            }
        })
        .await;
        assert!(
            ended.is_ok(),
            "the door should have given up on the response"
        );
        assert!(
            got < 64 * 1024 * 1024,
            "read {got} bytes of an endless body"
        );
        handle.shutdown();
    }

    /// A peer that reads what it is sent is not stalled, however slowly it
    /// asks for the rest: the clock runs only while a write cannot go through.
    #[tokio::test]
    async fn a_response_that_is_read_is_not_given_up_on() {
        let (addr, handle) = door(Limits {
            write: Duration::from_secs(1),
            ..limits()
        })
        .await;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /endless HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 256 * 1024];
        let mut got = 0usize;
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(2500) {
            let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
                .await
                .expect("bytes keep coming")
                .expect("read");
            assert!(read > 0, "the door closed a connection that was reading");
            got += read;
        }
        assert!(got > 1024 * 1024, "read {got} bytes");
        handle.shutdown();
    }

    /// The service's requests count as in flight until the response's last
    /// byte, and the body is the router's own: its length is still told.
    #[tokio::test]
    async fn the_body_keeps_its_length_and_its_request_counts_until_it_is_dropped() {
        let life = Arc::new(Life::new());
        let open = life.begin();
        assert_eq!(life.open(), 1);
        let counted = Counted {
            body: axum::body::Body::from("twelve bytes"),
            _open: open,
        };
        assert_eq!(counted.size_hint().exact(), Some(12));
        assert!(!counted.is_end_stream());
        assert_eq!(life.open(), 1);
        let before = life.idle_since();
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(counted);
        assert_eq!(life.open(), 0);
        assert!(life.idle_since() > before, "idle from the moment it ended");

        let empty = Counted {
            body: axum::body::Body::empty(),
            _open: life.begin(),
        };
        assert!(empty.is_end_stream());
        drop(empty);
        assert_eq!(life.open(), 0);
    }
}
