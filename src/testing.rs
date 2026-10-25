//! Places of their own for the tests, in the system's temporary directory.
//!
//! The one place the tests name a path there: each under an id nobody can
//! guess, so that two runs, or two tests of one run, never share one, and
//! no other user of the machine can have made it first. What the server
//! itself makes there goes through `backup::Work`, which makes it privately.

use std::path::PathBuf;

/// A path nothing holds yet, `ams-<label>-<id>`, for a test that makes the
/// file or the directory itself — or leaves it unmade, as a server does when
/// nothing is archived.
pub fn scratch_path(label: &str) -> PathBuf {
    // nosemgrep: rust.lang.security.temp-dir.temp-dir
    std::env::temp_dir().join(format!("ams-{label}-{}", uuid::Uuid::now_v7()))
}

/// A directory of its own, made.
pub fn scratch_dir(label: &str) -> PathBuf {
    let dir = scratch_path(label);
    std::fs::create_dir_all(&dir).expect("a test's own directory");
    dir
}
