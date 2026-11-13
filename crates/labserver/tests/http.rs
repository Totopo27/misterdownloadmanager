//! End-to-end: a real HTTP client against a running labserver.

use std::time::Duration;

use mdm_labserver::{Config, Faults, LabServer, LogEntry, content};

async fn server(faults: &str) -> LabServer {
    LabServer::start(Config {
        faults: Faults::parse(faults).unwrap(),
        ..Config::default()
    })
    .await
    .unwrap()
}

fn expected(seed: u64, start: u64, len: usize) -> Vec<u8> {
    let mut v = vec![0; len];
    content::fill(seed, start, &mut v);
    v
}

/// Log entries are written when a response body is dropped, which can trail the client slightly.
async fn wait_log(s: &LabServer, count: usize) -> Vec<LogEntry> {
    for _ in 0..200 {
        let log = s.log();
        if log.len() >= count {
            return log;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("expected {count} log entries, got {:?}", s.log());
}

#[tokio::test]
async fn full_get() {
    let s = server("").await;
    let r = reqwest::get(s.url("/gen/100K/a.bin?seed=3")).await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["accept-ranges"], "bytes");
    assert_eq!(
        r.headers()["content-disposition"],
        "attachment; filename=\"a.bin\""
    );
    assert_eq!(r.bytes().await.unwrap(), expected(3, 0, 100 * 1024));
    let log = wait_log(&s, 1).await;
    assert_eq!(log[0].outcome, "complete");
    assert_eq!(log[0].bytes_sent, 100 * 1024);
}

#[tokio::test]
async fn range_and_keepalive_reuse() {
    let s = server("").await;
    let c = reqwest::Client::new();
    for (i, (a, b)) in [(10u64, 19u64), (5000, 9999)].into_iter().enumerate() {
        let r = c
            .get(s.url("/gen/10000"))
            .header("Range", format!("bytes={a}-{b}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(r.headers()["content-range"], format!("bytes {a}-{b}/10000"));
        assert_eq!(
            r.bytes().await.unwrap(),
            expected(0, a, (b - a + 1) as usize)
        );
        let log = wait_log(&s, i + 1).await;
        assert_eq!(log[i].conn_req, i as u64 + 1, "same TCP connection reused");
    }
}

#[tokio::test]
async fn head_and_416() {
    let s = server("").await;
    let c = reqwest::Client::new();
    let r = c.head(s.url("/gen/1M")).send().await.unwrap();
    assert_eq!(r.headers()["content-length"], "1048576");
    let r = c
        .get(s.url("/gen/100"))
        .header("Range", "bytes=100-")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 416);
    assert_eq!(r.headers()["content-range"], "bytes */100");
    let log = wait_log(&s, 2).await;
    assert_eq!(log[0].outcome, "head");
    assert_eq!(log[1].status, 416);
}

#[tokio::test]
async fn no_range_and_if_range() {
    let s = server("").await;
    let c = reqwest::Client::new();
    let r = c
        .get(s.url("/gen/1000?no_range=true"))
        .header("Range", "bytes=10-")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers().get("accept-ranges").is_none());

    // Matching If-Range -> 206; stale validator -> whole file with 200.
    let etag = r.headers()["etag"].to_str().unwrap().to_owned();
    let get = |v: String| {
        c.get(s.url("/gen/1000"))
            .header("Range", "bytes=10-")
            .header("If-Range", v)
            .send()
    };
    assert_eq!(get(etag).await.unwrap().status(), 206);
    assert_eq!(get("\"stale\"".into()).await.unwrap().status(), 200);
}

#[tokio::test]
async fn etag_changes_every_n() {
    let s = server("etag_change_every=2").await;
    let c = reqwest::Client::new();
    let mut tags = Vec::new();
    for _ in 0..3 {
        let r = c.head(s.url("/gen/10")).send().await.unwrap();
        tags.push(r.headers()["etag"].to_str().unwrap().to_owned());
    }
    assert_ne!(tags[0], tags[1]);
    assert_eq!(tags[1], tags[2]);
}

#[tokio::test]
async fn drop_after() {
    let s = server("drop_after=10K").await;
    let r = reqwest::get(s.url("/gen/1M")).await.unwrap();
    assert!(r.bytes().await.is_err(), "body must fail mid-stream");
    let log = wait_log(&s, 1).await;
    assert_eq!(log[0].outcome, "dropped");
    assert_eq!(log[0].bytes_sent, 10 * 1024);
}

#[tokio::test]
async fn a_refusal_shows_how_far_the_slots_responses_were_sent() {
    // #224: the engine's test tells a reconnect the server refused because it still counted the
    // response it replaces (sent up to where another connection's part begins) from a
    // connection too many.
    let s = server("max_conns=2&rate=200K").await;
    let c = reqwest::Client::new();
    let mut first = c
        .get(s.url("/gen/1M"))
        .header("range", "bytes=4096-")
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 206);
    let mut got = 0;
    while got < 20_000 {
        got += first.chunk().await.unwrap().unwrap().len() as u64;
    }
    let ip = "127.0.0.1".parse().unwrap();
    let hold = s.hold_slot(ip);
    let refused = c.get(s.url("/gen/1M")).send().await.unwrap();
    assert_eq!(refused.status(), 503);
    drop(hold);
    drop(first);
    let log = wait_log(&s, 2).await;
    let r = log.iter().find(|e| e.outcome == "fault:max_conns").unwrap();
    assert_eq!(r.held.len(), 2, "{:?}", r.held);
    let response = r
        .held
        .iter()
        .find(|h| h.from == 4096)
        .expect("the response");
    assert!(
        (4096 + got..=1 << 20).contains(&response.sent_to),
        "{response:?}, {got} bytes received"
    );
    assert!(
        r.held.contains(&mdm_labserver::Held::default()),
        "the test's slot"
    );
    // Other entries have none.
    assert!(
        log.iter()
            .all(|e| e.outcome == "fault:max_conns" || e.held.is_empty())
    );
}

