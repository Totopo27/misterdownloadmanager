//! How a piece of work grows with its input, for tests: linear, or worse.
//!
//! A test that times its work against a fixed limit ("under 10 s") fails on a slow or busy
//! machine and passes a quadratic search on a fast one. This one times the same work at `n`
//! and at `SCALE × n` on the same machine, a moment apart, and compares the two: linear work
//! costs about `SCALE` times more, quadratic work `SCALE²` times more, whatever the machine's
//! speed.

use std::time::{Duration, Instant};

/// The larger input is this many times the smaller one.
pub const SCALE: usize = 8;

/// The most the larger input may cost, in times the smaller one's cost. Linear work costs
/// `SCALE` (8) times more, `n log n` work about 10, quadratic work 64: 24 leaves room for the
/// caches a larger input misses and a busy machine, and none for a square.
pub const LIMIT: f64 = 24.0;

/// Each size runs this many times, interleaved, and its fastest run counts: a pause of the
/// machine (another program, a busy shared runner) only ever adds time to a run.
const RUNS: usize = 3;

/// Asserts that `work` grows linearly with its input (see the module): `make(n)` builds an
/// input of size `n`, untimed; `work` is timed on it. `small` is the smaller size: large
/// enough that `work` takes a few milliseconds on it, so that a square would show.
///
/// # Panics
///
/// When the larger input costs more than `LIMIT` times the smaller one, with both times.
pub fn assert_linear<T, R>(
    small: usize,
    mut make: impl FnMut(usize) -> T,
    mut work: impl FnMut(T) -> R,
) {
    let mut time = |n: usize| {
        let input = make(n);
        let t = Instant::now();
        let out = work(input);
        let took = t.elapsed();
        drop(std::hint::black_box(out));
        took
    };
    // A warm-up: the first run pays for the allocator's first pages and lazy statics.
    time(small);
    let (mut a, mut b) = (Duration::MAX, Duration::MAX);
    for _ in 0..RUNS {
        a = a.min(time(small));
        b = b.min(time(small * SCALE));
    }
    let ratio = b.as_secs_f64() / a.as_secs_f64().max(1e-6);
    assert!(
        ratio < LIMIT,
        "{} items took {a:?}, {} took {b:?}: {ratio:.1} times as long (linear: {SCALE}, the limit \
         {LIMIT})",
        small,
        small * SCALE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_work_passes() {
        assert_linear(
            200_000,
            |n| (0..n as u64).collect::<Vec<_>>(),
            |v| v.iter().map(|x| x.wrapping_mul(31)).sum::<u64>(),
        );
    }

    #[test]
    #[should_panic(expected = "times as long")]
    fn quadratic_work_fails() {
        assert_linear(
            1_000,
            |n| (0..n as u64).collect::<Vec<_>>(),
            |v| {
                let mut seen = Vec::new();
                for x in &v {
                    if !seen.contains(x) {
                        seen.push(*x);
                    }
                }
                seen.len()
            },
        );
    }
}
