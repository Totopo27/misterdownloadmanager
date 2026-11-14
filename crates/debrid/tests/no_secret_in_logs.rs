//! A debrid account's token never reaches a log (#44): MDM's own log at its most detailed
//! (every crate's `tracing` output, the HTTP client's included), the download's event log, its
//! errors and debug dumps, while each service signs in, lists its hosts and unlocks a link
//! downloaded end to end against the lab server's fakes.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mdm_debrid::{Account, Credential, Debrid, Http, ServiceKind, Services, check};
use mdm_engine::{Destination, Download, Options, Request};
use mdm_labserver::{Config, LabServer};
use url::Url;

/// The token: distinctive, so finding it anywhere is a leak.
const TOKEN: &str = "good-9f3c1a7e-NEVER-LOGGED";

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn no_token_in_any_log() {
    let log = Captured::default();
    let sink = log.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || sink.clone())
        .with_ansi(false)
        .init();

    let lab = LabServer::start(Config::default()).await.unwrap();
    let base = Url::parse(&lab.url("/")).unwrap();
    let http = Arc::new(Http::new(&Options::default(), Services::at(&base)).unwrap());
    let mut seen = String::new();
    for (i, service) in ServiceKind::ALL.into_iter().enumerate() {
        let credential = Credential::Token {
            token: TOKEN.into(),
        };
        // Premium (or, Mega-Debrid's key, not said): the service was asked with the token.
        let info = check(&http, service, credential.clone()).await.unwrap();
        assert_ne!(info.premium, Some(false), "{service:?}");
        seen.push_str(&format!("{info:?} {credential:?}"));
        let d = Debrid::default();
        d.set_accounts(
            vec![Account {
                id: i as u64,
                service,
                credential,
            }],
            http.clone(),
        );
        d.refresh_hosts().await.unwrap();
        seen.push_str(&format!("{d:?}"));
        let dir = tempfile::tempdir().unwrap();
        for link in [
            "http://hoster.test/file/1M/a.bin",
            "http://hoster.test/file/1M/gone.bin?gone=1",
        ] {
            let url: Url = link.parse().unwrap();
            let unlocker = d.unlocker_for(&url);
            seen.push_str(&format!("{unlocker:?}"));
            let opts = Options {
                unlocker,
                max_retries: 0,
                retry_delay: Duration::from_millis(10),
                ..Options::default()
            };
            let req = Request {
                url,
                headers: Default::default(),
                dest: Destination::Dir(dir.path().to_owned()),
            };
            let dl = Download::start(req, opts);
            // The event log as the list's window reads it, to the end.
            let snap = loop {
                let s = dl.snapshot();
                if !matches!(
                    s.state,
                    mdm_engine::download::State::Probing
                        | mdm_engine::download::State::Downloading
                        | mdm_engine::download::State::Completing
                ) {
                    break s;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            seen.push_str(&format!("{:?} {:?}", snap.events, snap.error));
            let result = dl.wait().await;
            seen.push_str(&format!("{result:?}"));
        }
    }
    let log = String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned();
    assert!(
        log.len() > 1000,
        "the log was captured ({} bytes)",
        log.len()
    );
    assert!(!log.contains(TOKEN), "the token is in the log");
    assert!(
        !seen.contains(TOKEN),
        "the token is in an event, error or dump"
    );
}
