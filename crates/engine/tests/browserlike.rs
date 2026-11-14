//! #668: a site whose bot check refuses MDM's own connection (`Error::BotCheck`) is asked once
//! more the way a browser connects, and the download goes on through that connection only. The
//! lab server's `browser_only` fault stands in for the bot check: it refuses, with Cloudflare's
//! `cf-mitigated: challenge`, every request without the headers browsers send (`Sec-Fetch-Mode`).
#![cfg(feature = "browser-connection")]

use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

use mdm_engine::download::State;
use mdm_engine::{Destination, Download, Options, Request};
use mdm_labserver::{Config, Faults, LabServer, LogEntry, content};
use reqwest::header::{COOKIE, HeaderMap, REFERER, USER_AGENT};

const SETTLE: Duration = Duration::from_secs(20);
const CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";

async fn server(faults: &str) -> LabServer {
    LabServer::start(Config {
        faults: Faults::parse(faults).unwrap(),
        ..Config::default()
    })
    .await
    .unwrap()
}

fn opts() -> Options {
    Options {
        retry_delay: Duration::from_millis(50),
        user_agent: CHROME.to_owned(),
        ..Options::default()
    }
}

/// The browser's hand-over: its session cookie and the page as Referer.
fn handed_over() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(COOKIE, "sid=1".parse().unwrap());
    h.insert(REFERER, "http://127.0.0.1/app/".parse().unwrap());
    h
}

fn same_content(path: &std::path::Path, seed: u64, len: usize) {
    let data = std::fs::read(path).unwrap();
    let mut want = vec![0; len];
    content::fill(seed, 0, &mut want);
    assert!(data == want, "content mismatch");
}

async fn settled_log(s: &LabServer) -> Vec<LogEntry> {
    tokio::time::timeout(SETTLE, s.settled())
        .await
        .expect("every request ends");
    s.log()
}