#[tokio::test]
async fn max_conns_and_fail_every() {
    let s = server("max_conns=1&rate=20K").await;
    let c = reqwest::Client::new();
    let first = c.get(s.url("/gen/1M")).send().await.unwrap(); // body in flight, slot held
    let second = c.get(s.url("/gen/1M")).send().await.unwrap();
    assert_eq!(first.status(), 200);
    assert_eq!(second.status(), 503);
    drop(first);
    let log = wait_log(&s, 2).await;
    assert!(log.iter().any(|e| e.outcome == "fault:max_conns"));

    let s = server("fail_every=2").await;
    let codes: Vec<u16> = futures::future::join_all((0..4).map(|_| async {
        reqwest::get(s.url("/gen/10"))
            .await
            .unwrap()
            .status()
            .as_u16()
    }))
    .await;
    assert_eq!(codes.iter().filter(|c| **c == 503).count(), 2);
}

#[tokio::test]
async fn rate_limit_is_respected() {
    let s = server("").await;
    let t = std::time::Instant::now();
    let body = reqwest::get(s.url("/gen/100K?rate=200K"))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let secs = t.elapsed().as_secs_f64();
    assert_eq!(body.len(), 100 * 1024);
    assert!((0.4..1.5).contains(&secs), "100K at 200K/s took {secs:.2}s");
}

