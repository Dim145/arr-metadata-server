//! Stopping a run that can be stopped.
//!
//! A long run started by hand — refreshing every work — checks a flag between
//! works; asking it to stop sets the flag, and it ends at the next work, its
//! record saying how far it got. A run that cannot stop partway, an import
//! or an export, never registers a flag, and is not offered a stop.

use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

static FLAGS: LazyLock<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The flag a run checks, registered under its job id. Dropped with
/// [`Registered`], when the run ends however it ends.
pub fn register(job_id: &str) -> Registered {
    let flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut flags) = FLAGS.lock() {
        flags.insert(job_id.to_string(), flag.clone());
    }
    Registered {
        job_id: job_id.to_string(),
        flag,
    }
}

pub struct Registered {
    job_id: String,
    flag: Arc<AtomicBool>,
}

impl Registered {
    /// Whether somebody asked this run to stop.
    pub fn stopped(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

impl Drop for Registered {
    fn drop(&mut self) {
        if let Ok(mut flags) = FLAGS.lock() {
            flags.remove(&self.job_id);
        }
    }
}

pub fn is_cancelable(job_id: &str) -> bool {
    FLAGS.lock().is_ok_and(|flags| flags.contains_key(job_id))
}

/// Ask a run to stop. False when there is no such run, or it cannot stop.
pub fn cancel(job_id: &str) -> bool {
    FLAGS
        .lock()
        .ok()
        .and_then(|flags| flags.get(job_id).cloned())
        .map(|flag| flag.store(true, Ordering::Release))
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_is_stopped_while_it_runs_and_forgotten_after() {
        let run = register("job-under-test");
        assert!(is_cancelable("job-under-test"));
        assert!(!run.stopped());

        assert!(cancel("job-under-test"));
        assert!(run.stopped());

        drop(run);
        assert!(!is_cancelable("job-under-test"));
        assert!(!cancel("job-under-test"));
    }
}
