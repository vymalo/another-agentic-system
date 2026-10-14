//! The egress guard: whether the orchestrator may fetch a URL a person typed, and whether it may
//! connect to the address that URL's host resolves to (ADR 0046, decision 5).
//!
//! A person's MCP server is a stranger's URL. The orchestrator will call it with a credential, so
//! the URL is the first thing an attacker controls: `https://169.254.169.254/` turns a feature into
//! a way to read the cloud's metadata service, and a name that answers a public address to a check
//! and a private one to the connection (DNS rebinding) walks past a check made on the name. The
//! rules here are the model of `dev/searxng-mcp/server.mjs` (`fetch`), ported and not shared, and
//! they are **pure**: no resolver, no socket, no clock, so the broker (`CredentialBroker`) and the
//! relay of the thread-tools endpoint apply the same function.
//!
//! # The two checks
//!
//! 1. **The URL** ([`EgressPolicy::check_url`]), when a server is added or edited and again at
//!    every connection: `https` only (`http` behind [`EgressPolicy::allowing_http`], a switch the
//!    composition root turns on outside production), a host, no user information, no query, no
//!    fragment, no port 0, and at most [`MAX_URL_BYTES`]. The URL is parsed by the `url` crate,
//!    the parser `reqwest` uses, so the guard judges what a client would connect to; the
//!    [normal form](EgressTarget::url) is what a caller keeps and connects with, never the text it
//!    was given. An address written in the URL (`2130706433`, `0177.0.0.1`, `0x7f.1`, `127.1`,
//!    `[::ffff:7f00:1]`) is read by that parser as the address it names and judged as such.
//! 2. **The address** ([`EgressPolicy::check_resolved`]), at every connection: the list of
//!    [`SocketAddr`]s the host resolved to. Each is judged by [`classify_address`], and one that
//!    is not public refuses the whole answer (a resolver that mixes a public and a private address
//!    is how rebinding is tried).
//!
//! # The deployment's lists
//!
//! [`EgressPolicy`] carries the two lists of `userToolServers.egress` as [`HostPattern`]s. A pattern
//! is an exact host (`mcp.example.com`), an address (`203.0.113.7`, `[2001:db8::7]`) or `*.` and a
//! domain (`*.example.com`: any subdomain at any depth, not the domain itself). Names are compared
//! in their normal form: lower case, internationalised names as punycode, one trailing dot ignored.
//! An address is written in full, in the standard library's strict form (`010.0.0.1`, `127.1` and
//! `0x7f.1` are refused: the URL parser would read them as other addresses than they look like).
//!
//! **An address is compared as the address a connection is made to.** An IPv6 address that carries
//! an IPv4 one (mapped, compatible, translated, NAT64, 6to4) is that IPv4 address, so an entry
//! `8.8.8.8` also names `[::ffff:808:808]`, and the lists are applied to the host of the URL **and**
//! to every address it resolved to ([`check_resolved`](EgressPolicy::check_resolved)).
//!
//! - **`deny`**: a host or a resolved address that matches is refused, whatever else. Deny always wins.
//! - **`allow`**, when it is not empty, refuses every host it does not name. A name that no entry
//!   names is still asked about its addresses while the list holds an address entry: it is admitted
//!   only if **every** address it resolved to is named by an address entry. The default, empty, is
//!   every public host.
//! - **`allow` lifts the ban on private-use addresses** (loopback, RFC 1918, carrier-grade NAT, unique
//!   local) only for what an operator named exactly: a host written in full, for the addresses it
//!   resolves to, and an address entry, for that address. **A `*.` suffix never lifts it**: it
//!   restricts which hosts may be asked, but `*.vercel.app` names every project a stranger can make,
//!   and `evil.vercel.app` may resolve to `10.0.0.1`. To reach a private network, name the hosts or
//!   the addresses.
//! - Nothing in `allow` lifts the [`AddressClass::Special`] ranges: link-local (the metadata address
//!   among them), unspecified, multicast, broadcast, reserved, documentation and the rest. A pattern
//!   like `*.internal` would otherwise name `metadata.google.internal` and hand over the node's
//!   credentials.
//!
//! # How a caller connects
//!
//! The guard does not resolve names and does not connect: it judges addresses a caller got, so
//! that the address that was judged is the address connected to.
//!
//! ```
//! use std::net::SocketAddr;
//! use orch_ports::EgressPolicy;
//!
//! let policy = EgressPolicy::default();
//! // when a server is added: keep the normal form of the URL
//! let target = policy.check_url("https://mcp.example.com/mcp").unwrap();
//! assert_eq!(target.url(), "https://mcp.example.com/mcp");
//!
//! // at every connection: check the URL again (the lists may have changed), resolve the host
//! // once, check what it resolved to, and connect to those addresses and no other
//! let resolved: Vec<SocketAddr> = vec!["93.184.216.34:443".parse().unwrap()];
//! let checked = policy.check_resolved(&target, &resolved).unwrap();
//! assert_eq!(checked.as_slice(), resolved.as_slice());
//!
//! // a name that resolves to the metadata address is refused, and the refusal does not say where
//! let metadata: Vec<SocketAddr> = vec!["169.254.169.254:443".parse().unwrap()];
//! let refusal = policy.check_resolved(&target, &metadata).unwrap_err();
//! assert_eq!(refusal.rule(), "private-address");
//! ```
//!
//! With `reqwest` (the next slices' job) that is either a `reqwest::dns::Resolve` that resolves,
//! calls [`check_resolved`](EgressPolicy::check_resolved) and returns only the checked addresses,
//! or `ClientBuilder::resolve_to_addrs(host, checked.as_slice())` after resolving by hand. In both
//! the client follows no redirect and uses no proxy (a redirect's target is a new URL and a proxy
//! connects where it likes), and connects with the host of [`EgressTarget::host`], so TLS still
//! verifies the name the person typed.
//!
//! # What a refusal says
//!
//! [`EgressRefusal`] names the rule that refused (`private-address`, `deny-list`, ...), never the
//! address a host resolved to and never the URL, so that what a person sees cannot be used to map
//! the network behind the orchestrator.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

