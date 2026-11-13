//! Web pages for the browser extension's tests (tools/extension/e2e.mjs): `/page` (links, videos,
//! audio, adaptive streams; optionally under a strict Content-Security-Policy), `/formpage`
//! (forms that post to `/form`), `/cookie` (sets cookies) and their `/assets`. Styles and scripts
//! are files, never inline, so the pages work under `csp=strict`.

use std::collections::HashMap;

use axum::extract::{Path, RawQuery};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::faults::parse_flag;

const PAGE_CSS: &str = include_str!("../assets/page.css");
const STREAM_JS: &str = include_str!("../assets/stream.js");

/// What `video=1` (and a page that asks for nothing else) plays.
const DEFAULT_VIDEO: &str = "/gen/1M/clip.mp4?inline=1";
/// What `audio=1` plays.
const DEFAULT_AUDIO: &str = "/gen/512K/sound.mp3?inline=1";

/// Escapes text for HTML content and double-quoted attributes.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The query as ordered pairs (a bad query reads as empty).
fn pairs(q: Option<String>) -> Vec<(String, String)> {
    q.and_then(|q| serde_urlencoded::from_str(&q).ok())
        .unwrap_or_default()
}

fn head(title: &str) -> String {
    format!(
        "<!doctype html><meta charset=utf-8><title>{}</title>\
         <link rel=stylesheet href=/assets/page.css><body>",
        esc(title)
    )
}

fn html(body: String) -> Response {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()
}

fn video_tag(id: &str, src: &str, (w, h): (u32, u32), muted_loop: bool) -> String {
    let how = if muted_loop {
        "muted loop autoplay playsinline"
    } else {
        "controls"
    };
    format!(
        "<video id={id} src=\"{}\" width={w} height={h} {how}></video>",
        esc(src)
    )
}

