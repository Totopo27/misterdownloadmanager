//! #140: where a download's server is, as the download sees it: the address that answered and
//! its connections' round trip, from the look (Download File Info) and from the download itself;
//! a proxy said as such (the address is then the proxy's).

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use mdm_engine::download::{State, probe};
use mdm_engine::{Destination, Download, Options, Proxy, Request};
use mdm_labserver::{Config, LabServer};

fn opts() -> Options {
    Options {
        retry_delay: Duration::from_millis(50),
        max_retries: 1,
        ..Options::default()
    }
}

#[tokio::test]
async fn the_look_and_the_download_see_the_server() {
    let lab = LabServer::start(Config::default()).await.unwrap();
    let url: url::Url = lab
        .url("/gen/2M/where.bin?seed=140&rate=1M")
        .parse()
        .unwrap();
    let info = probe(url.clone(), Default::default(), &opts())
        .await
        .unwrap();
    let server = info.server.expect("the look saw the server");
    assert_eq!(server.ip, Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    assert!(!server.proxied && server.cdn.is_none());
    let rtt = server.rtt.expect("its connection was timed");
    assert!(rtt < Duration::from_secs(1), "{rtt:?}");

    let dir = tempfile::tempdir().unwrap();
    let d = Download::start(
        Request {
            url,
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        Options {
            max_connections: 2,
            ..opts()
        },
    );
    // Seen while it runs (the progress window), not only at the end; whole from the first
    // look on (#933: the round trip came before the address, as the connection opened).
    let mut seen = None;
    for _ in 0..100 {
        seen = d.snapshot().server;
        if seen.as_ref().is_some_and(|s| s.rtt.is_some()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let seen = seen.expect("the download saw the server");
    assert_eq!(seen.ip, Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    assert!(seen.rtt.is_some() && !seen.proxied);
    d.wait().await.unwrap();
}

/// #933: a server slow to answer (its headers 400 ms after the request) was told half, its
/// connection's time without its address, from the connection's opening to its answer; it is
/// told whole, once it answered.
#[tokio::test]
async fn a_server_slow_to_answer_is_told_whole() {
    let lab = LabServer::start(Config::default()).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let d = Download::start(
        Request {
            url: lab
                .url("/gen/256K/slow.bin?seed=933&latency_ms=400")
                .parse()
                .unwrap(),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        opts(),
    );
    // Looked at every 5 ms from the start, as the progress window does every frame.
    let mut told = None;
    loop {
        let view = d.snapshot();
        if let Some(seen) = view.server {
            assert_eq!(seen.ip, Some(IpAddr::V4(Ipv4Addr::LOCALHOST)), "{seen:?}");
            assert!(seen.rtt.is_some(), "{seen:?}");
            told = Some(seen);
        }
        if matches!(view.state, State::Done | State::Failed | State::Cancelled) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    d.wait().await.unwrap();
    told.expect("the download saw the server");
}

#[tokio::test]
async fn through_a_proxy_the_address_is_the_proxys_and_said_so() {
    // The lab server stands in for the proxy (a proxy is sent the whole address; it answers by
    // path), so a host that can't exist works only through it.
    let lab = LabServer::start(Config::default()).await.unwrap();
    let port = lab.url("/").parse::<url::Url>().unwrap().port().unwrap();
    let o = Options {
        proxy: Proxy::Manual {
            url: format!("http://127.0.0.1:{port}"),
            login: None,
            http: true,
            https: false,
        },
        ..opts()
    };
    let info = probe(
        "http://nowhere.invalid/gen/64K?seed=7".parse().unwrap(),
        Default::default(),
        &o,
    )
    .await
    .unwrap();
    let server = info.server.unwrap();
    assert!(server.proxied, "{server:?}");
    assert_eq!(server.ip, Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
}
