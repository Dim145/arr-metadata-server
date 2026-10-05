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
//! * hyper is given a clock, and with it `AMS_HEADER_READ_TIMEOUT` for a
//!   request's head — the first, and each one a connection kept alive waits
//!   for — and a head no larger than a head needs to be;
//! * the bytes before that, which tell HTTP/2 from HTTP/1 and which hyper
//!   does not time, are given the same;
//! * an idle HTTP/2 connection is pinged, and closed when it stops answering.

use std::{
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use axum::{Router, extract::Request};
use axum_server::{
    AddrListener, Address, Handle, Server,
    accept::{Accept, DefaultAcceptor},
    tls_rustls::{RustlsAcceptor, RustlsConfig},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore},
};

/// What every door is held to.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Connections at once.
    pub connections: usize,
    /// How long a connection has to say what it wants.
    pub head: Duration,
}

/// Connections a door bound by address alone holds; the configured doors
/// are given theirs.
const DEFAULT_CONNECTIONS: usize = 1024;

/// The most a request's head may take in a connection's buffer. A head is
/// a few kilobytes; hyper's default lets each connection hold four hundred
/// before refusing one, which a door full of slow connections turns into
/// memory the container does not have.
const HEAD_BUFFER: usize = 64 * 1024;

/// How often an idle HTTP/2 connection is asked whether it is still there,
/// and how long it has to answer.
const KEEP_ALIVE: Duration = Duration::from_secs(30);
const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// The address a connection came from, as the doors hand it on.
#[derive(Clone, Copy, Debug)]
pub struct Peer(pub SocketAddr);

impl Address for Peer {
    type Stream = Held;
    type Listener = Door;
}

/// What the handlers read as `ConnectInfo<SocketAddr>`: the peer's address,
/// as before.
impl axum::extract::connect_info::Connected<Peer> for SocketAddr {
    fn connect_info(peer: Peer) -> Self {
        peer.0
    }
}

/// A listener that holds no more connections than its share.
pub struct Door {
    listener: TcpListener,
    connections: usize,
    places: Arc<Semaphore>,
    /// When the door last said it was full, in seconds after [`EPOCH`] plus
    /// one; nought for never.
    said_full: AtomicU64,
}

static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

impl Door {
    pub fn new(listener: TcpListener, connections: usize) -> Self {
        let connections = connections.max(1);
        Self {
            listener,
            connections,
            places: Arc::new(Semaphore::new(connections)),
            said_full: AtomicU64::new(0),
        }
    }

    /// Said once a minute at most: a door that is full stays full a while.
    fn say_full(&self) {
        let now = EPOCH.elapsed().as_secs() + 1;
        let last = self.said_full.load(Ordering::Relaxed);
        if (last == 0 || now.saturating_sub(last) >= 60)
            && self
                .said_full
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            tracing::warn!(
                connections = self.connections,
                "a door holds as many connections as AMS_MAX_CONNECTIONS allows; new ones wait \
                 until one closes"
            );
        }
    }
}

impl AddrListener<Held, Peer> for Door {
    async fn bind_to(addr: Peer) -> io::Result<Self> {
        Ok(Self::new(
            TcpListener::bind(addr.0).await?,
            DEFAULT_CONNECTIONS,
        ))
    }

    /// A place first, then the connection: past the limit nothing more is
    /// accepted, so nothing more holds a descriptor.
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
        let (stream, addr) = self.listener.accept().await?;
        Ok((
            Held {
                stream,
                _place: place,
            },
            Peer(addr),
        ))
    }

    fn get_local_addr(&self) -> io::Result<Peer> {
        self.listener.local_addr().map(Peer)
    }
}

/// An accepted connection, holding its place at the door until it closes.
pub struct Held {
    stream: TcpStream,
    _place: OwnedSemaphorePermit,
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

/// Gives a connection's first bytes — after TLS, where there is any — the
/// time a request's head is given.
#[derive(Clone)]
pub struct Prompt<A> {
    inner: A,
    within: Duration,
}

impl<A, I, S> Accept<I, S> for Prompt<A>
where
    A: Accept<I, S>,
    A::Future: Send + 'static,
    A::Stream: Send + 'static,
    A::Service: Send + 'static,
{
    type Stream = Timed<A::Stream>;
    type Service = A::Service;
    type Future = Pin<Box<dyn Future<Output = io::Result<(Self::Stream, Self::Service)>> + Send>>;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let accepted = self.inner.accept(stream, service);
        let within = self.within;
        Box::pin(async move {
            let (stream, service) = accepted.await?;
            Ok((Timed::new(stream, within), service))
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
    let service =
        axum::ServiceExt::<Request>::into_make_service_with_connect_info::<SocketAddr>(router);
    match tls {
        Some(config) => {
            let mut server = Server::<Peer>::from_listener(door)
                .acceptor(Prompt {
                    inner: RustlsAcceptor::new(config),
                    within: limits.head,
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

    /// A door serving a single route, with the limits given; its address.
    async fn door(limits: Limits) -> (SocketAddr, Handle<Peer>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new().route("/", axum::routing::get(|| async { "here" }));
        let handle = Handle::new();
        tokio::spawn(serve(
            Door::new(listener, limits.connections),
            None,
            router,
            handle.clone(),
            limits,
        ));
        (addr, handle)
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

    #[tokio::test]
    async fn a_door_answers_through_its_limits() {
        let (addr, handle) = door(Limits {
            connections: 4,
            head: Duration::from_secs(5),
        })
        .await;
        let answer = ask(addr).await;
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert!(answer.ends_with("here"), "{answer}");
        handle.shutdown();
    }

    #[tokio::test]
    async fn a_connection_that_says_nothing_is_closed() {
        let (addr, handle) = door(Limits {
            connections: 4,
            head: Duration::from_millis(200),
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
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
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
}