#[tokio::test]
async fn bad_query_is_rejected() {
    let s = server("").await;
    let r = reqwest::get(s.url("/gen/10?speed=1")).await.unwrap();
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn log_records_what_the_client_sent() {
    let s = server("").await;
    let c = reqwest::Client::new();
    c.get(s.url("/gen/10/a.bin"))
        .header("Referer", "http://example.test/page")
        .header("Cookie", "sid=1")
        .header("User-Agent", "lab-test/1")
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let r = c
        .post(s.url("/form/10/f.bin?seed=2"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("token=abc&note=hi")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.bytes().await.unwrap(), expected(2, 0, 10));
    let log = wait_log(&s, 2).await;
    let get = &log[0];
    assert_eq!(get.method, "GET");
    assert_eq!(get.referer.as_deref(), Some("http://example.test/page"));
    assert_eq!(get.cookie.as_deref(), Some("sid=1"));
    assert_eq!(get.user_agent.as_deref(), Some("lab-test/1"));
    assert_eq!(get.req_body_len, None);
    let post = &log[1];
    assert_eq!(post.method, "POST");
    assert_eq!(post.path, "/form/10/f.bin");
    assert_eq!(post.req_body_len, Some(17));
    assert_eq!(post.req_body_head.as_deref(), Some("token=abc&note=hi"));
    assert_eq!(
        post.req_content_type.as_deref(),
        Some("application/x-www-form-urlencoded")
    );
    // SHA-256 of "abc", the well-known test vector, to pin the hash and its hex form.
    let r = c
        .post(s.url("/form/10?need=x%3D"))
        .body("abc")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let log = wait_log(&s, 3).await;
    assert_eq!(
        log[2].req_body_sha256.as_deref(),
        Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
    );
    assert_eq!(post.outcome, "complete");
    assert!(post.req > get.req);
}

#[tokio::test]
async fn pages_are_logged_too() {
    let s = server("").await;
    reqwest::get(s.url("/page?link=%2Fgen%2F1K%2Fa.zip"))
        .await
        .unwrap();
    let log = wait_log(&s, 1).await;
    assert_eq!(log[0].path, "/page");
    assert_eq!(log[0].status, 200);
    assert!(log[0].body_len > 0);
    // The redirect hop is not logged (the engine's tests read the log as the file requests).
    let c = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    c.get(s.url("/redirect?to=%2Fgen%2F10"))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(s.log().len(), 1);
}

#[tokio::test]
async fn cookie_page_sets_cookies() {
    let s = server("").await;
    let r = reqwest::get(s.url("/cookie?set=sid=1&httponly=auth=xyz&clear=old"))
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let set: Vec<_> = r
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        set,
        [
            "sid=1; Path=/",
            "auth=xyz; Path=/; HttpOnly",
            "old=; Path=/; Max-Age=0"
        ]
    );
    for bad in [
        "/cookie",
        "/cookie?set=nope",
        "/cookie?set=a=b%3Bc",
        "/cookie?x=1",
    ] {
        let r = reqwest::get(s.url(bad)).await.unwrap();
        assert_eq!(r.status(), 400, "{bad}");
    }
}

#[tokio::test]
async fn require_cookie_and_referer() {
    let s = server("").await;
    let c = reqwest::Client::new();
    let url = s.url("/gen/10/a.bin?require_cookie=sid=1");
    assert_eq!(c.get(&url).send().await.unwrap().status(), 403);
    let wrong = c.get(&url).header("Cookie", "sid=2").send().await.unwrap();
    assert_eq!(wrong.status(), 403);
    let ok = c
        .get(&url)
        .header("Cookie", "a=b; sid=1")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    ok.bytes().await.unwrap();

    let url = s.url("/gen/10/a.bin?require_referer=http%3A%2F%2F127.0.0.1%2Fpage");
    assert_eq!(c.get(&url).send().await.unwrap().status(), 403);
    let ok = c
        .get(&url)
        .header("Referer", "http://127.0.0.1/page?x=1")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    ok.bytes().await.unwrap();
    let log = wait_log(&s, 5).await;
    let count = |o: &str| log.iter().filter(|e| e.outcome == o).count();
    assert_eq!(count("fault:require_cookie"), 2);
    assert_eq!(count("fault:require_referer"), 1);

    // Server-wide, as --faults would set it.
    let s = server("require_cookie=sid").await;
    assert_eq!(reqwest::get(s.url("/gen/10")).await.unwrap().status(), 403);
}

#[tokio::test]
async fn disposition_names_and_inline_types() {
    let s = server("").await;
    let c = reqwest::Client::new();
    let head = async |p: &str| c.head(s.url(p)).send().await.unwrap();
    let r = head("/gen/10/download.php?cd=report%202026.zip").await;
    assert_eq!(
        r.headers()["content-disposition"],
        "attachment; filename=\"report 2026.zip\""
    );
    assert_eq!(r.headers()["content-type"], "application/octet-stream");
    let r = head("/gen/10/get?cdstar=%E6%8A%A5%E5%91%8A.zip").await;
    assert_eq!(
        r.headers()["content-disposition"],
        "attachment; filename=\"fallback-name.zip\"; filename*=UTF-8''%E6%8A%A5%E5%91%8A.zip"
    );
    for (name, ty) in [
        ("doc.pdf", "application/pdf"),
        ("song.mp3", "audio/mpeg"),
        ("clip.mp4", "video/mp4"),
    ] {
        let r = head(&format!("/gen/10/{name}?inline=1")).await;
        assert_eq!(r.headers()["content-type"], ty);
        assert_eq!(
            r.headers()["content-disposition"],
            format!("inline; filename=\"{name}\"")
        );
        let r = head(&format!("/gen/10/{name}?inline=bare")).await;
        assert_eq!(r.headers()["content-type"], ty);
        assert!(r.headers().get("content-disposition").is_none());
    }
    let r = head("/gen/10?inline=1").await;
    assert_eq!(r.headers()["content-disposition"], "inline");
    assert_eq!(head("/gen/10?inline=sometimes").await.status(), 400);
}

#[tokio::test]
async fn forms_answer_only_their_post() {
    let s = server("").await;
    let c = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let r = c.get(s.url("/form/1K/r.zip")).send().await.unwrap();
    assert_eq!(r.status(), 405);
    assert_eq!(r.headers()["allow"], "POST");
    let post = async |path: &str, ty: &str, body: &'static str| {
        c.post(s.url(path))
            .header("Content-Type", ty)
            .body(body)
            .send()
            .await
            .unwrap()
    };
    let url = "application/x-www-form-urlencoded";
    let r = post("/form/1K/r.zip?seed=5", url, "token=abc").await;
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers()["content-disposition"],
        "attachment; filename=\"r.zip\""
    );
    assert_eq!(r.bytes().await.unwrap(), expected(5, 0, 1024));
    assert_eq!(post("/form/1K/r.zip", url, "token=abd").await.status(), 403);
    let r = post("/form/1K/r.zip?need=code%3D7", url, "code=7").await;
    assert_eq!(r.status(), 200);
    r.bytes().await.unwrap();
    let mp = "--b0\r\nContent-Disposition: form-data; name=\"token\"\r\n\r\nabc\r\n--b0--\r\n";
    let r = post(
        "/form/1K/m.zip?seed=6",
        "multipart/form-data; boundary=b0",
        mp,
    )
    .await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.bytes().await.unwrap(), expected(6, 0, 1024));
    let r = post(
        "/form/1K/s.zip?seed=7&see_other=1&cd=x.zip",
        url,
        "token=abc",
    )
    .await;
    assert_eq!(r.status(), 303);
    assert_eq!(r.headers()["location"], "/gen/1K/s.zip?seed=7&cd=x.zip");
    let log = wait_log(&s, 6).await;
    let outcomes: Vec<_> = log.iter().map(|e| e.outcome.as_str()).collect();
    for o in ["method", "form", "see_other"] {
        assert!(outcomes.contains(&o), "{o}: {outcomes:?}");
    }
}