use orch_core::{Classify, ErrorClass};
use url::{Host, Url};

/// The longest URL the guard reads, in bytes: 2 KiB, of the text given and of its normal form.
pub const MAX_URL_BYTES: usize = 2048;

/// The longest host name, in bytes (RFC 1035).
const MAX_HOST_BYTES: usize = 253;

/// The longest label of a host name, in bytes (RFC 1035).
const MAX_LABEL_BYTES: usize = 63;

// ------------------------------------------------------------------------------------ addresses

/// What an address is, to a connection the orchestrator would make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressClass {
    /// A global unicast address: anything a stranger's server may be at.
    Public,
    /// Private-use unicast: loopback, RFC 1918, carrier-grade NAT (`100.64.0.0/10`), unique local
    /// (`fc00::/7`) and the deprecated site-local (`fec0::/10`). Refused, unless the deployment's
    /// allow list names the host exactly or the address (never by a `*.` suffix).
    Private,
    /// Not a destination at all, or one nobody may name: link-local (`169.254.0.0/16`, which holds
    /// the cloud metadata address, and `fe80::/10`), "this network" (`0.0.0.0/8`, `::`), multicast,
    /// broadcast, the reserved block, benchmarking, the documentation ranges, the deprecated 6to4
    /// relay anycast prefix (`192.88.99.0/24`), the IETF protocol assignments and every IPv6 address outside the global unicast block `2000::/3`. Refused
    /// whatever the lists say.
    Special,
}

/// IPv4 private-use ranges (RFC 1918, RFC 6598, loopback).
const PRIVATE_V4: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
    (Ipv4Addr::new(127, 0, 0, 0), 8),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
];

