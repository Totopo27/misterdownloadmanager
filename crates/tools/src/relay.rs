//! The helpers through the chosen network adapter (#35). yt-dlp, gallery-dl and the ffmpeg that
//! yt-dlp starts can't be held to an adapter themselves: yt-dlp's `--source-address` was seen not
//! to hold on Windows (#108: given an address without a route, it still connected from the
//! Ethernet adapter's), an address alone doesn't hold on Linux (it routes by destination, the weak
//! host model), and none of them can bind a device or an interface. So while an adapter is chosen
//! they are given this proxy (`--proxy`, and `HTTP_PROXY`… for what they start), and it opens
//! their connections through [`mdm_engine::net`], bound as MDM's own are, with the chosen DNS. The
//! same code holds them on Windows, Linux and macOS, IPv4 and IPv6: the binding is the engine's.
//!
//! - It listens on 127.0.0.1 only, on a port the system picks (no firewall asks about a socket
//!   that only this computer can reach), and wants a password of its own (Basic proxy
//!   authentication), so no other program uses it.
//! - It takes `CONNECT host:port` (HTTPS and anything else tunnelled) and plain `http://`
//!   requests (absolute form, sent on with `Connection: close`). Nothing else.
//! - It fails closed: while the adapter is down, or a host has no address of a family the adapter
//!   has, the helper is answered `502` with the reason and nothing is sent. When the route
//!   changes, every open connection is closed at once ([`Relay::route_changed`]); the app stops
//!   the downloads too, and starts them again through the new route.
//!
//! A video download goes through it with or without an adapter (#708): its own address
//! ([`Relay::lane`]) paces what comes down to MDM's speed limits as they are at each moment
//! ([`Pace`], the engine's own limiters), and goes on through the proxy yt-dlp would have used
//! ([`Upstream`]: Options -> Proxy / Socks, else the system's). yt-dlp's `--limit-rate` is fixed
//! when it starts, holds for each piece of a stream rather than for the download, and leaves out
//! the global cap shared with every other download; the relay sees all of a download's
//! connections, ffmpeg's too.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use mdm_engine::limiter::Limiter;
use mdm_engine::{DnsChoice, Route};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

/// A request's or a response's head may be at most this long.
const HEAD_MAX: usize = 64 * 1024;
/// A helper that connected has this long to send its request.
const HEAD_TIME: Duration = Duration::from_secs(30);
/// How long each of a server's addresses may take to connect.
const CONNECT_TIME: Duration = Duration::from_secs(10);
/// The user name in the relay's address (any is taken: the password is what counts).
const USER: &str = "mdm";

/// Where a relay's connections go, read for each one: the network adapter's route and the DNS.
pub type Net = Arc<dyn Fn() -> (Route, DnsChoice) + Send + Sync>;

/// One download's speed limits (#708), as a file download has them: its own (the Speed
/// Limiter's "for one file", or the limit set for it), and the global cap every download waits
/// on together. Both can change while it runs; what is waited for follows (`Limiter::set`).
#[derive(Debug)]
pub struct Pace {
    own: Limiter,
    global: Option<Arc<Limiter>>,
}

impl Pace {
    /// At most `rate` bytes/s (None: no limit of its own), and under `global` with the others.
    pub fn new(rate: Option<u64>, global: Option<Arc<Limiter>>) -> Arc<Pace> {
        Arc::new(Pace {
            own: Limiter::new(rate),
            global,
        })
    }

    /// Its own limit, now (None: unlimited).
    pub fn set(&self, rate: Option<u64>) {
        self.own.set(rate);
    }

    /// Its own limit.
    pub fn get(&self) -> Option<u64> {
        self.own.get()
    }

