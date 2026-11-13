//! A disk slower than the network, for tests and measurements (#361, #782): with
//! `Options::slow_disk`, the part file's writes and flushes go through it. It behaves like a
//! hard disk behind the system's write cache:
//! - a write lands in the cache, which the disk empties at `rate` bytes/s; once `cache` bytes
//!   wait there, a write waits for the disk (Windows and Linux hold writers to its pace);
//! - a flush waits until the disk has written everything waiting, plus `flush_takes`, and
//!   holds every write to the file meanwhile, as NTFS does (measured in #361).
//!
//! The real write or flush still runs, after the wait.

use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct SlowDisk {
    /// Bytes/s the disk writes. 0: as fast as the real disk (only `flush_takes` is slow).
    pub rate: u64,
    /// Bytes the system's cache takes in before writes wait for the disk.
    pub cache: u64,
    /// What every flush during the transfer takes on top of writing what waits in the cache.
    pub flush_takes: Duration,
    /// Held exclusively for each flush and shared for each write (tests watch it).
    pub gate: Arc<RwLock<()>>,
    backlog: Arc<Mutex<Backlog>>,
    stats: Arc<Mutex<DiskStats>>,
    clock: Clock,
}

/// The time the disk goes by: the real one, or in tests one that moves only when the disk
/// waits, so that their checks hold on a slow or busy machine.
#[derive(Clone, Debug)]
enum Clock {
    Real,
    #[cfg(test)]
    Manual(Arc<Mutex<Instant>>),
}

impl Clock {
    fn now(&self) -> Instant {
        match self {
            Clock::Real => Instant::now(),
            #[cfg(test)]
            Clock::Manual(t) => *t.lock().unwrap(),
        }
    }

    fn sleep(&self, d: Duration) {
        match self {
            Clock::Real => std::thread::sleep(d),
            #[cfg(test)]
            Clock::Manual(t) => *t.lock().unwrap() += d,
        }
    }
}

/// Bytes in the cache not yet on the disk, as of `at`.
#[derive(Debug)]
struct Backlog {
    bytes: f64,
    at: Instant,
}

/// What the disk did, for measurements.
#[derive(Clone, Debug, Default)]
pub struct DiskStats {
    /// How long each flush took, the wait for the disk included.
    pub flushes: Vec<Duration>,
    /// Writes that waited for the disk (its cache full, or a flush), and how long in all.
    pub writes_waited: u64,
    pub write_wait: Duration,
}

impl SlowDisk {
    pub fn new(rate: u64, cache: u64, flush_takes: Duration) -> Self {
        Self::with_clock(rate, cache, flush_takes, Clock::Real)
    }

    fn with_clock(rate: u64, cache: u64, flush_takes: Duration, clock: Clock) -> Self {
        SlowDisk {
            rate,
            cache,
            flush_takes,
            gate: Default::default(),
            backlog: Arc::new(Mutex::new(Backlog {
                bytes: 0.0,
                at: clock.now(),
            })),
            stats: Default::default(),
            clock,
        }
    }

    /// A disk that is only slow to flush (`takes`), its writes as fast as the real disk's.
    pub fn slow_flush(takes: Duration) -> Self {
        Self::new(0, u64::MAX, takes)
    }

    pub fn stats(&self) -> DiskStats {
        self.stats.lock().unwrap().clone()
    }

    /// Run `write` (of `n` bytes) once the disk takes them in.
    pub fn write<T>(&self, n: u64, write: impl FnOnce() -> T) -> T {
        let began = self.clock.now();
        let _flush_over = self.gate.read().unwrap();
        // A disk of `rate` 0 takes everything at once.
        while let Some(wait) = (self.rate > 0).then(|| self.room_in(n)).flatten() {
            self.clock.sleep(wait.min(Duration::from_millis(20)));
        }
        let waited = self.clock.now() - began;
        if waited >= Duration::from_millis(1) {
            let mut s = self.stats.lock().unwrap();
            s.writes_waited += 1;
            s.write_wait += waited;
        }
        write()
    }