/// `/page?title=TEXT&link=PATH&video=PATH|1&w=&h=&muted_loop=1&iframe=1&shadow=1&several=N&audio=PATH|1&
/// hls=1&dash=1&drm=1&hls2=1&dash2=1&hlsg=SITE&csp=strict`: a web page with what the browser
/// extension acts on (see `HELP`).
pub(crate) async fn page(RawQuery(q): RawQuery, headers: HeaderMap) -> Response {
    let pairs = pairs(q);
    let m: HashMap<&str, &str> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let flag = |k: &str| m.get(k).and_then(|v| parse_flag(v)).unwrap_or(false);
    let num = |k: &str, default: u32| m.get(k).and_then(|v| v.parse().ok()).unwrap_or(default);
    let size = (num("w", 640), num("h", 360));
    let (muted_loop, iframe, shadow) = (flag("muted_loop"), flag("iframe"), flag("shadow"));
    let several = num("several", 0).min(16);
    let csp = m.get("csp") == Some(&"strict");
    // #900: the stream on another site (`hlsg=SITE`), served only to this page's Origin and
    // Referer: SITE's address for this host's guarded stream.
    let guarded = m
        .get("hlsg")
        .zip(headers.get(header::HOST).and_then(|v| v.to_str().ok()))
        .map(|(site, host)| format!("http://{site}/hlsg/{host}/master.m3u8"));
    // A player for each stream asked for, in this order.
    let mut streams: Vec<(&str, &str)> = [
        ("hls", "/hls/master.m3u8"),
        ("dash", "/dash/manifest.mpd"),
        ("drm", "/dash/protected.mpd"),
        ("hls2", "/hls2/playlist?id=lab"),
        ("dash2", "/dash2/manifest?id=lab"),
    ]
    .into_iter()
    .filter(|(k, _)| flag(k))
    .collect();
    if let Some(manifest) = &guarded {
        streams.push(("hlsg", manifest));
    }
    // `link` may come again: one link each, in order.
    let links: Vec<&str> = pairs
        .iter()
        .filter(|(k, _)| k == "link")
        .map(|(_, v)| v.as_str())
        .collect();
    let link = links.first();
    let audio = m.get("audio").map(|a| {
        if parse_flag(a) == Some(true) {
            DEFAULT_AUDIO
        } else {
            a
        }
    });
    // A video when asked for, or when the page would otherwise be empty.
    let video = match m.get("video") {
        Some(v) if parse_flag(v) == Some(true) => Some(DEFAULT_VIDEO),
        Some(v) => Some(*v),
        None if muted_loop
            || iframe
            || shadow
            || (link.is_none() && audio.is_none() && several == 0 && streams.is_empty()) =>
        {
            Some(DEFAULT_VIDEO)
        }
        None => None,
    };

    let mut body = head(m.get("title").copied().unwrap_or("labserver page"));
    for (k, link) in links.iter().enumerate() {
        let id = if k == 0 {
            "link".to_owned()
        } else {
            format!("link{}", k + 1)
        };
        let name = link.rsplit('/').next().unwrap_or("download");
        let text = if k == 0 { "download" } else { name };
        body += &format!("<p><a id={id} href=\"{}\">{}</a></p>", esc(link), esc(text));
    }
    if let Some(src) = video {
        if iframe {
            // The same page without the iframe, holding just the video.
            let inner: Vec<(&str, &str)> = pairs
                .iter()
                .filter(|(k, _)| ["muted_loop", "w", "h", "shadow", "csp"].contains(&k.as_str()))
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .chain([("video", src)])
                .collect();
            let inner = serde_urlencoded::to_string(inner).unwrap_or_default();
            body += &format!(
                "<iframe id=frame title=video src=\"/page?{}\" width={} height={}></iframe>",
                esc(&inner),
                size.0 + 40,
                size.1 + 40
            );
        } else if shadow {
            body += &format!(
                "<div id=host><template shadowrootmode=open>\
                 <link rel=stylesheet href=/assets/page.css>{}</template></div>",
                video_tag("video", src, size, muted_loop)
            );
        } else {
            body += &video_tag("video", src, size, muted_loop);
        }
    }
    for k in 1..=several {
        body += &format!(
            "<video id=video{k} width={} height={} controls>\
             <source src=\"/gen/1M/clip{k}.mp4?seed={k}&amp;inline=1\" type=video/mp4></video>",
            size.0, size.1
        );
    }
    if let Some(src) = audio {
        body += &format!("<audio id=audio src=\"{}\" controls></audio>", esc(src));
    }
    for (k, (kind, manifest)) in streams.iter().enumerate() {
        let id = if k == 0 {
            "player".to_owned()
        } else {
            format!("player{}", k + 1)
        };
        // #171's have no .m3u8 or .mpd to tell: their kind, and a text answer first that is no
        // manifest (a player's settings).
        let more = match *kind {
            "hls2" => " data-kind=hls data-decoy=\"/hls2/config?id=lab\"",
            "dash2" => " data-kind=dash data-decoy=\"/hls2/config?id=lab\"",
            _ => "",
        };
        let manifest = esc(manifest);
        body += &format!(
            "<video id={id} width={} height={} controls></video>\
             <script src=/assets/stream.js data-manifest=\"{manifest}\" data-player={id}{more}>\
             </script>",
            size.0, size.1
        );
    }
    let mut resp = html(body);
    if csp {
        let media = if !streams.is_empty() {
            "'self' blob:"
        } else {
            "'self'"
        };
        let mut policy = format!(
            "default-src 'none'; style-src 'self'; img-src 'self'; media-src {media}; \
             script-src 'self'"
        );
        if !streams.is_empty() {
            policy += "; connect-src 'self'";
        }
        if iframe {
            policy += "; frame-src 'self'";
        }
        resp.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            policy.parse().expect("ASCII policy"),
        );
    }
    resp
}

/// `/leadon?to=PATH`: a page that goes to `to` by script as it loads, as a download site's
/// thank-you page does (#374).
pub(crate) async fn leadon(RawQuery(q): RawQuery) -> Response {
    let pairs = pairs(q);
    let to = pairs
        .iter()
        .find(|(k, _)| k == "to")
        .map_or("/", |(_, v)| v.as_str());
    let to = to.replace('\\', "\\\\").replace('"', "\\\"");
    html(format!(
        "{}Thank you<script>location.href = \"{to}\";</script>",
        head("Thank you")
    ))
}