    /// The lower of its own limit and the global cap's, now: what a helper that can only be
    /// given a fixed limit is started with.
    pub fn now(&self) -> Option<u64> {
        let global = self.global.as_ref().and_then(|g| g.get());
        match (self.own.get(), global) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Wait until `n` more bytes fit under both limits (as a file download's connections do).
    pub async fn acquire(&self, n: u64) {
        self.own.acquire(n).await;
        if let Some(g) = &self.global {
            g.acquire(n).await;
        }
    }
}

/// Where a download's connections go after the relay: to the server (through the route), or on
/// through a proxy. The relay speaks HTTP and SOCKS5 to a proxy; for any other kind
/// ([`Upstream::parse`] says None) the helper is given that proxy itself, as before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Upstream {
    Direct,
    /// `CONNECT` for tunnels, absolute-form requests for plain ones. `auth`: the Basic
    /// credential (`user:password` in base64).
    Http {
        host: String,
        port: u16,
        auth: Option<String>,
    },
    /// `remote_names`: `socks5h://`, the proxy looks the names up; `socks5://` looks them up
    /// here.
    Socks5 {
        host: String,
        port: u16,
        login: Option<(String, String)>,
        remote_names: bool,
    },
}

impl Upstream {
    /// A proxy's address as helpers take it (`http://user:pass@host:port`, `socks5://…`; with no
    /// scheme, HTTP). None for one the relay can't go on through (`https://`, `socks4://`).
    pub fn parse(proxy: &str) -> Option<Upstream> {
        let proxy = proxy.trim();
        let with_scheme = if proxy.contains("://") {
            proxy.to_owned()
        } else {
            format!("http://{proxy}")
        };
        let u = url::Url::parse(&with_scheme).ok()?;
        let host = u.host_str()?.trim_matches(['[', ']']).to_owned();
        let decode = |s: &str| {
            percent_encoding::percent_decode_str(s)
                .decode_utf8_lossy()
                .into_owned()
        };
        let login = (!u.username().is_empty()).then(|| {
            (
                decode(u.username()),
                decode(u.password().unwrap_or_default()),
            )
        });
        match u.scheme() {
            "http" => {
                use base64::Engine;
                Some(Upstream::Http {
                    host,
                    port: u.port().unwrap_or(80),
                    auth: login.map(|(user, pass)| {
                        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"))
                    }),
                })
            }
            scheme @ ("socks5" | "socks5h") => Some(Upstream::Socks5 {
                host,
                port: u.port().unwrap_or(1080),
                login,
                remote_names: scheme == "socks5h",
            }),
            _ => None,
        }
    }
}

/// A download's address on the relay: its pace and where its connections go. Held weakly: once
/// the download is gone, its address opens nothing more.
struct Lane {
    pace: Weak<Pace>,
    upstream: Upstream,
}

type Lanes = Mutex<HashMap<String, Lane>>;

/// A running relay; it stops when dropped.
pub struct Relay {
    addr: SocketAddr,
    secret: String,
    changed: watch::Sender<u64>,
    lanes: Arc<Lanes>,
    _rt: tokio::runtime::Runtime,
}

struct Inner {
    secret: String,
    net: Net,
    changed: watch::Receiver<u64>,
    lanes: Arc<Lanes>,
}

impl Relay {
    /// A relay on 127.0.0.1 whose connections go through `net`, on a runtime of its own (the
    /// helpers are started from threads without one).
    pub fn start(net: Net) -> io::Result<Relay> {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let mut bytes = [0u8; 18];
        aws_lc_rs::rand::fill(&mut bytes).map_err(|_| io::Error::other("no random numbers"))?;
        let secret = hex::encode(bytes);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("mdm-relay")
            .enable_all()
            .build()?;
        let listener = {
            let _in = rt.enter();
            TcpListener::from_std(listener)?
        };
        let (changed, rx) = watch::channel(0);
        let lanes: Arc<Lanes> = Default::default();
        let inner = Arc::new(Inner {
            secret: secret.clone(),
            net,
            changed: rx,
            lanes: lanes.clone(),
        });
        rt.spawn(accept(listener, inner));
        Ok(Relay {
            addr,
            secret,
            changed,
            lanes,
            _rt: rt,
        })
    }

    /// The proxy address to give a helper, its password in it.
    pub fn url(&self) -> String {
        format!("http://{USER}:{}@{}", self.secret, self.addr)
    }

    /// A proxy address for one download (#708): what comes down through it is paced by `pace`,
    /// and its connections go on through `upstream`. Its user name tells it from the others;
    /// the password is the relay's. It opens nothing once `pace` is dropped.
    pub fn lane(&self, pace: &Arc<Pace>, upstream: Upstream) -> io::Result<String> {
        let mut bytes = [0u8; 12];
        aws_lc_rs::rand::fill(&mut bytes).map_err(|_| io::Error::other("no random numbers"))?;
        let user = format!("lane-{}", hex::encode(bytes));
        let mut lanes = self.lanes.lock().unwrap();
        lanes.retain(|_, l| l.pace.strong_count() > 0);
        lanes.insert(
            user.clone(),
            Lane {
                pace: Arc::downgrade(pace),
                upstream,
            },
        );
        Ok(format!("http://{user}:{}@{}", self.secret, self.addr))
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The route changed: every open connection closes now (it was opened through the old one).
    pub fn route_changed(&self) {
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }
}

async fn accept(listener: TcpListener, inner: Arc<Inner>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) if peer.ip().is_loopback() => {
                let inner = inner.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, inner).await;
                });
            }
            Ok(_) => {}
            // Out of sockets for a moment: try again shortly.
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