    /// Run `flush` once everything in the cache is on the disk; `extra`: plus `flush_takes`
    /// (the saves while the connections run, not the forced ones at the start and the end).
    pub fn flush<T>(&self, extra: bool, flush: impl FnOnce() -> T) -> T {
        let began = self.clock.now();
        // A forced save of a disk only slow to flush is a plain one (the gate tells tests a slow
        // flush runs).
        let _writes_wait = (extra || self.rate > 0).then(|| self.gate.write().unwrap());
        let left = if self.rate > 0 {
            Duration::from_secs_f64(self.backlog().bytes / self.rate as f64)
        } else {
            Duration::ZERO
        };
        self.clock.sleep(
            left + if extra {
                self.flush_takes
            } else {
                Duration::ZERO
            },
        );
        self.backlog().bytes = 0.0;
        let done = flush();
        let took = self.clock.now() - began;
        self.stats.lock().unwrap().flushes.push(took);
        done
    }

    /// Take `n` bytes into the cache if they fit (or it is empty), else how long until they do.
    fn room_in(&self, n: u64) -> Option<Duration> {
        let mut b = self.backlog();
        let after = b.bytes + n as f64;
        if b.bytes == 0.0 || after <= self.cache as f64 {
            b.bytes = after;
            return None;
        }
        Some(Duration::from_secs_f64(
            (after - self.cache as f64) / self.rate as f64,
        ))
    }

    /// The cache's backlog now (the disk wrote `rate` bytes/s of it since last time).
    fn backlog(&self) -> MutexGuard<'_, Backlog> {
        let mut b = self.backlog.lock().unwrap();
        let now = self.clock.now();
        let gone = (now - b.at).as_secs_f64() * self.rate as f64;
        b.bytes = (b.bytes - gone).max(0.0);
        b.at = now;
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Within a microsecond of `want` (the backlog is counted in floating point).
    fn about(took: Duration, want: Duration) -> bool {
        took.abs_diff(want) < Duration::from_micros(1)
    }

    /// The model itself: writes go straight into the cache until it is full, then wait for the
    /// disk, which empties the cache meanwhile; a flush waits for what is left. Timed on the
    /// disk's own clock, which moves only when the disk waits: on a slow or busy machine the
    /// real one went on between two steps and the cache drained, so a flush took less than the
    /// test expected (#923's CI on macOS).
    #[test]
    fn writes_wait_once_the_cache_is_full() {
        let clock = Clock::Manual(Arc::new(Mutex::new(Instant::now())));
        let ms = Duration::from_millis;
        let d = SlowDisk::with_clock(100 << 20, 10 << 20, ms(30), clock.clone());
        // `mib` MiB written, one at a time: how long they waited.
        let writes = |mib: usize| {
            let t = clock.now();
            for _ in 0..mib {
                d.write(1 << 20, || ());
            }
            clock.now() - t
        };
        assert_eq!(writes(10), Duration::ZERO, "10 MiB fit in the cache");
        assert_eq!(d.stats().writes_waited, 0);
        // The cache is full: each MiB more waits for the disk to write one, at 100 MiB/s.
        let took = writes(10);
        assert!(about(took, ms(100)), "{took:?}");
        let s = d.stats();
        assert_eq!(s.writes_waited, 10);
        assert!(about(s.write_wait, ms(100)), "{:?}", s.write_wait);
        // In 50 ms the disk writes 5 MiB of it: 5 MiB more fit at once, the next one waits.
        clock.sleep(ms(50));
        assert_eq!(writes(5), Duration::ZERO);
        let took = writes(1);
        assert!(about(took, ms(10)), "{took:?}");
        // A flush waits for the 10 MiB in the cache, plus what a flush takes; then nothing is
        // left, and a forced flush (at the start and the end) doesn't take that either.
        d.flush(true, || ());
        d.flush(true, || ());
        d.flush(false, || ());
        let f = d.stats().flushes;
        assert!(about(f[0], ms(130)), "{f:?}");
        assert!(about(f[1], ms(30)) && f[2] == Duration::ZERO, "{f:?}");
    }
}
