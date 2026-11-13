//! Name resolution for downloads: a dead address must not stall every new connection.
//!
//! hyper tries a host's addresses of one family one after another, each with an equal share of
//! the connect timeout, so a host whose first address is dead costs 15 s (of 30) per new
//! connection. When a host has several addresses, this resolver races TCP connects to them,
//! staggered as in RFC 8305, and puts the first that answers in front; the winner is remembered
//! for a while so later connections don't race again. A single address is passed through as is.
//!
//! The names themselves are looked up by the system's DNS (the default), or by the DNS chosen in
//! Options → Connection → DNS (#36): a plain DNS server, DNS over TLS or DNS over HTTPS, asked
//! through hickory-resolver. The system's hosts file and local names (`localhost`, single-label
//! names, `.local`, `.lan`, `.home.arpa`, `.internal`) stay with the system either way: a public
//! DNS can't know them. When the chosen DNS can't be reached, the lookup fails unless the user
//! allowed falling back to the system's DNS; a name it says doesn't exist never falls back (that
//! would undo a filtering DNS).
//!
//! With a network adapter chosen (#35, `crate::net`), the chosen DNS is asked through that
//! adapter too, the address race connects through it, and addresses of an IP family it can't
//! reach are dropped. The system's DNS asks in its own way (Windows may ask over every adapter):
//! choosing a DNS here is what keeps the names inside the adapter as well.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use hickory_resolver::config::{
    ConnectionConfig, LookupIpStrategy, NameServerConfig, ResolveHosts, ResolverConfig,
    ServerOrderingStrategy,
};
use hickory_resolver::net::runtime::iocompat::AsyncIoTokioAsStd;
use hickory_resolver::net::runtime::{
    RuntimeProvider, TokioHandle, TokioRuntimeProvider, TokioTime,
};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tokio::task::JoinSet;

use crate::net::Route;

/// hickory's resolver, its connections opened through the chosen adapter.
type DnsResolver = hickory_resolver::Resolver<Through>;

/// hickory's tokio runtime with its TCP connections (plain DNS over TCP, TLS, HTTPS) opened by
/// `net::tcp_connect`, so they go through the chosen adapter. It never opens UDP sockets (see
/// `build`).
#[derive(Clone, Default)]
struct Through {
    tokio: TokioRuntimeProvider,
    route: Route,
}

impl RuntimeProvider for Through {
    type Handle = TokioHandle;
    type Timer = TokioTime;
    type Udp = tokio::net::UdpSocket;
    type Tcp = AsyncIoTokioAsStd<tokio::net::TcpStream>;

    fn create_handle(&self) -> Self::Handle {
        self.tokio.create_handle()
    }

    fn connect_tcp(
        &self,
        server_addr: SocketAddr,
        _bind_addr: Option<SocketAddr>,
        timeout: Option<Duration>,
    ) -> Pin<Box<dyn Send + Future<Output = Result<Self::Tcp, io::Error>>>> {
        let route = self.route.clone();
        Box::pin(async move {
            let wait = timeout.unwrap_or(crate::net::CONNECT);
            let stream = tokio::time::timeout(wait, crate::net::tcp_connect(&route, server_addr))
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "TCP connect timed out"))??;
            stream.set_nodelay(true)?;
            Ok(AsyncIoTokioAsStd(stream))
        })
    }

    fn bind_udp(
        &self,
        _local_addr: SocketAddr,
        _server_addr: SocketAddr,
    ) -> Pin<Box<dyn Send + Future<Output = Result<Self::Udp, io::Error>>>> {
        // Never asked for: every server is configured for TCP, TLS or HTTPS. Refused all the same:
        // a UDP socket would be a listening socket to Windows' firewall.
        Box::pin(async { Err(io::Error::other("MDM asks DNS over TCP only")) })
    }
}

