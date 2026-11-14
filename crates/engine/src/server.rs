//! Where a download's server is, as the download sees it (#140): the address that answered, how
//! long its own connections took to open (no extra probe: every connection the client opens is
//! timed), whether a proxy stood in between, and the CDN its answer names. The app places the
//! address on the map (`mdm-geo`, on this computer).
//!
//! The round trip: a TCP connection opens in one round trip; over https its TLS handshake takes
//! one more (TLS 1.3). The time a connection took to open, its name lookup left out (the
//! resolver reports it, `dns_spent`), divided by those round trips, is the round trip's
//! estimate; the quickest of the download's connections is kept.
//!
//! All of it is told once the first answer came (#933): the answer says which address sent it
//! and whether over https. Before then a connection's time alone would be half a record, with no
//! address, and twice the real round trip over https; a server slow to answer kept it so for
//! seconds, and a download stopped meanwhile kept it for good.
//!
//! The answer is told whichever connection it came through (#954): the first request's, MDM's
//! own or the browser-like one (#668), and when no first request came before the connections,
//! as with a resumed download, the first connection's.

use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crate::download::Head;

/// What the download learnt of its server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Server {
    /// The address that answered: the server's, or the proxy's when one carried the request.
    pub ip: Option<IpAddr>,
    /// The round trip to it, estimated from its connections' opening (see the module).
    pub rtt: Option<Duration>,
    /// A proxy carried the requests: `ip` is the proxy's, and the round trip is to it.
    pub proxied: bool,
    /// The CDN the answer names (its headers): the address is one of its servers near the
    /// downloader, not the site owner's.
    pub cdn: Option<&'static str>,
}

/// The download's [`Server`], filled as its connections open and its answers come.
#[derive(Debug, Default)]
pub struct ServerSeen {
    inner: Mutex<Seen>,
}

#[derive(Debug, Default)]
struct Seen {
    server: Server,
    /// The quickest connection's opening, and whether the answer came over https.
    quickest: Option<Duration>,
    tls: bool,
    /// An answer came: `server` and `tls` are its.
    answered: bool,
}

impl ServerSeen {
    pub fn get(&self) -> Option<Server> {
        let s = self.inner.lock().unwrap();
        if !s.answered {
            return None;
        }
        let mut server = s.server.clone();
        server.rtt = s.quickest.map(|d| d / if s.tls { 2 } else { 1 });
        (server.ip.is_some() || server.rtt.is_some()).then_some(server)
    }

    pub(crate) fn set_proxied(&self, proxied: bool) {
        self.inner.lock().unwrap().server.proxied = proxied;
    }

    fn connected_in(&self, d: Duration) {
        let mut s = self.inner.lock().unwrap();
        if s.quickest.is_none_or(|q| d < q) {
            s.quickest = Some(d);
        }
    }

    /// An answer, whichever connection it came through: who sent it, over what, and the CDN its
    /// headers name.
    pub(crate) fn answered(&self, head: &Head) {
        self.inner.lock().unwrap().answer(head);
    }

    /// A connection's answer (`download::request`): told when none was yet, as with a resumed
    /// download, whose connections ask first (#954). Once one is, it stands.
    pub(crate) fn connection_answered(&self, head: &Head) {
        let mut s = self.inner.lock().unwrap();
        if !s.answered {
            s.answer(head);
        }
    }
}

impl Seen {
    fn answer(&mut self, head: &Head) {
        if let Some(a) = head.from {
            self.server.ip = Some(a.ip());
        }
        self.tls = head.url.scheme() == "https";
        self.answered = true;
        if let Some(cdn) = cdn_of(&head.headers) {
            self.server.cdn = Some(cdn);
        }
    }
}

/// The CDN an answer's headers name, by the headers each one sets on what it serves.
pub(crate) fn cdn_of(h: &reqwest::header::HeaderMap) -> Option<&'static str> {
    let has = |name: &str| h.contains_key(name);
    let says = |name: &str, what: &str| {
        h.get(name)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.to_ascii_lowercase().contains(what))
    };
    if has("cf-ray") || says("server", "cloudflare") {
        Some("Cloudflare")
    } else if has("x-amz-cf-id") || says("via", "cloudfront") {
        Some("Amazon CloudFront")
    } else if says("x-served-by", "cache-") || has("x-fastly-request-id") {
        Some("Fastly")
    } else if says("server", "akamai") || has("x-akamai-transformed") || has("akamai-grn") {
        Some("Akamai")
    } else if has("x-azure-ref") || has("x-msedge-ref") {
        Some("Azure Front Door")
    } else if has("x-goog-generation") || says("server", "gws") || says("via", "google") {
        Some("Google")
    } else if has("x-bunny-cache") || says("server", "bunnycdn") || has("cdn-requestid") {
        Some("Bunny CDN")
    } else {
        None
    }
}

