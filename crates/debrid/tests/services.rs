//! AllDebrid, Premiumize, TorBox and Mega-Debrid against the lab server's fakes (labserver `debrid`), end to end
//! through MDM's engine, as `realdebrid.rs` does for Real-Debrid. The fake host `hoster.test`
//! resolves nowhere: a download that completes came through the service's direct link.

use std::sync::Arc;
use std::time::Duration;

use mdm_debrid::{Account, Credential, Debrid, Http, ServiceKind, Services, check, signin};
use mdm_engine::{Destination, Download, Error, Options, Refusal, Request, ServiceFault};
use mdm_labserver::{Config, LabServer, content, debrid::seed};
use url::Url;

async fn lab() -> (LabServer, Arc<Http>) {
    let lab = LabServer::start(Config::default()).await.unwrap();
    let base = Url::parse(&lab.url("/")).unwrap();
    let http = Arc::new(Http::new(&Options::default(), Services::at(&base)).unwrap());
    (lab, http)
}

async fn account(http: &Arc<Http>, service: ServiceKind, token: &str) -> Debrid {
    let d = Debrid::default();
    d.set_accounts(
        vec![Account {
            id: 3,
            service,
            credential: Credential::Token {
                token: token.into(),
            },
        }],
        http.clone(),
    );
    d.refresh_hosts().await.unwrap();
    d
}

async fn download(d: &Debrid, link: &str) -> Result<std::path::PathBuf, Error> {
    let url: Url = link.parse().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "mdm-debrid-svc-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let opts = Options {
        unlocker: d.unlocker_for(&url),
        max_retries: 1,
        retry_delay: Duration::from_millis(50),
        ..Options::default()
    };
    let req = Request {
        url,
        headers: Default::default(),
        dest: Destination::Dir(dir),
    };
    Download::start(req, opts).wait().await.map(|o| o.path)
}

fn same(path: &std::path::Path, link_path: &str) -> bool {
    let data = std::fs::read(path).unwrap();
    let mut want = vec![0; data.len()];
    content::fill(seed(&format!("hoster.test{link_path}")), 0, &mut want);
    data == want
}

