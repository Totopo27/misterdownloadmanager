//! Mega-Debrid's API, as documented at <https://www.mega-debrid.eu/index.php?page=api> (read
//! 2026-11-09). Only what MDM uses: signing in, the supported hosters, and direct links
//! (`getLink`).
//!
//! - One address for everything, `api.php?action=NAME`; every answer is JSON with
//!   `response_code` ("ok" or the error's code) and, on an error, `response_text`.
//! - Two ways to sign in: an application's API key (made on the account's page, "API Key"), sent
//!   as `token` with every call; or the user name and password (`connectUser`), which give a
//!   token valid until the next sign-in.
//! - The documentation puts the token, user name and password in the address; the service takes
//!   them in the POST body as well (tried 2026-11-09), so MDM sends every secret there, never in
//!   an address that could end up in a log.
//! - Only premium accounts can use the API; a free one signs in (its `vip_end` is in the past)
//!   and its links are refused.
//! - Four failed sign-ins, or more than 50 requests a second, and the IP address is banned for
//!   some minutes (HTTP 405 then).

use serde::Deserialize;
use url::Url;

use crate::{Fault, Http, ServiceKind};

/// Where Mega-Debrid answers (tests: the lab server's `/md/api.php`).
pub const BASE: &str = "https://www.mega-debrid.eu/api.php";

#[derive(Debug, Deserialize)]
struct Status {
    #[serde(default)]
    response_code: String,
    #[serde(default)]
    response_text: String,
}

/// What an error code means for MDM. The documentation lists no codes: these two are the ones
/// its answers were seen to carry (2026-11-09); any other is shown in the service's own words
/// (`response_text`), as the documentation recommends.
pub(crate) fn fault_of(code: &str, text: &str) -> Fault {
    use mdm_engine::ServiceFault as F;
    Fault::Service(match code {
        // "Token error, please log-in": a token a newer sign-in replaced, or a wrong key.
        // "User and password doesn't match".
        "TOKEN_ERROR" | "UNKNOWN_USER" => F::SignIn,
        _ if !text.is_empty() => F::Other(text.to_owned()),
        _ => F::Other(code.to_owned()),
    })
}

fn read<T: serde::de::DeserializeOwned>(
    status: u16,
    body: &[u8],
    retry: Option<u64>,
) -> Result<T, Fault> {
    use mdm_engine::ServiceFault as F;
    match status {
        429 => {
            return Err(Fault::Service(F::TooManyRequests(
                retry.map(std::time::Duration::from_secs),
            )));
        }
        401 => return Err(Fault::Service(F::SignIn)),
        // Banned for some minutes: too many failed sign-ins or requests.
        405 => return Err(Fault::Service(F::IpRefused)),
        _ => {}
    }
    let Ok(s) = serde_json::from_slice::<Status>(body) else {
        return Err(Fault::Service(if status >= 500 {
            F::ServiceDown
        } else {
            F::Other("an answer MDM can't read".into())
        }));
    };
    if s.response_code != "ok" {
        return Err(fault_of(&s.response_code, &s.response_text));
    }
    serde_json::from_slice(body)
        .map_err(|_| Fault::Service(F::Other("an answer MDM can't read".into())))
}

async fn call<T: serde::de::DeserializeOwned>(
    http: &Http,
    req: reqwest::RequestBuilder,
) -> Result<T, Fault> {
    let (status, body, retry) = http.send(ServiceKind::MegaDebrid, req).await?;
    read(status, &body, retry)
}

/// `POST api.php?action=NAME`, the parameters (secrets too) in the body.
fn post(http: &Http, action: &str, form: &[(&str, &str)]) -> reqwest::RequestBuilder {
    let mut url = http.services.md.clone();
    url.query_pairs_mut().append_pair("action", action);
    http.post_form(&url, form)
}

/// `connectUser`: the token for a user name and password, and when premium ends.
#[derive(Debug, Deserialize)]
pub(crate) struct Connected {
    pub token: String,
    /// Unix time; 0 or past for a free account.
    #[serde(default, deserialize_with = "number")]
    pub vip_end: i64,
    #[serde(default)]
    pub email: String,
}