/// IPv4 ranges that are no destination (RFC 6890).
const SPECIAL_V4: &[(Ipv4Addr, u8)] = &[
    (Ipv4Addr::new(0, 0, 0, 0), 8),
    (Ipv4Addr::new(169, 254, 0, 0), 16),
    (Ipv4Addr::new(192, 0, 0, 0), 24),
    (Ipv4Addr::new(192, 0, 2, 0), 24),
    // the deprecated 6to4 relay anycast prefix (RFC 7526)
    (Ipv4Addr::new(192, 88, 99, 0), 24),
    (Ipv4Addr::new(198, 18, 0, 0), 15),
    (Ipv4Addr::new(198, 51, 100, 0), 24),
    (Ipv4Addr::new(203, 0, 113, 0), 24),
    (Ipv4Addr::new(224, 0, 0, 0), 4),
    // the reserved block, and the limited broadcast address in it
    (Ipv4Addr::new(240, 0, 0, 0), 4),
];

/// IPv6 ranges inside the global unicast block that are still no destination: the IETF protocol
/// assignments (Teredo among them), the documentation prefixes (RFC 3849, RFC 9637).
const SPECIAL_V6: &[(Ipv6Addr, u8)] = &[
    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
    (Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0), 32),
    (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20),
];

fn v4_in(ip: Ipv4Addr, net: Ipv4Addr, prefix: u8) -> bool {
    let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
    u32::from(ip) & mask == u32::from(net) & mask
}

fn v6_in(ip: Ipv6Addr, net: Ipv6Addr, prefix: u8) -> bool {
    let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
    u128::from(ip) & mask == u128::from(net) & mask
}

/// The IPv4 address an IPv6 address carries, for the forms that are a way to reach an IPv4
/// address: IPv4-mapped (`::ffff:a.b.c.d`), IPv4-compatible (`::a.b.c.d`, deprecated),
/// IPv4-translated (`::ffff:0:a.b.c.d`, SIIT), NAT64 (`64:ff9b::a.b.c.d`) and 6to4
/// (`2002:aabb:ccdd::/48`, the IPv4 address in the second and third groups). Such an address is
/// judged as the IPv4 address, because that is where a gateway or the kernel sends the packet.
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let low = |hi: u16, lo: u16| {
        let [a, b] = hi.to_be_bytes();
        let [c, d] = lo.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    let zeros = |range: std::ops::Range<usize>| s[range].iter().all(|g| *g == 0);
    if zeros(0..6) // IPv4-compatible (this includes `::` and `::1`, judged as 0.0.0.0 and 0.0.0.1)
        || (zeros(0..5) && s[5] == 0xffff) // IPv4-mapped
        || (zeros(0..4) && s[4] == 0xffff && s[5] == 0) // IPv4-translated
        || (s[0] == 0x64 && s[1] == 0xff9b && zeros(2..6))
    // NAT64
    {
        return Some(low(s[6], s[7]));
    }
    if s[0] == 0x2002 {
        return Some(low(s[1], s[2]));
    }
    None
}

/// An address in the form two addresses are compared in: an IPv6 address that carries an IPv4 one
/// is that IPv4 address, because that is where the packet goes ([`embedded_v4`]). So an entry
/// written as `8.8.8.8` names `::ffff:8.8.8.8` and the other ways to reach it, and the reverse.
fn comparable(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => embedded_v4(v6).map_or(ip, IpAddr::V4),
        IpAddr::V4(_) => ip,
    }
}

fn classify_v4(ip: Ipv4Addr) -> AddressClass {
    if SPECIAL_V4
        .iter()
        .any(|(net, prefix)| v4_in(ip, *net, *prefix))
    {
        AddressClass::Special
    } else if PRIVATE_V4
        .iter()
        .any(|(net, prefix)| v4_in(ip, *net, *prefix))
    {
        AddressClass::Private
    } else {
        AddressClass::Public
    }
}

fn classify_v6(ip: Ipv6Addr) -> AddressClass {
    if ip == Ipv6Addr::LOCALHOST {
        return AddressClass::Private;
    }
    if let Some(v4) = embedded_v4(ip) {
        return classify_v4(v4);
    }
    let first = ip.segments()[0];
    // unique local fc00::/7 and the deprecated site-local fec0::/10
    if first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfec0 {
        return AddressClass::Private;
    }
    // Only the global unicast block is public. Everything else is a range that IANA marks as
    // link-local, multicast, discard, reserved or not yet assigned, and not yet assigned is
    // refused too: a new allocation is added here when a server lives in it.
    if first & 0xe000 != 0x2000 {
        return AddressClass::Special;
    }
    if SPECIAL_V6
        .iter()
        .any(|(net, prefix)| v6_in(ip, *net, *prefix))
    {
        return AddressClass::Special;
    }
    AddressClass::Public
}

