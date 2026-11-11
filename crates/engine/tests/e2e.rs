//! End-to-end: the engine downloads from a labserver (with faults) and the file is verified
//! byte for byte against the generator.

use mdm_engine::{Destination, Download, Options, Request};
use mdm_labserver::{Config, Faults, LabServer, LogEntry, content};

/// How long a test waits for the lab server's log to take in every request (`settled`).
const SETTLE: std::time::Duration = std::time::Duration::from_secs(20);

async fn server(faults: &str) -> LabServer {
    LabServer::start(Config {
        faults: Faults::parse(faults).unwrap(),
        ..Config::default()
    })
    .await
    .unwrap()
}

fn fast_opts() -> Options {
    Options {
        retry_delay: std::time::Duration::from_millis(50),
        ..Options::default()
    }
}

/// Download `path` from `s` into a temp dir; returns the log and the verified size.
async fn fetch(s: &LabServer, path: &str, seed: u64, opts: Options) -> (Vec<LogEntry>, u64) {
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url(path).parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let out = Download::start(req, opts)
        .wait()
        .await
        .expect("download succeeds");
    let data = std::fs::read(&out.path).unwrap();
    let mut want = vec![0; data.len()];
    content::fill(seed, 0, &mut want);
    assert!(data == want, "content mismatch");
    assert_eq!(data.len() as u64, out.size);
    // The server logs a request when its response body is dropped, which can trail the client:
    // wait until every one is in the log (and its connection slot is free).
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("every request ends");
    (s.log(), out.size)
}

fn max_concurrent(log: &[LogEntry]) -> usize {
    let mut edges: Vec<(u64, i32)> = log
        .iter()
        .flat_map(|e| [(e.t_start_ms, 1), (e.t_end_ms, -1)])
        .collect();
    edges.sort();
    edges
        .iter()
        .scan(0, |n, (_, d)| {
            *n += d;
            Some(*n)
        })
        .max()
        .unwrap_or(0) as usize
}

#[tokio::test]
async fn segmented_download_uses_many_connections() {
    let s = server("rate=4M").await;
    let (log, size) = fetch(&s, "/gen/8M/a.bin?seed=1", 1, fast_opts()).await;
    assert_eq!(size, 8 << 20);
    // The probe plus one ranged request per other connection (more only if a split happens,
    // which depends on timing when all connections run at the same speed).
    let ranged = log.iter().filter(|e| e.status == 206).count();
    let others = Options::default().max_connections as usize - 1;
    assert!(
        ranged >= others,
        "expected ≥ {others} ranged requests, got {ranged}"
    );
    assert!(max_concurrent(&log) >= 4, "connections ran in parallel");
}

#[tokio::test]
async fn connections_skip_a_dead_address() {
    let s = server("").await;
    let port = s.url("/").parse::<url::Url>().unwrap().port().unwrap();
    // The host's first address never answers (TEST-NET-1); the second is the lab server.
    // A connect timeout long enough that waiting on the dead address can't pass for a slow
    // machine (#136): 60 s per address, against the 30 s the whole download is given.
    let opts = Options {
        hosts: vec![(
            "mirror.test".into(),
            vec!["192.0.2.1".parse().unwrap(), "127.0.0.1".parse().unwrap()],
        )],
        connect_timeout: std::time::Duration::from_secs(120),
        ..Options::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: format!("http://mirror.test:{port}/gen/2M?seed=19")
            .parse()
            .unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    // Without the race every connection would wait 60 s on the dead address (half the connect
    // timeout) before trying the next. A 3 s budget failed on a starved CPU, where 2 MiB alone
    // can take longer (#136).
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        Download::start(req, opts).wait(),
    )
    .await
    .expect("the connections waited for the dead address")
    .expect("completes");
    let mut want = vec![0; 2 << 20];
    content::fill(19, 0, &mut want);
    assert!(
        std::fs::read(&out.path).unwrap() == want,
        "content mismatch"
    );
}

#[tokio::test]
async fn a_slow_download_does_not_rewrite_its_state_every_tick() {
    let s = server("rate=4K").await;
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/1M/slow.bin?seed=22").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let opts = Options {
        save_interval: std::time::Duration::from_millis(50),
        ..fast_opts()
    };
    let dl = Download::start(req, opts);
    let state = loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Some(f) = dl.snapshot().file {
            let p = mdm_engine::state::state_path(&f);
            if p.exists() {
                break p;
            }
        }
    };
    let first = std::fs::read(&state).unwrap();
    // Save ticks while bytes trickle in: no flush is due, nothing is rewritten. Waits for the
    // bytes themselves, not a time (#201: on a starved CPU none had come in a second): the first
    // ones, then more, over at least a second (20 ticks; fewer on a starved CPU, never none).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let running = &dl;
    let bytes_in = move |more_than: u64| async move {
        while running.snapshot().downloaded <= more_than {
            assert!(std::time::Instant::now() < deadline, "no bytes came in");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        running.snapshot().downloaded
    };
    let ticks = tokio::time::sleep(std::time::Duration::from_secs(1));
    let early = bytes_in(0).await;
    bytes_in(early).await;
    ticks.await;
    assert_eq!(
        std::fs::read(&state).unwrap(),
        first,
        "rewritten between flushes"
    );
    // Pausing always flushes and records the progress.
    dl.cancel();
    let _ = dl.wait().await;
    let last = mdm_engine::state::ResumeState::load(&state).unwrap();
    assert!(last.downloaded() > 0);
    assert_ne!(std::fs::read(&state).unwrap(), first);
}

#[tokio::test]
async fn a_global_cap_is_shared_by_all_downloads() {
    let s = server("").await;
    // Timed from the cap's start: its budget builds up from then on, however long the
    // downloads take to start (a burst of up to a second's), so 2 MiB can't take less than 2 s.
    let started = std::time::Instant::now();
    let global = std::sync::Arc::new(mdm_engine::limiter::Limiter::new(Some(1 << 20)));
    let opts = Options {
        global: Some(global),
        ..fast_opts()
    };
    let (a, b) = tokio::join!(
        fetch(&s, "/gen/1M/a.bin?seed=25", 25, opts.clone()),
        fetch(&s, "/gen/1M/b.bin?seed=26", 26, opts.clone()),
    );
    assert_eq!((a.1, b.1), (1 << 20, 1 << 20));
    // 2 MiB through one 1 MiB/s cap.
    let took = started.elapsed();
    assert!(
        took >= std::time::Duration::from_millis(1700),
        "took {took:?}"
    );
}

/// The SHA-256 of generated content `seed` of `size` bytes.
fn sha256_of(seed: u64, size: usize) -> mdm_engine::verify::Hash {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("want");
    let mut want = vec![0; size];
    content::fill(seed, 0, &mut want);
    std::fs::write(&p, &want).unwrap();
    let algo = mdm_engine::verify::Algo::Sha256;
    mdm_engine::verify::Hash {
        algo,
        value: mdm_engine::verify::digest(&p, algo).unwrap(),
    }
}

#[tokio::test]
async fn a_known_checksum_is_verified() {
    let s = server("").await;
    let opts = Options {
        checksum: Some(sha256_of(27, 1 << 20)),
        ..fast_opts()
    };
    let (_, size) = fetch(&s, "/gen/1M/ok.bin?seed=27", 27, opts).await;
    assert_eq!(size, 1 << 20);
}

#[tokio::test]
async fn a_wrong_checksum_fails_and_keeps_the_data() {
    let s = server("").await;
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/1M/bad.bin?seed=28").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let opts = Options {
        checksum: Some(sha256_of(99, 1 << 20)),
        ..fast_opts()
    };
    let err = Download::start(req, opts).wait().await.unwrap_err();
    assert!(
        matches!(err, mdm_engine::Error::ChecksumMismatch("SHA-256")),
        "{err}"
    );
    assert!(
        dir.path().join("bad.bin.mdm.part").exists(),
        "the data is kept"
    );
    assert!(!dir.path().join("bad.bin").exists());
}

#[tokio::test]
async fn the_event_log_tells_what_happened() {
    // Every response is cut after 300 KiB, so a retry is certain (#123). Cutting only every
    // third request left it to timing: on a busy machine that request could be a part under
    // 300 KiB, be taken over before it got there, or be cut after its part was already done,
    // and the download then had nothing to retry.
    let s = server("rate=8M&drop_after=300K").await;
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/4M/log.bin?seed=29").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let dl = Download::start(req, fast_opts());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let snap = loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let snap = dl.snapshot();
        if snap.state == mdm_engine::download::State::Done {
            break snap;
        }
        assert!(std::time::Instant::now() < deadline, "never finished");
    };
    let texts: Vec<&str> = snap.events.iter().map(|e| e.text.as_str()).collect();
    assert!(texts[0].starts_with("Connected to 127.0.0.1"), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("retrying")), "{texts:?}");
    assert!(texts.last().unwrap().starts_with("Complete"), "{texts:?}");
    assert!(snap.events.windows(2).all(|w| w[0].seq < w[1].seq));
}