#[tokio::test]
async fn one_time_links() {
    let s = server("").await;
    let c = reqwest::Client::new();
    let url = s.url("/gen/1K/once.zip?once=1&seed=1");
    let first = c.get(&url).send().await.unwrap();
    assert_eq!(first.status(), 200);
    assert_eq!(first.bytes().await.unwrap().len(), 1024);
    assert_eq!(c.get(&url).send().await.unwrap().status(), 410);
    assert_eq!(c.head(&url).send().await.unwrap().status(), 410);
    // Another query is another link; a HEAD uses it up.
    let other = s.url("/gen/1K/once.zip?once=1&seed=2");
    assert_eq!(c.head(&other).send().await.unwrap().status(), 200);
    assert_eq!(c.get(&other).send().await.unwrap().status(), 410);
    // A refused request (no cookie) doesn't.
    let gated = s.url("/gen/1K/g.zip?once=1&require_cookie=sid");
    assert_eq!(c.get(&gated).send().await.unwrap().status(), 403);
    let ok = c
        .get(&gated)
        .header("Cookie", "sid=1")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    ok.bytes().await.unwrap();
    let log = wait_log(&s, 7).await;
    assert_eq!(log.iter().filter(|e| e.outcome == "fault:once").count(), 3);
}

async fn text(s: &LabServer, path: &str) -> (reqwest::header::HeaderMap, String) {
    let r = reqwest::get(s.url(path)).await.unwrap();
    assert_eq!(r.status(), 200, "{path}");
    (r.headers().clone(), r.text().await.unwrap())
}

