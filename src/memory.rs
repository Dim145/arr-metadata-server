//! What the process holds in memory, and what its container is charged for
//! it, read when the scrape comes.
//!
//! Three readings, each telling apart what the one before cannot:
//!
//! * the bytes the program asked its allocator for and still holds, counted
//!   by the allocator below as they come and go: the program's own data;
//! * what the kernel keeps resident for the process (`/proc/self/status`):
//!   that data, what the allocator keeps beside it for reuse, the threads'
//!   stacks, and the pages of the files it maps — its binary among them;
//! * what the process's cgroup is charged (`memory.stat`): the container's
//!   account, page cache included, which is what `docker stats` shows —
//!   `memory.current` less `inactive_file`.
//!
//! The last two exist on Linux alone; elsewhere they are left out.

use std::{
    alloc::{GlobalAlloc, Layout},
    cell::Cell,
    path::{Path, PathBuf},
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering::Relaxed},
};

/// The allocator: mimalloc, which serves a server of many small,
/// short-lived allocations across many threads faster and with less
/// fragmentation than the system's — with what the program holds of it
/// counted on the way.
pub struct Counting;

/// The count is kept in shards, each on its own cache line, and a thread
/// adds to its own: threads allocating at once do not fight over one line.
const SHARDS: usize = 32;

#[repr(align(128))]
struct Shard(AtomicIsize);

static HELD: [Shard; SHARDS] = [const { Shard(AtomicIsize::new(0)) }; SHARDS];
static NEXT_SHARD: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// This thread's shard, given at its first allocation. A constant
    /// without a destructor: reading it allocates nothing, and does not fail
    /// while the thread ends.
    static SHARD: Cell<usize> = const { Cell::new(usize::MAX) };
}

#[inline]
fn count(delta: isize) {
    let shard = SHARD.with(|shard| {
        let mut at = shard.get();
        if at == usize::MAX {
            at = NEXT_SHARD.fetch_add(1, Relaxed) % SHARDS;
            shard.set(at);
        }
        at
    });
    HELD[shard].0.fetch_add(delta, Relaxed);
}

// SAFETY: every call is mimalloc's, with the arguments it was given; the
// count beside it allocates nothing. A layout's size never exceeds
// `isize::MAX`, so it converts without loss.
unsafe impl GlobalAlloc for Counting {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let at = unsafe { mimalloc::MiMalloc.alloc(layout) };
        if !at.is_null() {
            count(layout.size() as isize);
        }
        at
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let at = unsafe { mimalloc::MiMalloc.alloc_zeroed(layout) };
        if !at.is_null() {
            count(layout.size() as isize);
        }
        at
    }

    #[inline]
    unsafe fn dealloc(&self, at: *mut u8, layout: Layout) {
        unsafe { mimalloc::MiMalloc.dealloc(at, layout) };
        count(-(layout.size() as isize));
    }

    #[inline]
    unsafe fn realloc(&self, at: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { mimalloc::MiMalloc.realloc(at, layout, new_size) };
        // A failed realloc leaves the old block where it was, still held.
        if !moved.is_null() {
            count(new_size as isize - layout.size() as isize);
        }
        moved
    }
}

/// The bytes the program asked its allocator for and still holds: its own
/// data, without what the allocator keeps beside it for reuse.
pub fn heap_bytes() -> u64 {
    let held: isize = HELD.iter().map(|shard| shard.0.load(Relaxed)).sum();
    u64::try_from(held).unwrap_or(0)
}

/// What the kernel keeps resident for the process, in bytes, and the
/// threads it runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resident {
    pub total: u64,
    /// The most it ever held.
    pub peak: u64,
    /// Its own memory: the allocator's, the stacks.
    pub anonymous: u64,
    /// The pages of the files it maps, its binary among them.
    pub file: u64,
    pub shared: u64,
    pub threads: u64,
}

/// What the kernel keeps resident for this process; `None` off Linux.
pub fn resident() -> Option<Resident> {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .map(|status| parse_status(&status))
}

fn parse_status(status: &str) -> Resident {
    let mut resident = Resident::default();
    for line in status.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(n) = rest
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<u64>().ok())
        else {
            continue;
        };
        // Sizes come in kB; the threads are a count.
        let bytes = n.saturating_mul(1024);
        match key {
            "VmRSS" => resident.total = bytes,
            "VmHWM" => resident.peak = bytes,
            "RssAnon" => resident.anonymous = bytes,
            "RssFile" => resident.file = bytes,
            "RssShmem" => resident.shared = bytes,
            "Threads" => resident.threads = n,
            _ => {}
        }
    }
    resident
}

/// What the cgroup is charged for, by kind, as `memory.stat` names them:
/// the processes' own memory, the page cache — active, and inactive, which
/// the kernel takes back first — the tmpfs mounts, the kernel's own
/// structures and the sockets' buffers.
pub const CHARGED_KINDS: [&str; 7] = [
    "anon",
    "active_file",
    "inactive_file",
    "shmem",
    "kernel",
    "sock",
    "file",
];

