//! Putting a name to an address.
//!
//! An address in a refusal log is not enough to act on. `172.31.0.4` tells you
//! nothing; `sonarr` tells you which container to fix. Two sources answer that,
//! and between them they cover how people actually run this:
//!
//! * the **hosts file**, which carries whatever the operator pinned there —
//!   `extra_hosts` in a compose file lands here, and so does anything a
//!   hand-managed deployment wrote;
//! * the **system resolver**, which inside a user-defined Docker network is
//!   Docker's own and answers reverse lookups with the container's name.
//!
//! mDNS is deliberately not among them. It would mean a listener, a multicast
//! socket and a dependency, to answer for a class of host — an Apple device
//! announcing itself on a LAN — that does not call a metadata server.
//!
//! Both sources are best effort and neither blocks a request: a name is a
//! convenience for a person reading a table later.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Where a hosts file lives on the platforms this server runs on.
const HOSTS_FILE: &str = "/etc/hosts";

/// How long a resolved name is trusted. Containers keep their addresses for as
/// long as they live, and a name that is an hour stale is still the right one
/// far more often than it is wrong.
const TTL: Duration = Duration::from_secs(3600);

#[derive(Clone)]
pub struct Resolver {
    seen: Arc<Mutex<HashMap<IpAddr, Entry>>>,
}

#[derive(Clone)]
struct Entry {
    name: Option<String>,
    at: Instant,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver {
    pub fn new() -> Self {
        Self {
            seen: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// A cached name, if one was looked up recently enough.
    pub fn cached(&self, ip: IpAddr) -> Option<String> {
        let seen = self.seen.lock().ok()?;
        let entry = seen.get(&ip)?;

        (entry.at.elapsed() < TTL)
            .then(|| entry.name.clone())
            .flatten()
    }

    /// Whether this address is due a lookup.
    pub fn is_stale(&self, ip: IpAddr) -> bool {
        match self.seen.lock() {
            Ok(seen) => seen.get(&ip).is_none_or(|entry| entry.at.elapsed() >= TTL),
            // A poisoned lock is not a reason to resolve on every request.
            Err(_) => false,
        }
    }

    /// Look the address up and remember the answer, including a miss.
    ///
    /// Blocking — `getnameinfo` is a synchronous call that can wait on a
    /// network round trip — so callers hand it to a blocking thread rather than
    /// holding a request open on it.
    pub fn resolve(&self, ip: IpAddr) -> Option<String> {
        let name = from_hosts_file(ip).or_else(|| reverse_dns(ip));

        if let Ok(mut seen) = self.seen.lock() {
            seen.insert(
                ip,
                Entry {
                    name: name.clone(),
                    at: Instant::now(),
                },
            );

            // An unbounded map fed by whoever can reach the port is a slow leak.
            if seen.len() > 4096 {
                let cutoff = Instant::now();
                seen.retain(|_, entry| cutoff.duration_since(entry.at) < TTL);
            }
        }

        name
    }
}

/// The first name the hosts file gives this address.
fn from_hosts_file(ip: IpAddr) -> Option<String> {
    let contents = std::fs::read_to_string(HOSTS_FILE).ok()?;

    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        let mut parts = line.split_whitespace();
        let Some(address) = parts.next().and_then(|a| a.parse::<IpAddr>().ok()) else {
            continue;
        };

        if address == ip
            && let Some(name) = parts.next()
        {
            return Some(name.to_string());
        }
    }

    None
}

/// What the system resolver calls this address.
fn reverse_dns(ip: IpAddr) -> Option<String> {
    let name = dns_lookup::lookup_addr(&ip).ok()?;

    // A resolver with nothing to say hands back the address it was given.
    (name != ip.to_string()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_is_named_by_the_hosts_file() {
        // Every platform this runs on names 127.0.0.1 there. If it does not,
        // the parser is what is wrong, not the expectation.
        let name = from_hosts_file("127.0.0.1".parse().unwrap());

        assert!(
            name.as_deref().is_some_and(|n| n.contains("localhost")),
            "expected a localhost entry, got {name:?}",
        );
    }

    #[test]
    fn an_address_nobody_pinned_has_no_hosts_entry() {
        assert_eq!(from_hosts_file("203.0.113.199".parse().unwrap()), None);
    }

    #[test]
    fn a_name_is_remembered_and_a_miss_is_remembered_too() {
        let resolver = Resolver::new();
        let ip: IpAddr = "203.0.113.42".parse().unwrap();

        assert!(resolver.is_stale(ip), "nothing looked up yet");

        resolver.resolve(ip);

        // Either way the address is no longer due a lookup: remembering that
        // there is no name is what stops a miss being retried on every request.
        assert!(!resolver.is_stale(ip));
    }
}