/// One helper's connection: its request, then the server's connection through the route, then
/// the bytes both ways until either side ends or the route changes.
async fn serve(mut client: TcpStream, inner: Arc<Inner>) -> io::Result<()> {
    let _ = client.set_nodelay(true);
    let mut changed = inner.changed.clone();
    changed.mark_unchanged();
    let mut buf = Vec::new();
    let head = match tokio::time::timeout(HEAD_TIME, read_head(&mut client, &mut buf)).await {
        Ok(Ok(Some(head))) => head,
        _ => return Ok(()),
    };
    let Some(req) = Request::parse(&head) else {
        return reply(&mut client, 400, "Bad Request").await;
    };
    if !req.authorized(&inner.secret) {
        return client
            .write_all(
                b"HTTP/1.1 407 Proxy Authentication Required\r\n\
                  Proxy-Authenticate: Basic realm=\"MDM\"\r\n\
                  Content-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await;
    }
    let Some(target) = req.target() else {
        return reply(&mut client, 400, "Bad Request").await;
    };
    // A download's own address: its pace, and where its connections go. One whose download has
    // ended opens nothing (it may have been held to a proxy).
    let (pace, upstream) = match req.user().filter(|u| u != USER) {
        None => (None, Upstream::Direct),
        Some(user) => {
            let lane = inner.lanes.lock().unwrap().get(&user).and_then(|l| {
                l.pace
                    .upgrade()
                    .map(|pace| (Some(pace), l.upstream.clone()))
            });
            match lane {
                Some(lane) => lane,
                None => return reply(&mut client, 502, "the download has ended").await,
            }
        }
    };
    let (route, dns) = (inner.net)();
    let opened = match &upstream {
        Upstream::Direct => open(&route, &dns, &target.host, target.port).await,
        Upstream::Http { host, port, auth } => {
            open_http(&route, &dns, (host, *port), auth.as_deref(), &target).await
        }
        Upstream::Socks5 {
            host,
            port,
            login,
            remote_names,
        } => {
            let proxy = (host.as_str(), *port);
            open_socks5(&route, &dns, proxy, login.as_ref(), *remote_names, &target).await
        }
    };
    let mut server = match opened {
        Ok(s) => s,
        Err(why) => return reply(&mut client, 502, &why).await,
    };
    let _ = server.set_nodelay(true);
    let forward = match &target.forward {
        None => {
            client
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await?;
            false
        }
        Some(head) => {
            match &upstream {
                // A proxy takes the address whole, with its own login.
                Upstream::Http { auth, .. } => {
                    server
                        .write_all(&proxied_head(head, &target.absolute, auth.as_deref()))
                        .await?;
                }
                _ => server.write_all(head).await?,
            }
            true
        }
    };
    // What the helper sent after its head (the start of a body, or of TLS).
    server.write_all(&buf).await?;
    let (mut cr, mut cw) = client.into_split();
    let (mut sr, mut sw) = server.into_split();
    let up = async move {
        let _ = tokio::io::copy(&mut cr, &mut sw).await;
        let _ = sw.shutdown().await;
        // The helper is done sending; the answer may still be on its way.
        std::future::pending::<()>().await
    };
    let down = async move {
        if forward {
            pass_response_head(&mut sr, &mut cw).await?;
        }
        match &pace {
            Some(pace) => paced_copy(&mut sr, &mut cw, pace).await?,
            None => tokio::io::copy(&mut sr, &mut cw).await?,
        };
        cw.shutdown().await
    };
    tokio::select! {
        _ = up => {}
        _ = down => {}
        // The route changed (or the relay stopped): this connection was held to the old one.
        _ = changed.changed() => {}
    }
    Ok(())
}

/// A connection to `host:port` through `route`, its name looked up with `dns` (as downloads'
/// are); else why not, for the helper's error.
async fn open(route: &Route, dns: &DnsChoice, host: &str, port: u16) -> Result<TcpStream, String> {
    let ips = mdm_engine::dns::lookup(dns, host, port, &[], route)
        .await
        .map_err(|e| e.to_string())?;
    let mut why = format!("{host} has no address");
    for ip in ips {
        let addr = SocketAddr::new(ip, port);
        match tokio::time::timeout(CONNECT_TIME, mdm_engine::net::tcp_connect(route, addr)).await {
            Ok(Ok(s)) => return Ok(s),
            Ok(Err(e)) => why = e.to_string(),
            Err(_) => why = format!("{addr} didn't answer"),
        }
    }
    Err(why)
}

/// `host:port` as a request line or a SOCKS request names it (`[v6]:port`).
fn authority(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// A connection to `target` through an HTTP proxy at `proxy`: a tunnel (`CONNECT`) for a tunnel,
/// else the connection to the proxy itself, which the request is sent to whole
/// ([`proxied_head`]).
async fn open_http(
    route: &Route,
    dns: &DnsChoice,
    proxy: (&str, u16),
    auth: Option<&str>,
    target: &Target,
) -> Result<TcpStream, String> {
    let mut s = open(route, dns, proxy.0, proxy.1).await?;
    if target.forward.is_some() {
        return Ok(s);
    }
    let at = authority(&target.host, target.port);
    let mut head = format!("CONNECT {at} HTTP/1.1\r\nHost: {at}\r\n");
    if let Some(a) = auth {
        head.push_str(&format!("Proxy-Authorization: Basic {a}\r\n"));
    }
    head.push_str("\r\n");
    let fail = |e: io::Error| format!("the proxy {}: {e}", proxy.0);
    s.write_all(head.as_bytes()).await.map_err(fail)?;
    let mut buf = Vec::new();
    let answer = tokio::time::timeout(CONNECT_TIME, read_head(&mut s, &mut buf))
        .await
        .map_err(|_| format!("the proxy {} didn't answer", proxy.0))?
        .map_err(fail)?
        .ok_or_else(|| format!("the proxy {} closed the connection", proxy.0))?;
    let text = String::from_utf8_lossy(&answer);
    let status = text.lines().next().unwrap_or_default().to_owned();
    let code = status.split(' ').nth(1).and_then(|c| c.parse::<u16>().ok());
    // Nothing may come after the proxy's answer before the helper speaks (TLS waits for it).
    if !code.is_some_and(|c| (200..300).contains(&c)) || !buf.is_empty() {
        return Err(format!("the proxy {} refused: {status}", proxy.0));
    }
    Ok(s)
}

/// A plain request's head for a proxy: the address whole on its first line, and the proxy's
/// own login.
fn proxied_head(head: &[u8], absolute: &str, auth: Option<&str>) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let (first, rest) = text.split_once("\r\n").unwrap_or((&text, ""));
    let mut words = first.splitn(3, ' ');
    let method = words.next().unwrap_or_default();
    let version = words.nth(1).unwrap_or("HTTP/1.1");
    let mut out = format!("{method} {absolute} {version}\r\n");
    if let Some(a) = auth {
        out.push_str(&format!("Proxy-Authorization: Basic {a}\r\n"));
    }
    out.push_str(rest);
    out.into_bytes()
}

/// A connection to `target` through a SOCKS5 proxy at `proxy` (RFC 1928, with RFC 1929's user
/// name and password when given). `remote_names`: the proxy looks the name up; else it is looked
/// up here, with `dns`.
async fn open_socks5(
    route: &Route,
    dns: &DnsChoice,
    proxy: (&str, u16),
    login: Option<&(String, String)>,
    remote_names: bool,
    target: &Target,
) -> Result<TcpStream, String> {
    let mut s = open(route, dns, proxy.0, proxy.1).await?;
    let host = if remote_names || target.host.parse::<IpAddr>().is_ok() {
        target.host.clone()
    } else {
        mdm_engine::dns::lookup(dns, &target.host, target.port, &[], route)
            .await
            .map_err(|e| e.to_string())?
            .first()
            .ok_or_else(|| format!("{} has no address", target.host))?
            .to_string()
    };
    tokio::time::timeout(
        CONNECT_TIME,
        socks5_handshake(&mut s, login, &host, target.port),
    )
    .await
    .map_err(|_| format!("the proxy {} didn't answer", proxy.0))??;
    Ok(s)
}

/// SOCKS5's greeting, login and CONNECT on `s`, to `host` (an address, or a name) and `port`.
async fn socks5_handshake<S: AsyncRead + AsyncWrite + Unpin>(
    s: &mut S,
    login: Option<&(String, String)>,
    host: &str,
    port: u16,
) -> Result<(), String> {
    let io = |e: io::Error| format!("the SOCKS proxy: {e}");
    let methods: &[u8] = if login.is_some() { &[0, 2] } else { &[0] };
    let mut hello = vec![5, methods.len() as u8];
    hello.extend_from_slice(methods);
    s.write_all(&hello).await.map_err(io)?;
    let mut chosen = [0u8; 2];
    s.read_exact(&mut chosen).await.map_err(io)?;
    match (chosen, login) {
        ([5, 0], _) => {}
        ([5, 2], Some((user, pass))) => {
            let (user, pass) = (user.as_bytes(), pass.as_bytes());
            if user.len() > 255 || pass.len() > 255 {
                return Err("the SOCKS proxy's login is too long".into());
            }
            let mut auth = vec![1, user.len() as u8];
            auth.extend_from_slice(user);
            auth.push(pass.len() as u8);
            auth.extend_from_slice(pass);
            s.write_all(&auth).await.map_err(io)?;
            let mut ok = [0u8; 2];
            s.read_exact(&mut ok).await.map_err(io)?;
            if ok[1] != 0 {
                return Err("the SOCKS proxy refused the login".into());
            }
        }
        _ => return Err("the SOCKS proxy wants a login it wasn't given".into()),
    }
    let mut ask = vec![5, 1, 0];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(a)) => {
            ask.push(1);
            ask.extend_from_slice(&a.octets());
        }
        Ok(IpAddr::V6(a)) => {
            ask.push(4);
            ask.extend_from_slice(&a.octets());
        }
        Err(_) => {
            let name = host.as_bytes();
            if name.len() > 255 {
                return Err("the name is too long for a SOCKS proxy".into());
            }
            ask.push(3);
            ask.push(name.len() as u8);
            ask.extend_from_slice(name);
        }
    }
    ask.extend_from_slice(&port.to_be_bytes());
    s.write_all(&ask).await.map_err(io)?;
    let mut answer = [0u8; 4];
    s.read_exact(&mut answer).await.map_err(io)?;
    if answer[1] != 0 {
        return Err(format!(
            "the SOCKS proxy couldn't connect to {} (code {})",
            authority(host, port),
            answer[1]
        ));
    }
    // The address it connected from, and its port: skipped.
    let skip = match answer[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut len = [0u8; 1];
            s.read_exact(&mut len).await.map_err(io)?;
            usize::from(len[0])
        }
        _ => return Err("the SOCKS proxy gave an answer it shouldn't".into()),
    };
    let mut rest = vec![0u8; skip + 2];
    s.read_exact(&mut rest).await.map_err(io)?;
    Ok(())
}

