//! A fake Real-Debrid (#44) for MDM's tests, answering as its documentation says
//! (<https://api.real-debrid.com/>): the same paths under `/rd/`, the same JSON shapes, the same
//! error envelope (`{"error", "error_code"}` with the HTTP status). Nothing here is copied from
//! the service; the shapes are the documented ones.
//!
//! - Tokens (`Authorization: Bearer …`): `good` (or `good-…`) is a premium account, `free` a
//!   free one, and the
//!   device flow's access tokens (`at-…`) are premium. Anything else is refused (code 8).
//! - The fake file host: `http://hoster.test/file/<size>/<name>[?flags]`. Its direct link is the
//!   lab's own `/gen/<size>/<name>`, the same bytes on every unlock of the same link. Flags:
//!   `gone=1` (the file is unavailable, 24), `down=1` (the host is down, 19), `chunks=N` (the
//!   connection cap), `ttl_ms=N` (the direct link expires), `rate=SIZE` (it is that slow).
//!   Any other host is unsupported (16). A free account gets 20 (premium only).
//! - Device flow: the code is allowed after `approve_after` polls (default 2) of
//!   `device/credentials`.

use std::collections::HashMap;
use std::sync::Mutex;

use axum::extract::Query;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use serde_json::json;

use crate::server::unix_ms;

/// The fake file host's name: resolve it to nothing (the service reaches it, MDM doesn't).
pub const HOSTER: &str = "hoster.test";

static DEVICE_POLLS: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

fn error(status: StatusCode, error: &str, code: i64) -> Response {
    (
        status,
        axum::Json(json!({"error": error, "error_code": code})),
    )
        .into_response()
}

/// The account a request's token signs in to: Some(premium?).
fn account(headers: &HeaderMap) -> Option<bool> {
    let token = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    match token {
        // `good-…`: a premium token a test can look for in logs (it must never be there).
        t if t == "good" || t.starts_with("good-") => Some(true),
        "free" => Some(false),
        t if t.starts_with("at-") => Some(true),
        _ => None,
    }
}

fn bad_token() -> Response {
    error(StatusCode::UNAUTHORIZED, "bad_token", 8)
}

/// `GET /rd/rest/1.0/user`.
pub async fn user(headers: HeaderMap) -> Response {
    let Some(premium) = account(&headers) else {
        return bad_token();
    };
    axum::Json(json!({
        "id": 1, "username": "labuser", "email": "lab@example.invalid", "points": 0,
        "locale": "en", "avatar": "", "type": if premium { "premium" } else { "free" },
        "premium": if premium { 30 * 86400 } else { 0 },
        "expiration": "2027-01-01T00:00:00.000Z"
    }))
    .into_response()
}

/// `GET /rd/rest/1.0/hosts/domains` (no sign-in).
pub async fn domains() -> Response {
    axum::Json(json!([HOSTER, "hoster2.test"])).into_response()
}

/// `POST /rd/rest/1.0/unrestrict/link`, form `link`, `password`, `remote`.
pub async fn unrestrict(headers: HeaderMap, body: Bytes) -> Response {
    let Some(premium) = account(&headers) else {
        return bad_token();
    };
    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    if form.get("remote").map(String::as_str) != Some("0") {
        // MDM always sends remote=0; anything else is a test failure, said loudly.
        return error(StatusCode::BAD_REQUEST, "remote must be 0", 2);
    }
    let Some(link) = form.get("link").and_then(|l| url::Url::parse(l).ok()) else {
        return error(StatusCode::BAD_REQUEST, "bad_link", 2);
    };
    if link.host_str() != Some(HOSTER) {
        return error(StatusCode::SERVICE_UNAVAILABLE, "hoster_unsupported", 16);
    }
    if !premium {
        return error(StatusCode::FORBIDDEN, "hoster_not_free", 20);
    }
    let flags: HashMap<String, String> = link.query_pairs().into_owned().collect();
    if flags.contains_key("gone") {
        return error(StatusCode::SERVICE_UNAVAILABLE, "unavailable_file", 24);
    }
    if flags.contains_key("down") {
        return error(StatusCode::SERVICE_UNAVAILABLE, "hoster_unavailable", 19);
    }
    let parts: Vec<&str> = link
        .path_segments()
        .map(|s| s.collect())
        .unwrap_or_default();
    let (size, name) = match parts.as_slice() {
        ["file", size, name] => (*size, *name),
        _ => return error(StatusCode::SERVICE_UNAVAILABLE, "unavailable_file", 24),
    };
    let bytes = crate::content::parse_size(size).unwrap_or(0);
    let lab = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("127.0.0.1");
    let mut direct = format!(
        "http://{lab}/gen/{size}/{name}?seed={}",
        seed(&format!("{HOSTER}/file/{size}/{name}"))
    );
    if let Some(r) = flags.get("rate") {
        direct.push_str(&format!("&rate={r}"));
    }
    if let Some(ttl) = flags.get("ttl_ms").and_then(|t| t.parse::<u64>().ok()) {
        direct.push_str(&format!("&expires={}", unix_ms() + ttl));
    }
    let chunks: i64 = flags
        .get("chunks")
        .and_then(|c| c.parse().ok())
        .unwrap_or(16);
    axum::Json(json!({
        "id": "LAB", "filename": name, "mimeType": "application/octet-stream",
        "filesize": bytes, "link": link.as_str(), "host": HOSTER, "chunks": chunks,
        "crc": 1, "download": direct, "streamable": 0
    }))
    .into_response()
}