/// The cgroup's account, in bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Charged {
    /// Everything it is charged: `memory.current`.
    pub current: u64,
    /// Its ceiling, when it has one: `memory.max`.
    pub limit: Option<u64>,
    /// [`CHARGED_KINDS`], those `memory.stat` gives.
    pub kinds: Vec<(&'static str, u64)>,
    /// How many times it reached its ceiling and the kernel had to take
    /// memory back to stay under it: `max` in `memory.events`.
    pub at_ceiling: u64,
}

/// The account of the cgroup this process is in (v2); `None` off Linux, or
/// where the process sits at the root.
pub fn charged() -> Option<Charged> {
    let dir = cgroup_dir(&std::fs::read_to_string("/proc/self/cgroup").ok()?)?;
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
    let current = read("memory.current")?.trim().parse().ok()?;
    Some(Charged {
        current,
        // "max" when there is none.
        limit: read("memory.max").and_then(|max| max.trim().parse().ok()),
        kinds: read("memory.stat")
            .map(|stat| parse_stat(&stat))
            .unwrap_or_default(),
        at_ceiling: read("memory.events")
            .and_then(|events| field(&events, "max"))
            .unwrap_or(0),
    })
}

/// The directory of this process's cgroup: the `0::` line of
/// `/proc/self/cgroup`, under the v2 hierarchy's mount. A container with a
/// cgroup namespace of its own sees `/`, which is the mount itself.
fn cgroup_dir(cgroup: &str) -> Option<PathBuf> {
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    Some(Path::new("/sys/fs/cgroup").join(path.trim().trim_start_matches('/')))
}

fn parse_stat(stat: &str) -> Vec<(&'static str, u64)> {
    CHARGED_KINDS
        .iter()
        .filter_map(|kind| field(stat, kind).map(|n| (*kind, n)))
        .collect()
}

/// A `name value` line's value, in a cgroup file of such lines.
fn field(text: &str, name: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let (key, value) = line.split_once(' ')?;
        (key == name).then(|| value.trim().parse().ok()).flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_gives_bytes_and_threads() {
        let status = "Name:\tarr-metadata-se\nVmHWM:\t  461352 kB\nVmRSS:\t  316576 kB\n\
                      RssAnon:\t  286860 kB\nRssFile:\t   29716 kB\nRssShmem:\t       0 kB\n\
                      Threads:\t14\nSigQ:\t0/31511\n";
        assert_eq!(
            parse_status(status),
            Resident {
                total: 316_576 * 1024,
                peak: 461_352 * 1024,
                anonymous: 286_860 * 1024,
                file: 29_716 * 1024,
                shared: 0,
                threads: 14,
            }
        );
    }

    #[test]
    fn the_cgroup_is_found_under_the_mount() {
        assert_eq!(cgroup_dir("0::/\n"), Some(PathBuf::from("/sys/fs/cgroup/")));
        assert_eq!(
            cgroup_dir("0::/system.slice/docker-1f2e.scope\n"),
            Some(PathBuf::from(
                "/sys/fs/cgroup/system.slice/docker-1f2e.scope"
            ))
        );
        // A v1 hierarchy alone has no `0::` line.
        assert_eq!(cgroup_dir("12:memory:/docker/1f2e\n"), None);
    }

    #[test]
    fn the_stat_keeps_the_kinds_it_knows() {
        let stat = "anon 304611328\nfile 66588672\nkernel 11087872\nkernel_stack 229376\n\
                    sock 0\nshmem 0\nfile_mapped 0\ninactive_anon 0\nactive_anon 304611328\n\
                    inactive_file 44306432\nactive_file 22282240\n";
        assert_eq!(
            parse_stat(stat),
            vec![
                ("anon", 304_611_328),
                ("active_file", 22_282_240),
                ("inactive_file", 44_306_432),
                ("shmem", 0),
                ("kernel", 11_087_872),
                ("sock", 0),
                ("file", 66_588_672),
            ]
        );
        assert_eq!(field("low 0\nhigh 0\nmax 1208\noom 0\n", "max"), Some(1208));
        assert_eq!(field("low 0\n", "max"), None);
    }

    #[test]
    fn what_is_allocated_is_counted_until_it_is_given_back() {
        // Other tests allocate on their threads meanwhile: a block far
        // larger than they hold at once tells this one's apart.
        const BLOCK: usize = 256 * 1024 * 1024;
        let before = heap_bytes();
        let block = vec![0_u8; BLOCK];
        let during = heap_bytes();
        drop(block);
        let after = heap_bytes();
        assert!(during >= before + BLOCK as u64 / 2, "{before} → {during}");
        assert!(after + (BLOCK as u64) / 2 <= during, "{during} → {after}");
    }
}
