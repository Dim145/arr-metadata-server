//! What is kept for a few seconds, in this process's memory alone.
//!
//! Some questions anybody who can reach the server may ask as often as they
//! like, and whose answers move at the pace of the catalogue, not of whoever
//! asks: whether the database answers, what mark the catalogue bears, what a
//! link's preview says. Asked of the database afresh by each of ten thousand
//! requests they are a way to keep it busy; kept a few seconds, the ten
//! thousand ask it once.
//!
//! Not a space of the cache: nothing here is shared with another instance,
//! nothing is behind the switches the spaces have, and nothing outlives the
//! seconds it is given. It is what stands between a flood and the database,
//! not what makes a page quick.

use std::{future::Future, sync::Arc, time::Duration};

use moka::future::Cache as Moka;

/// Values kept `ttl` at the most, by key, no more than `capacity` of them.
pub struct Brief<V> {
    kept: Moka<String, V>,
}

impl<V: Clone + Send + Sync + 'static> Brief<V> {
    pub fn new(capacity: u64, ttl: Duration) -> Self {
        Self {
            kept: Moka::builder()
                .max_capacity(capacity.max(1))
                .time_to_live(ttl)
                .build(),
        }
    }

    pub async fn get(&self, key: &str) -> Option<V> {
        self.kept.get(key).await
    }

    pub async fn insert(&self, key: String, value: V) {
        self.kept.insert(key, value).await;
    }

    /// The value kept under `key`, or what `init` makes of it — run once for
    /// every caller that asks while it runs, the others waiting for its answer.
    pub async fn get_with(&self, key: String, init: impl Future<Output = V>) -> V {
        self.kept.get_with(key, init).await
    }

    /// [`Self::get_with`] for a computation that may fail: nothing is kept
    /// of a failure, and the callers that waited for it are all told of it.
    pub async fn try_get_with<E>(
        &self,
        key: String,
        init: impl Future<Output = Result<V, E>>,
    ) -> Result<V, Arc<E>>
    where
        E: Send + Sync + 'static,
    {
        self.kept.try_get_with(key, init).await
    }

    /// Forget everything kept.
    pub fn forget(&self) {
        self.kept.invalidate_all();
    }

    /// How many values are kept now: what a test counts.
    #[cfg(test)]
    pub async fn entries(&self) -> u64 {
        self.kept.run_pending_tasks().await;
        self.kept.entry_count()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn a_value_is_kept_for_its_time_and_no_longer() {
        let brief: Brief<u32> = Brief::new(8, Duration::from_millis(300));
        assert_eq!(brief.get("a").await, None);
        brief.insert("a".into(), 1).await;
        assert_eq!(brief.get("a").await, Some(1));

        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(brief.get("a").await, None, "a few seconds, not for good");

        brief.insert("b".into(), 2).await;
        brief.forget();
        assert_eq!(brief.get("b").await, None);
    }

    /// Ten callers asking at once make one computation: what a flood is
    /// kept from being.
    #[tokio::test]
    async fn callers_asking_together_wait_for_one_computation() {
        let brief: Arc<Brief<u32>> = Arc::new(Brief::new(8, Duration::from_secs(5)));
        let runs = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..10 {
            let (brief, runs) = (brief.clone(), runs.clone());
            tasks.push(tokio::spawn(async move {
                brief
                    .get_with("k".into(), async {
                        runs.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        7
                    })
                    .await
            }));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap(), 7);
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // Kept after: asked again, nothing runs.
        let again = brief
            .get_with("k".into(), async {
                runs.fetch_add(1, Ordering::SeqCst);
                8
            })
            .await;
        assert_eq!((again, runs.load(Ordering::SeqCst)), (7, 1));
    }

    #[tokio::test]
    async fn a_failure_is_told_and_not_kept() {
        let brief: Brief<u32> = Brief::new(8, Duration::from_secs(5));
        let failed = brief
            .try_get_with("k".into(), async { Err::<u32, _>(anyhow::anyhow!("down")) })
            .await;
        assert!(failed.is_err());
        assert_eq!(brief.get("k").await, None);
        let answered = brief
            .try_get_with("k".into(), async { Ok::<_, anyhow::Error>(3) })
            .await
            .unwrap();
        assert_eq!(answered, 3);
    }
}
