//! The loops that run for as long as the process does, kept.
//!
//! The schedules, the media worker and its sweeps, the cache server's
//! heartbeat and channel, the lease among several instances, the
//! certificate's renewals: each is a loop started once, at the start. Let
//! go of, one that panicked — a release unwinds — or returned stopped there,
//! in silence, until the process was restarted: no sweep, no renewal, and
//! nothing in the log to say so. Kept here instead, an end is said in the log
//! and the loop started again: a second later the first time, then two, four
//! and so on, five minutes apart at the most. A loop that ran that long
//! before it failed starts the count over.
//!
//! The stop is the one end that is not a failure. Once the process is asked
//! to stop, nothing is started again and a pause before a start is cut
//! short; and once the doors are closed, what still runs is ended — with
//! whatever it was in the middle of — before the database is closed.

use std::{any::Any, future::Future, sync::Arc, time::Duration};

use tokio::{
    sync::watch,
    task::JoinSet,
    time::{Instant, sleep},
};

/// What a loop's own return means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ends {
    /// Nothing short of the stop: a return is a failure as much as a panic
    /// is, and the loop is started again.
    Never,
    /// That it has nothing to do here — no media store, one instance alone,
    /// the cache server attached: left at that. A panic is still a failure.
    WhenDone,
}

/// Where the process stands, as every kept loop hears it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Running,
    /// Asked to stop: nothing is started again.
    Stopping,
    /// The doors are closed: what still runs is ended.
    Ending,
}

/// The pauses between two starts of a loop that keeps failing.
#[derive(Clone, Copy, Debug)]
struct Pauses {
    first: Duration,
    longest: Duration,
}

impl Pauses {
    const STANDARD: Self = Self {
        first: Duration::from_secs(1),
        longest: Duration::from_secs(300),
    };

    /// The pause after `failures` failures in a row, the first being `0`:
    /// doubled each time, and never longer than the longest.
    fn after(self, failures: u32) -> Duration {
        self.first
            .saturating_mul(2u32.saturating_pow(failures))
            .min(self.longest)
    }
}

/// The loops kept, and the word that the process is stopping. A clone is
/// the same: a kept loop may keep others — the cache server's, once it
/// answers.
#[derive(Clone)]
pub struct Supervisor(Arc<Inner>);

struct Inner {
    kept: parking_lot::Mutex<JoinSet<()>>,
    phase: watch::Sender<Phase>,
    pauses: Pauses,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Supervisor {
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            kept: parking_lot::Mutex::new(JoinSet::new()),
            phase: watch::Sender::new(Phase::Running),
            pauses: Pauses::STANDARD,
        }))
    }

    /// Keep a loop: `start` makes it, now and each time it has to be started
    /// again. `name` is what the log calls it. Nothing is started once the
    /// stop is said.
    pub fn keep<F, Fut>(&self, name: &'static str, ends: Ends, start: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        if *self.0.phase.borrow() != Phase::Running {
            return;
        }
        let watched = watch_over(
            name,
            ends,
            Arc::new(start),
            self.0.phase.subscribe(),
            self.0.pauses,
        );
        let mut kept = self.0.kept.lock();
        // What has ended for good — a loop with nothing to do here — is let
        // go of, rather than held until the stop.
        while kept.try_join_next().is_some() {}
        kept.spawn(watched);
    }

    /// The process is stopping: whatever ends from now on is the stop's
    /// doing, and is not started again.
    pub fn stop(&self) {
        self.0.phase.send_if_modified(|phase| {
            let running = *phase == Phase::Running;
            if running {
                *phase = Phase::Stopping;
            }
            running
        });
    }

    /// End what still runs, and return once all of it has ended.
    pub async fn shutdown(&self) {
        self.0.phase.send_replace(Phase::Ending);
        let mut kept = std::mem::take(&mut *self.0.kept.lock());
        while kept.join_next().await.is_some() {}
    }
}

/// One kept loop: started, watched, and started again after its pause for as
/// long as the process runs.
async fn watch_over<F, Fut>(
    name: &'static str,
    ends: Ends,
    start: Arc<F>,
    mut phase: watch::Receiver<Phase>,
    pauses: Pauses,
) where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut failures = 0u32;
    loop {
        let started = Instant::now();
        // In a set of its own: this watch ended, the loop ends with it,
        // rather than running on with nobody watching.
        let mut run = JoinSet::new();
        let make = start.clone();
        run.spawn(async move { make().await });

        let ended = tokio::select! {
            ended = run.join_next() => ended,
            () = until(&mut phase, |phase| *phase == Phase::Ending) => {
                run.shutdown().await;
                return;
            }
        };
        if *phase.borrow() != Phase::Running {
            return;
        }

        let why = match ended {
            Some(Ok(())) if ends == Ends::WhenDone => {
                tracing::debug!(task = name, "a background task has nothing to do here");
                return;
            }
            Some(Ok(())) => "it returned".to_string(),
            Some(Err(e)) if e.is_panic() => format!("it panicked: {}", said(e.into_panic())),
            // Ended from outside, which only the stop does.
            Some(Err(_)) | None => return,
        };

        if started.elapsed() >= pauses.longest {
            failures = 0;
        }
        let pause = pauses.after(failures);
        failures = failures.saturating_add(1);
        tracing::error!(
            task = name,
            why = %why,
            again_in_seconds = pause.as_secs_f64(),
            "a background task ended; it is started again"
        );

        tokio::select! {
            () = sleep(pause) => {}
            () = until(&mut phase, |phase| *phase != Phase::Running) => return,
        }
    }
}