/// Which DNS answers MDM's own lookups (Options → Connection → DNS).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Dns {
    /// The system's resolver (getaddrinfo: its cache, the hosts file, Windows' own encrypted DNS
    /// if it is set up there).
    #[default]
    System,
    /// A plain DNS server, asked over TCP (never UDP: see `build`); port 53 unless given.
    Plain(Vec<SocketAddr>),
    /// DNS over TLS (RFC 7858); `name` is what the servers' certificate must show. Port 853 unless
    /// given.
    Tls {
        name: String,
        servers: Vec<SocketAddr>,
    },
    /// DNS over HTTPS (RFC 8484, HTTP/2): `url` is `https://host[:port]/path`; `bootstrap` are
    /// its host's addresses (empty: the system's DNS finds them, once per lookup set).
    Https { url: String, bootstrap: Vec<IpAddr> },
}

/// The DNS chosen, how strict it is, and its name for messages.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct DnsChoice {
    pub dns: Dns,
    /// When the chosen DNS can't be reached (no answer, refused, a TLS failure), ask the
    /// system's. Never when it answered that a name doesn't exist.
    pub fallback: bool,
    /// Shown in errors: "Cloudflare (DNS over HTTPS)".
    pub label: String,
}

impl DnsChoice {
    pub fn is_system(&self) -> bool {
        self.dns == Dns::System
    }
}

/// Why a lookup through the chosen DNS failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DnsError {
    /// The DNS answered: no such name (or no address for it).
    #[error("{dns} says {host} doesn't exist")]
    NoSuchName { dns: String, host: String },
    /// The DNS couldn't be asked or didn't answer.
    #[error("couldn't look up {host} with {dns}: {why}")]
    Unreachable {
        dns: String,
        host: String,
        why: String,
    },
    /// The choice itself is wrong (an address that isn't one).
    #[error("{dns}: {why}")]
    Setup { dns: String, why: String },
    /// The chosen network adapter is down, or can't reach any address found (#35).
    #[error(transparent)]
    Adapter(#[from] crate::net::NetError),
}

/// How a preset is asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Plain,
    Tls,
    Https,
}

/// The public DNS services offered by name: id, name, plain servers, the TLS certificate name,
/// the DoH address, and the addresses DoT and DoH are asked at (so finding them needs no DNS).
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "cloudflare",
        name: "Cloudflare",
        plain: &[
            "1.1.1.1",
            "1.0.0.1",
            "2606:4700:4700::1111",
            "2606:4700:4700::1001",
        ],
        tls_name: "one.one.one.one",
        doh: "https://cloudflare-dns.com/dns-query",
    },
    Preset {
        id: "google",
        name: "Google",
        plain: &[
            "8.8.8.8",
            "8.8.4.4",
            "2001:4860:4860::8888",
            "2001:4860:4860::8844",
        ],
        tls_name: "dns.google",
        doh: "https://dns.google/dns-query",
    },
    Preset {
        id: "quad9",
        name: "Quad9",
        plain: &["9.9.9.9", "149.112.112.112", "2620:fe::fe", "2620:fe::9"],
        tls_name: "dns.quad9.net",
        doh: "https://dns.quad9.net/dns-query",
    },
    Preset {
        id: "adguard",
        name: "AdGuard",
        plain: &[
            "94.140.14.14",
            "94.140.15.15",
            "2a10:50c0::ad1:ff",
            "2a10:50c0::ad2:ff",
        ],
        tls_name: "dns.adguard-dns.com",
        doh: "https://dns.adguard-dns.com/dns-query",
    },
    Preset {
        id: "nextdns",
        name: "NextDNS",
        plain: &["45.90.28.0", "45.90.30.0", "2a07:a8c0::", "2a07:a8c1::"],
        tls_name: "dns.nextdns.io",
        doh: "https://dns.nextdns.io",
    },
];

/// A public DNS service (see [`PRESETS`]).
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub plain: &'static [&'static str],
    pub tls_name: &'static str,
    pub doh: &'static str,
}

