//! Per-download speed limiter (IDM: "Maximum download speed for one file", notes/ENGINE.md §4).
//! A token bucket shared by all of a download's connections; the limit can change while the
//! download runs, and a new limit applies to the waits already started too (#621). Throttling
//! the socket reads makes TCP slow the server down.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::Notify;
// Tokio's clock: the system's, except in tests that pause it.
use tokio::time::Instant;

/// Bytes per second; 0 = unlimited. One per download, and optionally one shared by all of them
/// (the global cap, `Options::global`).
#[derive(Debug)]
pub struct Limiter {
    /// Changed under the bucket's lock, read without it by [`Limiter::get`].
    rate: AtomicU64,
    bucket: Mutex<Bucket>,
    /// Every byte that went through it (less what was given back), limited or not: what the
    /// downloads under it took, for the global cap's split with torrents.
    taken: AtomicU64,
    /// Wakes the waits when the limit changes, so they count what is left at the new rate.
    changed: Notify,
}

#[derive(Debug)]
struct Bucket {
    /// Bytes that may be read right now (may go negative: debt to wait off).
    tokens: f64,
    last: Instant,
    /// The budget earned since the limiter was made, bytes, at whatever the rate was each
    /// moment: a wait ends when this reaches its mark, so a new rate applies to the rest of it.
    earned: f64,
    /// Goes up each time the limit is removed: the debt is forgiven then, and every wait from
    /// before ends, even if a limit is set again before that wait looks.
    era: u64,
}

impl Bucket {
    /// Add what `rate` earned since the last time.
    fn refill(&mut self, rate: u64, now: Instant) {
        let earned = now.saturating_duration_since(self.last).as_secs_f64() * rate as f64;
        self.last = now;
        if rate > 0 {
            self.earned += earned;
            self.tokens = (self.tokens + earned).min(rate as f64 * BURST_SECS);
        }
    }
}

/// Where a wait ends: once the bucket has earned `at` bytes, in the same `era`.
#[derive(Debug)]
struct Mark {
    at: f64,
    era: u64,
}

/// Burst allowance: up to one second's budget accumulates while no connection is reading (e.g.
/// all between requests), so short gaps don't lower the average below the limit.
const BURST_SECS: f64 = 1.0;

impl Limiter {
    pub fn new(rate: Option<u64>) -> Self {
        Limiter {
            rate: AtomicU64::new(rate.unwrap_or(0)),
            bucket: Mutex::new(Bucket {
                tokens: 0.0,
                last: Instant::now(),
                earned: 0.0,
                era: 0,
            }),
            taken: AtomicU64::new(0),
            changed: Notify::new(),
        }
    }

    /// The bytes that went through it so far (less those given back).
    pub fn taken(&self) -> u64 {
        self.taken.load(Ordering::Relaxed)
    }

    /// A new limit (None = unlimited), for the waits already started too: they wake and wait
    /// only for what is left of their debt at the new rate, and end at once without a limit.
    pub fn set(&self, rate: Option<u64>) {
        let rate = rate.unwrap_or(0);
        {
            let mut b = self.bucket.lock().unwrap();
            let old = self.rate.load(Ordering::Relaxed);
            if old == rate {
                return;
            }
            // What the old rate earned until now.
            b.refill(old, Instant::now());
            if rate == 0 {
                // No limit: the debt is forgiven (a limit set later starts afresh).
                b.tokens = 0.0;
                b.era += 1;
            }
            self.rate.store(rate, Ordering::Relaxed);
        }
        self.changed.notify_waiters();
    }

    pub fn get(&self) -> Option<u64> {
        Some(self.rate.load(Ordering::Relaxed)).filter(|r| *r > 0)
    }

    /// Account for `n` received bytes; where the caller's wait ends, if it must wait.
    fn take(&self, n: u64) -> Option<Mark> {
        self.taken.fetch_add(n, Ordering::Relaxed);
        if self.rate.load(Ordering::Relaxed) == 0 {
            return None;
        }
        let mut b = self.bucket.lock().unwrap();
        let rate = self.rate.load(Ordering::Relaxed);
        if rate == 0 {
            return None;
        }
        b.refill(rate, Instant::now());
        b.tokens -= n as f64;
        (b.tokens < 0.0).then(|| Mark {
            at: b.earned - b.tokens,
            era: b.era,
        })
    }

    /// How much longer the wait ending at `mark` lasts at the rate now; None once it is over.
    fn left(&self, mark: &Mark) -> Option<Duration> {
        let mut b = self.bucket.lock().unwrap();
        let rate = self.rate.load(Ordering::Relaxed);
        if rate == 0 || b.era != mark.era {
            return None;
        }
        b.refill(rate, Instant::now());
        let owed = mark.at - b.earned;
        // Under a byte is paid (the float's rounding would only add waits of nothing).
        (owed >= 1.0).then(|| Duration::from_secs_f64(owed / rate as f64))
    }

    /// Give back budget for `n` bytes that were paid for but not kept.
    pub fn refund(&self, n: u64) {
        let mut t = self.taken.load(Ordering::Relaxed);
        while let Err(now) = self.taken.compare_exchange_weak(
            t,
            t.saturating_sub(n),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            t = now;
        }
        if self.rate.load(Ordering::Relaxed) > 0 {
            self.bucket.lock().unwrap().tokens += n as f64;
        }
    }