#[tokio::test]
async fn page_variants() {
    let s = server("").await;
    let (h, page) = text(&s, "/page?video=%2Fgen%2F1M%2Fclip.mp4").await;
    assert!(h.get("content-security-policy").is_none());
    assert!(page.contains(
        "<video id=video src=\"/gen/1M/clip.mp4\" width=640 height=360 controls></video>"
    ));
    assert!(page.contains("href=/assets/page.css"));
    assert!(!page.contains("style="), "no inline styles (csp=strict)");

    let (h, page) = text(&s, "/page?csp=strict").await;
    assert_eq!(
        h["content-security-policy"],
        "default-src 'none'; style-src 'self'; img-src 'self'; media-src 'self'; script-src 'self'"
    );
    assert!(page.contains("<video id=video src=\"/gen/1M/clip.mp4?inline=1\""));

    let (_, page) = text(&s, "/page?muted_loop=1&w=320&h=180").await;
    assert!(page.contains("width=320 height=180 muted loop autoplay playsinline></video>"));
    assert!(!page.contains("controls"));

    let (h, page) = text(&s, "/page?iframe=1&csp=strict&w=320&h=180").await;
    assert!(page.contains("<iframe id=frame"));
    assert!(page.contains("width=360 height=220"));
    assert!(!page.contains("<video"));
    let csp = h["content-security-policy"].to_str().unwrap();
    assert!(csp.ends_with("; frame-src 'self'"));
    let inner = page.split("<iframe").nth(1).unwrap();
    let inner = inner.split("src=\"").nth(1).unwrap().split('"').next();
    let (h, inner) = text(&s, &inner.unwrap().replace("&amp;", "&")).await;
    assert!(inner.contains("<video id=video") && inner.contains("width=320 height=180"));
    assert!(h.contains_key("content-security-policy"));

    let (_, page) = text(&s, "/page?shadow=1").await;
    assert!(page.contains("<div id=host><template shadowrootmode=open>"));

    let (_, page) = text(&s, "/page?several=2").await;
    assert!(page.contains("id=video1") && page.contains("id=video2"));
    assert!(page.contains("<source src=\"/gen/1M/clip2.mp4?seed=2&amp;inline=1\""));
    assert!(!page.contains("id=video "));

    let (_, page) = text(&s, "/page?audio=1&link=%2Fx").await;
    assert!(page.contains("<audio id=audio src=\"/gen/512K/sound.mp3?inline=1\" controls>"));
    assert!(page.contains("<a id=link href=\"/x\">"));
    assert!(!page.contains("<video"));

    let (h, page) = text(&s, "/page?hls=1&csp=strict").await;
    assert!(page.contains(
        "<script src=/assets/stream.js data-manifest=\"/hls/master.m3u8\" data-player=player>"
    ));
    let csp = h["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("connect-src 'self'") && csp.contains("media-src 'self' blob:"));
    let (_, page) = text(&s, "/page?dash=1").await;
    assert!(page.contains("data-manifest=\"/dash/manifest.mpd\""));
    // Several streams: a player each, in order (#38's media list).
    let (_, page) = text(&s, "/page?hls=1&dash=1&drm=1&video=1").await;
    assert!(page.contains("<video id=player ") && page.contains("data-player=player>"));
    assert!(
        page.contains("<video id=player3 ")
            && page.contains("data-manifest=\"/dash/protected.mpd\" data-player=player3")
    );
    assert!(page.contains("<video id=video src=\"/gen/1M/clip.mp4?inline=1\""));

    let (h, js) = text(&s, "/assets/stream.js").await;
    assert!(
        h["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/javascript")
    );
    assert!(js.contains("fetch("));
    let (h, _) = text(&s, "/assets/page.css").await;
    assert!(h["content-type"].to_str().unwrap().starts_with("text/css"));
    let r = reqwest::get(s.url("/assets/x.js")).await.unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn form_page_posts_to_form() {
    let s = server("").await;
    let (_, page) = text(&s, "/formpage?size=2M&token=t1&with=once%3D1").await;
    assert!(page.contains(
        "<form id=form method=post enctype=application/x-www-form-urlencoded \
         action=\"/form/2M/report.zip?seed=5&amp;once=1\">"
    ));
    assert!(page.contains(
        "<form id=multipart method=post enctype=multipart/form-data \
         action=\"/form/2M/multipart-report.zip?seed=6&amp;once=1\">"
    ));
    assert!(
        page.contains("action=\"/form/2M/see-other-report.zip?seed=7&amp;see_other=1&amp;once=1\"")
    );
    assert!(page.contains("name=token value=\"t1\""));
    assert!(page.contains("<button id=submit-multipart>"));
}

#[tokio::test]
async fn hls_and_dash() {
    let s = server("").await;
    let (h, master) = text(&s, "/hls/master.m3u8").await;
    assert_eq!(h["content-type"], "application/vnd.apple.mpegurl");
    assert!(master.starts_with("#EXTM3U\n"));
    assert!(master.contains("RESOLUTION=1280x720") && master.contains("BANDWIDTH=640000"));
    assert!(master.contains("#EXT-X-MEDIA:TYPE=AUDIO"));
    assert!(master.contains("URI=\"audio/index.m3u8\""));
    let variants: Vec<&str> = master.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(variants, ["720p/index.m3u8", "360p/index.m3u8"]);
    let mut seen = Vec::new();
    for (track, len) in [("720p", 48128), ("360p", 24064), ("audio", 6016)] {
        let (_, media) = text(&s, &format!("/hls/{track}/index.m3u8")).await;
        assert!(media.contains("#EXT-X-ENDLIST"));
        let segs: Vec<&str> = media.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(segs, ["seg0.ts", "seg1.ts", "seg2.ts"]);
        for seg in segs {
            let r = reqwest::get(s.url(&format!("/hls/{track}/{seg}")))
                .await
                .unwrap();
            assert_eq!(r.headers()["content-type"], "video/mp2t");
            let b = r.bytes().await.unwrap();
            assert_eq!(b.len(), len);
            assert!(b.iter().step_by(188).all(|x| *x == 0x47));
            assert!(!seen.contains(&b), "segments differ");
            seen.push(b);
        }
    }
    for missing in ["/hls/720p/seg3.ts", "/hls/1080p/index.m3u8"] {
        let r = reqwest::get(s.url(missing)).await.unwrap();
        assert_eq!(r.status(), 404, "{missing}");
    }

    let (h, mpd) = text(&s, "/dash/manifest.mpd").await;
    assert_eq!(h["content-type"], "application/dash+xml");
    assert!(mpd.contains("mediaPresentationDuration=\"PT12S\""));
    assert!(mpd.contains("<Representation id=\"720p\""));
    assert!(mpd.contains("<Representation id=\"audio\""));
    assert!(!mpd.contains("ContentProtection"));
    let (h, mpd) = text(&s, "/dash/protected.mpd").await;
    assert_eq!(h["content-type"], "application/dash+xml");
    assert_eq!(
        mpd.matches("urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed")
            .count(),
        2
    );
    assert!(mpd.contains("<Representation id=\"720p\""));
    let r = reqwest::get(s.url("/dash/720p/init.mp4")).await.unwrap();
    assert_eq!(r.headers()["content-type"], "video/mp4");
    assert_eq!(&r.bytes().await.unwrap()[4..8], b"ftyp");
    let r = reqwest::get(s.url("/dash/audio/seg3.m4s")).await.unwrap();
    assert_eq!(r.headers()["content-type"], "audio/mp4");
    assert_eq!(r.bytes().await.unwrap().len(), 6016);
    let r = reqwest::get(s.url("/dash/audio/seg0.m4s")).await.unwrap();
    assert_eq!(r.status(), 404);
}

/// #171: the same streams with no .m3u8 or .mpd in any address, as text/plain and octet-stream,
/// and the pages that play them (a decoy text answer first).
#[tokio::test]
async fn hls_and_dash_without_their_endings_or_types() {
    let s = server("").await;
    let (h, master) = text(&s, "/hls2/playlist?id=x%201").await;
    assert_eq!(h["content-type"], "text/plain; charset=utf-8");
    assert!(master.starts_with("#EXTM3U\n") && master.contains("RESOLUTION=1280x720"));
    assert!(master.contains("URI=\"playlist?id=x%201&track=audio\""));
    let variants: Vec<&str> = master.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(
        variants,
        [
            "playlist?id=x%201&track=720p",
            "playlist?id=x%201&track=360p"
        ]
    );
    assert!(!master.contains(".m3u8"));
    for (track, len) in [("720p", 48128), ("audio", 6016)] {
        let (h, media) = text(&s, &format!("/hls2/playlist?id=x%201&track={track}")).await;
        assert_eq!(h["content-type"], "text/plain; charset=utf-8");
        assert!(media.contains("#EXT-X-ENDLIST"));
        let segs: Vec<&str> = media.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0], format!("seg?id=x%201&track={track}&i=0"));
        let r = reqwest::get(s.url(&format!("/hls2/{}", segs[2])))
            .await
            .unwrap();
        assert_eq!(r.headers()["content-type"], "application/octet-stream");
        let b = r.bytes().await.unwrap();
        assert_eq!(b.len(), len);
        assert!(b.iter().step_by(188).all(|x| *x == 0x47));
    }
    for missing in [
        "/hls2/playlist?id=a&track=1080p",
        "/hls2/seg?track=720p&i=3",
        "/hls2/seg?track=720p",
    ] {
        let r = reqwest::get(s.url(missing)).await.unwrap();
        assert_eq!(r.status(), 404, "{missing}");
    }
    let (h, mpd) = text(&s, "/dash2/manifest?id=lab").await;
    assert_eq!(h["content-type"], "application/octet-stream");
    assert!(mpd.contains("<MPD ") && mpd.contains("<Representation id=\"360p\""));
    let r = reqwest::get(s.url("/dash2/720p/init.mp4")).await.unwrap();
    assert_eq!(&r.bytes().await.unwrap()[4..8], b"ftyp");
    let (h, config) = text(&s, "/hls2/config?id=lab").await;
    assert_eq!(h["content-type"], "text/plain; charset=utf-8");
    assert!(config.starts_with('{') && !config.contains("#EXTM3U"));

    let (_, page) = text(&s, "/page?hls2=1&dash2=1").await;
    assert!(page.contains(
        "data-manifest=\"/hls2/playlist?id=lab\" data-player=player data-kind=hls \
         data-decoy=\"/hls2/config?id=lab\">"
    ));
    assert!(
        page.contains(
            "data-manifest=\"/dash2/manifest?id=lab\" data-player=player2 data-kind=dash"
        )
    );
    let (_, js) = text(&s, "/assets/stream.js").await;
    assert!(js.contains("dataset.kind") && js.contains("dataset.decoy"));
}