fn fault(e: Error) -> (String, ServiceFault) {
    match e {
        Error::Refused(Refusal::Service { service, fault }) => (service, fault),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn alldebrid_links_come_through_the_account_delayed_ones_too() {
    let (lab, http) = lab().await;
    let info = check(
        &http,
        ServiceKind::AllDebrid,
        Credential::Token {
            token: "good".into(),
        },
    )
    .await
    .unwrap();
    assert!(info.premium == Some(true) && info.premium_left.is_some());
    let d = account(&http, ServiceKind::AllDebrid, "good").await;
    let p = download(&d, "http://hoster.test/file/1M/a.bin")
        .await
        .unwrap();
    assert!(same(&p, "/file/1M/a.bin"));
    // A link the service needs time for: asked again until it is ready.
    let p = download(&d, "http://hoster.test/file/1M/slow.bin?delayed=2")
        .await
        .unwrap();
    assert!(same(&p, "/file/1M/slow.bin"));
    assert!(
        lab.log()
            .iter()
            .filter(|e| e.path.starts_with("/gen/"))
            .all(|e| e.authorization.is_none())
    );
    // Refusals.
    let (svc, f) = fault(
        download(&d, "http://hoster.test/file/1M/g.bin?gone=1")
            .await
            .unwrap_err(),
    );
    assert_eq!(
        (svc.as_str(), f),
        ("AllDebrid", ServiceFault::FileUnavailable)
    );
    let (_, f) = fault(
        download(&d, "http://hoster.test/file/1M/d.bin?down=1")
            .await
            .unwrap_err(),
    );
    assert_eq!(f, ServiceFault::HostDown);
    let free = account(&http, ServiceKind::AllDebrid, "free").await;
    let (_, f) = fault(
        download(&free, "http://hoster.test/file/1M/f.bin")
            .await
            .unwrap_err(),
    );
    assert_eq!(f, ServiceFault::NotPremium);
    assert!(
        d.unlocker_for(&"http://elsewhere.test/a".parse().unwrap())
            .is_none()
    );
}

#[tokio::test]
async fn alldebrid_signs_in_by_pin() {
    let (_lab, http) = lab().await;
    let code = signin::start(&http, ServiceKind::AllDebrid).await.unwrap();
    assert_eq!(code.shown, "LABP");
    assert!(code.page.starts_with("https://alldebrid.com/pin/"));
    let mut got = None;
    for _ in 0..5 {
        if let Some(c) = signin::poll(&http, &code).await.unwrap() {
            got = Some(c);
            break;
        }
    }
    assert_eq!(
        got,
        Some(Credential::Token {
            token: "good".into()
        })
    );
}

#[tokio::test]
async fn premiumize_links_come_through_the_account() {
    let (lab, http) = lab().await;
    let info = check(
        &http,
        ServiceKind::Premiumize,
        Credential::Token {
            token: "good".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!((info.user.as_str(), info.premium), ("1234567", Some(true)));
    let d = account(&http, ServiceKind::Premiumize, "good").await;
    let p = download(&d, "http://hoster.test/file/2M/b.bin")
        .await
        .unwrap();
    assert!(same(&p, "/file/2M/b.bin"));
    assert!(
        lab.log()
            .iter()
            .filter(|e| e.path.starts_with("/gen/"))
            .all(|e| e.authorization.is_none())
    );
    let (svc, f) = fault(
        download(&d, "http://hoster.test/file/1M/g.bin?gone=1")
            .await
            .unwrap_err(),
    );
    assert_eq!(
        (svc.as_str(), f),
        ("Premiumize", ServiceFault::FileUnavailable)
    );
    let (_, f) = fault(
        download(&d, "http://hoster.test/file/1M/d.bin?down=1")
            .await
            .unwrap_err(),
    );
    assert_eq!(f, ServiceFault::HostDown);
    assert!(matches!(
        check(
            &http,
            ServiceKind::Premiumize,
            Credential::Token {
                token: "bad".into()
            }
        )
        .await,
        Err(mdm_debrid::Fault::Service(ServiceFault::SignIn))
    ));
    // Premiumize signs in by its API key only.
    assert!(signin::start(&http, ServiceKind::Premiumize).await.is_err());
}

#[tokio::test]
async fn torbox_links_come_through_the_account_once_it_has_the_file() {
    let (lab, http) = lab().await;
    let key = |t: &str| Credential::Token { token: t.into() };
    let info = check(&http, ServiceKind::TorBox, key("good"))
        .await
        .unwrap();
    assert_eq!(
        (info.user.as_str(), info.premium),
        ("lab@example.invalid", Some(true))
    );
    assert!(info.premium_left.is_some());
    let free = check(&http, ServiceKind::TorBox, key("free"))
        .await
        .unwrap();
    assert_eq!(free.premium, Some(false));
    assert!(matches!(
        check(&http, ServiceKind::TorBox, key("bad")).await,
        Err(mdm_debrid::Fault::Service(ServiceFault::SignIn))
    ));
    // Or by its device code: the code shown, TorBox's page, then the key once allowed there.
    let code = signin::start(&http, ServiceKind::TorBox).await.unwrap();
    assert_eq!(code.shown, "123456");
    assert!(code.page.contains("torbox.app"), "{}", code.page);
    let credential = loop {
        if let Some(c) = signin::poll(&http, &code).await.unwrap() {
            break c;
        }
    };
    let info = check(&http, ServiceKind::TorBox, credential).await.unwrap();
    assert_eq!(info.premium, Some(true));

    let d = account(&http, ServiceKind::TorBox, "good").await;
    // Its hosters list: the fake's two domains, nothing else.
    assert!(
        d.account_for(&"http://hoster2.test/x".parse().unwrap())
            .is_some()
    );
    assert!(
        d.account_for(&"http://elsewhere.test/x".parse().unwrap())
            .is_none()
    );
    // Fetched at TorBox first (two polls), the download saying how far TorBox is meanwhile (the
    // list's "50% at TorBox"), then its direct link; the key never in an address the engine
    // downloads.
    let url: Url = "http://hoster.test/file/2M/b.bin".parse().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dl = Download::start(
        Request {
            url: url.clone(),
            headers: Default::default(),
            dest: Destination::Dir(dir.path().to_owned()),
        },
        Options {
            unlocker: d.unlocker_for(&url),
            ..Options::default()
        },
    );
    let mut seen = None;
    for _ in 0..300 {
        seen = seen.or(dl.snapshot().at_service);
        tokio::time::sleep(Duration::from_millis(50)).await;
        if dl.snapshot().state == mdm_engine::download::State::Done {
            break;
        }
    }
    assert_eq!(seen, Some(("TorBox".to_owned(), Some(0.5))));
    assert_eq!(dl.snapshot().at_service, None, "cleared once the link came");
    let p = dl.wait().await.unwrap().path;
    assert!(same(&p, "/file/2M/b.bin"));
    // Done: the web download leaves the account's TorBox list (found by its link); again, a
    // no-op. Only this test makes web downloads in this binary.
    let held = || async {
        let body = reqwest::get(lab.url("/_debrid/torrents"))
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        serde_json::from_slice::<Vec<String>>(&body).unwrap()
    };
    let before = held().await;
    d.release(&url).await.unwrap();
    let after = held().await;
    assert!(after.len() < before.len(), "{before:?} → {after:?}");
    d.release(&url).await.unwrap();
    assert_eq!(held().await, after);
    let log = lab.log();
    assert!(
        log.iter()
            .filter(|e| e.path.starts_with("/gen/"))
            .all(|e| e.authorization.is_none()
                && !e.query.as_deref().unwrap_or("").contains("token"))
    );
    let (svc, f) = fault(
        download(&d, "http://hoster.test/file/1M/g.bin?gone=1")
            .await
            .unwrap_err(),
    );
    assert_eq!((svc.as_str(), f), ("TorBox", ServiceFault::FileUnavailable));
    let (_, f) = fault(
        download(&d, "http://hoster.test/file/1M/d.bin?down=1")
            .await
            .unwrap_err(),
    );
    assert_eq!(f, ServiceFault::HostDown);
}

#[tokio::test]
async fn megadebrid_links_come_through_the_account_by_key_or_password() {
    let (lab, http) = lab().await;
    // An application's key: it works, and the API has no call that says whether its account is
    // premium, so MDM doesn't claim it is.
    let info = check(
        &http,
        ServiceKind::MegaDebrid,
        Credential::Token {
            token: "good".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!((info.premium, info.premium_left), (None, None));
    let d = account(&http, ServiceKind::MegaDebrid, "good").await;
    let p = download(&d, "http://hoster.test/file/2M/m.bin")
        .await
        .unwrap();
    assert!(same(&p, "/file/2M/m.bin"));
    // The token went in the form, never in an address.
    assert!(
        lab.log()
            .iter()
            .all(|e| !e.path.contains("token") && !e.path.contains("password"))
    );
    let (svc, f) = fault(
        download(&d, "http://hoster.test/file/1M/g.bin?gone=1")
            .await
            .unwrap_err(),
    );
    assert_eq!(
        (svc.as_str(), f),
        (
            "Mega-Debrid",
            ServiceFault::Other("lab: file not found".into())
        )
    );

    // The user name and password: premium's end from the sign-in, the e-mail as the name.
    let login = Credential::Login {
        user: "labuser".into(),
        password: "labpass".into(),
    };
    let info = check(&http, ServiceKind::MegaDebrid, login.clone())
        .await
        .unwrap();
    assert_eq!(
        (info.user.as_str(), info.premium),
        ("labuser@lab.test", Some(true))
    );
    assert!(info.premium_left.is_some());
    let d = Debrid::default();
    d.set_accounts(
        vec![Account {
            id: 5,
            service: ServiceKind::MegaDebrid,
            credential: login.clone(),
        }],
        http.clone(),
    );
    d.refresh_hosts().await.unwrap();
    assert!(
        download(&d, "http://hoster.test/file/1M/a.bin")
            .await
            .is_ok()
    );
    // Checking the account again (the app checks one when it is added) keeps the token the
    // downloads use: a sign-in would end it.
    let signins = || {
        lab.log()
            .iter()
            .filter(|e| e.path.contains("action=connectUser"))
            .count()
    };
    let before = signins();
    for _ in 0..2 {
        let info = d.account_info(5).await.unwrap();
        assert_eq!(info.premium, Some(true));
    }
    assert_eq!(signins(), before);
    // Another sign-in to the account (another program, Options' check) replaces the token the
    // downloads had: the next one signs in again by itself.
    check(&http, ServiceKind::MegaDebrid, login).await.unwrap();
    assert!(
        download(&d, "http://hoster.test/file/1M/b.bin")
            .await
            .is_ok()
    );

    // A free account signs in, says so, and its links are refused.
    let free = check(
        &http,
        ServiceKind::MegaDebrid,
        Credential::Login {
            user: "freeuser".into(),
            password: "labpass".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(free.premium, Some(false));
    assert!(matches!(
        check(
            &http,
            ServiceKind::MegaDebrid,
            Credential::Login {
                user: "labuser".into(),
                password: "wrong".into()
            }
        )
        .await,
        Err(mdm_debrid::Fault::Service(ServiceFault::SignIn))
    ));
    assert!(matches!(
        check(
            &http,
            ServiceKind::MegaDebrid,
            Credential::Token {
                token: "bad".into()
            }
        )
        .await,
        Err(mdm_debrid::Fault::Service(ServiceFault::SignIn))
    ));
    // No torrents through Mega-Debrid yet: they go to an account that takes them.
    assert_eq!(d.torrent_account(), None);
    assert!(signin::start(&http, ServiceKind::MegaDebrid).await.is_err());
}

#[tokio::test]
async fn accounts_are_asked_in_their_order() {
    let (_lab, http) = lab().await;
    let d = Debrid::default();
    d.set_accounts(
        vec![
            Account {
                id: 1,
                service: ServiceKind::Premiumize,
                credential: Credential::Token {
                    token: "good".into(),
                },
            },
            Account {
                id: 2,
                service: ServiceKind::AllDebrid,
                credential: Credential::Token {
                    token: "good".into(),
                },
            },
        ],
        http.clone(),
    );
    d.refresh_hosts().await.unwrap();
    // Both take hoster.test; the first in the list wins. Only AllDebrid takes hoster2.test.
    assert_eq!(
        d.account_for(&"http://hoster.test/x".parse().unwrap()),
        Some((1, ServiceKind::Premiumize))
    );
    assert_eq!(
        d.account_for(&"http://hoster2.test/x".parse().unwrap()),
        Some((2, ServiceKind::AllDebrid))
    );
}