/// Until the process reaches a phase — or nothing can say so any more.
/// What the channel lends is given back before this returns: held across
/// the wait that follows, it would hold up the word of the next phase.
async fn until(phase: &mut watch::Receiver<Phase>, reached: impl FnMut(&Phase) -> bool) {
    let _ = phase.wait_for(reached).await;
}

/// What a panic said, when it said it in words.
fn said(payload: Box<dyn Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    /// A loop that counts its starts, made by `body` from the count it was
    /// started at.
    fn counted<B, Fut>(
        body: B,
    ) -> (
        Arc<AtomicUsize>,
        impl Fn() -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync + 'static,
    )
    where
        B: Fn(usize) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let starts = Arc::new(AtomicUsize::new(0));
        let body = Arc::new(body);
        let counter = starts.clone();
        let start = move || {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            Box::pin(async move { body(n).await })
                as std::pin::Pin<Box<dyn Future<Output = ()> + Send>>
        };
        (starts, start)
    }

    /// Long enough for whatever was woken to have run, on a paused clock.
    async fn settle() {
        sleep(Duration::from_millis(10)).await;
    }

    #[test]
    fn the_pause_doubles_up_to_five_minutes() {
        let pauses = Pauses::STANDARD;
        let seconds: Vec<u64> = (0..12).map(|n| pauses.after(n).as_secs()).collect();
        assert_eq!(seconds, [1, 2, 4, 8, 16, 32, 64, 128, 256, 300, 300, 300]);
        assert_eq!(pauses.after(u32::MAX), Duration::from_secs(300));
    }

    #[tokio::test(start_paused = true)]
    async fn a_loop_that_panics_is_started_again_after_its_pause() {
        let supervisor = Supervisor::new();
        let (starts, start) = counted(|n| async move {
            if n < 2 {
                panic!("a test loop panicking on purpose, start {n}");
            }
            std::future::pending::<()>().await;
        });
        supervisor.keep("a test loop", Ends::Never, start);

        settle().await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        // A second, then two: the pause doubles.
        sleep(Duration::from_secs(1)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        sleep(Duration::from_secs(1)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        sleep(Duration::from_secs(1)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 3);
        // Running now, and left to run.
        sleep(Duration::from_secs(600)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 3);

        supervisor.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_return_is_a_failure_only_for_a_loop_that_never_ends() {
        let supervisor = Supervisor::new();
        let (forever, start) = counted(|_| async {});
        supervisor.keep("a loop that should not return", Ends::Never, start);
        let (done, start) = counted(|_| async {});
        supervisor.keep("a loop with nothing to do", Ends::WhenDone, start);

        settle().await;
        sleep(Duration::from_secs(3)).await;
        // Started at once, after a second, and after two more.
        assert_eq!(forever.load(Ordering::SeqCst), 3);
        assert_eq!(done.load(Ordering::SeqCst), 1);

        supervisor.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn nothing_is_started_again_once_the_stop_is_said() {
        let supervisor = Supervisor::new();
        let release = Arc::new(tokio::sync::Notify::new());
        let (starts, start) = counted({
            let release = release.clone();
            move |_| {
                let release = release.clone();
                async move { release.notified().await }
            }
        });
        supervisor.keep("a loop that ends at the stop", Ends::Never, start);
        settle().await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        // The stop is said, and the loop ends as a loop does at the stop.
        supervisor.stop();
        release.notify_waiters();
        sleep(Duration::from_secs(600)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        // Nor is anything kept after it started at all.
        let (late, start) = counted(|_| async {});
        supervisor.keep("a loop kept too late", Ends::Never, start);
        settle().await;
        assert_eq!(late.load(Ordering::SeqCst), 0);

        supervisor.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_pause_is_cut_short_by_the_stop() {
        let supervisor = Supervisor::new();
        let (starts, start) = counted(|_| async {});
        supervisor.keep("a loop between two starts", Ends::Never, start);
        settle().await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        supervisor.stop();
        sleep(Duration::from_secs(600)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        supervisor.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_shutdown_ends_what_still_runs() {
        struct Ended(Arc<AtomicBool>);
        impl Drop for Ended {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let supervisor = Supervisor::new();
        let ended = Arc::new(AtomicBool::new(false));
        let (starts, start) = counted({
            let ended = ended.clone();
            move |_| {
                let ended = ended.clone();
                async move {
                    let _ended = Ended(ended);
                    std::future::pending::<()>().await;
                }
            }
        });
        supervisor.keep("a loop that runs until it is ended", Ends::Never, start);
        settle().await;
        assert!(!ended.load(Ordering::SeqCst));

        supervisor.shutdown().await;
        // Ended, with what it held let go of — and not started again.
        assert!(ended.load(Ordering::SeqCst));
        sleep(Duration::from_secs(600)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_loop_that_ran_a_while_starts_its_pauses_over() {
        let supervisor = Supervisor::new();
        // Fails at once twice, then after ten minutes of running.
        let (starts, start) = counted(|n| async move {
            if n == 2 {
                sleep(Duration::from_secs(600)).await;
            }
            panic!("a test loop panicking on purpose, start {n}");
        });
        supervisor.keep("a loop that fails now and then", Ends::Never, start);

        settle().await;
        sleep(Duration::from_secs(3)).await;
        // At once, after a second, after two more: the third is running.
        assert_eq!(starts.load(Ordering::SeqCst), 3);
        sleep(Duration::from_secs(600)).await;
        // It failed after its ten minutes and is started a second later,
        // not four.
        sleep(Duration::from_secs(1)).await;
        assert_eq!(starts.load(Ordering::SeqCst), 4);

        supervisor.shutdown().await;
    }
}