pub(crate) async fn connect(http: &Http, login: &str, password: &str) -> Result<Connected, Fault> {
    let req = post(
        http,
        "connectUser",
        &[("login", login), ("password", password)],
    );
    call(http, req).await
}

/// `getUserHistory` (`limit=1`): only whether the token (an application's key) is good; the API
/// has no other call that says so without signing in again.
pub(crate) async fn token_works(http: &Http, token: &str) -> Result<(), Fault> {
    let req = post(http, "getUserHistory", &[("token", token), ("limit", "1")]);
    call::<serde_json::Value>(http, req).await.map(|_| ())
}

#[derive(Debug, Deserialize)]
struct Hosters {
    #[serde(default)]
    hosters: Vec<Hoster>,
}

#[derive(Debug, Deserialize)]
struct Hoster {
    #[serde(default)]
    domains: Vec<String>,
}

/// `getHostersList` (no token): every domain of every hoster it takes.
pub(crate) async fn domains(http: &Http) -> Result<Vec<String>, Fault> {
    let h: Hosters = call(http, post(http, "getHostersList", &[])).await?;
    Ok(h.hosters.into_iter().flat_map(|h| h.domains).collect())
}

/// `getLink` (POST `link`): the direct link.
#[derive(Debug, Deserialize)]
pub(crate) struct Link {
    #[serde(rename = "debridLink")]
    debrid_link: String,
}

impl Link {
    /// The address, without the quotes some answers wrap it in.
    pub fn url(&self) -> &str {
        self.debrid_link.trim().trim_matches('"')
    }
}

pub(crate) async fn unlock(http: &Http, token: &str, link: &Url) -> Result<Link, Fault> {
    call(
        http,
        post(
            http,
            "getLink",
            &[("token", token), ("link", link.as_str())],
        ),
    )
    .await
}

/// A number the service may send as text ("1767225600").
fn number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
        serde_json::Value::String(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdm_engine::ServiceFault as F;

    #[test]
    fn the_documented_answers_read() {
        let c: Connected = read(
            200,
            br#"{"response_code":"ok","response_text":"User logged","token":"abc","vip_end":"1767225600","email":"u@example.com"}"#,
            None,
        )
        .unwrap();
        assert_eq!((c.token.as_str(), c.vip_end), ("abc", 1767225600));
        let h: Hosters = read(
            200,
            br#"{"response_code":"ok","hosters":[{"name":"Rapidgator","status":"up","img":"x","domains":["rapidgator.net","rg.to"],"regexps":[]},{"name":"Other"}]}"#,
            None,
        )
        .unwrap();
        assert_eq!(
            h.hosters
                .into_iter()
                .flat_map(|h| h.domains)
                .collect::<Vec<_>>(),
            vec!["rapidgator.net", "rg.to"]
        );
        let l: Link = read(
            200,
            br#"{"response_code":"ok","debridLink":"\"https://s1.mega-debrid.eu/download/file/x\""}"#,
            None,
        )
        .unwrap();
        assert_eq!(l.url(), "https://s1.mega-debrid.eu/download/file/x");
        let err =
            br#"{"response_code":"TOKEN_ERROR","response_text":"Token error, please log-in"}"#;
        assert_eq!(
            read::<Link>(200, err, None).unwrap_err(),
            Fault::Service(F::SignIn)
        );
    }

    #[test]
    fn codes_and_statuses_mean_what_they_say() {
        assert_eq!(fault_of("UNKNOWN_USER", ""), Fault::Service(F::SignIn));
        assert_eq!(
            fault_of("SOMETHING_NEW", "Its words"),
            Fault::Service(F::Other("Its words".into()))
        );
        assert_eq!(
            read::<Link>(405, b"", None).unwrap_err(),
            Fault::Service(F::IpRefused)
        );
        assert_eq!(
            read::<Link>(429, b"", Some(30)).unwrap_err(),
            Fault::Service(F::TooManyRequests(Some(std::time::Duration::from_secs(30))))
        );
    }
}
