//! The helpers' relay (#35) against the lab server: what goes through it leaves from the adapter,
//! nothing goes while the adapter is down, and a route change closes what is open. 127.0.0.2
//! stands in for the adapter's address (bound even toward this computer, `loopback_too`), so the
//! lab server's log shows where each connection came from. Nothing leaves this computer.
//!
//! A download's own address on it (#708) keeps to MDM's speed limits as they change, and goes on
//! through the proxy it would have had.
//!
//! `real_helpers_go_through_the_relay` runs the real yt-dlp and ffmpeg (ignored; see it).

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mdm_engine::net::Binding;
use mdm_engine::{DnsChoice, Route};
use mdm_labserver::{Config, LabServer};
use mdm_tools::relay::Relay;

const TWO: IpAddr = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2));

fn bound() -> Route {
    Route::Bound(Arc::new(Binding {
        label: "Lab adapter".into(),
        name: "lo".into(),
        index: 1,
        v4: Some(Ipv4Addr::new(127, 0, 0, 2)),
        v6: None,
        loopback_too: true,
    }))
}

fn relay(route: &Arc<Mutex<Route>>) -> Relay {
    let r = route.clone();
    Relay::start(Arc::new(move || {
        (r.lock().unwrap().clone(), DnsChoice::default())
    }))
    .unwrap()
}

/// `user:password` of a relay's address, as a Basic credential.
fn auth(relay: &Relay) -> String {
    use base64::Engine;
    let url = url::Url::parse(&relay.url()).unwrap();
    let pair = format!("{}:{}", url.username(), url.password().unwrap());
    format!(
        "Proxy-Authorization: Basic {}\r\n",
        base64::engine::general_purpose::STANDARD.encode(pair)
    )
}