/// The content seed of a fake host's file (FNV-1a of its link without flags).
pub fn seed(link: &str) -> u64 {
    link.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

/// `GET /rd/oauth/v2/device/code?client_id&new_credentials=yes`.
pub async fn device_code(Query(q): Query<HashMap<String, String>>) -> Response {
    if q.get("new_credentials").map(String::as_str) != Some("yes") {
        return error(StatusCode::BAD_REQUEST, "new_credentials", 2);
    }
    let code = format!("dc-{}", unix_ms());
    DEVICE_POLLS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(code.clone(), 0);
    axum::Json(json!({
        "device_code": code, "user_code": "LABCODE1", "interval": 1, "expires_in": 120,
        "verification_url": "https://real-debrid.com/device"
    }))
    .into_response()
}

/// `GET /rd/oauth/v2/device/credentials?client_id&code`: refused until allowed.
pub async fn device_credentials(Query(q): Query<HashMap<String, String>>) -> Response {
    let code = q.get("code").cloned().unwrap_or_default();
    let approve_after: u32 = q
        .get("approve_after")
        .and_then(|a| a.parse().ok())
        .unwrap_or(2);
    let mut polls = DEVICE_POLLS.lock().unwrap();
    let Some(n) = polls.get_or_insert_with(HashMap::new).get_mut(&code) else {
        return error(StatusCode::FORBIDDEN, "unknown_code", 3);
    };
    *n += 1;
    if *n <= approve_after {
        return error(StatusCode::FORBIDDEN, "authorization_pending", 9);
    }
    axum::Json(json!({"client_id": format!("cid-{code}"), "client_secret": format!("cs-{code}")}))
        .into_response()
}

/// `POST /rd/oauth/v2/token`: the device grant (code = the device code) and refreshes
/// (code = a refresh token), for the user-bound client the device flow gave.
pub async fn token(body: Bytes) -> Response {
    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let field = |k: &str| form.get(k).cloned().unwrap_or_default();
    if field("grant_type") != "http://oauth.net/grant_type/device/1.0"
        || !field("client_id").starts_with("cid-")
        || !field("client_secret").starts_with("cs-")
    {
        return error(StatusCode::BAD_REQUEST, "invalid_client", 2);
    }
    let code = field("code");
    if !(code.starts_with("dc-") || code.starts_with("rt-")) {
        return error(StatusCode::BAD_REQUEST, "invalid_grant", 2);
    }
    let n = unix_ms();
    axum::Json(json!({
        "access_token": format!("at-{n}"), "expires_in": 3600, "token_type": "Bearer",
        "refresh_token": format!("rt-{n}")
    }))
    .into_response()
}

/// `GET /rd/rest/1.0/disable_access_token`.
pub async fn disable(headers: HeaderMap) -> Response {
    match account(&headers) {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => bad_token(),
    }
}

// ---- AllDebrid (#44 phase 2), as https://docs.alldebrid.com/ documents it ----------------------
//
// Envelopes `{"status":"success","data":…}` / `{"status":"error","error":{"code","message"}}`
// (HTTP 200). Tokens: `good` premium, `free` not. Links as Real-Debrid's fake, plus
// `delayed=N`: the link is "being generated" for N polls of `link/delayed`. The PIN is allowed
// after two polls of `pin/check`, giving the apikey `good`.

type Delayed = HashMap<i64, (u32, String)>;
static AD_DELAYED: Mutex<Option<Delayed>> = Mutex::new(None);
static AD_PINS: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

fn ad_ok(data: serde_json::Value) -> Response {
    axum::Json(json!({"status": "success", "data": data})).into_response()
}

fn ad_error(code: &str) -> Response {
    axum::Json(
        json!({"status": "error", "error": {"code": code, "message": format!("lab: {code}")}}),
    )
    .into_response()
}

/// Some(premium?) for the request's token (AllDebrid's and Premiumize's fakes).
fn bearer(headers: &HeaderMap) -> Option<bool> {
    match headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?
    {
        // `good-…`: a premium token a test can look for in logs (it must never be there).
        t if t == "good" || t.starts_with("good-") => Some(true),
        "free" => Some(false),
        _ => None,
    }
}

/// The fake host's direct link on this lab for `link` (its size, name, address), or why not:
/// "unsupported", "gone", "down".
pub(crate) fn direct_for(
    headers: &HeaderMap,
    link: &url::Url,
) -> Result<(u64, String, String), &'static str> {
    if link.host_str() != Some(HOSTER) {
        return Err("unsupported");
    }
    let flags: HashMap<String, String> = link.query_pairs().into_owned().collect();
    if flags.contains_key("gone") {
        return Err("gone");
    }
    if flags.contains_key("down") {
        return Err("down");
    }
    let parts: Vec<&str> = link
        .path_segments()
        .map(|s| s.collect())
        .unwrap_or_default();
    let ["file", size, name] = parts.as_slice() else {
        return Err("gone");
    };
    let lab = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("127.0.0.1");
    let mut direct = format!(
        "http://{lab}/gen/{size}/{name}?seed={}",
        seed(&format!("{HOSTER}/file/{size}/{name}"))
    );
    if let Some(r) = flags.get("rate") {
        direct.push_str(&format!("&rate={r}"));
    }
    if let Some(ttl) = flags.get("ttl_ms").and_then(|t| t.parse::<u64>().ok()) {
        direct.push_str(&format!("&expires={}", unix_ms() + ttl));
    }
    Ok((
        crate::content::parse_size(size).unwrap_or(0),
        name.to_string(),
        direct,
    ))
}