/// What `ip` is to a connection: [`Public`](AddressClass::Public), [`Private`](AddressClass::Private)
/// or [`Special`](AddressClass::Special). An IPv6 address that carries an IPv4 one (mapped,
/// compatible, translated, NAT64, 6to4) is judged as the IPv4 address it carries.
pub fn classify_address(ip: IpAddr) -> AddressClass {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

/// Whether a connection to `ip` may be made with no word from the deployment: it is
/// [`AddressClass::Public`].
pub fn is_public_address(ip: IpAddr) -> bool {
    classify_address(ip) == AddressClass::Public
}

// ---------------------------------------------------------------------------------- host names

/// The host of a URL, in the form the guard compares and a client connects with.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EgressHost {
    /// A name: ASCII, lower case, letters, digits, `-` and `_` in labels of 1 to 63 bytes, at most
    /// 253 bytes, no trailing dot. An internationalised name is its punycode.
    Domain(String),
    /// An address written in the URL, in any spelling the URL parser reads.
    Ip(IpAddr),
}

impl EgressHost {
    /// A name from a host the URL parser produced: lower case already, possibly with one trailing
    /// dot. `None` when it is not a name the guard can compare.
    fn domain(name: &str) -> Option<Self> {
        let name = name.strip_suffix('.').unwrap_or(name);
        let label_ok = |label: &str| {
            (1..=MAX_LABEL_BYTES).contains(&label.len())
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        };
        if name.is_empty() || name.len() > MAX_HOST_BYTES || !name.split('.').all(label_ok) {
            return None;
        }
        Some(EgressHost::Domain(name.to_owned()))
    }

    fn from_url<S: AsRef<str>>(host: &Host<S>) -> Option<Self> {
        match host {
            Host::Domain(name) => Self::domain(name.as_ref()),
            Host::Ipv4(ip) => Some(EgressHost::Ip(IpAddr::V4(*ip))),
            Host::Ipv6(ip) => Some(EgressHost::Ip(IpAddr::V6(*ip))),
        }
    }
}

/// Why a string is not a [`HostPattern`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "not a host pattern: expected a host name (mcp.example.com), an address (203.0.113.7 or \
     [2001:db8::7]) or *. and a domain (*.example.com)"
)]
pub struct HostPatternError;

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatternKind {
    /// A name or an address, compared whole.
    Exact(EgressHost),
    /// `*.` and this domain: every host that ends in `.` and this.
    Suffix(String),
}

/// One entry of the deployment's `allow` or `deny` list: an exact host name, an address, or `*.`
/// and a domain. Read in the normal form of a host (see the module's docs), compared against the
/// host of a URL in the same form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPattern(PatternKind);

impl HostPattern {
    /// Whether `host` is named by this pattern.
    ///
    /// An address is compared as the address a connection would be made to: an IPv6 address that
    /// carries an IPv4 one (mapped, compatible, translated, NAT64, 6to4) is that IPv4 address, in the
    /// pattern and in the host alike, so `8.8.8.8` names `[::ffff:808:808]`.
    pub fn matches(&self, host: &EgressHost) -> bool {
        match (&self.0, host) {
            (PatternKind::Exact(EgressHost::Ip(pattern)), EgressHost::Ip(host)) => {
                comparable(*pattern) == comparable(*host)
            }
            (PatternKind::Exact(pattern), host) => pattern == host,
            (PatternKind::Suffix(suffix), EgressHost::Domain(name)) => name
                .strip_suffix(suffix.as_str())
                .is_some_and(|rest| rest.len() > 1 && rest.ends_with('.')),
            (PatternKind::Suffix(_), EgressHost::Ip(_)) => false,
        }
    }

    /// Whether this is an address entry that names `ip`, as [`matches`](Self::matches) compares
    /// them. For the addresses a name resolved to.
    fn matches_addr(&self, ip: IpAddr) -> bool {
        matches!(&self.0, PatternKind::Exact(EgressHost::Ip(pattern))
            if comparable(*pattern) == comparable(ip))
    }