    /// Wait until `n` more bytes fit under the limit: the limit at each moment of the wait, not
    /// only the one when it started (#621).
    pub async fn acquire(&self, n: u64) {
        let Some(mark) = self.take(n) else { return };
        loop {
            let mut changed = std::pin::pin!(self.changed.notified());
            // Listening before counting: a change from here on wakes this wait.
            changed.as_mut().enable();
            let Some(wait) = self.left(&mark) else { return };
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = changed => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Tokio's paused clock, as the tests below: the waits are the limiter's alone, not a
    /// busy machine's.
    #[tokio::test(start_paused = true)]
    async fn limits_average_rate() {
        let l = Limiter::new(Some(400 * 1024));
        let t = Instant::now();
        for _ in 0..20 {
            l.acquire(10 * 1024).await; // 200 KB at 400 KB/s ≈ 0.5 s
        }
        let secs = t.elapsed().as_secs_f64();
        assert!((0.4..0.75).contains(&secs), "{secs:.2}s");
    }

    #[tokio::test(start_paused = true)]
    async fn unlimited_and_changeable() {
        let l = Limiter::new(None);
        let t = Instant::now();
        l.acquire(100 << 20).await;
        assert!(t.elapsed() < Duration::from_millis(20));
        l.set(Some(1024));
        assert_eq!(l.get(), Some(1024));
        l.set(None);
        assert_eq!(l.get(), None);
    }

    #[tokio::test]
    async fn counts_what_went_through() {
        let l = Limiter::new(None);
        l.acquire(1000).await;
        l.set(Some(1 << 30));
        l.acquire(500).await;
        l.refund(200);
        assert_eq!(l.taken(), 1300, "limited or not, less what was given back");
    }

    /// #621, the reporter's witness: a wait that has started ends as soon as the limit is
    /// raised or removed, not at the deadline the old limit set (64 s here).
    #[tokio::test(start_paused = true)]
    async fn a_new_limit_applies_to_a_wait_already_started() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};
        for new_rate in [None, Some(1024 * 1024)] {
            let limiter = Limiter::new(Some(1024));
            let mut pending = Box::pin(limiter.acquire(64 * 1024));
            let mut cx = Context::from_waker(Waker::noop());
            assert!(pending.as_mut().poll(&mut cx).is_pending());
            limiter.set(new_rate);
            assert_eq!(limiter.get(), new_rate);
            tokio::time::advance(Duration::from_secs(1)).await;
            assert!(
                matches!(pending.as_mut().poll(&mut cx), Poll::Ready(())),
                "still waiting after the limit went to {new_rate:?}"
            );
        }
    }

    /// The time from `t` to now, to the millisecond (Tokio's paused clock).
    fn ms_since(t: Instant) -> u128 {
        t.elapsed().as_millis()
    }

    /// Two waits under one limit share it, before and after it changes: what each still owes
    /// is counted at the new rate, in turn.
    #[tokio::test(start_paused = true)]
    async fn waits_count_what_is_left_at_the_new_rate() {
        let l = Limiter::new(Some(1024));
        let t = Instant::now();
        let l = &l;
        let wait = |n| async move {
            l.acquire(n).await;
            ms_since(t)
        };
        // 1 KB each at 1 KB/s: at 1 s and 2 s. At 0.5 s the limit doubles: the first has 512
        // bytes left (0.25 s more), the second 1536 (0.75 s).
        let (a, b, ()) = tokio::join!(wait(1024), wait(1024), async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            l.set(Some(2048));
        });
        assert!((750..760).contains(&a), "{a} ms");
        assert!((1250..1260).contains(&b), "{b} ms");
        // Halved: 2 KB at 2 KB/s, and at 0.5 s 1 KB left at 1 KB/s.
        let t = Instant::now();
        let (a, ()) = tokio::join!(
            async {
                l.acquire(2048).await;
                ms_since(t)
            },
            async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                l.set(Some(1024));
            }
        );
        assert!((1500..1510).contains(&a), "{a} ms");
    }

    /// Without a limit the debt is gone: a limit set again starts afresh, and a wait from
    /// before ends even when the limit came back before it looked.
    #[tokio::test(start_paused = true)]
    async fn removing_the_limit_forgives_the_debt() {
        use std::future::Future;
        use std::task::{Context, Waker};
        let l = Limiter::new(Some(1024));
        let mut owed = Box::pin(l.acquire(64 * 1024));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(owed.as_mut().poll(&mut cx).is_pending());
        l.set(None);
        l.set(Some(1024));
        assert!(owed.as_mut().poll(&mut cx).is_ready());
        let t = Instant::now();
        l.acquire(1024).await;
        assert!((1000..1010).contains(&ms_since(t)), "{} ms", ms_since(t));
    }

    /// The same limit again changes nothing (the app sets the global cap's every frame).
    #[tokio::test(start_paused = true)]
    async fn the_same_limit_again_changes_nothing() {
        let l = Limiter::new(Some(1024));
        let t = Instant::now();
        tokio::join!(l.acquire(2048), async {
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                l.set(Some(1024));
            }
        });
        assert!((2000..2010).contains(&ms_since(t)), "{} ms", ms_since(t));
    }
}
