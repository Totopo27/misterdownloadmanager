//! Video downloads keep to MDM's speed limits as they change (#708): the real yt-dlp, started
//! through `mdm_tools::video` as the app starts it, against the lab server, its speed measured
//! from the bytes it reports. Nothing leaves this computer.
//!
//! Ignored: it needs yt-dlp's program.
//!
//!   MDM_TEST_YTDLP=…\yt-dlp.exe cargo test -p mdm-tools --test pace -- --ignored --nocapture

use std::sync::Arc;
use std::time::{Duration, Instant};

use mdm_engine::limiter::Limiter;
use mdm_labserver::{Config, LabServer};
use mdm_tools::relay::Pace;
use mdm_tools::video::{State, Subs, VideoJob};

const KB: u64 = 1024;
const TWO: std::net::IpAddr = std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 2));

fn bound() -> mdm_engine::Route {
    mdm_engine::Route::Bound(Arc::new(mdm_engine::net::Binding {
        label: "Lab adapter".into(),
        name: "lo".into(),
        index: 1,
        v4: Some(std::net::Ipv4Addr::new(127, 0, 0, 2)),
        v6: None,
        loopback_too: true,
    }))
}
const MB: u64 = 1024 * 1024;

fn start(exe: &std::path::Path, url: &str, dir: &std::path::Path, pace: &Arc<Pace>) -> VideoJob {
    VideoJob::start(
        exe,
        url,
        "b",
        dir,
        "clip",
        Some(pace.clone()),
        4,
        None,
        None,
        &Default::default(),
        &Subs::default(),
    )
    .unwrap()
}