    /// Whether this is an address entry (and not a name or a suffix).
    fn is_address(&self) -> bool {
        matches!(&self.0, PatternKind::Exact(EgressHost::Ip(_)))
    }

    /// Whether naming a host with this entry may lift the ban on private-use addresses: an exact
    /// host or an address, the operator naming that one thing. A `*.` suffix names a whole family
    /// of names, some of which a stranger can make, so it only admits them.
    fn lifts_private(&self) -> bool {
        matches!(&self.0, PatternKind::Exact(_))
    }
}

impl FromStr for HostPattern {
    type Err = HostPatternError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if let Some(domain) = text.strip_prefix("*.") {
            // the part after `*.` is a name, and only a name
            return match Host::parse(domain) {
                Ok(Host::Domain(name)) => match EgressHost::domain(&name) {
                    Some(EgressHost::Domain(name)) => Ok(HostPattern(PatternKind::Suffix(name))),
                    _ => Err(HostPatternError),
                },
                _ => Err(HostPatternError),
            };
        }
        // An address is written in full, in the standard library's strict form: the URL parser
        // reads `010.0.0.1` as 8.0.0.1 (octal) and `127.1` as 127.0.0.1, and an entry that names an
        // address its author did not mean is worse than an entry that is refused. A bare IPv6
        // address, or one in brackets.
        let bare = text
            .strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .unwrap_or(text);
        if let Ok(ip) = bare.parse::<Ipv6Addr>() {
            return Ok(HostPattern(PatternKind::Exact(EgressHost::Ip(IpAddr::V6(
                ip,
            )))));
        }
        if let Ok(ip) = text.parse::<Ipv4Addr>() {
            return Ok(HostPattern(PatternKind::Exact(EgressHost::Ip(IpAddr::V4(
                ip,
            )))));
        }
        match Host::parse(text) {
            Ok(Host::Domain(name)) => match EgressHost::domain(&name) {
                Some(host) => Ok(HostPattern(PatternKind::Exact(host))),
                None => Err(HostPatternError),
            },
            // an address in a spelling only the URL parser reads (octal, hex, decimal, short,
            // fullwidth) is not a pattern
            Ok(Host::Ipv4(_) | Host::Ipv6(_)) | Err(_) => Err(HostPatternError),
        }
    }
}

impl fmt::Display for HostPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            PatternKind::Exact(EgressHost::Domain(name)) => f.write_str(name),
            PatternKind::Exact(EgressHost::Ip(IpAddr::V4(ip))) => write!(f, "{ip}"),
            PatternKind::Exact(EgressHost::Ip(IpAddr::V6(ip))) => write!(f, "[{ip}]"),
            PatternKind::Suffix(suffix) => write!(f, "*.{suffix}"),
        }
    }
}

// ------------------------------------------------------------------------------------ refusals

/// The rule that refused a URL or an address. It names the rule and nothing else: no URL, no host
/// and never a resolved address (see the module's docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum EgressRefusal {
    /// The URL is longer than [`MAX_URL_BYTES`].
    #[error("the URL is longer than {MAX_URL_BYTES} bytes")]
    TooLong,
    /// The text is not an absolute URL.
    #[error("that is not an absolute URL")]
    NotAUrl,
    /// The scheme is not `https` (or `http`, where the caller switched it on).
    #[error("the URL scheme is not allowed: https only")]
    Scheme,
    /// The URL has no host, or one the guard cannot compare (an empty label, a character a name
    /// does not have).
    #[error("the URL has no usable host")]
    Host,
    /// The URL carries a user name or a password.
    #[error("the URL must not carry a user name or a password")]
    UserInfo,
    /// The URL has a query.
    #[error("the URL must not carry a query")]
    Query,
    /// The URL has a fragment.
    #[error("the URL must not carry a fragment")]
    Fragment,
    /// The URL's port is 0.
    #[error("the URL's port is not allowed")]
    Port,
    /// The host matches the deployment's deny list.
    #[error("the host is on the deployment's deny list")]
    Denied,
    /// The deployment's allow list is not empty and the host is not on it.
    #[error("the host is not on the deployment's allow list")]
    NotAllowed,
    /// The host is, or resolves to, an address a connection may not be made to: private, loopback,
    /// link-local, multicast, reserved (see [`AddressClass`]), or a name that is always loopback.
    #[error("the host is, or resolves to, a private or reserved address")]
    PrivateAddress,
    /// The host resolved to no address at all.
    #[error("the host resolved to no address")]
    NoAddress,
}

