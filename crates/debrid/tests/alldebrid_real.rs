//! AllDebrid's real API with its public demo key (`staticDemoApikeyPrem`, from its documentation:
//! "The endpoints will respond with demo static data when called with the following static
//! apikeys"). No account, nothing bought, nothing downloaded from a file host: it checks that
//! MDM reads the real service's answers. Run by hand:
//! `cargo test -p mdm-debrid --test alldebrid_real -- --ignored --nocapture`.

use std::sync::Arc;

use mdm_debrid::{Account, Credential, Debrid, Http, ServiceKind, Services, check, signin};
use mdm_engine::Options;
use tokio_util::sync::CancellationToken;

const DEMO: &str = "staticDemoApikeyPrem";

#[tokio::test]
#[ignore = "asks AllDebrid's real API (with its public demo key)"]
async fn the_real_service_answers_the_demo_key() {
    let http = Arc::new(Http::new(&Options::default(), Services::default()).unwrap());
    let info = check(
        &http,
        ServiceKind::AllDebrid,
        Credential::Token { token: DEMO.into() },
    )
    .await
    .expect("the account");
    eprintln!(
        "account: {} premium={:?} left={:?}",
        info.user, info.premium, info.premium_left
    );
    assert_eq!(info.premium, Some(true));
    let d = Debrid::default();
    d.set_accounts(
        vec![Account {
            id: 1,
            service: ServiceKind::AllDebrid,
            credential: Credential::Token { token: DEMO.into() },
        }],
        http.clone(),
    );
    d.refresh_hosts().await.expect("the host list");
    // The documentation's own example link (example.com): the demo key answers it with static
    // data; the unlock path is the real one.
    let link = "http://example.com/somefile".parse().unwrap();
    let known = d.account_for(&link);
    eprintln!("example.com taken by an account: {known:?}");
    // Unlock straight through the account (whatever the host list says about example.com).
    let unlocker = d
        .unlocker_for(&"https://1fichier.com/?demo".parse().unwrap())
        .or_else(|| d.unlocker_for(&link));
    match unlocker {
        Some(u) => {
            let got = u
                .unlock(
                    &link,
                    &|n, _| eprintln!("note: {n}"),
                    &CancellationToken::new(),
                )
                .await;
            eprintln!("unlock: {got:?}");
        }
        None => eprintln!("no host in the list matched; unlock not asked"),
    }
    let code = signin::start(&http, ServiceKind::AllDebrid)
        .await
        .expect("a PIN");
    eprintln!(
        "PIN: {} chars, page {}, expires in {} s",
        code.shown.len(),
        code.page.split('?').next().unwrap_or(""),
        code.expires_in
    );
}