/// Wait until `job` has `bytes`; when it got there.
fn at(job: &VideoJob, bytes: u64) -> Instant {
    let end = Instant::now() + Duration::from_secs(120);
    loop {
        let p = job.progress();
        if p.downloaded >= bytes || matches!(p.state, State::Done(_)) {
            return Instant::now();
        }
        assert!(
            !matches!(p.state, State::Failed(_) | State::Stopped),
            "{:?}",
            p.state
        );
        assert!(Instant::now() < end, "stuck at {} bytes", p.downloaded);
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Wait for the end; the saved file's size.
fn done(job: &VideoJob) -> u64 {
    let end = Instant::now() + Duration::from_secs(120);
    loop {
        match job.progress().state {
            State::Done(f) => return std::fs::metadata(f).unwrap().len(),
            State::Running => {}
            s => panic!("{s:?}"),
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// KB/s between two counts.
fn rate(bytes: u64, from: Instant, to: Instant) -> f64 {
    bytes as f64 / KB as f64 / to.duration_since(from).as_secs_f64()
}

fn within(rate: f64, target: f64, share: f64) -> bool {
    (rate - target).abs() <= target * share
}

/// Each job's speed over the next `secs` seconds, KB/s: the slope of its byte count sampled
/// every 100 ms (least squares: yt-dlp reports in blocks, which a two-point difference would
/// make jumpy).
fn speeds(jobs: &[&VideoJob], secs: f64) -> Vec<f64> {
    let t0 = Instant::now();
    let mut samples: Vec<Vec<(f64, f64)>> = vec![Vec::new(); jobs.len()];
    while t0.elapsed().as_secs_f64() < secs {
        let t = t0.elapsed().as_secs_f64();
        for (j, s) in jobs.iter().zip(samples.iter_mut()) {
            s.push((t, j.progress().downloaded as f64 / KB as f64));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    samples
        .iter()
        .map(|s| {
            let n = s.len() as f64;
            let (mt, mb) = (
                s.iter().map(|p| p.0).sum::<f64>() / n,
                s.iter().map(|p| p.1).sum::<f64>() / n,
            );
            let cov: f64 = s.iter().map(|p| (p.0 - mt) * (p.1 - mb)).sum();
            let var: f64 = s.iter().map(|p| (p.0 - mt).powi(2)).sum();
            cov / var
        })
        .collect()
}

#[test]
#[ignore]
fn videos_keep_to_the_speed_limits() {
    let Ok(exe) = std::env::var("MDM_TEST_YTDLP") else {
        panic!("set MDM_TEST_YTDLP");
    };
    let exe = std::path::PathBuf::from(exe);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let lab = rt.block_on(LabServer::start(Config::default())).unwrap();
    let mut rows: Vec<String> = Vec::new();

    // 1. The Speed Limiter's 500 KB/s for one file, over 8 s once it runs (yt-dlp's start-up
    //    and the limiter's first second of allowance left out, as for every case).
    let dir = tempfile::tempdir().unwrap();
    let pace = Pace::new(Some(500 * KB), Some(Arc::new(Limiter::new(None))));
    let job = start(&exe, &lab.url("/gen/32M/one.mp4"), dir.path(), &pace);
    at(&job, 600 * KB);
    let limited = speeds(&[&job], 8.0)[0];
    rows.push(format!(
        "| Speed Limiter 500 KB/s, one video | {limited:.0} KB/s |"
    ));
    // 2. Turned off while it runs: the rest comes at once.
    let before = job.progress().downloaded;
    pace.set(None);
    let c = Instant::now();
    let size = done(&job);
    let lifted = rate(size - before, c, Instant::now());
    rows.push(format!(
        "| Then the limiter turned off: the rest ({:.1} MB) | {lifted:.0} KB/s |",
        (size - before) as f64 / MB as f64
    ));
    assert_eq!(size, 32 * MB);
    assert!(within(limited, 500.0, 0.10), "{limited:.0} KB/s");
    assert!(
        lifted > 5.0 * limited,
        "{lifted:.0} KB/s after the limit was lifted"
    );

    // 3. The global cap, 500 KB/s, two videos at once: shared, as file downloads share it.
    let dir = tempfile::tempdir().unwrap();
    let global = Arc::new(Limiter::new(Some(500 * KB)));
    let (pa, pb) = (
        Pace::new(None, Some(global.clone())),
        Pace::new(None, Some(global.clone())),
    );
    let (da, db) = (dir.path().join("a"), dir.path().join("b"));
    std::fs::create_dir_all(&da).unwrap();
    std::fs::create_dir_all(&db).unwrap();
    let ja = start(&exe, &lab.url("/gen/12M/a.mp4"), &da, &pa);
    let jb = start(&exe, &lab.url("/gen/12M/b.mp4"), &db, &pb);
    at(&ja, 300 * KB);
    at(&jb, 300 * KB);
    let r = speeds(&[&ja, &jb], 8.0);
    let (ra, rb) = (r[0], r[1]);
    rows.push(format!(
        "| Global cap 500 KB/s, two videos at once | {ra:.0} + {rb:.0} = {:.0} KB/s |",
        ra + rb
    ));
    assert!(within(ra + rb, 500.0, 0.10), "{:.0} KB/s together", ra + rb);
    assert!(within(ra, rb, 0.35), "shared: {ra:.0} and {rb:.0} KB/s");
    // 4. The other one stopped, and this one given its own 200 KB/s (the progress window's
    //    Speed Limiter tab) under the 500 KB/s cap: it keeps to its own.
    jb.stop();
    pa.set(Some(200 * KB));
    std::thread::sleep(Duration::from_millis(1500));
    let ra = speeds(&[&ja], 8.0)[0];
    rows.push(format!(
        "| Then the other stopped, this one given its own 200 KB/s | {ra:.0} KB/s |"
    ));
    assert!(within(ra, 200.0, 0.10), "{ra:.0} KB/s");
    ja.stop();

    // 5. Through a proxy with a login (Options -> Proxy / Socks, Manual; another relay stands
    //    in for it): the download goes on through it, at its pace.
    //    It leaves from 127.0.0.2 (an adapter's address standing in), so the lab's log shows the
    //    requests came through it (Windows; Linux routes loopback by destination).
    let proxy = mdm_tools::relay::Relay::start(Arc::new(|| (bound(), Default::default()))).unwrap();
    let u = url::Url::parse(&proxy.url()).unwrap();
    mdm_tools::proc::set_proxy(mdm_engine::Proxy::Manual {
        url: format!("http://{}", proxy.addr()),
        login: Some((u.username().into(), u.password().unwrap().into())),
        http: true,
        https: true,
    });
    let dir = tempfile::tempdir().unwrap();
    let pace = Pace::new(Some(300 * KB), None);
    let job = start(&exe, &lab.url("/gen/4M/proxied.mp4"), dir.path(), &pace);
    at(&job, 400 * KB);
    let proxied = speeds(&[&job], 8.0)[0];
    assert_eq!(done(&job), 4 * MB);
    mdm_tools::proc::set_proxy(mdm_engine::Proxy::None);
    rows.push(format!(
        "| Own limit 300 KB/s, through an HTTP proxy with a login | {proxied:.0} KB/s |"
    ));
    let through: Vec<_> = lab
        .log()
        .into_iter()
        .filter(|e| e.path == "/gen/4M/proxied.mp4")
        .map(|e| e.peer.ip())
        .collect();
    println!("proxied requests came from {through:?}");
    assert!(!through.is_empty());
    if cfg!(windows) {
        assert!(through.iter().all(|ip| *ip == TWO), "through the proxy");
    }
    assert!(within(proxied, 300.0, 0.10), "{proxied:.0} KB/s");

    println!("| Case | Measured |\n|---|---|");
    for r in rows {
        println!("{r}");
    }
}