impl EgressRefusal {
    /// A stable name of the rule, for a log line, a metric label or an API's problem document:
    /// `too-long`, `syntax`, `scheme`, `host`, `user-info`, `query`, `fragment`, `port`,
    /// `deny-list`, `allow-list`, `private-address`, `no-address`.
    pub const fn rule(self) -> &'static str {
        match self {
            EgressRefusal::TooLong => "too-long",
            EgressRefusal::NotAUrl => "syntax",
            EgressRefusal::Scheme => "scheme",
            EgressRefusal::Host => "host",
            EgressRefusal::UserInfo => "user-info",
            EgressRefusal::Query => "query",
            EgressRefusal::Fragment => "fragment",
            EgressRefusal::Port => "port",
            EgressRefusal::Denied => "deny-list",
            EgressRefusal::NotAllowed => "allow-list",
            EgressRefusal::PrivateAddress => "private-address",
            EgressRefusal::NoAddress => "no-address",
        }
    }
}

impl Classify for EgressRefusal {
    fn class(&self) -> ErrorClass {
        match self {
            // a resolver that found nothing may find something later
            EgressRefusal::NoAddress => ErrorClass::Transient,
            EgressRefusal::TooLong
            | EgressRefusal::NotAUrl
            | EgressRefusal::Scheme
            | EgressRefusal::Host
            | EgressRefusal::UserInfo
            | EgressRefusal::Query
            | EgressRefusal::Fragment
            | EgressRefusal::Port
            | EgressRefusal::Denied
            | EgressRefusal::NotAllowed
            | EgressRefusal::PrivateAddress => ErrorClass::Invalid,
        }
    }
}

// ------------------------------------------------------------------------------------- policy

/// A URL the guard accepted, in its normal form.
///
/// Keep and connect with [`url`](EgressTarget::url), never the text that was given: the parser has
/// lower-cased the host, written an address in its canonical form, dropped a default port and
/// resolved the spellings a client would resolve the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressTarget {
    url: String,
    host: EgressHost,
    port: u16,
    https: bool,
}

impl EgressTarget {
    /// The URL in its normal form. It holds no credential: user information, a query and a
    /// fragment are refused.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The host.
    pub fn host(&self) -> &EgressHost {
        &self.host
    }

    /// The port to connect to: the URL's, else the scheme's (443, or 80 for `http`).
    pub const fn port(&self) -> u16 {
        self.port
    }

    /// Whether the scheme is `https`.
    pub const fn is_https(&self) -> bool {
        self.https
    }

    /// The socket address, when the URL writes an address and not a name: there is nothing to
    /// resolve, and this is what [`EgressPolicy::check_resolved`] is asked about.
    pub fn literal_addr(&self) -> Option<SocketAddr> {
        match self.host {
            EgressHost::Ip(ip) => Some(SocketAddr::new(ip, self.port)),
            EgressHost::Domain(_) => None,
        }
    }
}

/// Addresses [`EgressPolicy::check_resolved`] accepted: the only ones to connect to. They are in
/// the order the resolver gave them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedAddrs(Vec<SocketAddr>);

impl CheckedAddrs {
    /// The addresses.
    pub fn as_slice(&self) -> &[SocketAddr] {
        &self.0
    }

    /// The addresses, owned.
    pub fn into_vec(self) -> Vec<SocketAddr> {
        self.0
    }

    /// How many addresses (always at least one).
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Never true: an answer with no address is refused ([`EgressRefusal::NoAddress`]).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// How the allow list names a host (see [`EgressPolicy::naming`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Naming {
    /// No entry names it.
    Unnamed,
    /// A `*.` suffix does: the host may be asked, and no private-use address is lifted for it.
    Admitted,
    /// An exact host or an address does: the operator named it, and private-use addresses are
    /// lifted (never the special ones).
    Lifting,
}