/// `/formpage?size=1M&name=report.zip&token=abc&with=QUERY`: three forms that post to `/form`
/// (urlencoded, multipart, and urlencoded answered with 303).
pub(crate) async fn formpage(RawQuery(q): RawQuery) -> Response {
    let m: HashMap<String, String> = pairs(q).into_iter().collect();
    let get = |k: &str, default: &str| m.get(k).cloned().unwrap_or_else(|| default.to_owned());
    let (size, name, token) = (
        get("size", "1M"),
        get("name", "report.zip"),
        get("token", "abc"),
    );
    let with = m
        .get("with")
        .filter(|w| !w.is_empty())
        .map(|w| format!("&{w}"))
        .unwrap_or_default();
    let fields = format!(
        "<input type=hidden name=token value=\"{}\"><input name=note value=\"hello world\">",
        esc(&token)
    );
    let mut body = head("labserver forms");
    for (id, enctype, file, query, label) in [
        (
            "form",
            "application/x-www-form-urlencoded",
            name.clone(),
            "seed=5",
            "Download",
        ),
        (
            "multipart",
            "multipart/form-data",
            format!("multipart-{name}"),
            "seed=6",
            "Download (multipart)",
        ),
        (
            "see-other",
            "application/x-www-form-urlencoded",
            format!("see-other-{name}"),
            "seed=7&see_other=1",
            "Download (303)",
        ),
    ] {
        body += &format!(
            "<form id={id} method=post enctype={enctype} action=\"{}\">{fields}\
             <button id=submit-{id}>{label}</button></form>",
            esc(&format!("/form/{size}/{file}?{query}{with}"))
        );
    }
    html(body)
}

/// `/cookie?set=NAME=VALUE&httponly=NAME=VALUE&clear=NAME` (each repeatable): a small page that
/// sets (or clears) cookies for the whole site.
pub(crate) async fn cookie(RawQuery(q): RawQuery) -> Response {
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_graphic() && !b"()<>@,;:\\\"/[]?={}".contains(&b))
    };
    let value_ok = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_graphic() && !b"\",;\\".contains(&b))
    };
    let mut cookies = Vec::new();
    for (k, v) in pairs(q) {
        let line = match k.as_str() {
            "set" | "httponly" => match v.split_once('=') {
                Some((n, val)) if token(n) && value_ok(val) => {
                    let http_only = if k == "httponly" { "; HttpOnly" } else { "" };
                    format!("{n}={val}; Path=/{http_only}")
                }
                _ => {
                    return (StatusCode::BAD_REQUEST, format!("bad cookie `{v}`\n"))
                        .into_response();
                }
            },
            "clear" if token(&v) => format!("{v}=; Path=/; Max-Age=0"),
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("bad parameter `{k}={v}`\n"),
                )
                    .into_response();
            }
        };
        cookies.push(line);
    }
    if cookies.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "/cookie?set=NAME=VALUE&httponly=NAME=VALUE&clear=NAME\n",
        )
            .into_response();
    }
    let mut body = head("labserver cookies");
    body += "<ul id=cookies>";
    for c in &cookies {
        body += &format!("<li>{}</li>", esc(c));
    }
    body += "</ul>";
    let mut resp = html(body);
    for c in cookies {
        resp.headers_mut()
            .append(header::SET_COOKIE, c.parse().expect("checked cookie"));
    }
    resp
}

/// `/assets/{file}`: the pages' stylesheet and the stream player's script.
pub(crate) async fn asset(Path(file): Path<String>) -> Response {
    let (ty, text) = match file.as_str() {
        "page.css" => ("text/css; charset=utf-8", PAGE_CSS),
        "stream.js" => ("text/javascript; charset=utf-8", STREAM_JS),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(header::CONTENT_TYPE, ty)], text).into_response()
}