/// Send `head` to the relay and read until it closes.
fn ask(relay: &Relay, head: &str) -> Vec<u8> {
    let mut s = TcpStream::connect(relay.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(head.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    out
}

fn lab() -> (tokio::runtime::Runtime, LabServer) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let lab = rt.block_on(LabServer::start(Config::default())).unwrap();
    (rt, lab)
}

fn lab_addr(lab: &LabServer) -> String {
    let u = url::Url::parse(&lab.url("/")).unwrap();
    format!("{}:{}", u.host_str().unwrap(), u.port().unwrap())
}

fn lab_port(lab: &LabServer) -> u16 {
    url::Url::parse(&lab.url("/")).unwrap().port().unwrap()
}

fn body(answer: &[u8]) -> &[u8] {
    let at = answer.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    &answer[at + 4..]
}

#[test]
fn plain_and_tunnelled_requests_leave_from_the_adapter() {
    if cfg!(target_os = "macos") {
        // 127.0.0.2 only answers there once aliased.
        return;
    }
    let (_rt, lab) = lab();
    let at = lab_addr(&lab);
    let route = Arc::new(Mutex::new(bound()));
    let relay = relay(&route);

    // A plain http:// request, sent on with Connection: close.
    let plain = ask(
        &relay,
        &format!(
            "GET http://{at}/gen/64K/a.bin?seed=1 HTTP/1.1\r\nHost: {at}\r\n{}\
             Proxy-Connection: keep-alive\r\n\r\n",
            auth(&relay)
        ),
    );
    let text = String::from_utf8_lossy(&plain[..plain.len().min(400)]).to_string();
    assert!(text.starts_with("HTTP/1.1 200"), "{text}");
    assert!(text.contains("Connection: close") || text.contains("connection: close"));
    let mut want = vec![0; 64 * 1024];
    mdm_labserver::content::fill(1, 0, &mut want);
    assert_eq!(body(&plain), &want[..], "plain bytes");

    // A tunnel (what HTTPS takes), then a request inside it.
    let mut s = TcpStream::connect(relay.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(
        format!(
            "CONNECT {at} HTTP/1.1\r\nHost: {at}\r\n{}\r\n",
            auth(&relay)
        )
        .as_bytes(),
    )
    .unwrap();
    let mut first = [0u8; 39];
    s.read_exact(&mut first).unwrap();
    assert_eq!(&first[..], b"HTTP/1.1 200 Connection established\r\n\r\n");
    s.write_all(
        format!("GET /gen/32K/b.bin?seed=2 HTTP/1.1\r\nHost: {at}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
    .unwrap();
    let mut tunnelled = Vec::new();
    let _ = s.read_to_end(&mut tunnelled);
    let mut want = vec![0; 32 * 1024];
    mdm_labserver::content::fill(2, 0, &mut want);
    assert_eq!(body(&tunnelled), &want[..], "tunnelled bytes");

    let log = lab.log();
    assert_eq!(log.len(), 2, "{log:?}");
    // Windows binds the address; Linux binds the device (`lo`), whose source is 127.0.0.1.
    if cfg!(windows) {
        let from: Vec<IpAddr> = log.iter().map(|e| e.peer.ip()).collect();
        assert!(from.iter().all(|ip| *ip == TWO), "{from:?}");
        for e in &log {
            println!(
                "#35: {} {} reached the lab from {}",
                e.method, e.path, e.peer
            );
        }
    }
    // The relay's password never reaches the server.
    assert!(log.iter().all(|e| e.authorization.is_none()));
}

#[test]
fn nothing_goes_without_the_password_or_the_adapter() {
    let (_rt, lab) = lab();
    let at = lab_addr(&lab);
    let route = Arc::new(Mutex::new(bound()));
    let relay = relay(&route);

    let no_pass = ask(&relay, &format!("CONNECT {at} HTTP/1.1\r\n\r\n"));
    assert!(
        no_pass.starts_with(b"HTTP/1.1 407 "),
        "{}",
        String::from_utf8_lossy(&no_pass)
    );
    let wrong = ask(
        &relay,
        &format!("CONNECT {at} HTTP/1.1\r\nProxy-Authorization: Basic bWRtOm5vcGU=\r\n\r\n"),
    );
    assert!(wrong.starts_with(b"HTTP/1.1 407 "));
    // Not a proxy request (a web page asking 127.0.0.1): refused.
    let page = ask(
        &relay,
        &format!("GET / HTTP/1.1\r\nHost: {at}\r\n{}\r\n", auth(&relay)),
    );
    assert!(page.starts_with(b"HTTP/1.1 400 "));

    // The adapter disconnected (a VPN switched off): refused, saying why.
    *route.lock().unwrap() = Route::Down {
        label: "Lab adapter".into(),
    };
    for head in [
        format!("CONNECT {at} HTTP/1.1\r\n{}\r\n", auth(&relay)),
        format!(
            "GET http://{at}/gen/1K/x.bin HTTP/1.1\r\n{}\r\n",
            auth(&relay)
        ),
    ] {
        let down = String::from_utf8_lossy(&ask(&relay, &head)).to_string();
        assert!(
            down.starts_with("HTTP/1.1 502 the network adapter Lab adapter is not connected"),
            "{down}"
        );
    }
    // An address of a family the adapter has none of (IPv6, the adapter IPv4 only).
    *route.lock().unwrap() = bound();
    let v6 = String::from_utf8_lossy(&ask(
        &relay,
        &format!(
            "CONNECT [::1]:{} HTTP/1.1\r\n{}\r\n",
            lab_port(&lab),
            auth(&relay)
        ),
    ))
    .to_string();
    assert!(v6.starts_with("HTTP/1.1 502 "), "{v6}");
    assert!(lab.log().is_empty(), "nothing reached the server");
}

#[test]
fn a_route_change_closes_what_is_open() {
    if cfg!(target_os = "macos") {
        return;
    }
    // A server that accepts and never answers: the tunnel stays open until the relay closes it.
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let at = server.local_addr().unwrap();
    let held = std::thread::spawn(move || {
        let (mut s, peer) = server.accept().unwrap();
        let mut buf = [0u8; 16];
        // Its end: the relay closed the connection.
        let n = s.read(&mut buf).unwrap_or(0);
        (peer, n)
    });
    let route = Arc::new(Mutex::new(bound()));
    let relay = relay(&route);
    let mut s = TcpStream::connect(relay.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(format!("CONNECT {at} HTTP/1.1\r\n{}\r\n", auth(&relay)).as_bytes())
        .unwrap();
    let mut first = [0u8; 39];
    s.read_exact(&mut first).unwrap();
    assert!(first.starts_with(b"HTTP/1.1 200"));
    std::thread::sleep(Duration::from_millis(200));
    // The adapter goes away.
    *route.lock().unwrap() = Route::Down {
        label: "Lab adapter".into(),
    };
    let t = Instant::now();
    relay.route_changed();
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest);
    let (peer, n) = held.join().unwrap();
    assert!(rest.is_empty() && n == 0);
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
    if cfg!(windows) {
        assert_eq!(peer.ip(), TWO);
    }
    println!(
        "#35: the tunnel closed {:?} after the route changed",
        t.elapsed()
    );
}

/// `user:password` of a proxy address (a lane's, `Relay::lane`), as a Basic credential.
fn auth_of(proxy: &str) -> String {
    use base64::Engine;
    let url = url::Url::parse(proxy).unwrap();
    let pair = format!("{}:{}", url.username(), url.password().unwrap());
    format!(
        "Proxy-Authorization: Basic {}\r\n",
        base64::engine::general_purpose::STANDARD.encode(pair)
    )
}

/// A plain request for `path` on the lab through the proxy address `lane` (its relay `relay`):
/// the open answer, its head read.
fn open_through(relay: &Relay, lane: &str, lab: &LabServer, path: &str) -> TcpStream {
    let mut s = TcpStream::connect(relay.addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let at = lab_addr(lab);
    s.write_all(
        format!(
            "GET http://{at}{path} HTTP/1.1\r\nHost: {at}\r\n{}\r\n",
            auth_of(lane)
        )
        .as_bytes(),
    )
    .unwrap();
    // The head, a byte at a time (what follows is the body).
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(s.read(&mut b).unwrap(), 1, "the answer ended in its head");
        head.push(b[0]);
    }
    assert!(
        head.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&head)
    );
    s
}

/// Read `n` bytes of a body; the seconds it took.
fn read_for(s: &mut TcpStream, n: usize) -> f64 {
    let t = Instant::now();
    let mut buf = vec![0u8; 64 << 10];
    let mut left = n;
    while left > 0 {
        let want = left.min(buf.len());
        let got = s.read(&mut buf[..want]).unwrap();
        assert!(got > 0, "the body ended early");
        left -= got;
    }
    t.elapsed().as_secs_f64()
}

/// #708: what comes down a download's address keeps to its limits as they are at each moment:
/// its own, the global cap shared with the other downloads, and none once it is lifted.
#[test]
fn a_download_keeps_to_its_limits_as_they_change() {
    use mdm_engine::limiter::Limiter;
    use mdm_tools::relay::{Pace, Upstream};
    let (_rt, lab) = lab();
    let relay = relay(&Arc::new(Mutex::new(Route::Any)));

    // Its own limit, 400 KB/s: 800 KB take about 2 s.
    let pace = Pace::new(Some(400 << 10), None);
    let lane = relay.lane(&pace, Upstream::Direct).unwrap();
    let mut s = open_through(&relay, &lane, &lab, "/gen/8M/own.bin");
    let secs = read_for(&mut s, 800 << 10);
    let rate = 800.0 / secs;
    println!("own 400 KB/s: {rate:.0} KB/s");
    assert!((340.0..=460.0).contains(&rate), "{rate:.0} KB/s");
    // Lifted while it runs: the rest comes at once.
    pace.set(None);
    let secs = read_for(&mut s, 4 << 20);
    println!("lifted: {:.0} KB/s", 4096.0 / secs);
    assert!(secs < 2.0, "4 MB took {secs:.2} s without a limit");
    drop(s);

    // The global cap, 500 KB/s, shared by two downloads at once: 1 MB together in about 2 s.
    let global = Arc::new(Limiter::new(Some(500 << 10)));
    let (a, b) = (
        Pace::new(None, Some(global.clone())),
        Pace::new(None, Some(global.clone())),
    );
    let (la, lb) = (
        relay.lane(&a, Upstream::Direct).unwrap(),
        relay.lane(&b, Upstream::Direct).unwrap(),
    );
    let mut sa = open_through(&relay, &la, &lab, "/gen/4M/a.bin");
    let mut sb = open_through(&relay, &lb, &lab, "/gen/4M/b.bin");
    let t = Instant::now();
    std::thread::scope(|scope| {
        let ta = scope.spawn(|| read_for(&mut sa, 512 << 10));
        let tb = scope.spawn(|| read_for(&mut sb, 512 << 10));
        let (ta, tb) = (ta.join().unwrap(), tb.join().unwrap());
        println!("global 500 KB/s, two at once: {ta:.2} s and {tb:.2} s for 512 KB each");
    });
    let rate = 1024.0 / t.elapsed().as_secs_f64();
    println!("global 500 KB/s, together: {rate:.0} KB/s");
    assert!((420.0..=580.0).contains(&rate), "{rate:.0} KB/s");
    assert!(global.taken() >= 1 << 20, "counted in the cap");
    // The cap lowered while they run (the torrents took their share): 200 KB/s together.
    global.set(Some(200 << 10));
    let t = Instant::now();
    std::thread::scope(|scope| {
        scope.spawn(|| read_for(&mut sa, 200 << 10));
        scope.spawn(|| read_for(&mut sb, 200 << 10));
    });
    let rate = 400.0 / t.elapsed().as_secs_f64();
    println!("global lowered to 200 KB/s: {rate:.0} KB/s");
    assert!((160.0..=240.0).contains(&rate), "{rate:.0} KB/s");
}

/// #708: a download's address goes on through the proxy it would have had (here an HTTP proxy
/// with a login, another relay standing in for it): tunnels and plain requests both, and its
/// pace holds there too.
#[test]
fn a_download_goes_on_through_its_proxy() {
    use mdm_tools::relay::{Pace, Upstream};
    let (_rt, lab) = lab();
    let any = Arc::new(Mutex::new(Route::Any));
    let proxy = relay(&any);
    let ours = relay(&any);
    let upstream = Upstream::parse(&proxy.url()).unwrap();
    assert!(matches!(&upstream, Upstream::Http { auth: Some(_), .. }));
    let pace = Pace::new(None, None);
    let lane = ours.lane(&pace, upstream).unwrap();

    // A plain request.
    let mut s = open_through(&ours, &lane, &lab, "/gen/64K/plain.bin");
    assert!(read_for(&mut s, 64 << 10) < 5.0);
    // A tunnel.
    let at = lab_addr(&lab);
    let mut t = TcpStream::connect(ours.addr()).unwrap();
    t.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    t.write_all(format!("CONNECT {at} HTTP/1.1\r\n{}\r\n", auth_of(&lane)).as_bytes())
        .unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(t.read(&mut b).unwrap(), 1);
        head.push(b[0]);
    }
    assert!(
        head.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&head)
    );
    t.write_all(
        format!("GET /gen/1K/tunnel.bin HTTP/1.1\r\nHost: {at}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
    .unwrap();
    let mut out = Vec::new();
    let _ = t.read_to_end(&mut out);
    assert!(
        out.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&out)
    );
    assert_eq!(body(&out).len(), 1024);
    let log = lab.log();
    assert!(log.iter().any(|e| e.path == "/gen/64K/plain.bin"));
    assert!(log.iter().any(|e| e.path == "/gen/1K/tunnel.bin"));

    // A proxy that refuses: the helper is told, nothing reaches the lab.
    let mut wrong = url::Url::parse(&proxy.url()).unwrap();
    wrong.set_password(Some("wrong")).unwrap();
    let wrong = Upstream::parse(wrong.as_str()).unwrap();
    let lane = ours.lane(&pace, wrong).unwrap();
    let before = lab.log().len();
    let answer = ask(
        &ours,
        &format!(
            "GET http://{at}/gen/1K/no.bin HTTP/1.1\r\nHost: {at}\r\n{}\r\n",
            auth_of(&lane)
        ),
    );
    let answer = String::from_utf8_lossy(&answer);
    let tunnel = ask(
        &ours,
        &format!("CONNECT {at} HTTP/1.1\r\n{}\r\n", auth_of(&lane)),
    );
    let tunnel = String::from_utf8_lossy(&tunnel);
    assert!(
        answer.starts_with("HTTP/1.1 407"),
        "the proxy's own answer: {answer}"
    );
    assert!(tunnel.starts_with("HTTP/1.1 502"), "{tunnel}");
    assert!(tunnel.contains("refused"), "{tunnel}");
    assert_eq!(lab.log().len(), before);
}

/// #708: once its download is gone, a download's address opens nothing (it may have been held to
/// a proxy: never around it).
#[test]
fn an_ended_download_opens_nothing() {
    use mdm_tools::relay::{Pace, Upstream};
    let (_rt, lab) = lab();
    let relay = relay(&Arc::new(Mutex::new(Route::Any)));
    let pace = Pace::new(None, None);
    let lane = relay.lane(&pace, Upstream::Direct).unwrap();
    drop(pace);
    let at = lab_addr(&lab);
    let answer = ask(
        &relay,
        &format!("CONNECT {at} HTTP/1.1\r\n{}\r\n", auth_of(&lane)),
    );
    let answer = String::from_utf8_lossy(&answer);
    assert!(answer.starts_with("HTTP/1.1 502"), "{answer}");
    assert!(lab.log().is_empty());
}

/// The real yt-dlp and ffmpeg, through `mdm_tools::proc` as the app starts them, against the lab
/// server. Ignored: it needs `MDM_TEST_YTDLP` (yt-dlp's program) and `MDM_TEST_FFMPEG` (ffmpeg's).
///
///   MDM_TEST_YTDLP=…\yt-dlp.exe MDM_TEST_FFMPEG=…\ffmpeg.exe cargo test -p mdm-tools --test relay -- --ignored --nocapture
#[test]
#[ignore]
fn real_helpers_go_through_the_relay() {
    use std::process::Stdio;
    let (Ok(ytdlp), Ok(ffmpeg)) = (
        std::env::var("MDM_TEST_YTDLP"),
        std::env::var("MDM_TEST_FFMPEG"),
    ) else {
        panic!("set MDM_TEST_YTDLP and MDM_TEST_FFMPEG");
    };
    let ffmpeg_dir = std::path::Path::new(&ffmpeg).parent().unwrap().to_owned();
    let (_rt, lab) = lab();
    let dir = tempfile::tempdir().unwrap();
    let ours = |what: &str, extra: &[&str]| -> Vec<String> {
        let mut ours: Vec<String> = vec![
            "--no-playlist".into(),
            "--ffmpeg-location".into(),
            ffmpeg_dir.to_string_lossy().into_owned(),
            // The lab's files are bytes, not media: nothing to fix up afterwards.
            "--fixup".into(),
            "never".into(),
            "-o".into(),
            dir.path()
                .join(format!("{what}.%(ext)s"))
                .to_string_lossy()
                .into_owned(),
        ];
        ours.extend(extra.iter().map(|s| s.to_string()));
        ours
    };
    let finish = |what: &str, cmd: Result<std::process::Command, mdm_tools::proc::ProcError>| {
        let mut cmd = match cmd {
            Ok(c) => c,
            Err(e) => return (false, format!("refused before starting: {e}")),
        };
        cmd.stdout(Stdio::null()).stderr(Stdio::piped());
        let out = cmd.output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        if std::env::var_os("MDM_TEST_VERBOSE").is_some() {
            println!(
                "===== {what}
{err}"
            );
        }
        for line in err.lines().filter(|l| {
            l.contains("ffmpeg command line") || l.contains("Protocol") || l.contains("filter")
        }) {
            println!("{what}: {line}");
        }
        let tail = err.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
        (out.status.success(), tail)
    };
    let run = |what: &str, url: &str, extra: &[&str]| {
        let cmd =
            mdm_tools::proc::ytdlp(std::path::Path::new(&ytdlp), &[], &ours(what, extra), url);
        finish(what, cmd)
    };
    // A looked-up page whose one format is `url` over `protocol` (what the app hands yt-dlp).
    let run_info = |what: &str, url: &str, protocol: &str| {
        let info = dir.path().join(format!("{what}.info.json"));
        let json = serde_json::json!({
            "id": what, "title": what, "extractor": "generic", "extractor_key": "Generic",
            "webpage_url": lab.url("/page"), "url": url, "ext": "mp4", "protocol": protocol,
        });
        std::fs::write(&info, json.to_string()).unwrap();
        let cmd =
            mdm_tools::proc::ytdlp_info(std::path::Path::new(&ytdlp), &[], &ours(what, &[]), &info);
        finish(what, cmd)
    };
    let print_log = |label: &str| {
        let log = lab.log();
        println!("--- {label}: {} requests", log.len());
        for e in &log {
            println!(
                "  {} {} from {} ({})",
                e.method,
                e.path,
                e.peer,
                e.user_agent.as_deref().unwrap_or("-")
            );
        }
        log
    };

    // 1. Through the adapter: a progressive file (yt-dlp's own requests), and an HLS stream
    //    fetched by ffmpeg (`--downloader m3u8:ffmpeg`).
    mdm_tools::proc::set_route(bound());
    let (ok, why) = run("progressive", &lab.url("/gen/2M/clip.mp4"), &[]);
    println!("progressive: ok={ok} {why}");
    let (ok2, why2) = run(
        "hls",
        &lab.url("/hls/master.m3u8"),
        &["-v", "--downloader", "m3u8:ffmpeg", "-f", "bv*[height=360]"],
    );
    println!("hls by ffmpeg: ok={ok2} {why2}");
    let log = print_log("bound to 127.0.0.2");
    assert!(ok, "{why}");
    assert!(
        log.iter().any(|e| e.path.starts_with("/hls/360p/")),
        "ffmpeg fetched segments"
    );
    // The lab's HLS segments are real MPEG-TS (#236): ffmpeg remuxes them to the end, any
    // version (6.1, Ubuntu 24.04's, as 8.0).
    assert!(ok2, "{why2}");
    assert!(dir.path().join("hls.mp4").is_file(), "the stream saved");
    if cfg!(windows) {
        assert!(
            log.iter().all(|e| e.peer.ip() == TWO),
            "every request from the adapter"
        );
    }

    // 2. Formats over what can't go through the relay are refused, never handed to a program
    //    that ignores the proxy (rtmpdump, yt-dlp's own FTP, mpv): nothing may connect to the
    //    server that waits there.
    let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    other.set_nonblocking(true).unwrap();
    let at = other.local_addr().unwrap();
    for protocol in ["rtmp", "ftp", "rtsp", "mms"] {
        let (ok, why) = run_info(protocol, &format!("{protocol}://{at}/live/x"), protocol);
        println!("{protocol}: ok={ok} {why}");
        assert!(!ok);
    }
    assert!(other.accept().is_err(), "nothing connected to that server");

    // 3. gallery-dl (when `MDM_TEST_GALLERYDL` names it): a picture by its direct address.
    if let Ok(gdl) = std::env::var("MDM_TEST_GALLERYDL") {
        lab.clear_log();
        let ours = vec!["-D".into(), dir.path().to_string_lossy().into_owned()];
        let tool = mdm_tools::chosen::ask(
            mdm_tools::chosen::Helper::GalleryDl,
            std::path::Path::new(&gdl),
        )
        .expect("gallery-dl answers its version");
        let cmd = mdm_tools::proc::gallery_dl(&tool, &ours, &lab.url("/gen/64K/picture.jpg"));
        let (ok, why) = finish("gallery-dl", cmd);
        println!("gallery-dl: ok={ok} {why}");
        let log = print_log("gallery-dl bound to 127.0.0.2");
        assert!(ok && !log.is_empty(), "{why}");
        let saved = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().contains("picture"))
            .expect("the picture saved");
        assert_eq!(saved.metadata().unwrap().len(), 64 * 1024);
        if cfg!(windows) {
            assert!(log.iter().all(|e| e.peer.ip() == TWO));
        }
    }

    // 4. The adapter down: refused before yt-dlp starts.
    lab.clear_log();
    mdm_tools::proc::set_route(Route::Down {
        label: "Lab adapter".into(),
    });
    let (ok4, why4) = run("down", &lab.url("/gen/2M/clip2.mp4"), &[]);
    println!("down: ok={ok4} {why4}");
    assert!(!ok4 && why4.contains("not connected"));
    assert!(print_log("down").is_empty());
}