tokio::task_local! {
    /// The time the name lookup took within a connection's opening (the resolver adds it).
    static DNS_SPENT: std::cell::Cell<Duration>;
}

/// The resolver's lookup took `d`: left out of the connection's time.
pub(crate) fn dns_spent(d: Duration) {
    let _ = DNS_SPENT.try_with(|c| c.set(c.get() + d));
}

/// A layer around the client's connector: each connection's opening timed into `seen`.
#[derive(Clone)]
pub(crate) struct Timed(pub Arc<ServerSeen>);

impl<S> tower_layer::Layer<S> for Timed {
    type Service = TimedService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TimedService {
            inner,
            seen: self.0.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct TimedService<S> {
    inner: S,
    seen: Arc<ServerSeen>,
}

impl<S, R> tower_service::Service<R> for TimedService<S>
where
    S: tower_service::Service<R>,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: R) -> Self::Future {
        let seen = self.seen.clone();
        let fut = self.inner.call(req);
        Box::pin(async move {
            let started = Instant::now();
            let dns = std::cell::Cell::new(Duration::ZERO);
            let (res, dns) = DNS_SPENT
                .scope(dns, async move {
                    let res = fut.await;
                    (res, DNS_SPENT.with(|c| c.get()))
                })
                .await;
            if res.is_ok() {
                seen.connected_in(started.elapsed().saturating_sub(dns));
            }
            res
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdns_are_named_by_their_headers() {
        let h = |pairs: &[(&'static str, &'static str)]| {
            let mut m = reqwest::header::HeaderMap::new();
            for (k, v) in pairs {
                m.insert(*k, v.parse().unwrap());
            }
            cdn_of(&m)
        };
        assert_eq!(h(&[("cf-ray", "8a1-FRA")]), Some("Cloudflare"));
        assert_eq!(h(&[("server", "cloudflare")]), Some("Cloudflare"));
        assert_eq!(h(&[("x-amz-cf-id", "abc")]), Some("Amazon CloudFront"));
        assert_eq!(h(&[("x-served-by", "cache-fra-123-FRA")]), Some("Fastly"));
        assert_eq!(h(&[("server", "AkamaiGHost")]), Some("Akamai"));
        assert_eq!(h(&[("server", "nginx")]), None);
        assert_eq!(h(&[]), None);
    }

    #[test]
    fn the_round_trip_is_the_quickest_connection_over_its_round_trips() {
        let seen = ServerSeen::default();
        assert_eq!(seen.get(), None);
        seen.connected_in(Duration::from_millis(80));
        // #933: a connection open, no answer yet: nothing told (not a round trip without its
        // address, perhaps twice too long).
        assert_eq!(seen.get(), None);
        seen.inner.lock().unwrap().answered = true;
        seen.connected_in(Duration::from_millis(60));
        seen.connected_in(Duration::from_millis(90));
        assert_eq!(seen.get().unwrap().rtt, Some(Duration::from_millis(60)));
        seen.inner.lock().unwrap().tls = true;
        assert_eq!(seen.get().unwrap().rtt, Some(Duration::from_millis(30)));
    }

    /// #954: a connection's answer is told when none was yet (a resumed download's), and leaves
    /// one already told as it is.
    #[test]
    fn a_connections_answer_is_told_when_none_was() {
        let head = |ip: [u8; 4], url: &str| Head {
            status: reqwest::StatusCode::PARTIAL_CONTENT,
            headers: Default::default(),
            url: url.parse().unwrap(),
            len: None,
            from: Some((ip, 443).into()),
        };
        let seen = ServerSeen::default();
        seen.connected_in(Duration::from_millis(80));
        seen.connection_answered(&head([192, 0, 2, 1], "https://a.test/f"));
        let told = seen.get().unwrap();
        assert_eq!(told.ip, Some(IpAddr::from([192, 0, 2, 1])));
        assert_eq!(told.rtt, Some(Duration::from_millis(40)));
        seen.connection_answered(&head([192, 0, 2, 2], "http://a.test/f"));
        assert_eq!(seen.get(), Some(told));
    }
}