/// The guard: the rules of the module's docs, with the deployment's lists. Cheap to clone, holds no
/// state, and decides from its arguments only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EgressPolicy {
    allow: Vec<HostPattern>,
    deny: Vec<HostPattern>,
    allow_http: bool,
}

impl EgressPolicy {
    /// The default: `https` only, every public host, no list.
    pub fn new() -> Self {
        Self::default()
    }

    /// The same policy that also takes `http` URLs: for development and tests, never for a
    /// deployment that holds real credentials. The caller decides (the composition root, from the
    /// environment it runs in); nothing here reads one.
    #[must_use]
    pub fn allowing_http(mut self) -> Self {
        self.allow_http = true;
        self
    }

    /// The same policy with `pattern` added to the allow list.
    #[must_use]
    pub fn with_allow(mut self, pattern: HostPattern) -> Self {
        self.allow.push(pattern);
        self
    }

    /// The same policy with `pattern` added to the deny list.
    #[must_use]
    pub fn with_deny(mut self, pattern: HostPattern) -> Self {
        self.deny.push(pattern);
        self
    }

    /// A policy from the two lists as the configuration writes them
    /// (`userToolServers.egress.allow` and `.deny`).
    ///
    /// # Errors
    /// [`HostPatternError`] for the first entry that is not a [`HostPattern`].
    pub fn from_lists<A, D>(allow: A, deny: D) -> Result<Self, HostPatternError>
    where
        A: IntoIterator,
        A::Item: AsRef<str>,
        D: IntoIterator,
        D::Item: AsRef<str>,
    {
        fn read<I>(list: I) -> Result<Vec<HostPattern>, HostPatternError>
        where
            I: IntoIterator,
            I::Item: AsRef<str>,
        {
            list.into_iter().map(|text| text.as_ref().parse()).collect()
        }
        Ok(EgressPolicy {
            allow: read(allow)?,
            deny: read(deny)?,
            allow_http: false,
        })
    }

    /// How the allow list names a host: by a name or an address entry that lifts the ban on private
    /// addresses, by a `*.` suffix that only admits it, or not at all.
    fn naming(&self, host: &EgressHost) -> Naming {
        let mut naming = Naming::Unnamed;
        for pattern in self.allow.iter().filter(|pattern| pattern.matches(host)) {
            if pattern.lifts_private() {
                return Naming::Lifting;
            }
            naming = Naming::Admitted;
        }
        naming
    }

    /// The deployment's lists applied to a host: refuses what `deny` names and, when `allow` is
    /// not empty, what it does not name. A **name** that no entry names is not refused while the
    /// allow list holds an address entry: it may still be admitted by what it resolves to, which
    /// [`check_resolved`](Self::check_resolved) judges.
    fn review(&self, host: &EgressHost) -> Result<Naming, EgressRefusal> {
        if self.deny.iter().any(|pattern| pattern.matches(host)) {
            return Err(EgressRefusal::Denied);
        }
        let naming = self.naming(host);
        if !self.allow.is_empty() && naming == Naming::Unnamed {
            let may_resolve_into_the_list = matches!(host, EgressHost::Domain(_))
                && self.allow.iter().any(HostPattern::is_address);
            if !may_resolve_into_the_list {
                return Err(EgressRefusal::NotAllowed);
            }
        }
        Ok(naming)
    }

    /// Whether a connection to an address of this class may be made, when the lists `lift` the ban
    /// on private-use addresses for it.
    fn admits(class: AddressClass, lift: bool) -> bool {
        match class {
            AddressClass::Public => true,
            AddressClass::Private => lift,
            AddressClass::Special => false,
        }
    }