/// The HLS video tracks and the audio track merge as yt-dlp merges a site's (its `[Merger]`:
/// `-map 0:v:0 -map 1:a:0`, copied into MP4), and the result decodes: a video track that held
/// only sound failed with "Error opening output files: Invalid argument". Ignored: it needs
/// `MDM_TEST_FFMPEG` (ffmpeg's program).
///
///   MDM_TEST_FFMPEG=…\ffmpeg.exe cargo test -p mdm-labserver --test http -- --ignored hls_tracks
#[tokio::test]
#[ignore]
async fn hls_tracks_merge_as_yt_dlp_merges_them() {
    use std::process::Command;
    let ffmpeg = std::env::var("MDM_TEST_FFMPEG").expect("set MDM_TEST_FFMPEG");
    let s = server("").await;
    let dir = tempfile::tempdir().unwrap();
    let joined = |track: &str| dir.path().join(format!("{track}.ts"));
    for track in ["720p", "360p", "audio"] {
        let mut all = Vec::new();
        for i in 0..3 {
            let r = reqwest::get(s.url(&format!("/hls/{track}/seg{i}.ts")))
                .await
                .unwrap();
            all.extend(r.bytes().await.unwrap());
        }
        std::fs::write(joined(track), all).unwrap();
    }
    for video in ["720p", "360p"] {
        let out = dir.path().join(format!("{video}.mp4"));
        let merged = Command::new(&ffmpeg)
            .args(["-v", "error", "-y", "-i"])
            .arg(joined(video))
            .arg("-i")
            .arg(joined("audio"))
            .args(["-c", "copy", "-map", "0:v:0", "-map", "1:a:0"])
            .args(["-bsf:a:0", "aac_adtstoasc", "-movflags", "+faststart"])
            .arg(&out)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&merged.stderr);
        assert!(merged.status.success(), "{video}: {said}");
        // Every picture and sound decodes, with nothing to say.
        let decoded = Command::new(&ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&out)
            .args(["-f", "null", "-"])
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&decoded.stderr);
        assert!(decoded.status.success() && said.trim().is_empty(), "{said}");
        // What it holds (`-i` alone lists it, then stops for want of an output).
        let listed = Command::new(&ffmpeg).arg("-i").arg(&out).output().unwrap();
        let said = String::from_utf8_lossy(&listed.stderr);
        let size = if video == "720p" {
            "1280x720"
        } else {
            "640x360"
        };
        assert!(
            said.contains("Video: h264 (Main)") && said.contains(size),
            "{said}"
        );
        assert!(said.contains("Audio: aac (LC)"), "{said}");
        assert!(said.contains("Duration: 00:00:12.00"), "{said}");
    }
}