/// `GET /ad/v4/user`.
pub async fn ad_user(headers: HeaderMap) -> Response {
    let Some(premium) = bearer(&headers) else {
        return ad_error("AUTH_BAD_APIKEY");
    };
    let until = if premium {
        unix_ms() / 1000 + 30 * 86400
    } else {
        0
    };
    ad_ok(
        json!({"user": {"username": "labuser", "email": "lab@example.invalid",
        "isPremium": premium, "isSubscribed": premium, "isTrial": false, "premiumUntil": until}}),
    )
}

/// `GET /ad/v4/hosts/domains` (public).
pub async fn ad_domains() -> Response {
    ad_ok(json!({"hosts": [HOSTER, "hoster2.test"], "streams": [], "redirectors": []}))
}

/// `POST /ad/v4/link/unlock`, form `link`, `password`.
pub async fn ad_unlock(headers: HeaderMap, body: Bytes) -> Response {
    let Some(premium) = bearer(&headers) else {
        return ad_error("AUTH_BAD_APIKEY");
    };
    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let Some(link) = form.get("link").and_then(|l| url::Url::parse(l).ok()) else {
        return ad_error("LINK_IS_MISSING");
    };
    if link.host_str() == Some(HOSTER) && !premium {
        return ad_error("MUST_BE_PREMIUM");
    }
    let (size, name, direct) = match direct_for(&headers, &link) {
        Ok(d) => d,
        Err("unsupported") => return ad_error("LINK_HOST_NOT_SUPPORTED"),
        Err("down") => return ad_error("LINK_HOST_UNAVAILABLE"),
        Err(_) => return ad_error("LINK_DOWN"),
    };
    let polls: u32 = link
        .query_pairs()
        .find(|(k, _)| k == "delayed")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    if polls > 0 {
        let id = (unix_ms() % 1_000_000_000) as i64;
        AD_DELAYED
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(id, (polls, direct));
        return ad_ok(json!({"link": "", "host": "hostertest", "filename": name,
            "filesize": size, "id": "LAB", "hostDomain": HOSTER, "delayed": id}));
    }
    ad_ok(
        json!({"link": direct, "host": "hostertest", "filename": name, "paws": false,
        "filesize": size, "streams": [], "id": "LAB", "hostDomain": HOSTER}),
    )
}