    /// Whether `text` is a URL a person's MCP server may be at, and its normal form if it is.
    ///
    /// For a host written as a name the address is not known yet: the answer is "not yet refused",
    /// and [`check_resolved`](Self::check_resolved) judges what the name resolves to. A name that
    /// is always loopback (`localhost`, `*.localhost`, RFC 6761) is refused here unless the allow
    /// list names it **exactly** (a `*.localhost` entry does not lift it), because no resolver could
    /// make it a public address.
    ///
    /// # Errors
    /// [`EgressRefusal`], the first rule the URL breaks, in the order of the module's docs.
    pub fn check_url(&self, text: &str) -> Result<EgressTarget, EgressRefusal> {
        if text.len() > MAX_URL_BYTES {
            return Err(EgressRefusal::TooLong);
        }
        let url = Url::parse(text).map_err(|_| EgressRefusal::NotAUrl)?;
        // the limit is on the normal form too: it is what is kept, and percent-encoding and
        // punycode make it longer than the text
        if url.as_str().len() > MAX_URL_BYTES {
            return Err(EgressRefusal::TooLong);
        }
        let https = match url.scheme() {
            "https" => true,
            "http" if self.allow_http => false,
            _ => return Err(EgressRefusal::Scheme),
        };
        if !url.username().is_empty() || url.password().is_some() {
            return Err(EgressRefusal::UserInfo);
        }
        let host = url
            .host()
            .and_then(|host| EgressHost::from_url(&host))
            .ok_or(EgressRefusal::Host)?;
        if url.query().is_some() {
            return Err(EgressRefusal::Query);
        }
        if url.fragment().is_some() {
            return Err(EgressRefusal::Fragment);
        }
        if url.port() == Some(0) {
            return Err(EgressRefusal::Port);
        }
        let naming = self.review(&host)?;
        let lift = naming == Naming::Lifting;
        match &host {
            EgressHost::Ip(ip) => {
                if !Self::admits(classify_address(*ip), lift) {
                    return Err(EgressRefusal::PrivateAddress);
                }
            }
            EgressHost::Domain(name) => {
                let loopback = name == "localhost" || name.ends_with(".localhost");
                if loopback && !lift {
                    return Err(EgressRefusal::PrivateAddress);
                }
            }
        }
        Ok(EgressTarget {
            url: url.as_str().to_owned(),
            host,
            // `http` and `https` both have a default port
            port: url
                .port_or_known_default()
                .unwrap_or(if https { 443 } else { 80 }),
            https,
        })
    }

    /// Whether a connection may be made to the addresses `target`'s host resolved to, and if so
    /// the addresses to connect to.
    ///
    /// The lists are applied to the host again, so a definition stored under an older policy is
    /// connected to under the current one, **and to every address**, in any spelling (see the
    /// module's docs): an address on the deny list refuses the answer wherever it is in it, and an
    /// address entry of the allow list names the addresses a name may resolve to. Every address is
    /// judged by [`classify_address`] and one that is refused refuses them all. For a target that
    /// writes an address, pass [`EgressTarget::literal_addr`].
    ///
    /// # Errors
    /// [`EgressRefusal::Denied`], [`EgressRefusal::NotAllowed`], [`EgressRefusal::NoAddress`] for an
    /// empty answer, [`EgressRefusal::PrivateAddress`] for an address that is refused.
    pub fn check_resolved(
        &self,
        target: &EgressTarget,
        resolved: &[SocketAddr],
    ) -> Result<CheckedAddrs, EgressRefusal> {
        let naming = self.review(&target.host)?;
        if resolved.is_empty() {
            return Err(EgressRefusal::NoAddress);
        }
        // deny wins over everything, for an address in any spelling, wherever it is in the answer
        if resolved.iter().any(|addr| {
            self.deny
                .iter()
                .any(|pattern| pattern.matches_addr(addr.ip()))
        }) {
            return Err(EgressRefusal::Denied);
        }
        for addr in resolved {
            let ip = addr.ip();
            let listed = self.allow.iter().any(|pattern| pattern.matches_addr(ip));
            if !self.allow.is_empty() && naming == Naming::Unnamed && !listed {
                return Err(EgressRefusal::NotAllowed);
            }
            let lift = naming == Naming::Lifting || listed;
            if !Self::admits(classify_address(ip), lift) {
                return Err(EgressRefusal::PrivateAddress);
            }
        }
        Ok(CheckedAddrs(resolved.to_vec()))
    }
}