/// What comes down, to the helper, at the download's pace: each piece read waits under its
/// limits before it goes on, as a file download's connections wait (so TCP slows the server).
async fn paced_copy<R, W>(r: &mut R, w: &mut W, pace: &Pace) -> io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 16 * 1024];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            return Ok(total);
        }
        pace.acquire(n as u64).await;
        w.write_all(&buf[..n]).await?;
        total += n as u64;
    }
}

/// An answer of the relay's own, then the connection closes. `why` becomes the reason phrase,
/// which is what yt-dlp shows ("Tunnel connection failed: 502 the network adapter … is not
/// connected").
async fn reply(client: &mut TcpStream, code: u16, why: &str) -> io::Result<()> {
    let why: String = why
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(200)
        .collect();
    let text = format!(
        "HTTP/1.1 {code} {why}\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{why}",
        why.len()
    );
    client.write_all(text.as_bytes()).await?;
    client.shutdown().await
}

/// The next head (up to and with its blank line) from `r`; what came after it stays in `buf`.
/// None: the stream ended first.
async fn read_head<R: AsyncRead + Unpin>(
    r: &mut R,
    buf: &mut Vec<u8>,
) -> io::Result<Option<Vec<u8>>> {
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            return Ok(Some(std::mem::replace(buf, rest)));
        }
        if buf.len() > HEAD_MAX {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "head too long"));
        }
        let n = r.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// A plain request's answer: its head goes to the helper with `Connection: close` (so the helper
