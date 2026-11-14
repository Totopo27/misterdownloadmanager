//! Torrents through each service (#44), end to end against the lab server's fakes: a magnet link
//! sent to the account, its files downloaded over HTTP by MDM's engine into the torrent's folder,
//! byte for byte, and the torrent removed from the account's list afterwards. Nothing connects to
//! a peer. Also: a torrent that dies at the service, and one whose file names climb out of the
//! folder (refused, nothing written outside it).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mdm_debrid::torrents::{Order, RemoteTorrent};
use mdm_debrid::{Account, Credential, Debrid, Http, ServiceKind, Services};
use mdm_engine::download::State;
use mdm_engine::{Options, ServiceFault};
use mdm_labserver::{Config, LabServer, content, debrid::seed};
use url::Url;

async fn setup(service: ServiceKind) -> (LabServer, Debrid) {
    let lab = LabServer::start(Config::default()).await.unwrap();
    let base = Url::parse(&lab.url("/")).unwrap();
    let http = Arc::new(Http::new(&Options::default(), Services::at(&base)).unwrap());
    let d = Debrid::default();
    d.set_accounts(
        vec![Account {
            id: 7,
            service,
            credential: Credential::Token {
                token: "good".into(),
            },
        }],
        http,
    );
    d.refresh_hosts().await.unwrap();
    (lab, d)
}

async fn run(d: &Debrid, hash: &str, folder: &Path) -> (RemoteTorrent, mdm_engine::Snapshot) {
    let order = Order {
        account: d.torrent_account().expect("an account").0,
        magnet: format!("magnet:?xt=urn:btih:{hash}&dn=Lab%20Show")
            .parse()
            .unwrap(),
        transfer: None,
        folder: folder.to_owned(),
        net: Options {
            max_retries: 1,
            retry_delay: Duration::from_millis(50),
            ..Options::default()
        },
        poll: Duration::from_millis(50),
    };
    let t = RemoteTorrent::start(d, order);
    for _ in 0..600 {
        let s = t.snapshot();
        if matches!(s.state, State::Done | State::Failed | State::Cancelled) {
            return (t.clone(), s);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("not finished: {:?}", t.snapshot().events);
}

fn same(path: &Path, size: &str, name: &str) -> bool {
    let data = std::fs::read(path).unwrap_or_default();
    let mut want = vec![0; data.len()];
    content::fill(
        seed(&format!("hoster.test/file/{size}/{name}")),
        0,
        &mut want,
    );
    !data.is_empty() && data == want
}

async fn held(lab: &LabServer) -> Vec<String> {
    let body = reqwest::get(lab.url("/_debrid/torrents"))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn a_magnet_through_each_service_comes_down_as_files() {
    for service in ServiceKind::ALL.into_iter().filter(|s| s.takes_magnets()) {
        let (lab, d) = setup(service).await;
        let dir = tempfile::tempdir().unwrap();
        let (t, s) = run(&d, "1234abcd", dir.path()).await;
        assert_eq!(
            s.state,
            State::Done,
            "{service:?}: {:?} {:?}",
            s.error,
            s.events
        );
        let show = dir.path().join("Lab Show");
        assert_eq!(s.file.as_deref(), Some(show.as_path()), "{service:?}");
        assert!(
            same(&show.join("video.mkv"), "2M", "video.mkv"),
            "{service:?}"
        );
        assert!(
            same(&show.join("extras").join("notes.txt"), "64K", "notes.txt"),
            "{service:?}"
        );
        assert_eq!(s.downloaded, (2 << 20) + (64 << 10), "{service:?}");
        let id = t.transfer().expect("sent");
        assert!(
            !held(&lab).await.contains(&id),
            "{service:?}: removed afterwards"
        );
        let log: Vec<String> = s.events.iter().map(|e| e.text.clone()).collect();
        assert!(
            log.iter().any(|e| e.contains("is downloading the torrent")),
            "{service:?}: {log:?}"
        );
    }
}

#[tokio::test]
async fn a_torrent_of_one_file_goes_into_the_folder_itself() {
    // The service keeps it as `Lab Show/video.mkv`: it lands as `video.mkv`, no folder made.
    for service in ServiceKind::ALL.into_iter().filter(|s| s.takes_magnets()) {
        let (_lab, d) = setup(service).await;
        let dir = tempfile::tempdir().unwrap();
        let (_t, s) = run(&d, "one00001", dir.path()).await;
        assert_eq!(s.state, State::Done, "{service:?}: {:?}", s.error);
        let file = dir.path().join("video.mkv");
        assert!(same(&file, "2M", "video.mkv"), "{service:?}");
        assert_eq!(s.file.as_deref(), Some(file.as_path()), "{service:?}");
        assert!(!dir.path().join("Lab Show").exists(), "{service:?}");
    }
}

#[tokio::test]
async fn a_torrent_the_service_cant_get_fails_with_its_reason() {
    for service in ServiceKind::ALL.into_iter().filter(|s| s.takes_magnets()) {
        let (_lab, d) = setup(service).await;
        let dir = tempfile::tempdir().unwrap();
        let (t, s) = run(&d, "dead0001", dir.path()).await;
        assert_eq!(s.state, State::Failed, "{service:?}");
        let f = t.failure().unwrap();
        assert!(
            matches!(f, ServiceFault::TorrentUnavailable | ServiceFault::Other(_)),
            "{service:?}: {f:?}"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn file_names_that_climb_out_of_the_folder_are_refused() {
    for service in [ServiceKind::RealDebrid, ServiceKind::AllDebrid] {
        let (_lab, d) = setup(service).await;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("a").join("b");
        std::fs::create_dir_all(&dir).unwrap();
        let (_t, s) = run(&d, "bad00001", &dir).await;
        assert_eq!(s.state, State::Failed, "{service:?}");
        assert!(
            s.error
                .unwrap_or_default()
                .contains("can't be written safely")
        );
        assert!(!root.path().join("escape.bin").exists());
        assert!(!root.path().join("a").join("escape.bin").exists());
    }
}

#[tokio::test]
async fn a_stopped_torrent_continues_from_its_transfer() {
    // Stopped while the service works: started again with the transfer's id, nothing is sent
    // twice and the files come down.
    let (lab, d) = setup(ServiceKind::RealDebrid).await;
    let dir = tempfile::tempdir().unwrap();
    let order = Order {
        account: 7,
        magnet: "magnet:?xt=urn:btih:5678&dn=Again".parse().unwrap(),
        transfer: None,
        folder: dir.path().to_owned(),
        net: Options::default(),
        poll: Duration::from_millis(300),
    };
    let t = RemoteTorrent::start(&d, order.clone());
    let id = loop {
        if let Some(id) = t.transfer() {
            break id;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    t.cancel();
    for _ in 0..100 {
        if t.snapshot().state == State::Cancelled {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(t.snapshot().state, State::Cancelled);
    assert!(held(&lab).await.contains(&id), "kept at the service");
    let again = RemoteTorrent::start(
        &d,
        Order {
            transfer: Some(id.clone()),
            poll: Duration::from_millis(50),
            ..order
        },
    );
    for _ in 0..600 {
        if again.snapshot().state != State::Probing && again.snapshot().state != State::Downloading
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let s = again.snapshot();
    assert_eq!(s.state, State::Done, "{:?}", s.error);
    assert_eq!(again.transfer().as_ref(), Some(&id));
    assert!(same(
        &dir.path().join("Again").join("video.mkv"),
        "2M",
        "video.mkv"
    ));
    assert!(!held(&lab).await.contains(&id));
}
