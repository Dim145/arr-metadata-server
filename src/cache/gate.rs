//! A few places for work that costs more than it takes to ask for.
//!
//! A search nobody has made before, the facets of a filter nobody has chosen,
//! a picture handed on from TMDB: each takes a request's worth of effort to
//! ask and a good deal more to answer, and the answer is kept only once it
//! has been made. Asked faster than they are answered, they would take every
//! connection the database has, and the pages everybody reads with them.
//! A gate lets a few through at once; the others wait their turn for a while,
//! and are turned away — told when to come back — once they have waited
//! longer than anyone should.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub struct Gate {
    places: Arc<Semaphore>,
    /// How long a caller waits for a place, in milliseconds.
    wait: AtomicU64,
}

impl Gate {
    pub fn new(places: usize, wait: Duration) -> Self {
        Self {
            places: Arc::new(Semaphore::new(places.max(1))),
            wait: AtomicU64::new(millis(wait)),
        }
    }

    /// A place, held until it is dropped, or none once every place has stayed
    /// taken for as long as a caller waits.
    pub async fn take(&self) -> Option<OwnedSemaphorePermit> {
        if let Ok(place) = self.places.clone().try_acquire_owned() {
            return Some(place);
        }
        let wait = Duration::from_millis(self.wait.load(Ordering::Relaxed));
        match tokio::time::timeout(wait, self.places.clone().acquire_owned()).await {
            Ok(Ok(place)) => Some(place),
            // Waited in vain; or the semaphore is closed, which nothing here does.
            _ => None,
        }
    }

    /// How many places are free now: what a test counts.
    #[cfg(test)]
    pub fn free(&self) -> usize {
        self.places.available_permits()
    }

    /// Wait for as long as `wait` from now on: what a test shortens, so as
    /// not to wait out the seconds a caller is given.
    #[cfg(test)]
    pub fn wait_at_most(&self, wait: Duration) {
        self.wait.store(millis(wait), Ordering::Relaxed);
    }
}

fn millis(wait: Duration) -> u64 {
    u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[tokio::test]
    async fn a_gate_lets_so_many_through_and_gives_a_place_back() {
        let gate = Gate::new(2, Duration::from_millis(40));
        let first = gate.take().await.expect("a place");
        let second = gate.take().await.expect("a place");
        assert_eq!(gate.free(), 0);

        // Every place taken: the caller waits its time, then is turned away.
        let started = Instant::now();
        assert!(gate.take().await.is_none());
        assert!(started.elapsed() >= Duration::from_millis(30));

        // A place given back is the next caller's.
        drop(first);
        assert_eq!(gate.free(), 1);
        let third = gate.take().await.expect("the place given back");
        assert_eq!(gate.free(), 0);
        drop((second, third));
        assert_eq!(gate.free(), 2);
    }

    /// A caller that waits is let in as soon as a place is free, not when
    /// its time is up.
    #[tokio::test]
    async fn a_caller_that_waits_is_let_in_when_a_place_is_free() {
        let gate = Arc::new(Gate::new(1, Duration::from_secs(5)));
        let held = gate.take().await.expect("a place");
        let waiting = tokio::spawn({
            let gate = gate.clone();
            async move { gate.take().await.is_some() }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished());
        drop(held);
        let in_time = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .expect("let in once the place was free")
            .unwrap();
        assert!(in_time);
    }
}