#[tokio::test]
async fn help_lists_the_routes() {
    let s = server("").await;
    let help = reqwest::get(s.url("/"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for word in [
        "cdstar=",
        "inline=bare",
        "require_cookie=",
        "require_referer=",
        "once=1",
        "POST /form",
        "/formpage",
        "/cookie?set=",
        "csp=strict",
        "muted_loop",
        "iframe=1",
        "shadow=1",
        "several=N",
        "audio=",
        "hls=1",
        "/hls/master.m3u8",
        "/dash/manifest.mpd",
        "/dash/protected.mpd",
        "drm=1",
        "hls2=1",
        "/hls2/playlist?id=",
        "/dash2/manifest?id=",
        "/hls2/config?id=",
        "req_body_sha256",
        "referer",
    ] {
        assert!(help.contains(word), "{word}");
    }
}

/// `stall_at` (#123): a response stops at that byte and goes on once the server's faults are
/// changed; a request that starts past it isn't held; `settled` waits for every request's entry.
#[tokio::test]
async fn a_stall_holds_a_byte_back_until_lifted() {
    let s = server("stall_at=64K").await;
    let mut r = reqwest::get(s.url("/gen/256K/s.bin?seed=5")).await.unwrap();
    let mut got = Vec::new();
    while got.len() < 64 * 1024 {
        got.extend_from_slice(&r.chunk().await.unwrap().unwrap());
    }
    assert_eq!(got.len(), 64 * 1024, "nothing past the stall");
    // Held: no byte comes while the stall stands.
    let more = tokio::time::timeout(Duration::from_millis(300), r.chunk()).await;
    assert!(more.is_err(), "the stall let bytes through");
    // Past the stall point, a range is served at once.
    let tail = reqwest::Client::new()
        .get(s.url("/gen/256K/s.bin?seed=5"))
        .header("Range", "bytes=131072-")
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(tail, expected(5, 128 * 1024, 128 * 1024));
    s.set_faults(Faults::default());
    while let Some(c) = r.chunk().await.unwrap() {
        got.extend_from_slice(&c);
    }
    assert_eq!(got, expected(5, 0, 256 * 1024));
    tokio::time::timeout(Duration::from_secs(20), s.settled())
        .await
        .expect("every request is logged");
    assert_eq!(s.log().len(), 2);
    assert!(s.log().iter().all(|e| e.outcome == "complete"));
}