/// doesn't send its next request, maybe for another server, down this connection), interim
/// answers (`100 Continue`) as they are, then the rest as it comes.
async fn pass_response_head<R, W>(sr: &mut R, cw: &mut W) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = Vec::new();
    loop {
        let Some(head) = read_head(sr, &mut buf).await? else {
            break;
        };
        let code = std::str::from_utf8(&head)
            .ok()
            .and_then(|h| h.split(' ').nth(1))
            .and_then(|c| c.parse::<u16>().ok())
            .unwrap_or(0);
        if (100..200).contains(&code) && code != 101 {
            cw.write_all(&head).await?;
            continue;
        }
        if code == 101 {
            cw.write_all(&head).await?;
        } else {
            cw.write_all(&closing(&head)).await?;
        }
        break;
    }
    cw.write_all(&buf).await
}

/// `head` without its connection headers, ending with `Connection: close`.
fn closing(head: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.trim_end_matches("\r\n").split("\r\n");
    let mut out = String::new();
    out.push_str(lines.next().unwrap_or_default());
    out.push_str("\r\n");
    for line in lines {
        let name = line.split(':').next().unwrap_or_default().trim();
        if ["connection", "keep-alive", "proxy-connection"]
            .iter()
            .any(|h| name.eq_ignore_ascii_case(h))
        {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    out.into_bytes()
}

/// A helper's request head.
struct Request {
    method: String,
    target: String,
    version: String,
    headers: Vec<(String, String)>,
}

/// Where a request goes: a tunnel (`forward` None) or a plain request sent on with this head;
/// `absolute`: a plain request's address whole, for a proxy after the relay ([`proxied_head`]).
struct Target {
    host: String,
    port: u16,
    forward: Option<Vec<u8>>,
    absolute: String,
}

impl Request {
    fn parse(head: &[u8]) -> Option<Request> {
        let text = std::str::from_utf8(head).ok()?;
        let mut lines = text.trim_end_matches("\r\n").split("\r\n");
        let mut first = lines.next()?.split(' ');
        let (method, target, version) = (first.next()?, first.next()?, first.next()?);
        if first.next().is_some() || !version.starts_with("HTTP/1.") {
            return None;
        }
        let mut headers = Vec::new();
        for line in lines {
            let (name, value) = line.split_once(':')?;
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
        Some(Request {
            method: method.to_owned(),
            target: target.to_owned(),
            version: version.to_owned(),
            headers,
        })
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The Basic credential it carries, decoded (`user:password`).
    fn credential(&self) -> Option<Vec<u8>> {
        use base64::Engine;
        self.header("proxy-authorization")
            .and_then(|v| v.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("basic"))
            .and_then(|(_, b64)| base64::engine::general_purpose::STANDARD.decode(b64).ok())
    }

    /// The user name it gave (a download's lane, [`Relay::lane`]).
    fn user(&self) -> Option<String> {
        let given = self.credential()?;
        let at = given.iter().position(|b| *b == b':')?;
        String::from_utf8(given[..at].to_vec()).ok()
    }

    /// Whether it carries the relay's password (any user name).
    fn authorized(&self, secret: &str) -> bool {
        let Some(given) = self.credential() else {
            return false;
        };
        let Some(at) = given.iter().position(|b| *b == b':') else {
            return false;
        };
        let pass = &given[at + 1..];
        // The same time for every wrong password of the right length.
        pass.len() == secret.len()
            && pass
                .iter()
                .zip(secret.as_bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    }

    /// Where it goes: `CONNECT host:port`, or an absolute `http://` address. Nothing else.
    fn target(&self) -> Option<Target> {
        if self.method.eq_ignore_ascii_case("CONNECT") {
            let (host, port) = split_host_port(&self.target)?;
            return Some(Target {
                host,
                port,
                forward: None,
                absolute: String::new(),
            });
        }
        let url = url::Url::parse(&self.target).ok()?;
        if url.scheme() != "http" {
            return None;
        }
        let host = url.host_str()?.trim_matches(['[', ']']).to_owned();
        let port = url.port_or_known_default()?;
        let mut path = url.path().to_owned();
        if let Some(q) = url.query() {
            path.push('?');
            path.push_str(q);
        }
        let mut head = format!("{} {path} {}\r\n", self.method, self.version);
        let mut has_host = false;
        for (name, value) in &self.headers {
            if [
                "proxy-authorization",
                "proxy-connection",
                "connection",
                "keep-alive",
            ]
            .iter()
            .any(|h| name.eq_ignore_ascii_case(h))
            {
                continue;
            }
            has_host |= name.eq_ignore_ascii_case("host");
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if !has_host {
            let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
            head.push_str(&format!("Host: {authority}\r\n"));
        }
        head.push_str("Connection: close\r\n\r\n");
        Some(Target {
            host,
            port,
            forward: Some(head.into_bytes()),
            absolute: url.to_string(),
        })
    }
}

/// `host:port` or `[v6]:port`; the port is required.
fn split_host_port(s: &str) -> Option<(String, u16)> {
    let (host, port) = s.rsplit_once(':')?;
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    let host = host.trim_matches(['[', ']']);
    if host.is_empty() || host.contains(['/', ' ', '@']) {
        return None;
    }
    Some((host.to_owned(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_read_and_rewritten() {
        let r = Request::parse(
            b"GET http://files.test:8080/a/b?c=1 HTTP/1.1\r\nHost: files.test:8080\r\n\
              Proxy-Authorization: Basic bWRtOnNlY3JldA==\r\nProxy-Connection: keep-alive\r\n\
              Connection: keep-alive\r\nRange: bytes=0-\r\n\r\n",
        )
        .unwrap();
        assert!(r.authorized("secret"));
        assert!(!r.authorized("secreT"));
        assert!(!r.authorized("secre"));
        let t = r.target().unwrap();
        assert_eq!((t.host.as_str(), t.port), ("files.test", 8080));
        let head = String::from_utf8(t.forward.unwrap()).unwrap();
        assert_eq!(
            head,
            "GET /a/b?c=1 HTTP/1.1\r\nHost: files.test:8080\r\nRange: bytes=0-\r\n\
             Connection: close\r\n\r\n",
            "the password never goes on"
        );
        let c = Request::parse(b"CONNECT [2001:db8::1]:443 HTTP/1.1\r\n\r\n").unwrap();
        let t = c.target().unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.forward),
            ("2001:db8::1", 443, None)
        );
        assert!(!c.authorized("secret"), "no password, no relay");
        // Only CONNECT and absolute http:// addresses.
        for bad in [
            &b"GET /index.html HTTP/1.1\r\n\r\n"[..],
            b"GET https://x.test/ HTTP/1.1\r\n\r\n",
            b"GET ftp://x.test/f HTTP/1.1\r\n\r\n",
            b"CONNECT x.test HTTP/1.1\r\n\r\n",
            b"CONNECT x.test:0 HTTP/1.1\r\n\r\n",
        ] {
            assert!(
                Request::parse(bad).and_then(|r| r.target()).is_none(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        assert!(Request::parse(b"GET / HTTP/2\r\n\r\n").is_none());
    }

    #[test]
    fn answers_say_connection_close() {
        let h = closing(b"HTTP/1.1 200 OK\r\nConnection: keep-alive\r\nKeep-Alive: timeout=5\r\nContent-Length: 3\r\n\r\n");
        assert_eq!(
            String::from_utf8(h).unwrap(),
            "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n"
        );
    }

    /// #708: the proxies a paced download goes on through, as helpers are given them.
    #[test]
    fn proxies_the_relay_goes_on_through() {
        use base64::Engine;
        let http = |host: &str, port, auth: Option<&str>| Upstream::Http {
            host: host.into(),
            port,
            auth: auth.map(|a| base64::engine::general_purpose::STANDARD.encode(a)),
        };
        assert_eq!(
            Upstream::parse("http://proxy.test:3128"),
            Some(http("proxy.test", 3128, None))
        );
        // The system's, as it comes (a slash after it), or without a scheme.
        assert_eq!(
            Upstream::parse("http://10.0.0.1:8080/"),
            Some(http("10.0.0.1", 8080, None))
        );
        assert_eq!(
            Upstream::parse("10.0.0.1:8080"),
            Some(http("10.0.0.1", 8080, None))
        );
        assert_eq!(
            Upstream::parse("http://[::1]:3128"),
            Some(http("::1", 3128, None))
        );
        // A login as Options gives it (escaped in the address).
        assert_eq!(
            Upstream::parse("http://us%40er:p%3Ass@proxy.test:8080"),
            Some(http("proxy.test", 8080, Some("us@er:p:ss")))
        );
        assert_eq!(
            Upstream::parse("socks5h://me:pw@socks.test"),
            Some(Upstream::Socks5 {
                host: "socks.test".into(),
                port: 1080,
                login: Some(("me".into(), "pw".into())),
                remote_names: true,
            })
        );
        assert_eq!(
            Upstream::parse("socks5://socks.test:9050"),
            Some(Upstream::Socks5 {
                host: "socks.test".into(),
                port: 9050,
                login: None,
                remote_names: false,
            })
        );
        // Kinds it can't speak: the helper is given them itself.
        assert_eq!(Upstream::parse("https://proxy.test:443"), None);
        assert_eq!(Upstream::parse("socks4://socks.test:1080"), None);
        assert_eq!(Upstream::parse("not a proxy"), None);
    }

    /// #708: a download's address names its lane; the password is still the relay's.
    #[test]
    fn a_lane_is_told_by_its_user_name() {
        let r = Request::parse(
            b"CONNECT files.test:443 HTTP/1.1\r\n\
              Proxy-Authorization: Basic bGFuZS0wMTpzZWNyZXQ=\r\n\r\n",
        )
        .unwrap();
        assert_eq!(r.user().as_deref(), Some("lane-01"));
        assert!(r.authorized("secret"));
        assert_eq!(
            Request::parse(b"CONNECT files.test:443 HTTP/1.1\r\n\r\n")
                .unwrap()
                .user(),
            None
        );
    }

    /// #708: a plain request goes to a proxy after the relay whole, with that proxy's login and
    /// never the relay's.
    #[test]
    fn a_plain_request_goes_to_a_proxy_whole() {
        let r = Request::parse(
            b"GET http://files.test:8080/a?b=1 HTTP/1.1\r\nHost: files.test:8080\r\n\
              Proxy-Authorization: Basic bWRtOnNlY3JldA==\r\n\r\n",
        )
        .unwrap();
        let t = r.target().unwrap();
        let head = proxied_head(t.forward.as_ref().unwrap(), &t.absolute, Some("dXA6cHc="));
        assert_eq!(
            String::from_utf8(head).unwrap(),
            "GET http://files.test:8080/a?b=1 HTTP/1.1\r\nProxy-Authorization: Basic dXA6cHc=\r\n\
             Host: files.test:8080\r\nConnection: close\r\n\r\n"
        );
    }

    /// #708: SOCKS5 with a login, a name the proxy looks up, and its answer read to the end
    /// (what comes after it is the server's).
    #[tokio::test]
    async fn socks5_with_a_login() {
        let (mut ours, mut proxy) = tokio::io::duplex(1024);
        let server = tokio::spawn(async move {
            let mut hello = [0u8; 4];
            proxy.read_exact(&mut hello).await.unwrap();
            assert_eq!(hello, [5, 2, 0, 2], "no login, or a user name and password");
            proxy.write_all(&[5, 2]).await.unwrap();
            let mut login = [0u8; 9];
            proxy.read_exact(&mut login).await.unwrap();
            assert_eq!(&login, b"\x01\x04user\x02pw");
            proxy.write_all(&[1, 0]).await.unwrap();
            let mut ask = [0u8; 17];
            proxy.read_exact(&mut ask).await.unwrap();
            assert_eq!(&ask[..5], &[5, 1, 0, 3, 10]);
            assert_eq!(&ask[5..15], b"files.test");
            assert_eq!(&ask[15..], &443u16.to_be_bytes());
            // Connected, from a name (its length first), then the server's first bytes.
            proxy
                .write_all(b"\x05\x00\x00\x03\x05proxy\x01\xbbHELLO")
                .await
                .unwrap();
        });
        let login = ("user".to_owned(), "pw".to_owned());
        socks5_handshake(&mut ours, Some(&login), "files.test", 443)
            .await
            .unwrap();
        let mut first = [0u8; 5];
        ours.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"HELLO");
        server.await.unwrap();

        // Refused: said, nothing sent on.
        let (mut ours, mut proxy) = tokio::io::duplex(1024);
        tokio::spawn(async move {
            let mut hello = [0u8; 3];
            proxy.read_exact(&mut hello).await.unwrap();
            proxy.write_all(&[5, 0]).await.unwrap();
            let mut ask = [0u8; 10];
            proxy.read_exact(&mut ask).await.unwrap();
            assert_eq!(&ask[3..8], &[1, 10, 0, 0, 7], "an address as itself");
            proxy
                .write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
        });
        let e = socks5_handshake(&mut ours, None, "10.0.0.7", 80)
            .await
            .unwrap_err();
        assert!(e.contains("10.0.0.7:80") && e.contains("code 5"), "{e}");
    }

    /// #708: a download's pace is the lower of its own limit and the global cap's, each as it is
    /// now.
    #[tokio::test]
    async fn a_pace_is_both_limits_now() {
        let global = Arc::new(Limiter::new(None));
        let p = Pace::new(None, Some(global.clone()));
        assert_eq!(p.now(), None);
        global.set(Some(500 << 10));
        assert_eq!(p.now(), Some(500 << 10));
        p.set(Some(200 << 10));
        assert_eq!((p.get(), p.now()), (Some(200 << 10), Some(200 << 10)));
        global.set(Some(100 << 10));
        assert_eq!(p.now(), Some(100 << 10));
        global.set(None);
        assert_eq!(p.now(), Some(200 << 10));
        // What it lets through counts in the global cap (its split with torrents).
        p.set(None);
        p.acquire(4096).await;
        assert_eq!(global.taken(), 4096);
    }
}