impl Preset {
    pub fn by_id(id: &str) -> Option<&'static Preset> {
        PRESETS.iter().find(|p| p.id == id)
    }

    /// NextDNS's plain servers answer only an IP linked to an account, so it isn't offered plain.
    pub fn offers(&self, mode: Mode) -> bool {
        !(self.id == "nextdns" && mode == Mode::Plain)
    }

    /// The choice for `mode`. `profile`: a NextDNS configuration ID (six hex digits), which
    /// goes into its DoT name and DoH address; empty for its default.
    pub fn choice(&self, mode: Mode, profile: &str, fallback: bool, label: String) -> DnsChoice {
        let ips: Vec<IpAddr> = self.plain.iter().filter_map(|a| a.parse().ok()).collect();
        let profile = profile.trim();
        let dns = match mode {
            Mode::Plain => Dns::Plain(ips.iter().map(|ip| SocketAddr::new(*ip, 53)).collect()),
            Mode::Tls => Dns::Tls {
                name: if self.id == "nextdns" && !profile.is_empty() {
                    format!("{profile}.{}", self.tls_name)
                } else {
                    self.tls_name.to_owned()
                },
                servers: ips.iter().map(|ip| SocketAddr::new(*ip, 853)).collect(),
            },
            Mode::Https => Dns::Https {
                url: if self.id == "nextdns" && !profile.is_empty() {
                    format!("{}/{profile}", self.doh)
                } else {
                    self.doh.to_owned()
                },
                bootstrap: ips,
            },
        };
        DnsChoice {
            dns,
            fallback,
            label,
        }
    }
}