#[tokio::test]
async fn a_bot_check_is_passed_the_way_a_browser_connects() {
    let s = server("browser_only=1&require_cookie=sid=1").await;
    let dir = tempfile::tempdir().unwrap();
    let dl = Download::start(
        Request {
            url: s.url("/gen/3M/app.apk?seed=668").parse().unwrap(),
            headers: handed_over(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
    );
    let out = dl
        .wait()
        .await
        .expect("the browser-like connection gets through");
    same_content(&out.path, 668, 3 << 20);
    let log = settled_log(&s).await;
    // MDM's own request first, refused; then the browser-like connection's, one at a time.
    assert_eq!(log[0].outcome, "fault:browser_only", "{log:?}");
    assert!(log[0].sec_fetch_mode.is_none());
    let through: Vec<_> = log[1..].iter().collect();
    assert!(!through.is_empty());
    for e in &through {
        assert!(e.sec_fetch_mode.is_some(), "{e:?}");
        // The download's own User-Agent, the browser's cookie for its site, its Referer.
        assert_eq!(e.user_agent.as_deref(), Some(CHROME), "{e:?}");
        assert_eq!(e.cookie.as_deref(), Some("sid=1"), "{e:?}");
        assert_eq!(e.referer.as_deref(), Some("http://127.0.0.1/app/"), "{e:?}");
        assert!(e.status == 200 || e.status == 206, "{e:?}");
    }
    assert_eq!(
        log.iter()
            .filter(|e| e.outcome == "fault:browser_only")
            .count(),
        1,
        "asked once more, not again and again: {log:?}"
    );
}

#[tokio::test]
async fn the_user_agent_handed_over_is_the_one_sent() {
    let s = server("browser_only=1").await;
    let dir = tempfile::tempdir().unwrap();
    let firefox = "Mozilla/5.0 (X11; Linux x86_64; rv:145.0) Gecko/20100101 Firefox/145.0";
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, firefox.parse().unwrap());
    let out = Download::start(
        Request {
            url: s.url("/gen/256K/f.bin?seed=2").parse().unwrap(),
            headers,
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
    )
    .wait()
    .await
    .unwrap();
    same_content(&out.path, 2, 256 << 10);
    let log = settled_log(&s).await;
    assert!(
        log.iter()
            .filter(|e| e.sec_fetch_mode.is_some())
            .all(|e| e.user_agent.as_deref() == Some(firefox)),
        "{log:?}"
    );
}

#[tokio::test]
async fn cookies_stay_with_their_site_through_redirects() {
    let s = server("browser_only=1").await;
    let port = s.addr().port();
    // 127.0.0.1 redirects to localhost: another host, as when a site hands off to a CDN.
    let target = format!("http://localhost:{port}/gen/1M/r.bin?seed=3");
    let enc: String = target
        .chars()
        .map(|c| match c {
            ':' => "%3A".to_owned(),
            '/' => "%2F".to_owned(),
            '?' => "%3F".to_owned(),
            '=' => "%3D".to_owned(),
            c => c.to_string(),
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let out = Download::start(
        Request {
            url: s.url(&format!("/redirect?to={enc}")).parse().unwrap(),
            headers: handed_over(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
    )
    .wait()
    .await
    .unwrap();
    same_content(&out.path, 3, 1 << 20);
    let log = settled_log(&s).await;
    assert!(log.iter().any(|e| e.sec_fetch_mode.is_some()), "{log:?}");
    assert!(
        log.iter().all(|e| e.cookie.is_none()),
        "the cookie for 127.0.0.1 reached localhost: {log:?}"
    );
}

#[tokio::test]
async fn a_stopped_download_resumes_through_the_same_connection() {
    let s = server("browser_only=1&rate=1M&stall_at=1M").await;
    let dir = tempfile::tempdir().unwrap();
    let dl = Download::start(
        Request {
            url: s.url("/gen/3M/r.bin?seed=7").parse().unwrap(),
            headers: handed_over(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        Options {
            save_interval: Duration::from_millis(100),
            ..opts()
        },
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let file = loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let snap = dl.snapshot();
        if let Some(f) = snap.file.filter(|_| snap.downloaded >= 256 << 10) {
            break f;
        }
        assert!(Instant::now() < deadline, "the download never started");
    };
    dl.cancel();
    assert!(matches!(dl.wait().await, Err(mdm_engine::Error::Cancelled)));
    s.set_faults(Faults::parse("browser_only=1&rate=4M").unwrap());
    tokio::time::timeout(SETTLE, s.settled()).await.unwrap();
    let state = mdm_engine::state::state_path(&file);
    let saved = mdm_engine::state::ResumeState::load(&state).unwrap();
    assert!(saved.browser_like, "the state says how it got through");
    s.clear_log();
    let out = Download::resume(state.clone(), handed_over(), opts())
        .unwrap()
        .wait()
        .await
        .expect("the resume goes on");
    same_content(&out.path, 7, 3 << 20);
    let log = settled_log(&s).await;
    // Straight through the browser-like connection, from where it stopped, its bytes as they
    // are (no compression asked for a range).
    assert!(!log.is_empty());
    for e in &log {
        assert!(e.sec_fetch_mode.is_some(), "{e:?}");
        assert_eq!(e.status, 206, "{e:?}");
        assert!(
            e.range
                .as_deref()
                .is_some_and(|r| !r.starts_with("bytes=0-")),
            "{e:?}"
        );
        assert_eq!(e.accept_encoding.as_deref(), Some("identity"), "{e:?}");
    }
}

#[tokio::test]
async fn the_speed_limit_holds() {
    let s = server("browser_only=1").await;
    let dir = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let out = Download::start(
        Request {
            url: s.url("/gen/1M/s.bin?seed=9").parse().unwrap(),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        Options {
            speed_limit: Some(512 << 10),
            ..opts()
        },
    )
    .wait()
    .await
    .unwrap();
    same_content(&out.path, 9, 1 << 20);
    // 1 MiB at 512 KiB/s: two seconds, one at least with the limiter's first second of burst.
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_look_gets_past_the_bot_check_too() {
    let s = server("browser_only=1").await;
    let info = mdm_engine::probe(
        s.url("/gen/2M/look.apk?seed=1").parse().unwrap(),
        Default::default(),
        &opts(),
    )
    .await
    .expect("the look gets through");
    assert_eq!(info.size, Some(2 << 20));
    assert_eq!(info.file_name, "look.apk");
}

/// #954: a look and a download that got through only on the browser-like connection tell their
/// server: its address, and the round trip of MDM's own connection, which the bot check refused
/// at the same server.
#[tokio::test]
async fn through_the_browser_like_connection_the_server_is_told() {
    let s = server("browser_only=1&rate=1M").await;
    let url: url::Url = s.url("/gen/2M/where.apk?seed=954").parse().unwrap();
    let localhost = Some(IpAddr::V4(Ipv4Addr::LOCALHOST));
    let info = mdm_engine::probe(url.clone(), Default::default(), &opts())
        .await
        .expect("the look gets through");
    let server = info.server.expect("the look told the server");
    assert_eq!(server.ip, localhost, "{server:?}");
    assert!(server.rtt.is_some(), "{server:?}");

    let dir = tempfile::tempdir().unwrap();
    let dl = Download::start(
        Request {
            url,
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
    );
    // Looked at every 5 ms, as the progress window does every frame.
    let mut told = None;
    loop {
        let view = dl.snapshot();
        if let Some(seen) = view.server {
            assert_eq!(seen.ip, localhost, "{seen:?}");
            assert!(seen.rtt.is_some(), "{seen:?}");
            told = Some(seen);
        }
        if matches!(view.state, State::Done | State::Failed | State::Cancelled) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    dl.wait()
        .await
        .expect("the browser-like connection gets through");
    told.expect("the download told the server");
}

#[tokio::test]
async fn stop_ends_it() {
    let s = server("browser_only=1&rate=256K").await;
    let dir = tempfile::tempdir().unwrap();
    let dl = Download::start(
        Request {
            url: s.url("/gen/8M/c.bin?seed=4").parse().unwrap(),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while dl.snapshot().downloaded == 0 {
        assert!(Instant::now() < deadline, "never started");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    dl.cancel();
    let stopped = tokio::time::timeout(Duration::from_secs(5), dl.wait())
        .await
        .expect("stop is prompt");
    assert!(matches!(stopped, Err(mdm_engine::Error::Cancelled)));
}

/// Over HTTPS the lab's bot check looks at the TLS handshake itself, as real ones do: a plain
/// client that sends a browser's headers is still refused; MDM's own request is refused, and the
/// browser-like connection, whose handshake is Chrome's, gets the file.
#[tokio::test]
async fn over_https_the_handshake_itself_is_what_gets_through() {
    let s = LabServer::start(Config {
        faults: Faults::parse("browser_only=1").unwrap(),
        tls: true,
        ..Config::default()
    })
    .await
    .unwrap();
    let ca = s.ca_der().unwrap();
    // A program's TLS with every header a browser sends: refused all the same.
    let plain = reqwest::Client::builder()
        .tls_certs_merge([reqwest::Certificate::from_der(&ca).unwrap()])
        .build()
        .unwrap();
    let refused = plain
        .get(s.url("/gen/64K/x.bin?seed=1"))
        .header(USER_AGENT, CHROME)
        .header("Sec-Fetch-Mode", "navigate")
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert_eq!(refused.headers()["cf-mitigated"], "challenge");
    s.clear_log();

    let dir = tempfile::tempdir().unwrap();
    let out = Download::start(
        Request {
            url: s.url("/gen/2M/app.apk?seed=21").parse().unwrap(),
            headers: handed_over(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        Options {
            extra_roots: vec![ca],
            ..opts()
        },
    )
    .wait()
    .await
    .expect("the browser-like connection gets through");
    same_content(&out.path, 21, 2 << 20);
    let log = settled_log(&s).await;
    assert_eq!(log[0].outcome, "fault:browser_only", "{log:?}");
    assert_eq!(log[0].browser_tls, Some(false), "{log:?}");
    assert!(log.len() >= 2, "{log:?}");
    for e in &log[1..] {
        assert_eq!(e.browser_tls, Some(true), "{e:?}");
        assert!(e.status == 200 || e.status == 206, "{e:?}");
    }
}

/// The file's information window looks first (`probe_kept`): its browser-like answer is kept
/// for the download, so a click costs one refused request and one that gets through, no more.
#[tokio::test]
async fn a_look_and_its_download_ask_once_more_in_all() {
    let s = server("browser_only=1").await;
    let url: url::Url = s.url("/gen/1M/kept.apk?seed=5").parse().unwrap();
    let (info, kept) = mdm_engine::probe_kept(url.clone(), handed_over(), &opts())
        .await
        .expect("the look gets through");
    assert_eq!(info.size, Some(1 << 20));
    assert!(kept.is_some(), "the browser-like answer is kept");
    let dir = tempfile::tempdir().unwrap();
    let out = Download::start_with(
        Request {
            url,
            headers: handed_over(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
        kept,
    )
    .wait()
    .await
    .unwrap();
    same_content(&out.path, 5, 1 << 20);
    let log = settled_log(&s).await;
    assert_eq!(log.len(), 2, "{log:?}");
    assert_eq!(log[0].outcome, "fault:browser_only");
    assert!(
        log[1].sec_fetch_mode.is_some() && log[1].status == 200,
        "{log:?}"
    );
}

/// Options → Proxy: the browser-like connection goes through the proxy too. The lab server
/// stands in for the proxy (it answers by path), so a host that can't exist only works there.
#[tokio::test]
async fn it_goes_through_the_proxy() {
    let s = server("browser_only=1").await;
    let dir = tempfile::tempdir().unwrap();
    let out = Download::start(
        Request {
            url: "http://nowhere.invalid/gen/512K/p.bin?seed=8"
                .parse()
                .unwrap(),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        Options {
            proxy: mdm_engine::Proxy::Manual {
                url: s.url(""),
                login: None,
                http: true,
                https: true,
            },
            ..opts()
        },
    )
    .wait()
    .await
    .expect("downloaded through the proxy");
    same_content(&out.path, 8, 512 << 10);
    let log = settled_log(&s).await;
    assert_eq!(log[0].outcome, "fault:browser_only", "{log:?}");
    assert!(
        log[1..].iter().all(|e| e.sec_fetch_mode.is_some()),
        "{log:?}"
    );
}

/// A site that refuses the browser-like connection too (a challenge to solve, say): the
/// download fails with the bot check's refusal, as before, after one more request only.
#[tokio::test]
async fn a_second_refusal_stays_the_bot_check() {
    let s = server("browser_only=1&refuse_agent=Chrome").await;
    let dir = tempfile::tempdir().unwrap();
    let err = Download::start(
        Request {
            url: s.url("/gen/64K/no.bin?seed=1").parse().unwrap(),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
    )
    .wait()
    .await
    .unwrap_err();
    assert!(matches!(err, mdm_engine::Error::BotCheck), "{err:?}");
    let log = settled_log(&s).await;
    assert_eq!(log.len(), 2, "{log:?}");
}

/// FileHippo's way (#630): 412 Precondition Failed, empty, to whatever isn't a browser. MDM's
/// first request carries no precondition, so that 412 can only refuse the program: it is asked
/// once more the way a browser connects too, and the file comes.
#[tokio::test]
async fn a_412_to_a_request_without_preconditions_is_asked_once_more_too() {
    let s = server("refuse_programs=412").await;
    let dir = tempfile::tempdir().unwrap();
    let out = Download::start(
        Request {
            url: s.url("/gen/1M/winrar-x64.exe?seed=630").parse().unwrap(),
            headers: handed_over(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
    )
    .wait()
    .await
    .expect("the browser-like connection gets through");
    same_content(&out.path, 630, 1 << 20);
    let log = settled_log(&s).await;
    assert_eq!(log[0].status, 412, "{log:?}");
    assert_eq!(log.iter().filter(|e| e.status == 412).count(), 1, "{log:?}");
    assert!(
        log[1..].iter().all(|e| e.sec_fetch_mode.is_some()),
        "{log:?}"
    );
}
