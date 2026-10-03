//! On-the-wire formats of the clients this server impersonates.
//!
//! Each submodule owns one client's vocabulary and converts in both directions:
//! canonical → wire to answer a request, wire → canonical to absorb a fallback
//! response from the real upstream. Keeping both directions together means the
//! field mapping is stated once.

pub mod radarr;
pub mod sonarr;

use serde::Deserialize;
use serde_json::Value;

/// A text the upstream may leave out or write as `null`, read as empty.
///
/// For the fields this server always writes, because a client calls a method
/// on them without a null check, but must still read from an upstream reply
/// that has none: Radarr's original language, the title of an episode Skyhook
/// has no name for.
pub(crate) fn string_or_null<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// A list the upstream may leave out or write as `null`, read entry by entry:
/// an entry this server cannot read is left out, not the answer it came in.
///
/// Skyhook leaves out whatever it has nothing for, and one episode it has no
/// name for, read as a missing field, used to fail the whole series — its
/// broadcast instants, its anime ids and its episodes with it.
pub(crate) fn readable_entries<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let entries = match Value::deserialize(deserializer)? {
        Value::Array(entries) => entries,
        Value::Null => Vec::new(),
        _ => {
            tracing::warn!(
                "the upstream sent something other than a list where one belongs; read as none"
            );
            Vec::new()
        }
    };

    let mut read = Vec::with_capacity(entries.len());
    let mut unreadable = 0usize;
    let mut first = None;
    for entry in entries {
        match serde_json::from_value(entry) {
            Ok(entry) => read.push(entry),
            Err(e) => {
                unreadable += 1;
                first.get_or_insert(e);
            }
        }
    }
    if let Some(error) = first {
        tracing::warn!(unreadable, %error, "left out entries the upstream sent that could not be read");
    }

    Ok(read)
}

/// A value the upstream may leave out, write as `null` or write in a shape
/// this server cannot read: none, rather than the answer failing with it.
pub(crate) fn readable_or_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }

    match serde_json::from_value(value) {
        Ok(read) => Ok(Some(read)),
        Err(error) => {
            tracing::warn!(%error, "left out a value the upstream sent that could not be read");
            Ok(None)
        }
    }
}