/// A NextDNS configuration ID: six lowercase hex digits (or none).
pub fn valid_nextdns_profile(id: &str) -> bool {
    let id = id.trim();
    id.is_empty() || (id.len() == 6 && id.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Server addresses as typed: IPv4 or IPv6, with an optional port (`9.9.9.9:5353`,
/// `[2620:fe::fe]:853`), separated by spaces, commas or semicolons. `default_port` for those
/// without. Err names the first one that isn't an address.
pub fn parse_servers(text: &str, default_port: u16) -> Result<Vec<SocketAddr>, String> {
    text.split([' ', ',', ';', '\n', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<SocketAddr>()
                .or_else(|_| {
                    s.trim_matches(['[', ']'])
                        .parse::<IpAddr>()
                        .map(|ip| SocketAddr::new(ip, default_port))
                })
                .map_err(|_| s.to_owned())
        })
        .collect()
}

/// Names a public DNS can't know: they stay with the system's resolver.
fn local_name(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    !host.contains('.')
        || ["localhost", "local", "lan", "home.arpa", "internal"]
            .iter()
            .any(|tld| host == *tld || host.ends_with(&format!(".{tld}")))
}

/// How long one server gets to answer, and how many tries.
const DNS_TIMEOUT: Duration = Duration::from_secs(4);

/// `servers` with the IPv4 ones first (each family in the order given). They are asked in that
/// order, so a machine without IPv6 doesn't spend a timeout on an IPv6 server before the IPv4
/// ones; no route probe is needed for that (a probe socket could raise a firewall prompt).
fn ipv4_first<T: Copy>(servers: &[T], ip: impl Fn(&T) -> IpAddr) -> Vec<T> {
    let (v4, v6): (Vec<T>, Vec<T>) = servers.iter().partition(|s| ip(s).is_ipv4());
    v4.into_iter().chain(v6).collect()
}

/// The chosen DNS as a hickory resolver (its own cache; dropped with its owner).
async fn build(
    choice: &DnsChoice,
    extra_roots: &[Vec<u8>],
    route: &Route,
) -> Result<DnsResolver, DnsError> {
    let setup = |why: String| DnsError::Setup {
        dns: choice.label.clone(),
        why,
    };
    let with_port = |mut c: ConnectionConfig, port: u16| {
        c.port = port;
        c
    };
    // Only the servers the chosen adapter can reach (its IP family).
    let keep = |servers: &[SocketAddr]| {
        let reachable: Vec<SocketAddr> = servers
            .iter()
            .copied()
            .filter(|s| route.check(s.ip()).is_ok())
            .collect();
        ipv4_first(&reachable, SocketAddr::ip)
    };
    let servers: Vec<NameServerConfig> = match &choice.dns {
        Dns::System => return Err(setup("the system's DNS isn't asked through hickory".into())),
        // Over TCP only: a UDP socket waiting for answers counts as listening to Windows'
        // firewall, which would ask the user to allow MDM. An outgoing TCP connection never
        // does, and every public resolver answers on TCP port 53 (RFC 7766).
        Dns::Plain(servers) => keep(servers)
            .iter()
            .map(|a| {
                NameServerConfig::new(
                    a.ip(),
                    true,
                    vec![with_port(ConnectionConfig::tcp(), a.port())],
                )
            })
            .collect(),
        Dns::Tls { name, servers } => keep(servers)
            .iter()
            .map(|a| {
                NameServerConfig::new(
                    a.ip(),
                    true,
                    vec![with_port(
                        ConnectionConfig::tls(Arc::from(name.as_str())),
                        a.port(),
                    )],
                )
            })
            .collect(),
        Dns::Https { url, bootstrap } => {
            let u = url::Url::parse(url).map_err(|e| setup(format!("{url}: {e}")))?;
            if u.scheme() != "https" {
                return Err(setup(format!("{url}: not an https:// address")));
            }
            let host = u
                .host_str()
                .ok_or_else(|| setup(format!("{url}: no server")))?
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_owned();
            let port = u.port().unwrap_or(443);
            let ips: Vec<IpAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
                vec![ip]
            } else if !bootstrap.is_empty() {
                bootstrap.clone()
            } else {
                // No fixed addresses: the system finds the DNS server itself, once.
                tokio::net::lookup_host((host.as_str(), port))
                    .await
                    .map_err(|e| DnsError::Unreachable {
                        dns: choice.label.clone(),
                        host: host.clone(),
                        why: e.to_string(),
                    })?
                    .map(|a| a.ip())
                    .collect()
            };
            let path: Arc<str> = Arc::from(if u.path().is_empty() { "/" } else { u.path() });
            ipv4_first(&route.keep(ips), |ip| *ip)
                .into_iter()
                .map(|ip| {
                    NameServerConfig::new(
                        ip,
                        true,
                        vec![with_port(
                            ConnectionConfig::https(Arc::from(host.as_str()), Some(path.clone())),
                            port,
                        )],
                    )
                })
                .collect()
        }
    };
    if servers.is_empty() {
        if let Route::Down { label } = route {
            return Err(crate::net::NetError::Down {
                label: label.clone(),
            }
            .into());
        }
        if !route.is_any() {
            return Err(setup(format!(
                "none of its servers can be reached through {}: it has no address of their kind (IPv4 or IPv6)",
                route.label().unwrap_or_default()
            )));
        }
        return Err(setup("no server given".into()));
    }
    let tls = crate::ftp::client_config(extra_roots).map_err(|e| setup(e.to_string()))?;
    let mut builder = DnsResolver::builder_with_config(
        ResolverConfig::from_name_servers(servers),
        Through {
            tokio: TokioRuntimeProvider::default(),
            route: route.clone(),
        },
    )
    .with_tls_config(tls);
    let opts = builder.options_mut();
    opts.timeout = DNS_TIMEOUT;
    opts.attempts = 1;
    opts.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
    // IPv4 servers first, as given (see `ipv4_first`), not reordered by past answers.
    opts.server_ordering_strategy = ServerOrderingStrategy::UserProvidedOrder;
    // The hosts file stays the user's word, whichever DNS answers the rest.
    opts.use_hosts_file = ResolveHosts::Always;
    builder.build().map_err(|e| setup(e.to_string()))
}

/// `host`'s addresses from `resolver`, falling back to the system's as `choice` allows.
async fn ask(
    resolver: &DnsResolver,
    choice: &DnsChoice,
    host: &str,
    port: u16,
) -> Result<Vec<IpAddr>, DnsError> {
    match resolver.lookup_ip(host).await {
        Ok(found) => {
            let ips: Vec<IpAddr> = found.iter().collect();
            if ips.is_empty() {
                Err(DnsError::NoSuchName {
                    dns: choice.label.clone(),
                    host: host.to_owned(),
                })
            } else {
                Ok(ips)
            }
        }
        Err(e) if e.is_nx_domain() || e.is_no_records_found() => Err(DnsError::NoSuchName {
            dns: choice.label.clone(),
            host: host.to_owned(),
        }),
        Err(_) if choice.fallback => {
            system(host, port)
                .await
                .map_err(|why| DnsError::Unreachable {
                    dns: choice.label.clone(),
                    host: host.to_owned(),
                    why,
                })
        }
        Err(e) => Err(DnsError::Unreachable {
            dns: choice.label.clone(),
            host: host.to_owned(),
            why: e.to_string(),
        }),
    }
}

async fn system(host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
    Ok(tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| e.to_string())?
        .map(|a| a.ip())
        .collect())
}

/// The resolver for a reqwest client outside the engine (release lists, the update check), so it
/// follows `choice` and `route` as downloads do (see `net::client_builder`).
pub fn client_resolver(choice: &DnsChoice, route: &Route) -> Arc<dyn Resolve> {
    Arc::new(Resolver::new(&[], 443, choice, &[], route))
}

/// `host`'s addresses as `choice` finds them, those `route` can reach (FTP, and anything outside
/// a download client).
pub async fn lookup(
    choice: &DnsChoice,
    host: &str,
    port: u16,
    extra_roots: &[Vec<u8>],
    route: &Route,
) -> Result<Vec<IpAddr>, DnsError> {
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        route.check(ip)?;
        return Ok(vec![ip]);
    }
    if let Route::Down { label } = route {
        return Err(crate::net::NetError::Down {
            label: label.clone(),
        }
        .into());
    }
    let found = if choice.is_system() || local_name(host) {
        system(host, port)
            .await
            .map_err(|why| DnsError::Unreachable {
                dns: "the system's DNS".into(),
                host: host.to_owned(),
                why,
            })?
    } else {
        let resolver = build(choice, extra_roots, route).await?;
        ask(&resolver, choice, host, port).await?
    };
    reachable(found, route)
}