/// `POST /ad/v4/link/delayed`, form `id`.
pub async fn ad_delayed(headers: HeaderMap, body: Bytes) -> Response {
    if bearer(&headers).is_none() {
        return ad_error("AUTH_BAD_APIKEY");
    }
    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let id: i64 = form.get("id").and_then(|i| i.parse().ok()).unwrap_or(-1);
    let mut all = AD_DELAYED.lock().unwrap();
    let Some((left, link)) = all.get_or_insert_with(HashMap::new).get_mut(&id) else {
        return ad_error("DELAYED_INVALID_ID");
    };
    if *left > 0 {
        *left -= 1;
        return ad_ok(json!({"status": 1, "time_left": 1}));
    }
    ad_ok(json!({"status": 2, "time_left": 0, "link": link.clone()}))
}

/// `GET /ad/v4.1/pin/get`.
pub async fn ad_pin() -> Response {
    let check = format!("chk-{}", unix_ms());
    AD_PINS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(check.clone(), 0);
    ad_ok(json!({"pin": "LABP", "check": check, "expires_in": 120,
        "user_url": "https://alldebrid.com/pin/?pin=LABP", "base_url": "https://alldebrid.com/pin/"}))
}

/// `POST /ad/v4/pin/check`, form `check`, `pin`.
pub async fn ad_pin_check(body: Bytes) -> Response {
    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let check = form.get("check").cloned().unwrap_or_default();
    if form.get("pin").map(String::as_str) != Some("LABP") {
        return ad_error("PIN_INVALID");
    }
    let mut all = AD_PINS.lock().unwrap();
    let Some(n) = all.get_or_insert_with(HashMap::new).get_mut(&check) else {
        return ad_error("PIN_EXPIRED");
    };
    *n += 1;
    if *n <= 2 {
        return ad_ok(json!({"activated": false, "expires_in": 100}));
    }
    ad_ok(json!({"activated": true, "expires_in": 100, "apikey": "good"}))
}

// ---- Premiumize (#44 phase 2), as https://www.premiumize.me/api documents it ------------------
//
// `{"status":"success",…}` / `{"status":"error","message","code"}`, HTTP 200 for business
// errors. Tokens `good` (premium) and `free`. Its service "hostertest" has the alias hoster.test.

fn pm_error(code: &str) -> Response {
    axum::Json(json!({"status": "error", "message": format!("lab: {code}"), "code": code}))
        .into_response()
}

/// `GET /pm/api/account/info`.
pub async fn pm_account(headers: HeaderMap) -> Response {
    let Some(premium) = bearer(&headers) else {
        return pm_error("authentication_failed");
    };
    let until = premium.then(|| unix_ms() / 1000 + 30 * 86400);
    axum::Json(json!({"status": "success", "customer_id": "1234567",
        "premium_until": until, "limit_used": 0.1, "booster_points": 0}))
    .into_response()
}

/// `GET /pm/api/services/list`.
pub async fn pm_services(headers: HeaderMap) -> Response {
    if bearer(&headers).is_none() {
        return pm_error("authentication_failed");
    }
    axum::Json(
        json!({"status": "success", "cache": [], "directdl": ["hostertest"],
        "queue": [], "fairusefactor": {"hostertest": 1},
        "aliases": {"hostertest": [HOSTER]}, "regexpatterns": {}}),
    )
    .into_response()
}

/// `POST /pm/api/transfer/directdl`, form `src`.
pub async fn pm_directdl(headers: HeaderMap, body: Bytes) -> Response {
    let Some(premium) = bearer(&headers) else {
        return pm_error("authentication_failed");
    };
    if !premium {
        return pm_error("permission_denied");
    }
    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let Some(link) = form.get("src").and_then(|l| url::Url::parse(l).ok()) else {
        return pm_error("not_found");
    };
    match direct_for(&headers, &link) {
        Ok((size, name, direct)) => axum::Json(json!({"status": "success",
            "content": [{"path": name, "size": size, "link": direct}]}))
        .into_response(),
        Err("unsupported") => pm_error("service_unsupported"),
        Err("down") => pm_error("service_down"),
        Err(_) => pm_error("not_found"),
    }
}