#[tokio::test]
async fn a_host_connection_limit_is_learned_and_reused() {
    // The cap is learned from the connections seen receiving at once when others are refused.
    // On a starved CPU the first connection (the probe) could finish its part before the second
    // had its first bytes, so only one was ever seen and the cap learned was 1 (#136). Now the
    // byte at 256 KiB, inside the probe's part however the file is split, is held back until
    // the cap is learned: the probe keeps receiving while the other connection fetches the rest.
    let s = server("rate=2M&max_conns=2&stall_at=256K").await;
    let limits = std::sync::Arc::new(mdm_engine::hosts::HostLimits::default());
    let opts = Options {
        host_limits: Some(limits.clone()),
        ..fast_opts()
    };
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/4M/a.bin?seed=30").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let dl = Download::start(req, opts.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while limits.cap("127.0.0.1") != Some(2) {
        assert!(
            std::time::Instant::now() < deadline,
            "the cap learned: {:?}",
            limits.learned()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    s.set_faults(Faults::parse("rate=2M&max_conns=2").unwrap());
    let out = dl.wait().await.expect("download succeeds");
    let mut want = vec![0; 4 << 20];
    content::fill(30, 0, &mut want);
    assert!(
        std::fs::read(&out.path).unwrap() == want,
        "content mismatch"
    );
    // Every request of the first download has ended and given its slot back.
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("every request ends");
    assert!(s.log().iter().any(|e| e.outcome == "fault:max_conns"));
    assert_eq!(limits.cap("127.0.0.1"), Some(2), "{:?}", limits.learned());
    // The next download from the host opens only what it allows: nothing is refused for a
    // connection too many. A connection that reconnects (its part done, it takes over or
    // splits another) can still be refused once: the server sees that the client dropped its
    // previous response only when it next writes to it, and on a starved CPU the new request
    // can come first (#224). That refusal is told by the server's count at the time: one of
    // the responses holding the slots had been sent up to where another part begins, so the
    // client had all it needed of it and had dropped it, or was about to.
    s.clear_log();
    let (second, size) = fetch(&s, "/gen/4M/b.bin?seed=31", 31, opts).await;
    assert_eq!(size, 4 << 20);
    let starts: Vec<u64> = second
        .iter()
        .filter(|e| matches!(e.status, 200 | 206))
        .map(|e| e.body_start)
        .collect();
    let done = |h: &mdm_labserver::Held| starts.iter().any(|&s| h.from < s && s <= h.sent_to);
    let too_many: Vec<_> = second
        .iter()
        .filter(|e| e.outcome == "fault:max_conns" && !e.held.iter().any(done))
        .collect();
    assert!(
        too_many.is_empty(),
        "refused: {:?}; requests: {:?}",
        too_many.iter().map(|e| &e.held).collect::<Vec<_>>(),
        second
            .iter()
            .map(|e| (e.body_start, e.bytes_sent, &e.outcome))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_refusal_below_the_learned_cap_is_retried() {
    // The cap (2) is known, but a slot is still held elsewhere (here another client; in practice
    // a connection of ours that just closed and the server hasn't noticed yet): the second
    // connection is refused while only one works. It must come back once the slot frees, not
    // leave the whole file to one connection (benchmark 2026-10-29).
    // The other client's slot is held by the test until our download has been refused (#136:
    // a blocking request could end before our second connection asked). Our download can't
    // finish meanwhile either: the server holds back its byte at 6 MiB until a second connection
    // has joined, so on a starved CPU the first one can't fetch the whole file alone before it.
    let s = server("rate=2M&max_conns=2&stall_at=6M").await;
    let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
    let limits = std::sync::Arc::new(mdm_engine::hosts::HostLimits::default());
    limits.refused("127.0.0.1", 2);
    limits.refused("127.0.0.1", 2);
    let blocker = s.hold_slot(ip);
    let opts = Options {
        host_limits: Some(limits),
        // Room to come back after the slot frees, however slowly the test notices the refusal.
        max_retries: 8,
        ..fast_opts()
    };
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/8M/c.bin?seed=33").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let dl = Download::start(req, opts);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !s
        .log()
        .iter()
        .any(|e| e.path.contains("c.bin") && e.outcome == "fault:max_conns")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the blocker held a slot"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // The slot frees: a second connection can join, and once it has the first may go on.
    drop(blocker);
    let receiving = |dl: &Download| {
        dl.snapshot()
            .connections
            .iter()
            .filter(|c| matches!(c.status, mdm_engine::download::ConnStatus::Receiving))
            .count()
    };
    while receiving(&dl) < 2 {
        assert!(
            std::time::Instant::now() < deadline,
            "no second connection joined"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    s.set_faults(Faults::parse("rate=2M&max_conns=2").unwrap());
    let out = dl.wait().await.expect("download succeeds");
    let size = out.size;
    let mut want = vec![0; 8 << 20];
    content::fill(33, 0, &mut want);
    assert!(
        std::fs::read(&out.path).unwrap() == want,
        "content mismatch"
    );
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("every request ends");
    let log = s.log();
    assert_eq!(size, 8 << 20);
    let ours: Vec<_> = log.iter().filter(|e| e.path.contains("c.bin")).collect();
    assert!(
        ours.iter().any(|e| e.outcome == "fault:max_conns"),
        "the blocker held a slot"
    );
    // Retired, connection 2's half would wait for connection 1 to finish its own: sequential.
    let served: Vec<LogEntry> = ours
        .iter()
        .filter(|e| e.status != 503)
        .map(|e| (*e).clone())
        .collect();
    assert_eq!(
        max_concurrent(&served),
        2,
        "a second connection joined once the slot freed: {:?}",
        ours.iter()
            .map(|e| (e.status, e.t_start_ms, e.t_end_ms, &e.outcome))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn slow_connections_reconnect_instead_of_holding_the_end() {
    // Every third request crawls at 1 KB/s. The other connections split such a request's part
    // and take its halves over, down to pieces too small to split again (64 KiB here): without
    // shedding, a slow request holds the end on such a piece for over a minute.
    let s = server("rate=8M&slow_every=3&slow_rate=1K").await;
    let opts = Options {
        min_split: 64 << 10,
        ..fast_opts()
    };
    let (log, size) = fetch(&s, "/gen/8M/slow.bin?seed=32", 32, opts).await;
    assert_eq!(size, 8 << 20);
    // Timed from each slow request's own start, not the download's (#201: on a starved CPU the
    // whole download took 8-25 s against an 8 s budget, and at 20 KB/s it took under 8 s without
    // shedding too). Shedding drops a slow request once 2 s have shown it slow (or lets it end
    // with under 2 s of its own pace left): about 2 s on an idle machine, up to 8 s on a starved
    // one. Without shedding the last ones run 64 s or more.
    let slow: Vec<_> = log
        .iter()
        .filter(|e| e.rate == Some(1024))
        .map(|e| (e.t_end_ms - e.t_start_ms, e.bytes_sent, e.body_len))
        .collect();
    assert!(!slow.is_empty(), "the fault was there");
    assert!(
        slow.iter().all(|(ms, ..)| *ms < 20_000),
        "a slow request held its piece (ms, sent, asked): {slow:?}"
    );
}

#[tokio::test]
async fn a_weak_etag_means_no_if_range() {
    let s = server("rate=4M&etag=weak").await;
    let (log, _) = fetch(&s, "/gen/4M?seed=20", 20, fast_opts()).await;
    assert!(log.iter().filter(|e| e.status == 206).count() > 1);
    assert!(
        log.iter().all(|e| e.if_range.is_none()),
        "neither the weak ETag nor the date behind it"
    );
}

#[tokio::test]
async fn per_node_etags_fall_back_to_the_date() {
    // Every answer carries a new ETag for the same file, as CDN nodes do.
    let s = server("rate=4M&etag=per-request").await;
    let (log, size) = fetch(&s, "/gen/8M?seed=21", 21, fast_opts()).await;
    assert_eq!(size, 8 << 20);
    assert!(
        log.iter().filter(|e| e.status == 206).count() > 1,
        "still segmented, not one connection"
    );
    assert!(
        log.iter()
            .any(|e| e.status == 206 && e.if_range.as_deref().is_some_and(|v| !v.starts_with('"'))),
        "validated with Last-Modified"
    );
}

#[tokio::test]
async fn sockets_are_reused_between_segments() {
    // Every other connection is slow: the fast ones finish early and take over the rest of the
    // slow ones' segments, on the keep-alive sockets they already have.
    let s = server("rate=4M&slow_every=2&slow_rate=512K").await;
    let opts = Options {
        max_connections: 4,
        ..fast_opts()
    };
    let (log, _) = fetch(&s, "/gen/8M?seed=18", 18, opts).await;
    let sockets: std::collections::HashSet<_> = log.iter().map(|e| e.peer).collect();
    assert!(
        log.iter().any(|e| e.conn_req > 1) && sockets.len() < log.len(),
        "{} sockets for {} requests",
        sockets.len(),
        log.len()
    );
}

#[tokio::test]
async fn survives_drops_and_503s() {
    let s = server("rate=8M&drop_after=300K&drop_every=3&fail_every=5").await;
    let (log, _) = fetch(&s, "/gen/6M?seed=2", 2, fast_opts()).await;
    assert!(log.iter().any(|e| e.outcome == "dropped"));
}

/// A server busy when the download starts (its first answers 503): the first request is retried
/// like any connection instead of failing the download (night e2e, 2026-10-30).
#[tokio::test]
async fn a_server_busy_at_the_start_is_retried() {
    let s = server("fail_first=3").await;
    let (log, _) = fetch(&s, "/gen/2M?seed=4", 4, fast_opts()).await;
    assert_eq!(
        log.iter()
            .filter(|e| e.outcome == "fault:fail_first")
            .count(),
        3
    );
}

/// A dead link still fails at once: only failures a later try may not have are retried.
#[tokio::test]
async fn a_dead_link_is_not_retried() {
    let s = server("").await;
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/not-a-size").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let err = Download::start(req, fast_opts()).wait().await.unwrap_err();
    assert!(err.to_string().contains("404"), "{err}");
    assert_eq!(s.log().len(), 1, "one request, no retries");
}

#[tokio::test]
async fn respects_server_connection_limit() {
    let s = server("rate=4M&max_conns=2").await;
    let (log, _) = fetch(&s, "/gen/4M?seed=3", 3, fast_opts()).await;
    assert!(log.iter().any(|e| e.outcome == "fault:max_conns"));
}

#[tokio::test]
async fn no_range_server_uses_one_connection() {
    // Rate-limited: unlimited, the 200 sent to a range attempt can fit whole into the socket
    // buffers before the engine drops it, and the server logs it complete (macOS runners). The
    // byte at 2 MiB is held back until the engine knows the server has no ranges, so the range
    // attempts always come while the probe is still streaming, however busy the machine (#136).
    let s = server("no_range=true&rate=8M&stall_at=2M").await;
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/3M?seed=4").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let dl = Download::start(req, fast_opts());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while dl.snapshot().resumable != Some(false) {
        assert!(
            std::time::Instant::now() < deadline,
            "never learned the server has no ranges"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    s.set_faults(Faults::parse("no_range=true&rate=8M").unwrap());
    let out = dl.wait().await.expect("download succeeds");
    let size = out.size;
    let mut want = vec![0; 3 << 20];
    content::fill(4, 0, &mut want);
    assert!(
        std::fs::read(&out.path).unwrap() == want,
        "content mismatch"
    );
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("every request ends");
    let log = s.log();
    assert_eq!(size, 3 << 20);
    // Like IDM (lab S6): range attempts get 200 and are dropped; the probe finishes alone.
    let complete: Vec<_> = log.iter().filter(|e| e.outcome == "complete").collect();
    let requests: Vec<_> = log
        .iter()
        .map(|e| {
            (
                e.range.clone(),
                e.outcome.clone(),
                e.bytes_sent,
                e.t_start_ms,
                e.t_end_ms,
            )
        })
        .collect();
    assert_eq!(complete.len(), 1, "{requests:#?}");
    assert_eq!(complete[0].range, None, "the probe is a plain GET");
}

/// Part of #136: a server without ranges sends the file once, even when the range attempts are
/// answered after the probe's plain GET has run on (a busy machine): every part is taken by the
/// probe, none is fetched again by a fresh start on one connection.
#[tokio::test]
async fn a_server_without_ranges_sends_the_file_once() {
    // The probe takes about 375 ms; every range attempt is answered after 300 ms, so the first
    // answers come while it runs and a retry's after it has ended. The range attempts' answers
    // (the whole file, which the engine drops at their headers) come at 32 KB/s: at the probe's
    // 8 MB/s the server filled each one's socket (1.3 MiB on Windows) before a starved CPU had
    // dropped them, and 7 of them made 12.8 MB served for 3 MiB (#201).
    let s = server("no_range=true&rate=8M&range_latency_ms=300&range_rate=32K").await;
    let (log, size) = fetch(&s, "/gen/3M?seed=41", 41, fast_opts()).await;
    assert_eq!(size, 3 << 20);
    let sent: u64 = log.iter().map(|e| e.bytes_sent).sum();
    let requests: Vec<_> = log
        .iter()
        .map(|e| (e.range.clone(), e.outcome.clone(), e.bytes_sent))
        .collect();
    // One plain GET, the probe: no start over on one connection. The bytes served beyond the
    // file are what the server had written into the range attempts' sockets when the engine
    // dropped them (32 KB a second each); a second copy would double them.
    assert_eq!(
        log.iter().filter(|e| e.range.is_none()).count(),
        1,
        "{requests:#?}"
    );
    assert!(
        sent < 2 * size,
        "{sent} bytes served for a {size}-byte file: {requests:#?}"
    );
    assert_eq!(
        log.iter().filter(|e| e.outcome == "complete").count(),
        1,
        "{requests:#?}"
    );
}

/// A CDN whose edges sometimes ignore ranges, with the same file and the same ETag: a full
/// answer to a range request is retried, and the download stays on several connections (#136:
/// telling it from a server without ranges must not cost it its speed). `every` = 2: the very
/// first range request is the one answered in full; 3: one in three.
async fn a_cdn_ignoring_ranges_now_and_then(every: u32, seed: u64) {
    let s = server(&format!("rate=2M&no_range_every={every}")).await;
    let path = format!("/gen/8M/cdn.bin?seed={seed}");
    let (log, size) = fetch(&s, &path, seed, fast_opts()).await;
    assert_eq!(size, 8 << 20);
    let ranged: Vec<LogEntry> = log.iter().filter(|e| e.status == 206).cloned().collect();
    assert!(
        max_concurrent(&ranged) >= 2,
        "one connection only: {:?}",
        log.iter()
            .map(|e| (e.range.clone(), e.status, e.outcome.clone()))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_cdn_ignoring_the_first_range_stays_segmented() {
    a_cdn_ignoring_ranges_now_and_then(2, 44).await;
}

#[tokio::test]
async fn a_cdn_ignoring_one_range_in_three_stays_segmented() {
    a_cdn_ignoring_ranges_now_and_then(3, 45).await;
}

#[tokio::test]
async fn probe_is_a_plain_get() {
    let s = server("rate=4M").await;
    let (log, _) = fetch(&s, "/gen/2M?seed=6", 6, fast_opts()).await;
    assert_eq!(log.iter().filter(|e| e.range.is_none()).count(), 1);
    assert!(
        log.iter()
            .all(|e| e.user_agent.as_deref() == Some(mdm_engine::agent::automatic().as_str()))
    );
}

#[tokio::test]
async fn tiny_file_is_not_split() {
    let s = server("").await;
    let (log, _) = fetch(&s, "/gen/16K?seed=5", 5, fast_opts()).await;
    assert_eq!(log.iter().filter(|e| e.bytes_sent > 0).count(), 1);
}

/// Start a download, cancel it once some data is on disk; returns (dir, state file). The server
/// holds the byte at 4 MiB back meanwhile, so the download can't finish before it is stopped
/// however busy the machine (#123: on a slow CI runner the whole file could arrive before the
/// stop). On return the server's faults are as before and every request of the stopped
/// download is in its log.
async fn paused(s: &LabServer, path: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let faults = s.faults();
    s.set_faults(Faults {
        stall_at: Some(4 << 20),
        ..faults.clone()
    });
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url(path).parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let opts = Options {
        save_interval: std::time::Duration::from_millis(100),
        ..fast_opts()
    };
    let dl = Download::start(req, opts);
    // Wait for the real condition (not a fixed time: a busy machine is slower): the probe is
    // done and a MiB is in (the server holds back only what follows its stall point).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let file = loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let snap = dl.snapshot();
        if let Some(f) = snap.file.filter(|_| snap.downloaded >= 1 << 20) {
            break f;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the download never started"
        );
    };
    dl.cancel();
    let stopped = dl.wait().await;
    assert!(
        matches!(stopped, Err(mdm_engine::Error::Cancelled)),
        "{:?}",
        stopped.map(|o| o.size)
    );
    // The held-back responses go on (into connections the download closed), and end.
    s.set_faults(faults);
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("the stopped download's requests end");
    (dir, mdm_engine::state::state_path(&file))
}

/// Bytes of `range` (a `Range` header, `bytes=a-b`; none: the whole file) that the saved
/// segments say were already on disk: outside every segment's part still to fetch.
fn on_disk(range: Option<&str>, saved: &mdm_engine::state::ResumeState) -> u64 {
    let (from, to) = match range.and_then(|r| r.strip_prefix("bytes=")?.split_once('-')) {
        Some((a, b)) => (
            a.parse::<u64>().unwrap(),
            b.parse::<u64>().map_or(saved.size, |b| b + 1),
        ),
        None => (0, saved.size),
    };
    let to_fetch: u64 = saved
        .segments
        .iter()
        .map(|g| to.min(g.end).saturating_sub(from.max(g.cursor)))
        .sum();
    (to - from) - to_fetch
}

/// Watch `dl` to its end: the file its view last named, and the path it reports. The view is what
/// the app's row follows.
async fn view_and_outcome(dl: Download) -> (Option<std::path::PathBuf>, std::path::PathBuf) {
    let until = std::time::Instant::now() + SETTLE;
    let file = loop {
        let v = dl.snapshot();
        if v.state == mdm_engine::download::State::Done {
            break v.file;
        }
        assert!(
            !matches!(
                v.state,
                mdm_engine::download::State::Failed | mdm_engine::download::State::Cancelled
            ),
            "{:?}",
            v.error
        );
        assert!(std::time::Instant::now() < until, "no end: {:?}", v.state);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    };
    (file, dl.wait().await.unwrap().path)
}

/// #442: a file took the destination while the download ran, so it was saved under the next
/// number. The view says that name (the row used to keep the old one and open the other file).
#[tokio::test]
async fn the_view_names_the_file_when_its_place_was_taken_meanwhile() {
    // Several connections, and one connection on a server that refuses ranges.
    for query in ["seed=11&rate=4M", "seed=12&rate=4M&no_range=true"] {
        let s = server("").await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("master.mp4");
        let dl = Download::start(
            Request {
                url: s
                    .url(&format!("/gen/2M/master.mp4?{query}"))
                    .parse()
                    .unwrap(),
                headers: Default::default(),
                dest: Destination::File(dest.clone()),
            },
            fast_opts(),
        );
        // Another program's file, once the download is under way.
        while dl.snapshot().file.is_none() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        std::fs::write(&dest, b"someone else's").unwrap();
        let (file, path) = view_and_outcome(dl).await;
        assert_eq!(path, dir.path().join("master (1).mp4"), "{query}");
        assert_eq!(file.as_deref(), Some(path.as_path()), "{query}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"someone else's");
    }
}

/// #743: a name given before its folder was looked at (Download File Info doesn't wait for a
/// slow disk) is numbered as the download starts when it is taken by then, by a file or by
/// another download's part file: neither is touched, and the view names the file made.
#[tokio::test]
async fn a_name_given_unlooked_is_numbered_as_it_starts() {
    let s = server("").await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("master.mp4");
    std::fs::write(&dest, b"someone else's").unwrap();
    let part = mdm_engine::naming::part_path(&dir.path().join("master (1).mp4"));
    std::fs::write(&part, b"another download's").unwrap();
    let dl = Download::start(
        Request {
            url: s.url("/gen/256K/master.mp4?seed=7").parse().unwrap(),
            headers: Default::default(),
            dest: Destination::Free(dest.clone()),
        },
        fast_opts(),
    );
    let (file, path) = view_and_outcome(dl).await;
    assert_eq!(path, dir.path().join("master (2).mp4"));
    assert_eq!(file.as_deref(), Some(path.as_path()));
    assert_eq!(std::fs::read(&dest).unwrap(), b"someone else's");
    assert_eq!(std::fs::read(&part).unwrap(), b"another download's");
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 256 * 1024);
    // Free, it keeps its name.
    let free = dir.path().join("free.mp4");
    let dl = Download::start(
        Request {
            url: s.url("/gen/64K/free.mp4?seed=8").parse().unwrap(),
            headers: Default::default(),
            dest: Destination::Free(free.clone()),
        },
        fast_opts(),
    );
    assert_eq!(view_and_outcome(dl).await.1, free);
}

/// The same for a resumed download: the row follows, and the resume itself still works.
#[tokio::test]
async fn the_view_names_the_file_when_a_resume_finds_its_place_taken() {
    let s = server("rate=1M").await;
    let (dir, state) = paused(&s, "/gen/8M/master.mp4?seed=13").await;
    let dest = mdm_engine::state::ResumeState::load(&state).unwrap().dest;
    std::fs::write(&dest, b"someone else's").unwrap();
    s.set_faults(Faults::default());
    let dl = Download::resume(state.clone(), Default::default(), fast_opts()).unwrap();
    let (file, path) = view_and_outcome(dl).await;
    assert_eq!(path.parent(), Some(dir.path()));
    assert_ne!(path, dest);
    assert_eq!(file.as_deref(), Some(path.as_path()));
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 8 << 20);
    assert!(!state.exists(), "state file removed on completion");
    assert_eq!(std::fs::read(&dest).unwrap(), b"someone else's");
}

#[tokio::test]
async fn pause_and_resume_continues_where_it_stopped() {
    let s = server("rate=1M").await;
    let (_dir, state) = paused(&s, "/gen/8M/r.bin?seed=7").await;
    let saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    let before = saved.downloaded();
    assert!(
        before > 0 && before < 8 << 20,
        "partial progress saved: {before}"
    );
    // Every request of the stopped download is in the log (`paused`): only the resume's follow.
    s.clear_log();

    let out = Download::resume(state.clone(), Default::default(), fast_opts())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let data = std::fs::read(&out.path).unwrap();
    let mut want = vec![0; 8 << 20];
    content::fill(7, 0, &mut want);
    assert!(data == want, "content mismatch after resume");
    assert!(!state.exists(), "state file removed on completion");
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("the resume's requests end");
    // Nothing that was on disk is asked for again, but for the few bytes before each part that
    // a connection re-reads to check the file is the same (verify_overlap). How much of the
    // rest comes twice depends on timing (a part split while its bytes are on the way), so a
    // total would be flaky on a busy machine (#123).
    let overlap = fast_opts().verify_overlap;
    for e in s.log() {
        assert!(
            on_disk(e.range.as_deref(), &saved) <= overlap,
            "the resume asked again for data it had: {:?} ({} on disk)",
            e.range,
            on_disk(e.range.as_deref(), &saved)
        );
    }
    assert!(!s.log().is_empty());
    assert!(
        s.log().iter().all(|e| e.if_range.is_some()),
        "resume validates with If-Range"
    );
}

#[tokio::test]
async fn resume_detects_changed_file() {
    let s = server("rate=1M").await;
    let (_dir, state) = paused(&s, "/gen/8M?seed=8").await;
    // Pretend the data so far was fetched under another version of the file (another ETag
    // and date; another ETag alone is what CDN nodes do — see per_node_etags_...).
    let mut saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    saved.validators.etag = Some("\"another-version\"".into());
    saved.validators.last_modified = Some("Wed, 21 Oct 2015 07:28:00 GMT".into());
    std::fs::write(&state, serde_json::to_vec(&saved).unwrap()).unwrap();
    let err = Download::resume(state, Default::default(), fast_opts())
        .unwrap()
        .wait()
        .await
        .unwrap_err();
    assert!(matches!(err, mdm_engine::Error::FileChanged), "{err}");
}

/// Pause, then drop the saved validator (a server without ETag/Last-Modified): the resume can
/// only notice a changed file through the overlap check.
async fn paused_without_validator(
    s: &LabServer,
    path: &str,
) -> (tempfile::TempDir, std::path::PathBuf) {
    let (dir, state) = paused(s, path).await;
    let mut saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    saved.validators = Default::default();
    std::fs::write(&state, serde_json::to_vec(&saved).unwrap()).unwrap();
    (dir, state)
}

#[tokio::test]
async fn resume_without_validator_checks_overlap_and_completes() {
    let s = server("rate=1M").await;
    let (_dir, state) = paused_without_validator(&s, "/gen/8M/o.bin?seed=11").await;
    s.clear_log();
    let out = Download::resume(state, Default::default(), fast_opts())
        .unwrap()
        .wait()
        .await
        .unwrap();
    let mut want = vec![0; 8 << 20];
    content::fill(11, 0, &mut want);
    assert!(
        std::fs::read(&out.path).unwrap() == want,
        "content mismatch"
    );
}

#[tokio::test]
async fn resume_overlap_check_detects_changed_data() {
    let s = server("rate=1M").await;
    let (_dir, state) = paused_without_validator(&s, "/gen/8M/c.bin?seed=12").await;
    // Make the data on disk differ from the server's just before every resume point, as if
    // the file had changed on the server since.
    let mut saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    let part = mdm_engine::naming::part_path(&saved.dest);
    let mut bytes = std::fs::read(&part).unwrap();
    // Every part with data resumes inside it: a finished one is saved as half done (its data is
    // on disk all the same). Which parts are under way when the download stops depends on the
    // machine's speed (#123: on a busy one only the first connection had any).
    for seg in &mut saved.segments {
        if seg.cursor == seg.end && seg.end - seg.start > 4096 {
            seg.cursor = seg.start + (seg.end - seg.start) / 2;
        }
    }
    std::fs::write(&state, serde_json::to_vec(&saved).unwrap()).unwrap();
    let mut flipped = 0;
    for seg in &saved.segments {
        if seg.cursor > seg.start && seg.cursor < seg.end {
            bytes[seg.cursor as usize - 1] ^= 0xff;
            flipped += 1;
        }
    }
    assert!(flipped > 0, "some segment was partly downloaded");
    std::fs::write(&part, bytes).unwrap();
    let err = Download::resume(state, Default::default(), fast_opts())
        .unwrap()
        .wait()
        .await
        .unwrap_err();
    assert!(matches!(err, mdm_engine::Error::FileChanged), "{err}");
}

#[tokio::test]
async fn cookies_do_not_follow_a_cross_host_redirect() {
    let s = server("").await;
    let port = s.addr().port();
    // 127.0.0.1 redirects to localhost: another host, as when a site hands off to a CDN.
    let target = format!("http://localhost:{port}/gen/2M/r.bin?seed=3");
    let path = format!(
        "/redirect?to={}",
        target
            .replace(':', "%3A")
            .replace('/', "%2F")
            .replace('?', "%3F")
            .replace('=', "%3D")
    );
    let dir = tempfile::tempdir().unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::COOKIE, "sid=secret".parse().unwrap());
    let req = Request {
        url: s.url(&path).parse().unwrap(),
        headers,
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let out = Download::start(req, fast_opts()).wait().await.unwrap();
    let mut want = vec![0; 2 << 20];
    content::fill(3, 0, &mut want);
    assert!(
        std::fs::read(&out.path).unwrap() == want,
        "content mismatch"
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let log = s.log();
    assert!(!log.is_empty());
    assert!(
        log.iter().all(|e| e.cookie.is_none()),
        "the cookie for 127.0.0.1 reached localhost: {:?}",
        log.iter().map(|e| &e.cookie).collect::<Vec<_>>()
    );
}

/// `/redirect?to=<target>` with the target percent-encoded.
fn redirect_path(target: &str) -> String {
    let enc: String = target
        .chars()
        .map(|c| match c {
            ':' => "%3A".to_owned(),
            '/' => "%2F".to_owned(),
            '?' => "%3F".to_owned(),
            '=' => "%3D".to_owned(),
            '&' => "%26".to_owned(),
            c => c.to_string(),
        })
        .collect();
    format!("/redirect?to={enc}")
}

/// Every kind of redirect in one chain (#28): 301, 307, a page with a meta refresh (and a wait),
/// 308, then a page with a `Refresh` header that sets a cookie, and the file; named by its own
/// address, its wait kept, the cookie on every connection.
#[tokio::test]
async fn every_kind_of_redirect_is_followed() {
    let s = server("").await;
    let enc = |u: &str| url::form_urlencoded::byte_serialize(u.as_bytes()).collect::<String>();
    let file = s.url("/gen/8M/the-real-name.bin?seed=31");
    let header_page = s.url(&format!(
        "/hop?how=header&cookie={}&to={}",
        enc("step=2"),
        enc(&file)
    ));
    let to_header_page = s.url(&format!("/redirect?code=308&to={}", enc(&header_page)));
    let meta_page = s.url(&format!("/hop?how=meta&wait=1&to={}", enc(&to_header_page)));
    let to_meta_page = s.url(&format!("/redirect?code=307&to={}", enc(&meta_page)));
    let start = format!("/redirect?code=301&to={}", enc(&to_meta_page));

    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url(&start).parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let started = std::time::Instant::now();
    let out = Download::start(req, fast_opts()).wait().await.unwrap();
    // The meta page's 1-second wait was kept.
    assert!(started.elapsed() >= std::time::Duration::from_secs(1));
    assert_eq!(out.path.file_name().unwrap(), "the-real-name.bin");
    let mut want = vec![0; 8 << 20];
    content::fill(31, 0, &mut want);
    assert!(
        std::fs::read(&out.path).unwrap() == want,
        "content mismatch"
    );
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let log = s.log();
    // Several connections fetched the file, every one with the page's cookie.
    assert!(log.len() > 1, "one connection only: {}", log.len());
    assert!(
        log.iter().all(|e| e.cookie.as_deref() == Some("step=2")),
        "the page's cookie went on to the file: {:?}",
        log.iter().map(|e| &e.cookie).collect::<Vec<_>>()
    );
}

/// A web page that leads nowhere is the download itself, asked for again after the look.
#[tokio::test]
async fn a_plain_web_page_is_still_downloaded() {
    let s = server("").await;
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/8K/page.html?seed=32").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let out = Download::start(req, fast_opts()).wait().await.unwrap();
    assert_eq!(out.size, 8 << 10);
}

#[tokio::test]
async fn signed_links_that_expire_while_downloading_are_refreshed() {
    // Each redirect hands out a link good for 4 requests; connections drop every 256 KB, so they
    // keep asking for new ranges after their link expired. Counted in requests, not time (#201:
    // a link valid for 400 ms could expire on a starved CPU before the refresh that asked for it
    // had used it, and the download failed with LinkExpired).
    let s = server("rate=1M&drop_after=256K").await;
    let target = s.url("/gen/4M/signed.bin?seed=23");
    let path = format!("{}&uses=4", redirect_path(&target));
    let opts = Options {
        max_connections: 4,
        ..fast_opts()
    };
    let (log, size) = fetch(&s, &path, 23, opts).await;
    assert_eq!(size, 4 << 20);
    assert!(
        log.iter().any(|e| e.outcome == "expired"),
        "a link expired during the download"
    );
}

#[tokio::test]
async fn a_link_that_cannot_be_refreshed_reports_link_expired() {
    let s = server("rate=1M").await;
    let target = s.url("/gen/8M/x.bin?seed=24");
    let (_dir, state) = paused(&s, &redirect_path(&target)).await;
    // The stored link has expired and the original address is gone too.
    let mut saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    saved.final_url = format!("{target}&expires=1").parse().unwrap();
    saved.url = s.url("/gone").parse().unwrap();
    std::fs::write(&state, serde_json::to_vec(&saved).unwrap()).unwrap();
    let err = Download::resume(state.clone(), Default::default(), fast_opts())
        .unwrap()
        .wait()
        .await
        .unwrap_err();
    assert!(matches!(err, mdm_engine::Error::LinkExpired), "{err}");
    assert!(state.exists(), "the data is kept for a fresh address");
}

#[tokio::test]
async fn resume_follows_the_original_address_when_the_redirected_one_expired() {
    let s = server("rate=1M").await;
    let target = s.url("/gen/8M/e.bin?seed=14");
    let (_dir, state) = paused(&s, &redirect_path(&target)).await;
    // The signed CDN link has expired: the stored final address now answers 404.
    let mut saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    assert_ne!(saved.url, saved.final_url, "the download was redirected");
    saved.final_url = s.url("/expired-link").parse().unwrap();
    std::fs::write(&state, serde_json::to_vec(&saved).unwrap()).unwrap();
    let out = Download::resume(state, Default::default(), fast_opts())
        .unwrap()
        .wait()
        .await
        .expect("resumed from the refreshed address");
    let mut want = vec![0; 8 << 20];
    content::fill(14, 0, &mut want);
    assert!(
        std::fs::read(&out.path).unwrap() == want,
        "content mismatch"
    );
}

#[tokio::test]
async fn a_dead_link_fails_fast_instead_of_retrying() {
    let s = server("rate=1M").await;
    let (_dir, state) = paused(&s, "/gen/8M/d.bin?seed=15").await;
    // The address now answers 404 (and there is no other address to fall back to); the server
    // logs it ("gone" is no size).
    let mut saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    let gone: url::Url = s.url("/gen/gone/d.bin").parse().unwrap();
    saved.url = gone.clone();
    saved.final_url = gone;
    std::fs::write(&state, serde_json::to_vec(&saved).unwrap()).unwrap();
    s.clear_log();
    // Default options: a retry would wait 1 s first, then longer.
    let err = Download::resume(state, Default::default(), Options::default())
        .unwrap()
        .wait()
        .await
        .unwrap_err();
    assert!(
        matches!(err, mdm_engine::Error::Status(s) if s.as_u16() == 404),
        "{err}"
    );
    asked_once(&s).await;
}

/// Every request the server got since its log was cleared asks for another part: nothing was
/// asked again, so nothing was retried. Counted, not timed (#201: a 3 s budget, against the 1 s
/// a first retry waits, failed on a starved CPU).
async fn asked_once(s: &LabServer) {
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("every request ends");
    let log = s.log();
    let asked: Vec<_> = log
        .iter()
        .map(|e| (e.range.clone(), e.status, e.outcome.clone()))
        .collect();
    let mut ranges: Vec<_> = log.iter().map(|e| e.range.clone()).collect();
    ranges.sort();
    ranges.dedup();
    assert!(!log.is_empty(), "nothing was asked");
    assert_eq!(ranges.len(), log.len(), "a part was asked again: {asked:?}");
}

/// Point a paused download's state at its own address plus the fault overrides `faults`.
fn with_faults(state: &std::path::Path, faults: &str) {
    let mut saved = mdm_engine::state::ResumeState::load(state).unwrap();
    for url in [&mut saved.url, &mut saved.final_url] {
        let q = format!("{}&{faults}", url.query().unwrap_or(""));
        url.set_query(Some(&q));
    }
    std::fs::write(state, serde_json::to_vec(&saved).unwrap()).unwrap();
}

#[tokio::test]
async fn a_web_page_instead_of_the_file_stops_the_resume() {
    let s = server("rate=1M").await;
    let (_dir, state) = paused(&s, "/gen/8M/w.bin?seed=16").await;
    // The link now leads to a sign-in page.
    with_faults(&state, "html=true");
    s.clear_log();
    // Default options: a retry would wait 1 s first, then longer.
    let err = Download::resume(state.clone(), Default::default(), Options::default())
        .unwrap()
        .wait()
        .await
        .unwrap_err();
    assert!(matches!(err, mdm_engine::Error::GotWebPage), "{err}");
    asked_once(&s).await;
    assert!(state.exists(), "the data is kept for a later resume");
}

#[tokio::test]
async fn a_server_that_sometimes_ignores_ranges_still_completes() {
    let s = server("rate=1M").await;
    let (_dir, state) = paused(&s, "/gen/8M/r.bin?seed=17").await;
    // Every other request is answered in full (same file, same validator): not a changed file.
    with_faults(&state, "no_range_every=2");
    let out = Download::resume(state, Default::default(), fast_opts())
        .unwrap()
        .wait()
        .await
        .expect("completes");
    let mut want = vec![0; 8 << 20];
    content::fill(17, 0, &mut want);
    assert!(
        std::fs::read(&out.path).unwrap() == want,
        "content mismatch"
    );
}

#[tokio::test]
async fn a_missing_folder_is_reported_and_nothing_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("unplugged");
    let err = Download::resume(gone.join("a.bin.mdm.json"), Default::default(), fast_opts())
        .err()
        .expect("refused");
    assert!(
        matches!(&err, mdm_engine::Error::DestinationUnavailable(p) if *p == gone),
        "{err}"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn a_path_too_long_for_windows_is_refused_before_anything_is_created() {
    let s = server("").await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join(format!("{}.bin", "n".repeat(250)));
    let req = Request {
        url: s.url("/gen/64K/x.bin").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::File(dest),
    };
    let err = Download::start(req, fast_opts()).wait().await.unwrap_err();
    assert!(matches!(err, mdm_engine::Error::PathTooLong(_)), "{err}");
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn a_file_bigger_than_the_drive_is_refused_before_it_starts() {
    let s = server("").await;
    let dir = tempfile::tempdir().unwrap();
    // 1 PiB: no test machine has that free.
    let req = Request {
        url: s.url("/gen/1125899906842624/huge.bin").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let err = Download::start(req, fast_opts()).wait().await.unwrap_err();
    assert!(matches!(err, mdm_engine::Error::NoSpace { .. }), "{err}");
    assert!(
        std::fs::read_dir(dir.path()).unwrap().next().is_none(),
        "nothing is left behind"
    );
}

#[tokio::test]
async fn cookies_are_sent_to_their_own_host() {
    let s = server("").await;
    let dir = tempfile::tempdir().unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::COOKIE, "sid=secret".parse().unwrap());
    let req = Request {
        url: s.url("/gen/2M/c.bin?seed=4").parse().unwrap(),
        headers,
        dest: Destination::Dir(dir.path().to_owned()),
    };
    Download::start(req, fast_opts()).wait().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        s.log()
            .iter()
            .all(|e| e.cookie.as_deref() == Some("sid=secret"))
    );
}

#[tokio::test]
async fn a_download_completing_during_a_progress_save_finishes() {
    // #116: progress saved every millisecond, so one is under way (flushing the part file) as
    // the last bytes arrive. The download must wait for it, then finish: no "part file still
    // in use", and no progress file written beside the finished file afterwards (#106).
    let s = server("rate=4M").await;
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/8M/s.bin?seed=34").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let opts = Options {
        save_interval: std::time::Duration::from_millis(1),
        ..fast_opts()
    };
    let out = Download::start(req, opts).wait().await.expect("finished");
    assert_eq!(std::fs::metadata(&out.path).unwrap().len(), 8 << 20);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        !mdm_engine::state::state_path(&out.path).exists(),
        "no progress file beside the finished file"
    );
}

#[tokio::test]
async fn crash_after_the_final_rename_completes_on_resume() {
    let s = server("rate=1M").await;
    let (_dir, state) = paused(&s, "/gen/8M/f.bin?seed=13").await;
    // Simulate a crash right after the part file was renamed, before the state was removed.
    let saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    std::fs::rename(mdm_engine::naming::part_path(&saved.dest), &saved.dest).unwrap();
    let out = Download::resume(state.clone(), Default::default(), fast_opts())
        .unwrap()
        .wait()
        .await
        .expect("recognised as complete");
    assert_eq!(out.path, saved.dest);
    assert!(!state.exists(), "state removed");
}

#[tokio::test]
async fn resume_rejects_damaged_part_file() {
    let s = server("rate=1M").await;
    let (_dir, state) = paused(&s, "/gen/8M?seed=9").await;
    let saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(mdm_engine::naming::part_path(&saved.dest))
        .unwrap()
        .set_len(10)
        .unwrap();
    let err = Download::resume(state, Default::default(), fast_opts())
        .unwrap()
        .wait()
        .await
        .unwrap_err();
    assert!(matches!(err, mdm_engine::Error::BadState(_)), "{err}");
}

#[tokio::test]
async fn changed_file_fails() {
    // Requests 1–4 see the original validators (probe + 3 range answers confirm ranges);
    // from request 5 on the file has "changed", so If-Range yields 200.
    let s = server("rate=2M&etag_change_every=5").await;
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/4M").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let err = Download::start(req, fast_opts()).wait().await.unwrap_err();
    assert!(matches!(err, mdm_engine::Error::FileChanged), "{err}");
}

#[tokio::test]
async fn speed_limit_caps_the_whole_download() {
    // 8 connections, but the limit is per download (IDM: "for one file").
    let s = server("").await;
    // The download alone is timed, not the wait for the server's log (#201: `fetch` waits for
    // it, which a starved CPU stretched to seconds).
    let timed = |path: &str, seed: u64, opts: Options| {
        let dir = tempfile::tempdir().unwrap();
        let req = Request {
            url: s.url(path).parse().unwrap(),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        };
        async move {
            let started = std::time::Instant::now();
            let out = Download::start(req, opts)
                .wait()
                .await
                .expect("download succeeds");
            let secs = started.elapsed().as_secs_f64();
            let mut want = vec![0; 2 << 20];
            content::fill(seed, 0, &mut want);
            assert!(
                std::fs::read(&out.path).unwrap() == want,
                "content mismatch"
            );
            drop(dir);
            secs
        }
    };
    // At the same time, a yardstick: the same 2 MiB at 1 MiB/s, paced by the server on one
    // connection, with no limit of ours. It takes 2 s on an idle machine, and as long as the
    // machine makes it on a busy one.
    // An unlimited global cap beside the limit counts the bytes the connections pay for.
    let paid = std::sync::Arc::new(mdm_engine::limiter::Limiter::new(None));
    let limited = Options {
        speed_limit: Some(1 << 20),
        global: Some(paid.clone()),
        ..fast_opts()
    };
    let one = Options {
        max_connections: 1,
        ..fast_opts()
    };
    let (secs, paced) = tokio::join!(
        timed("/gen/2M/limited.bin?seed=12", 12, limited),
        timed("/gen/2M/paced.bin?seed=13&rate=1M", 13, one),
    );
    // At least the time the limit allows: the budget builds up from the download's start, so
    // 2 MiB can't come in under 2 s (without the limit: well under a second).
    assert!(secs >= 1.9, "2 MB at 1 MB/s took {secs:.2}s");
    // Every byte kept was paid for once: no byte waits for the limit twice (or for bytes thrown
    // away), whatever the machine's speed.
    assert_eq!(paid.taken(), 2 << 20, "bytes paid for");
    // The upper bound only catches a limit far too strict: 5 times the yardstick, about 10 s on
    // an idle machine (#201: a fixed 6 s failed on a starved CPU or a busy CI runner, which took
    // 6-17 s; there the limited download took up to 3.9 times as long as the yardstick).
    assert!(
        secs < 5.0 * paced,
        "2 MB at 1 MB/s took {secs:.2}s (paced by the server: {paced:.2}s)"
    );
}

/// #39: under a low limit each connection waits seconds for its turn; Stop ends those waits
/// at once instead of waiting them out (it took about 4 s at 400 KB/s in the app).
#[tokio::test]
async fn stop_is_prompt_under_a_low_limit() {
    let s = server("").await;
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/8M/slow.bin?seed=41").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    // The global cap starts unlimited: it only counts the bytes each connection pays for, until
    // the test needs a connection in a long wait.
    let global = std::sync::Arc::new(mdm_engine::limiter::Limiter::new(None));
    let opts = Options {
        speed_limit: Some(32 << 10),
        global: Some(global.clone()),
        ..fast_opts()
    };
    let dl = Download::start(req, opts);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while dl.snapshot().downloaded < 64 << 10 {
        assert!(std::time::Instant::now() < deadline, "never started");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // A wait that can't pass for a slow machine (#201: a 1 s budget failed on a starved CPU,
    // where the stop itself took up to 3.4 s, about as long as the waits it must not sit out).
    // The cap goes to 32 KB/s with 4 MiB owed: any connection paying from now on waits two
    // minutes. Stop is pressed once one of them is in that wait.
    global.set(Some(32 << 10));
    let _ = tokio::time::timeout(std::time::Duration::ZERO, global.acquire(4 << 20)).await;
    let owed = global.taken();
    while global.taken() == owed {
        assert!(
            std::time::Instant::now() < deadline,
            "no connection paid again"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    // Measured from Stop: a stop that waited would take 2 minutes, a prompt one well under 10 s
    // however busy the machine (it still writes what arrived, and the resume state).
    dl.cancel();
    let stopped = tokio::time::timeout(std::time::Duration::from_secs(10), dl.wait())
        .await
        .expect("Stop waited out the speed limit");
    assert!(
        matches!(stopped, Err(mdm_engine::Error::Cancelled)),
        "{:?}",
        stopped.map(|o| o.size)
    );
}

/// #621: a limit raised or removed while connections wait under the old one applies to those
/// waits at once. Here they wait at 256 bytes/s, where a chunk owes half a minute (the wait used
/// to be sat out: 1 MiB took 29 s after the limit went up), then the limit goes away (the global
/// cap) or up four million times (this download's).
#[tokio::test]
async fn a_raised_limit_wakes_the_connections_waiting() {
    let s = server("").await;
    for (seed, global) in [(44u64, true), (43, false)] {
        let dir = tempfile::tempdir().unwrap();
        let req = Request {
            url: s
                .url(&format!("/gen/1M/raised.bin?seed={seed}"))
                .parse()
                .unwrap(),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        };
        let cap = std::sync::Arc::new(mdm_engine::limiter::Limiter::new(Some(256)));
        let opts = Options {
            speed_limit: (!global).then_some(256),
            global: global.then(|| cap.clone()),
            ..fast_opts()
        };
        let dl = Download::start(req, opts);
        // Long enough for the connections to be waiting their turn.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let before = dl.snapshot().downloaded;
        let raised = std::time::Instant::now();
        if global {
            cap.set(None);
        } else {
            dl.set_speed_limit(Some(1 << 30));
        }
        let out = tokio::time::timeout(std::time::Duration::from_secs(60), dl.wait())
            .await
            .expect("still waiting a minute after the limit went up")
            .expect("download succeeds");
        let took = raised.elapsed();
        println!("global: {global}, {before} bytes before, the rest in {took:?}");
        assert_eq!(out.size, 1 << 20);
        let mut want = vec![0; 1 << 20];
        content::fill(seed, 0, &mut want);
        assert!(
            std::fs::read(&out.path).unwrap() == want,
            "content mismatch"
        );
        // Unlimited, 1 MiB from this machine takes well under a second; the slack is for a
        // busy one. The old waits took a minute.
        assert!(
            took < std::time::Duration::from_secs(10),
            "global: {global}, took {took:?} after the limit went up"
        );
    }
}

#[tokio::test]
async fn the_finished_file_can_take_the_servers_date() {
    // Options → Downloads → "Set file creation date as provided by the server": the lab server
    // dates its files 1_790_000_000 s after the epoch.
    let s = server("").await;
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/1M/dated.bin?seed=40").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let opts = Options {
        server_time: true,
        ..fast_opts()
    };
    let out = Download::start(req, opts)
        .wait()
        .await
        .expect("download succeeds");
    let meta = std::fs::metadata(&out.path).unwrap();
    let server = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_790_000_000);
    assert_eq!(meta.modified().unwrap(), server);
    #[cfg(windows)]
    assert_eq!(meta.created().unwrap(), server);
    // Without the option the file keeps the time it was written.
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: s.url("/gen/1M/undated.bin?seed=41").parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let out = Download::start(req, fast_opts()).wait().await.unwrap();
    assert_ne!(
        std::fs::metadata(&out.path).unwrap().modified().unwrap(),
        server
    );
}

#[tokio::test]
async fn a_pac_script_routes_through_its_proxy() {
    // The lab server stands in for the proxy: a proxy is sent the whole address and the lab
    // server answers by path, so a host that can't exist only works through the script's proxy.
    let s = server("").await;
    let port = s.url("/").parse::<url::Url>().unwrap().port().unwrap();
    let script = format!(
        "function FindProxyForURL(url, host) {{ \
           return dnsDomainIs(host, '.invalid') ? 'PROXY 127.0.0.1:{port}' : 'DIRECT'; }}"
    );
    let opts = Options {
        proxy: mdm_engine::Proxy::Pac {
            script: std::sync::Arc::from(script.as_str()),
        },
        max_connections: 1,
        ..fast_opts()
    };
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url: "http://nowhere.invalid/gen/64K?seed=23".parse().unwrap(),
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    let out = Download::start(req, opts)
        .wait()
        .await
        .expect("downloaded through the proxy");
    let mut want = vec![0; 64 << 10];
    content::fill(23, 0, &mut want);
    assert!(
        std::fs::read(&out.path).unwrap() == want,
        "content mismatch"
    );
}

/// #28: a one-time link (the second request for it is refused). The look Download File Info
/// makes is kept and the download reads it, with one connection or several (the others are
/// refused and connection 1 goes on alone); without it the download is refused, as reported.
#[tokio::test]
async fn a_one_time_link_is_read_from_the_kept_answer() {
    let s = server("").await;
    for (n, connections) in [(1u64, 1u32), (2, 8)] {
        let url: url::Url = s
            .url(&format!("/gen/2M/once{n}.zip?once=1&seed={n}"))
            .parse()
            .unwrap();
        let opts = Options {
            max_connections: connections,
            ..fast_opts()
        };
        let (info, kept) = mdm_engine::probe_kept(url.clone(), Default::default(), &opts)
            .await
            .unwrap();
        assert_eq!(info.size, Some(2 << 20));
        assert!(kept.is_some(), "a 200 from byte 0 is kept");
        let dir = tempfile::tempdir().unwrap();
        let req = Request {
            url: url.clone(),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        };
        let out = Download::start_with(req, opts.clone(), kept)
            .wait()
            .await
            .unwrap_or_else(|e| panic!("{connections} connection(s): {e}"));
        let data = std::fs::read(&out.path).unwrap();
        let mut want = vec![0; data.len()];
        content::fill(n, 0, &mut want);
        assert!(data == want && data.len() == 2 << 20, "content");
    }
    // Asked again (the look's answer not kept): refused.
    let url: url::Url = s.url("/gen/1M/once9.zip?once=1&seed=9").parse().unwrap();
    mdm_engine::probe(url.clone(), Default::default(), &fast_opts())
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let req = Request {
        url,
        headers: Default::default(),
        dest: Destination::Dir(dir.path().to_owned()),
    };
    assert!(Download::start(req, fast_opts()).wait().await.is_err());
}

/// A download watched from outside while it runs (`a_slow_flush_does_not_stop_the_transfer`).
struct Watched {
    /// From the start until every byte had arrived (`Snapshot::downloaded` counts the bytes
    /// waiting in memory for the disk too): the transfer, without what the disk takes after it.
    received: std::time::Duration,
    /// While the first slow flush held the writes (`gate` taken exclusively): the bytes that
    /// arrived meanwhile, and the bytes still to come when it began.
    during_flush: Option<(u64, u64)>,
}

async fn watch(
    d: Download,
    size: u64,
    gate: Option<std::sync::Arc<std::sync::RwLock<()>>>,
) -> (Result<mdm_engine::Outcome, mdm_engine::Error>, Watched) {
    use mdm_engine::download::State;
    let started = std::time::Instant::now();
    let mut received = None;
    // `downloaded` when the flush was first seen holding the gate.
    let mut flush_began: Option<u64> = None;
    let mut during_flush = None;
    loop {
        let v = d.snapshot();
        let flushing = gate.as_ref().is_some_and(|g| g.try_read().is_err());
        if flushing && flush_began.is_none() && during_flush.is_none() {
            flush_began = Some(v.downloaded);
        } else if !flushing && let Some(at) = flush_began.take() {
            during_flush = Some((v.downloaded - at, size - at));
        }
        if received.is_none() && v.downloaded >= size {
            received = Some(started.elapsed());
        }
        if matches!(v.state, State::Done | State::Failed | State::Cancelled) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    let received = received.unwrap_or_else(|| started.elapsed());
    (
        d.wait().await,
        Watched {
            received,
            during_flush,
        },
    )
}

/// #361: a flush that holds up every write (as on NTFS) must not hold up the download: the
/// connections keep receiving meanwhile and none of them reconnects for it. Before: the
/// connections stood still during each 1.5 s flush, one per save tick, and were then told apart
/// as slow (reconnections, on top of the stalls).
///
/// It measures the transfer, not the disk: the bytes received while the flush runs, and the
/// time until every byte arrived. The time until the download completes also counts the real
/// disk taking in 160 MB: on GitHub's Windows runners that alone took 1 s to over 13 s (Windows
/// held the writes into its cache to the disk's pace: 19 MB writes took up to 5.9 s with no
/// flush running), and failed the old wall-clock bound now and then (#518): 7.9 s against
/// 2.6 s, with every byte received in 1.4 s against 1.0 s.
#[tokio::test]
async fn a_slow_flush_does_not_stop_the_transfer() {
    const SIZE: u64 = 160 << 20;
    let s = server("rate=20M").await; // 8 connections, about 160 MB/s together
    let dir = tempfile::tempdir().unwrap();
    // The same download with a disk that never stalls, on this machine at this moment: the
    // yardstick the slow flush is measured against (#518: a fixed limit failed on busy runners).
    let (done, yardstick) = watch(
        Download::start(
            Request {
                url: s.url("/gen/160M/a.bin?seed=518").parse().unwrap(),
                headers: Default::default(),
                dest: Destination::Dir(dir.path().to_owned()),
            },
            Options {
                save_interval: std::time::Duration::from_millis(100),
                ..fast_opts()
            },
        ),
        SIZE,
        None,
    )
    .await;
    done.expect("the yardstick download succeeds");
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("every request ends");
    let before = s.log().len();
    let slow = mdm_engine::download::SlowFlush {
        takes: std::time::Duration::from_millis(1500),
        gate: Default::default(),
    };
    let gate = slow.gate.clone();
    let opts = Options {
        save_interval: std::time::Duration::from_millis(100),
        slow_flush: Some(slow),
        ..fast_opts()
    };
    let (out, slowed) = watch(
        Download::start(
            Request {
                url: s.url("/gen/160M/a.bin?seed=361").parse().unwrap(),
                headers: Default::default(),
                dest: Destination::Dir(dir.path().to_owned()),
            },
            opts,
        ),
        SIZE,
        Some(gate),
    )
    .await;
    let out = out.expect("download succeeds");
    assert_eq!(out.size, SIZE);
    let data = std::fs::read(&out.path).unwrap();
    let mut want = vec![0; data.len()];
    content::fill(361, 0, &mut want);
    assert!(data == want, "content mismatch");
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("every request ends");
    let log = s.log().split_off(before);
    // The connections kept receiving while the flush held their writes: 16 MiB in its 1.5 s
    // (or the rest of the file), where connections waiting for the disk take in a batch each at
    // most (8 x 256 KiB). Measured: 143 MB; with the connections writing during the flush, 0.6
    // to 2.8 MB (and every byte in 2.2 s against 1.0 s, which the bound below lets through).
    let (grew, left) = slowed
        .during_flush
        .expect("a slow flush ran while the connections received");
    assert!(
        grew >= (16 << 20).min(left),
        "{grew} bytes received during the slow flush ({left} left when it began)"
    );
    // Every byte arrived about as soon as without the slow flush, with room for a busy machine:
    // 4.5 s more, or most of the yardstick again when it is long (a busy runner slows this run as
    // much as the yardstick's). Before the fix every tick's flush stalled all writes for 1.5 s,
    // one per 0.1 s tick: 7.6 s measured against about 1 s.
    let allowed = (yardstick.received + std::time::Duration::from_millis(4500))
        .max(yardstick.received.mul_f64(1.8) + std::time::Duration::from_millis(1500));
    assert!(
        slowed.received < allowed,
        "every byte in {:?}, the same download without the slow flush {:?}",
        slowed.received,
        yardstick.received
    );
    // No connection reconnected because of the disk's stall: the probe, seven more, and the
    // planner's closing splits only.
    assert!(log.len() < 48, "{} requests: {log:?}", log.len());
}

/// #372: a server that refuses a browser's User-Agent with 403 (seen on a public
/// PDF) is asked once more with MDM's own, and the download goes on.
#[tokio::test]
async fn a_refused_browser_agent_is_asked_again_as_mdm() {
    let s = server("rate=20M&refuse_agent=Chrome").await;
    let browser = "Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 Chrome/156.0 Safari/537.36";
    let dir = tempfile::tempdir().unwrap();
    // Without the fallback, the 403 is the end of it.
    let refused = Download::start(
        Request {
            url: s.url("/gen/2M/a.bin?seed=372").parse().unwrap(),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        Options {
            user_agent: browser.to_owned(),
            automatic_agent: false,
            ..fast_opts()
        },
    )
    .wait()
    .await;
    assert!(
        matches!(refused, Err(mdm_engine::Error::Status(s)) if s.as_u16() == 403),
        "{refused:?}"
    );
    // With it, the same download succeeds under MDM's agent, every connection.
    let out = Download::start(
        Request {
            url: s.url("/gen/2M/a.bin?seed=372").parse().unwrap(),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        Options {
            user_agent: browser.to_owned(),
            automatic_agent: false,
            agent_fallback: Some("MDM-test".to_owned()),
            ..fast_opts()
        },
    )
    .wait()
    .await
    .expect("download succeeds after the fallback");
    assert_eq!(out.size, 2 << 20);
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("every request ends");
    let log = s.log();
    assert!(
        log.iter().any(|e| e.outcome.contains("refuse_agent")),
        "{log:?}"
    );
    assert!(
        log.iter()
            .filter(|e| e.status == 200 || e.status == 206)
            .all(|e| e.user_agent.as_deref() == Some("MDM-test")),
        "{log:?}"
    );
}

/// A download handed over by the browser: `link` with a session cookie and the page as Referer,
/// and where the browser's own download of it ended (`browser_final`).
async fn handed_over(
    link: String,
    browser_final: Option<&str>,
    dir: &std::path::Path,
) -> Result<mdm_engine::Outcome, mdm_engine::Error> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::COOKIE, "sid=1".parse().unwrap());
    headers.insert(
        reqwest::header::REFERER,
        "http://127.0.0.1/program/".parse().unwrap(),
    );
    Download::start(
        Request {
            url: link.parse().unwrap(),
            headers,
            dest: Destination::Dir(dir.to_owned()),
        },
        Options {
            browser_final: browser_final.map(|u| u.parse().unwrap()),
            ..fast_opts()
        },
    )
    .wait()
    .await
}

/// #630: a site that refuses MDM the link (FileHippo's 412) while the browser's own download of
/// it ended on another host (its CDN): the download goes on from there, without the link's
/// cookie (another host) but with the page as Referer.
#[tokio::test]
async fn a_refused_link_goes_on_where_the_browser_download_ended() {
    let s = server("rate=20M").await;
    let port = s.addr().port();
    let link = s.url("/gen/2M/fh.bin?seed=630&refuse=412");
    let cdn = format!("http://localhost:{port}/gen/2M/fh.bin?seed=630");
    let dir = tempfile::tempdir().unwrap();
    // Without the browser's address, the 412 is the end of it (what 0.5.2 did).
    let refused = handed_over(link.clone(), None, dir.path()).await;
    assert!(
        matches!(refused, Err(mdm_engine::Error::Status(s)) if s.as_u16() == 412),
        "{refused:?}"
    );
    let out = handed_over(link, Some(&cdn), dir.path())
        .await
        .expect("the download goes on where the browser's ended");
    let mut want = vec![0; 2 << 20];
    content::fill(630, 0, &mut want);
    assert!(
        std::fs::read(&out.path).unwrap() == want,
        "content mismatch"
    );
    assert_eq!(out.path.file_name().unwrap(), "fh.bin");
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("every request ends");
    let log = s.log();
    let refused = |browser_like: bool| {
        log.iter()
            .filter(|e| e.outcome == "fault:refuse" && e.sec_fetch_mode.is_some() == browser_like)
            .count()
    };
    assert_eq!(
        refused(false),
        2,
        "the link was asked once per download: {log:?}"
    );
    // Without the browser's address, it was asked once more the way a browser connects (#668),
    // refused too; with it, the browser's address got the file first.
    let once_more = if cfg!(feature = "browser-connection") {
        1
    } else {
        0
    };
    assert_eq!(refused(true), once_more, "{log:?}");
    let served: Vec<_> = log
        .iter()
        .filter(|e| e.status == 200 || e.status == 206)
        .collect();
    assert!(!served.is_empty());
    for e in served {
        assert!(
            e.host
                .as_deref()
                .is_some_and(|h| h.starts_with("localhost")),
            "{e:?}"
        );
        assert_eq!(e.cookie, None, "the link's cookie reached another host");
        assert_eq!(e.referer.as_deref(), Some("http://127.0.0.1/program/"));
    }
}

/// #630: a one-time link the browser already spent (410) is followed on the same site, where
/// the session's cookie still goes; when the browser's address is refused too, the link's own
/// refusal is the one told.
#[tokio::test]
async fn a_spent_link_goes_on_where_the_browser_download_ended() {
    let s = server("rate=20M").await;
    let link = s.url("/gen/1M/once.bin?seed=631&once=1");
    // The browser's own download used it up.
    let spent = reqwest::get(&link).await.unwrap();
    assert_eq!(spent.status(), 200);
    drop(spent.bytes().await);
    let dir = tempfile::tempdir().unwrap();
    let final_url = s.url("/gen/1M/once.bin?seed=631&require_cookie=sid=1");
    let out = handed_over(link.clone(), Some(&final_url), dir.path())
        .await
        .expect("the download goes on where the browser's ended");
    let mut want = vec![0; 1 << 20];
    content::fill(631, 0, &mut want);
    assert!(
        std::fs::read(&out.path).unwrap() == want,
        "content mismatch"
    );
    // Both refused: the link's 410 is told, not the other address's 403.
    let gone = s.url("/gen/1M/once.bin?seed=631&refuse=403");
    let refused = handed_over(link, Some(&gone), dir.path()).await;
    assert!(
        matches!(refused, Err(mdm_engine::Error::Status(s)) if s.as_u16() == 410),
        "{refused:?}"
    );
}