/// `ips` without those `route` can't reach; an error when none is left.
fn reachable(ips: Vec<IpAddr>, route: &Route) -> Result<Vec<IpAddr>, DnsError> {
    let first = ips.first().copied();
    let kept = route.keep(ips);
    match first {
        Some(ip) if kept.is_empty() => Err(route.check(ip).unwrap_err().into()),
        _ => Ok(kept),
    }
}

/// Options → DNS → Test: how long `choice` takes to answer for `host`, and what, never
/// falling back to the system's; asked through `route`, as downloads ask.
pub async fn test(
    choice: &DnsChoice,
    host: &str,
    extra_roots: &[Vec<u8>],
    route: &Route,
) -> Result<(Duration, Vec<IpAddr>), DnsError> {
    let strict = DnsChoice {
        fallback: false,
        ..choice.clone()
    };
    let started = Instant::now();
    let ips = if strict.is_system() {
        system(host, 443)
            .await
            .map_err(|why| DnsError::Unreachable {
                dns: "the system's DNS".into(),
                host: host.to_owned(),
                why,
            })?
    } else {
        let resolver = build(&strict, extra_roots, route).await?;
        ask(&resolver, &strict, host, 443).await?
    };
    Ok((started.elapsed(), ips))
}

/// Head start each address gets before the next one is tried too (RFC 8305: 250 ms).
const STAGGER: Duration = Duration::from_millis(250);
/// The whole race; after it the addresses keep the resolver's order.
const RACE_BUDGET: Duration = Duration::from_secs(3);
/// How long a winner is preferred without racing again.
const REMEMBER: Duration = Duration::from_secs(300);