// ---- Mega-Debrid, as https://www.mega-debrid.eu/index.php?page=api documents it -------------
//
// One address, `POST /md/api.php?action=NAME`, the parameters (the token, user name and
// password too) in the form, as MDM sends them. Every answer has `response_code`: "ok", or the
// error's code and `response_text`, with HTTP 200. Keys `good` (or `good-…`, premium) and `free`;
// the user `labuser` (premium) or `freeuser` signs in with the password `labpass`, and each
// sign-in replaces the user's last token ("valid until the next connection attempt").

static MD_TOKENS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
static MD_SIGNINS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn md_error(code: &str, text: &str) -> Response {
    axum::Json(json!({"response_code": code, "response_text": text})).into_response()
}

/// Some(premium?) for a token: a key, or a sign-in's token that no newer sign-in replaced.
fn md_token(form: &HashMap<String, String>) -> Option<bool> {
    let t = form.get("token")?;
    if t == "good" || (t.starts_with("good-") && !t.starts_with("good-md-")) {
        return Some(true);
    }
    if t == "free" {
        return Some(false);
    }
    let tokens = MD_TOKENS.lock().unwrap();
    let user = tokens
        .as_ref()?
        .iter()
        .find(|(_, current)| *current == t)?
        .0;
    Some(user == "labuser")
}

/// `POST /md/api.php?action=…`.
pub async fn md_api(
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let form: HashMap<String, String> = serde_urlencoded::from_bytes(&body).unwrap_or_default();
    let action = q.get("action").map(String::as_str).unwrap_or("");
    let token_error = || md_error("TOKEN_ERROR", "Token error, please log-in");
    match action {
        "connectUser" => {
            let user = form.get("login").cloned().unwrap_or_default();
            if !matches!(user.as_str(), "labuser" | "freeuser")
                || form.get("password").map(String::as_str) != Some("labpass")
            {
                return md_error("UNKNOWN_USER", "User and password doesn't match");
            }
            let n = MD_SIGNINS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let token = format!("good-md-{n}");
            MD_TOKENS
                .lock()
                .unwrap()
                .get_or_insert_default()
                .insert(user.clone(), token.clone());
            let now = unix_ms() as i64 / 1000;
            let vip_end = if user == "labuser" {
                now + 30 * 86400
            } else {
                now - 86400
            };
            axum::Json(
                json!({"response_code": "ok", "response_text": "User logged",
                "token": token, "vip_end": vip_end.to_string(),
                "email": format!("{user}@lab.test")}),
            )
            .into_response()
        }
        "getUserHistory" => match md_token(&form) {
            Some(_) => axum::Json(json!({"response_code": "ok", "history": []})).into_response(),
            None => token_error(),
        },
        "getHostersList" => axum::Json(json!({"response_code": "ok", "response_text": "",
            "hosters": [{"name": "hostertest", "url": "Hostertest", "img": "",
                "domains": [HOSTER], "status": "up", "regexps": [], "type": "file"}]}))
        .into_response(),
        "getLink" => {
            let Some(premium) = md_token(&form) else {
                return token_error();
            };
            if !premium {
                return md_error("VIP_END", "lab: premium only");
            }
            let Some(link) = form.get("link").and_then(|l| url::Url::parse(l).ok()) else {
                return md_error("PARAMETER_MISSING", "Parameter link is missing");
            };
            match direct_for(&headers, &link) {
                // Quoted, as some of its answers are.
                Ok((_, _, direct)) => axum::Json(json!({"response_code": "ok",
                    "response_text": "", "debridLink": format!("\"{direct}\"")}))
                .into_response(),
                Err("unsupported") => md_error("UNKNOWN_HOSTER", "lab: unsupported hoster"),
                Err("down") => md_error("HOSTER_DOWN", "lab: hoster down"),
                Err(_) => md_error("FILE_NOT_FOUND", "lab: file not found"),
            }
        }
        _ => md_error("UNKNOWN_ACTION", "This action is unknown"),
    }
}