pub(crate) struct Resolver {
    /// Fixed addresses for host names, like hosts-file entries (`Options::hosts`).
    hosts: HashMap<String, Vec<IpAddr>>,
    /// The port the race connects to: the download's (redirects usually keep the scheme).
    port: u16,
    /// The chosen DNS, and its resolver once the first lookup made it (shared by the client's
    /// connections, so its cache serves all of them).
    dns: Arc<Chosen>,
}

struct Chosen {
    choice: DnsChoice,
    extra_roots: Vec<Vec<u8>>,
    /// The network adapter connections go through (#35): the DNS is asked through it, the race
    /// connects through it, and only addresses it can reach are handed on.
    route: Route,
    resolver: tokio::sync::OnceCell<Result<DnsResolver, DnsError>>,
}

impl Resolver {
    pub(crate) fn new(
        hosts: &[(String, Vec<IpAddr>)],
        port: u16,
        dns: &DnsChoice,
        extra_roots: &[Vec<u8>],
        route: &Route,
    ) -> Self {
        Resolver {
            hosts: hosts
                .iter()
                .map(|(h, ips)| (h.to_ascii_lowercase(), ips.clone()))
                .collect(),
            port,
            dns: Arc::new(Chosen {
                choice: dns.clone(),
                extra_roots: extra_roots.to_vec(),
                route: route.clone(),
                resolver: tokio::sync::OnceCell::new(),
            }),
        }
    }
}

impl Resolve for Resolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_ascii_lowercase();
        let fixed = self.hosts.get(&host).cloned();
        let port = self.port;
        let dns = self.dns.clone();
        Box::pin(async move {
            // Its time is left out of the connection's (#140, `server::dns_spent`).
            let started = std::time::Instant::now();
            let found: Result<Addrs, Box<dyn std::error::Error + Send + Sync>> = async {
                if let Route::Down { label } = &dns.route {
                    return Err(Box::new(DnsError::from(crate::net::NetError::Down {
                        label: label.clone(),
                    }))
                        as Box<dyn std::error::Error + Send + Sync>);
                }
                let ips: Vec<IpAddr> = match fixed {
                    Some(ips) => ips,
                    None if dns.choice.is_system() || local_name(&host) => {
                        tokio::net::lookup_host((host.as_str(), port))
                            .await?
                            .map(|a| a.ip())
                            .collect()
                    }
                    None => {
                        let resolver = dns
                            .resolver
                            .get_or_init(|| build(&dns.choice, &dns.extra_roots, &dns.route))
                            .await
                            .as_ref()
                            .map_err(|e| {
                                Box::new(e.clone()) as Box<dyn std::error::Error + Send + Sync>
                            })?;
                        ask(resolver, &dns.choice, &host, port).await?
                    }
                };
                // An address of the family the adapter lacks would be connected unbound (#35).
                let ips = reachable(ips, &dns.route)?;
                let ips = reachable_first(&host, ips, port, &dns.route).await;
                // Port 0: reqwest then uses the URL's port (a non-zero port here would win).
                let addrs: Addrs = Box::new(ips.into_iter().map(|ip| SocketAddr::new(ip, 0)));
                Ok(addrs)
            }
            .await;
            crate::server::dns_spent(started.elapsed());
            found
        })
    }
}

fn winners() -> &'static Mutex<HashMap<String, (IpAddr, Instant)>> {
    static WINNERS: OnceLock<Mutex<HashMap<String, (IpAddr, Instant)>>> = OnceLock::new();
    WINNERS.get_or_init(Default::default)
}

/// `ips` with an address that accepts connections first (unchanged when none answers in time).
async fn reachable_first(
    host: &str,
    mut ips: Vec<IpAddr>,
    port: u16,
    route: &Route,
) -> Vec<IpAddr> {
    if ips.len() < 2 {
        return ips;
    }
    let remembered = winners()
        .lock()
        .unwrap()
        .get(host)
        .filter(|(_, at)| at.elapsed() < REMEMBER)
        .map(|(ip, _)| *ip);
    let first = match remembered.and_then(|ip| ips.iter().position(|x| *x == ip)) {
        Some(i) => Some(i),
        None => race(&ips, port, route).await,
    };
    if let Some(i) = first {
        winners()
            .lock()
            .unwrap()
            .insert(host.to_owned(), (ips[i], Instant::now()));
        let ip = ips.remove(i);
        ips.insert(0, ip);
    }
    ips
}

/// Index of the first address that accepts a TCP connection. Each attempt gets a `STAGGER`
/// head start; a failed attempt starts the next one at once. The probe sockets are closed. They
/// connect through `route`, as the download will (an unbound probe would leave by the normal
/// route).
async fn race(ips: &[IpAddr], port: u16, route: &Route) -> Option<usize> {
    let deadline = tokio::time::Instant::now() + RACE_BUDGET;
    let mut attempts = JoinSet::new();
    let mut next = 0;
    loop {
        if next < ips.len() {
            let addr = SocketAddr::new(ips[next], port);
            let i = next;
            let route = route.clone();
            attempts
                .spawn(async move { crate::net::tcp_connect(&route, addr).await.ok().map(|_| i) });
            next += 1;
        } else if attempts.is_empty() {
            return None;
        }
        let wait = if next < ips.len() {
            STAGGER
        } else {
            deadline.saturating_duration_since(tokio::time::Instant::now())
        };
        match tokio::time::timeout(wait, attempts.join_next()).await {
            Ok(Some(Ok(Some(i)))) => return Some(i),
            // Failed (or nothing left): go on with the next address at once.
            Ok(_) => {}
            // The head start is over: add the next address to the race.
            Err(_) if next < ips.len() => {}
            Err(_) => return None,
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_listening_address_wins_over_a_dead_one() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // 192.0.2.1 is TEST-NET-1: never routed (silently dropped or unreachable).
        let dead: IpAddr = "192.0.2.1".parse().unwrap();
        let live: IpAddr = "127.0.0.1".parse().unwrap();
        let started = Instant::now();
        let ips = reachable_first("race.test", vec![dead, live], port, &Route::Any).await;
        assert_eq!(ips, vec![live, dead]);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn local_names_stay_with_the_system() {
        for n in [
            "localhost",
            "nas",
            "printer.local",
            "a.b.lan",
            "x.home.arpa",
            "svc.internal",
            "app.localhost",
        ] {
            assert!(local_name(n), "{n}");
        }
        for n in [
            "github.com",
            "example.org",
            "local.example.com",
            "internal.net",
        ] {
            assert!(!local_name(n), "{n}");
        }
    }

    #[test]
    fn servers_parse_with_and_without_ports() {
        let s = parse_servers("9.9.9.9, 1.1.1.1:5353; [2620:fe::fe]:853 2620:fe::9", 53).unwrap();
        let want: Vec<SocketAddr> = vec![
            "9.9.9.9:53".parse().unwrap(),
            "1.1.1.1:5353".parse().unwrap(),
            "[2620:fe::fe]:853".parse().unwrap(),
            "[2620:fe::9]:53".parse().unwrap(),
        ];
        assert_eq!(s, want);
        assert_eq!(
            parse_servers("9.9.9.9 dns.google", 53),
            Err("dns.google".into())
        );
        assert_eq!(parse_servers("", 53), Ok(vec![]));
    }

    #[test]
    fn presets_make_their_choices() {
        let n = Preset::by_id("nextdns").unwrap();
        assert!(!n.offers(Mode::Plain) && n.offers(Mode::Https));
        let c = n.choice(Mode::Https, "abc123", false, "NextDNS".into());
        assert_eq!(
            c.dns,
            Dns::Https {
                url: "https://dns.nextdns.io/abc123".into(),
                bootstrap: n.plain.iter().map(|a| a.parse().unwrap()).collect()
            }
        );
        match n.choice(Mode::Tls, "abc123", false, String::new()).dns {
            Dns::Tls { name, servers } => {
                assert_eq!(name, "abc123.dns.nextdns.io");
                assert!(servers.iter().all(|s| s.port() == 853));
            }
            other => panic!("{other:?}"),
        }
        assert!(valid_nextdns_profile("") && valid_nextdns_profile("a1b2c3"));
        assert!(!valid_nextdns_profile("xyz") && !valid_nextdns_profile("a1b2c3d"));
    }

    #[tokio::test]
    async fn an_unreachable_dns_fails_without_asking_the_system() {
        // TEST-NET-1: never answers. Strict: the error names the DNS, no system answer.
        let choice = DnsChoice {
            dns: Dns::Plain(vec!["192.0.2.1:53".parse().unwrap()]),
            fallback: false,
            label: "Dead DNS".into(),
        };
        let started = Instant::now();
        match lookup(&choice, "github.com", 443, &[], &Route::Any).await {
            Err(DnsError::Unreachable { dns, host, .. }) => {
                assert_eq!((dns.as_str(), host.as_str()), ("Dead DNS", "github.com"))
            }
            other => panic!("{other:?}"),
        }
        assert!(
            started.elapsed() < DNS_TIMEOUT * 3,
            "{:?}",
            started.elapsed()
        );
        // Local names don't go to it at all.
        assert!(
            lookup(&choice, "localhost", 80, &[], &Route::Any)
                .await
                .is_ok()
        );
    }

    /// Every preset in every mode it offers, against the real services, plus fallback and an
    /// IP-literal DoH address (`cargo test -p mdm-engine dns::tests::real -- --ignored`).
    #[tokio::test]
    #[ignore = "asks the real public DNS services"]
    async fn real_providers_answer_in_every_mode() {
        let mut failed = Vec::new();
        for p in PRESETS {
            for mode in [Mode::Plain, Mode::Tls, Mode::Https] {
                if !p.offers(mode) {
                    continue;
                }
                let label = format!("{} {mode:?}", p.name);
                let choice = p.choice(mode, "", false, label.clone());
                match test(&choice, "github.com", &[], &Route::Any).await {
                    Ok((took, ips)) => eprintln!("{label}: {took:?} {ips:?}"),
                    Err(e) => {
                        eprintln!("{label}: {e}");
                        failed.push(label);
                    }
                }
            }
        }
        let literal = DnsChoice {
            dns: Dns::Https {
                url: "https://1.1.1.1/dns-query".into(),
                bootstrap: vec![],
            },
            fallback: false,
            label: "1.1.1.1 DoH".into(),
        };
        match test(&literal, "github.com", &[], &Route::Any).await {
            Ok((took, ips)) => eprintln!("1.1.1.1 DoH: {took:?} {ips:?}"),
            Err(e) => failed.push(format!("1.1.1.1 DoH: {e}")),
        }
        // A name that doesn't exist is reported as such, not as unreachable.
        let q9 = Preset::by_id("quad9")
            .unwrap()
            .choice(Mode::Https, "", false, "Quad9".into());
        assert!(matches!(
            lookup(
                &q9,
                "no-such-name.invalid-test-mdm.example",
                443,
                &[],
                &Route::Any
            )
            .await,
            Err(DnsError::NoSuchName { .. })
        ));
        // Fallback: a dead DNS with fallback on is answered by the system's.
        let dead = DnsChoice {
            dns: Dns::Plain(vec!["192.0.2.1:53".parse().unwrap()]),
            fallback: true,
            label: "Dead DNS".into(),
        };
        assert!(
            lookup(&dead, "github.com", 443, &[], &Route::Any)
                .await
                .is_ok()
        );
        assert!(failed.is_empty(), "failed: {failed:?}");
    }

    /// On Tokio's paused clock: a race would wait for its connection's time limit, which the
    /// clock skips to; asked nothing, no time passes, however busy the machine.
    #[tokio::test(start_paused = true)]
    async fn one_address_is_not_raced() {
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        let started = tokio::time::Instant::now();
        assert_eq!(
            reachable_first("single.test", vec![ip], 9, &Route::Any).await,
            vec![ip]
        );
        assert!(started.elapsed() < Duration::from_millis(50));
    }
}
