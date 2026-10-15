//! The egress guard (ADR 0046, decision 5), through its public API: which URLs may be added, which
//! addresses a connection may be made to, and what the deployment's lists change.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::net::{IpAddr, SocketAddr};

use orch_core::{Classify, ErrorClass};
use orch_ports::{
    AddressClass, EgressHost, EgressPolicy, EgressRefusal, HostPattern, LOOKUP_TIMEOUT,
    LookupFailure, MAX_URL_BYTES, classify_address, is_public_address, judge_lookup,
};

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

fn sock(text: &str) -> SocketAddr {
    SocketAddr::new(ip(text), 443)
}

fn policy(allow: &[&str], deny: &[&str]) -> EgressPolicy {
    EgressPolicy::from_lists(allow.iter().copied(), deny.iter().copied()).unwrap()
}

// ------------------------------------------------------------------------------------ addresses

#[test]
fn ipv4_addresses_on_either_side_of_every_range_boundary() {
    // (address, public?) - each range of the table has its last-below, first, last and first-above
    let cases = [
        ("0.0.0.0", false),
        ("0.255.255.255", false),
        ("1.0.0.0", true),
        ("1.1.1.1", true),
        ("8.8.8.8", true),
        ("9.255.255.255", true),
        ("10.0.0.0", false),
        ("10.255.255.255", false),
        ("11.0.0.0", true),
        ("100.63.255.255", true),
        ("100.64.0.0", false),
        ("100.127.255.255", false),
        ("100.128.0.0", true),
        ("126.255.255.255", true),
        ("127.0.0.0", false),
        ("127.0.0.1", false),
        ("127.255.255.255", false),
        ("128.0.0.0", true),
        ("169.253.255.255", true),
        ("169.254.0.0", false),
        ("169.254.169.254", false),
        ("169.254.255.255", false),
        ("169.255.0.0", true),
        ("172.15.255.255", true),
        ("172.16.0.0", false),
        ("172.31.255.255", false),
        ("172.32.0.0", true),
        ("191.255.255.255", true),
        ("192.0.0.0", false),
        ("192.0.0.255", false),
        ("192.0.1.0", true),
        ("192.0.1.255", true),
        ("192.0.2.0", false),
        ("192.0.2.255", false),
        ("192.0.3.0", true),
        ("192.88.98.255", true),
        ("192.88.99.0", false),
        ("192.88.99.1", false),
        ("192.88.99.255", false),
        ("192.88.100.0", true),
        ("192.167.255.255", true),
        ("192.168.0.0", false),
        ("192.168.255.255", false),
        ("192.169.0.0", true),
        ("198.17.255.255", true),
        ("198.18.0.0", false),
        ("198.19.255.255", false),
        ("198.20.0.0", true),
        ("198.51.99.255", true),
        ("198.51.100.0", false),
        ("198.51.100.255", false),
        ("198.51.101.0", true),
        ("203.0.112.255", true),
        ("203.0.113.0", false),
        ("203.0.113.255", false),
        ("203.0.114.0", true),
        ("223.255.255.255", true),
        ("224.0.0.0", false),
        ("224.0.0.1", false),
        ("239.255.255.255", false),
        ("240.0.0.0", false),
        ("255.255.255.254", false),
        ("255.255.255.255", false),
    ];
    for (address, public) in cases {
        assert_eq!(is_public_address(ip(address)), public, "{address}");
    }
}

#[test]
fn ipv6_addresses_in_and_around_every_range() {
    let cases = [
        // unspecified, loopback, and everything outside the global unicast block 2000::/3
        ("::", false),
        ("::1", false),
        ("::2", false),
        ("1fff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", false),
        ("4000::1", false),
        ("8000::1", false),
        ("c000::1", false),
        // unique local fc00::/7
        ("fbff:ffff::1", false),
        ("fc00::1", false),
        ("fd12:3456:789a::1", false),
        ("fdff:ffff:ffff:ffff::1", false),
        // link-local fe80::/10, site-local fec0::/10
        ("fe80::1", false),
        ("febf:ffff::1", false),
        ("fec0::1", false),
        // multicast ff00::/8, whatever the scope
        ("ff00::1", false),
        ("ff02::1", false),
        ("ff0e::1", false),
        ("ffff::1", false),
        // documentation
        ("2001:db8::1", false),
        ("2001:db8:ffff:ffff::1", false),
        ("2001:db7::1", true),
        ("2001:db9::1", true),
        ("3fff::1", false),
        ("3fff:fff:ffff::1", false),
        ("3ffe::1", true),
        ("4000::", false),
        // discard-only 100::/64, IETF protocol assignments 2001::/23 (Teredo among them)
        ("100::1", false),
        ("100:0:0:1::1", false),
        ("2001::1", false),
        ("2001:0:4136:e378:8000:63bf:3fff:fdd2", false),
        ("2001:1::1", false),
        ("2001:1ff:ffff::1", false),
        ("2001:200::1", true),
        // SRv6 5f00::/16, local-use NAT64 64:ff9b:1::/48
        ("5f00::1", false),
        ("64:ff9b:1::1", false),
        ("64:ff9b:1:ffff::1", false),
        // addresses on the public internet
        ("2606:4700:4700::1111", true),
        ("2001:4860:4860::8888", true),
        ("2a00:1450:4009:81f::200e", true),
        ("2620:fe::fe", true),
    ];
    for (address, public) in cases {
        assert_eq!(is_public_address(ip(address)), public, "{address}");
    }
}

#[test]
fn an_ipv6_address_that_embeds_an_ipv4_one_is_judged_as_that_ipv4_address() {
    let cases = [
        // IPv4-mapped ::ffff:0:0/96
        ("::ffff:127.0.0.1", false),
        ("::ffff:7f00:1", false),
        ("::ffff:10.1.2.3", false),
        ("::ffff:169.254.169.254", false),
        ("::ffff:192.168.0.1", false),
        ("::ffff:100.64.0.1", false),
        ("::ffff:0.0.0.0", false),
        ("::ffff:224.0.0.1", false),
        ("::ffff:255.255.255.255", false),
        ("::ffff:8.8.8.8", true),
        ("::ffff:808:808", true),
        // IPv4-compatible ::/96 (deprecated)
        ("::127.0.0.1", false),
        ("::a00:1", false),
        ("::10.0.0.1", false),
        ("::169.254.169.254", false),
        ("::0.0.0.5", false),
        ("::8.8.8.8", true),
        // IPv4-translated ::ffff:0:0:0/96 (SIIT)
        ("::ffff:0:127.0.0.1", false),
        ("::ffff:0:10.0.0.1", false),
        ("::ffff:0:8.8.8.8", true),
        // NAT64 64:ff9b::/96
        ("64:ff9b::7f00:1", false),
        ("64:ff9b::127.0.0.1", false),
        ("64:ff9b::a00:1", false),
        ("64:ff9b::a9fe:a9fe", false),
        ("64:ff9b::c0a8:1", false),
        ("64:ff9b::8.8.8.8", true),
        ("64:ff9b::808:808", true),
        // 6to4 2002::/16 carries the IPv4 address in its second and third groups
        ("2002:7f00:1::1", false),
        ("2002:a00:1::1", false),
        ("2002:a9fe:a9fe::1", false),
        ("2002:c0a8:1::1", false),
        ("2002:808:808::1", true),
    ];
    for (address, public) in cases {
        assert_eq!(is_public_address(ip(address)), public, "{address}");
    }
}

#[test]
fn addresses_are_classed_public_private_or_special() {
    use AddressClass::{Private, Public, Special};
    let cases = [
        ("8.8.8.8", Public),
        ("2606:4700:4700::1111", Public),
        // private-use: what a deployment's allow list can lift
        ("127.0.0.1", Private),
        ("::1", Private),
        ("10.0.0.5", Private),
        ("172.16.0.1", Private),
        ("192.168.1.1", Private),
        ("100.64.0.1", Private),
        ("fd00::1", Private),
        ("fec0::1", Private),
        ("::ffff:10.0.0.5", Private),
        ("64:ff9b::a00:5", Private),
        // special: never a destination, whatever the lists say
        ("169.254.169.254", Special),
        ("fe80::1", Special),
        ("::ffff:169.254.169.254", Special),
        ("0.0.0.0", Special),
        ("::", Special),
        ("224.0.0.1", Special),
        ("ff02::1", Special),
        ("255.255.255.255", Special),
        ("240.0.0.1", Special),
        ("192.0.2.1", Special),
        ("2001:db8::1", Special),
        ("198.18.0.1", Special),
        ("100::1", Special),
        ("2001::1", Special),
        ("4000::1", Special),
    ];
    for (address, class) in cases {
        assert_eq!(classify_address(ip(address)), class, "{address}");
    }
}

// ------------------------------------------------------------------------------------ the URL

#[test]
fn a_public_https_url_is_a_target() {
    let target = policy(&[], &[])
        .check_url("https://mcp.example.com/mcp")
        .unwrap();
    assert_eq!(target.url(), "https://mcp.example.com/mcp");
    assert_eq!(target.host(), &EgressHost::Domain("mcp.example.com".into()));
    assert_eq!(target.port(), 443);
    assert!(target.is_https());
}

#[test]
fn the_url_is_normalised_and_the_normal_form_is_what_is_kept() {
    let p = policy(&[], &[]);
    // case, the default port, a bare host, a trailing dot, an internationalised name
    let cases = [
        (
            "HTTPS://MCP.Example.COM/Path",
            "https://mcp.example.com/Path",
        ),
        (
            "https://mcp.example.com:443/mcp",
            "https://mcp.example.com/mcp",
        ),
        ("https://mcp.example.com", "https://mcp.example.com/"),
        (
            "https://mcp.example.com./mcp",
            "https://mcp.example.com./mcp",
        ),
        (
            "https://bücher.example/mcp",
            "https://xn--bcher-kva.example/mcp",
        ),
        // what the parser reads one way, the client reads the same way: the kept form is the one
        // that is judged, so these are different spellings of hosts that are checked as they are
        ("https:///mcp", "https://mcp/"),
        ("https:mcp.example.com/mcp", "https://mcp.example.com/mcp"),
        (
            "https://mcp.example.com\\mcp",
            "https://mcp.example.com/mcp",
        ),
        (
            "https://@mcp.example.com/mcp",
            "https://mcp.example.com/mcp",
        ),
        (
            " https://mcp.example.com/mcp\n",
            "https://mcp.example.com/mcp",
        ),
    ];
    for (given, kept) in cases {
        assert_eq!(p.check_url(given).unwrap().url(), kept, "{given}");
    }
    assert_eq!(
        p.check_url("https://mcp.example.com:8443/mcp")
            .unwrap()
            .port(),
        8443
    );
    // a trailing dot names the same host
    assert_eq!(
        p.check_url("https://mcp.example.com./mcp").unwrap().host(),
        &EgressHost::Domain("mcp.example.com".into())
    );
}

#[test]
fn only_https_unless_the_caller_switches_http_on() {
    let strict = policy(&[], &[]);
    for url in [
        "http://mcp.example.com/mcp",
        "ftp://mcp.example.com/mcp",
        "file:///etc/passwd",
        "ws://mcp.example.com/mcp",
        "wss://mcp.example.com/mcp",
        "gopher://mcp.example.com/",
        "javascript:alert(1)",
        "data:text/plain,hello",
        "mailto:a@b.example",
    ] {
        assert_eq!(
            strict.check_url(url).unwrap_err(),
            EgressRefusal::Scheme,
            "{url}"
        );
    }
    let dev = EgressPolicy::new().allowing_http();
    let target = dev.check_url("http://mcp.example.com/mcp").unwrap();
    assert!(!target.is_https());
    assert_eq!(target.port(), 80);
    // the switch is for http and nothing else
    assert_eq!(
        dev.check_url("ftp://mcp.example.com/").unwrap_err(),
        EgressRefusal::Scheme
    );
}

#[test]
fn a_url_needs_a_host() {
    let p = policy(&[], &[]);
    for url in [
        "https://",
        "https://:443/mcp",
        "https://./mcp",
        "https://..",
    ] {
        let error = p.check_url(url).unwrap_err();
        assert!(
            matches!(error, EgressRefusal::NotAUrl | EgressRefusal::Host),
            "{url}: {error:?}"
        );
    }
    for url in ["", "mcp.example.com/mcp", "/mcp", "https", "not a url"] {
        assert_eq!(
            p.check_url(url).unwrap_err(),
            EgressRefusal::NotAUrl,
            "{url}"
        );
    }
}

#[test]
fn a_host_with_an_empty_label_is_refused() {
    let p = policy(&[], &[]);
    for url in [
        "https://a..example.com/",
        "https://.example.com/",
        "https://example.com../",
    ] {
        assert!(p.check_url(url).is_err(), "{url}");
    }
}

#[test]
fn user_information_a_query_and_a_fragment_are_refused() {
    let p = policy(&[], &[]);
    for url in [
        "https://user@mcp.example.com/mcp",
        "https://user:pass@mcp.example.com/mcp",
        "https://:pass@mcp.example.com/mcp",
        "https://mcp.example.com:pass@evil.example/mcp",
    ] {
        assert_eq!(
            p.check_url(url).unwrap_err(),
            EgressRefusal::UserInfo,
            "{url}"
        );
    }
    for url in [
        "https://mcp.example.com/mcp?token=abc",
        "https://mcp.example.com/mcp?",
        "https://mcp.example.com/?a=b",
    ] {
        assert_eq!(p.check_url(url).unwrap_err(), EgressRefusal::Query, "{url}");
    }
    for url in [
        "https://mcp.example.com/mcp#frag",
        "https://mcp.example.com/mcp#",
        "https://mcp.example.com/#a",
    ] {
        assert_eq!(
            p.check_url(url).unwrap_err(),
            EgressRefusal::Fragment,
            "{url}"
        );
    }
}

#[test]
fn a_port_of_zero_and_a_url_that_is_too_long_are_refused() {
    let p = policy(&[], &[]);
    assert_eq!(
        p.check_url("https://mcp.example.com:0/mcp").unwrap_err(),
        EgressRefusal::Port
    );
    let long = format!("https://mcp.example.com/{}", "a".repeat(MAX_URL_BYTES));
    assert_eq!(p.check_url(&long).unwrap_err(), EgressRefusal::TooLong);
    let fits = format!(
        "https://mcp.example.com/{}",
        "a".repeat(MAX_URL_BYTES - "https://mcp.example.com/".len())
    );
    assert!(p.check_url(&fits).is_ok());

    // the limit is on the normal form, which is what is kept and can be longer than the text:
    // each `é` is two bytes written and six once percent-encoded
    let expands = format!("https://mcp.example.com/{}", "é".repeat(400));
    assert!(expands.len() < MAX_URL_BYTES);
    assert_eq!(p.check_url(&expands).unwrap_err(), EgressRefusal::TooLong);
    // and whatever is accepted fits a definition
    let accepted = p
        .check_url(&format!("https://mcp.example.com/{}", "é".repeat(300)))
        .unwrap();
    assert!(accepted.url().len() <= MAX_URL_BYTES);
}

#[test]
fn an_address_written_in_the_url_is_judged_whatever_its_spelling() {
    let p = policy(&[], &[]);
    // the URL parser reads these as the address they name: decimal, octal, hex, short forms,
    // fullwidth digits, a trailing dot, brackets
    for url in [
        "https://127.0.0.1/",
        "https://2130706433/",
        "https://017700000001/",
        "https://0x7f000001/",
        "https://0x7f.0.0.1/",
        "https://0177.0.0.1/",
        "https://127.1/",
        "https://127.0.1/",
        "https://0/",
        "https://0.0.0.0/",
        "https://１２７.０.０.１/",
        "https://127.0.0.1./",
        "https://10.0.0.1/",
        "https://167772161/",
        "https://0xa.0.0.1/",
        "https://169.254.169.254/latest/meta-data/",
        "https://2852039166/",
        "https://[::1]/",
        "https://[0:0:0:0:0:0:0:1]/",
        "https://[::]/",
        "https://[::ffff:127.0.0.1]/",
        "https://[::ffff:7f00:1]/",
        "https://[::127.0.0.1]/",
        "https://[64:ff9b::7f00:1]/",
        "https://[fe80::1]/",
        "https://[fd00::1]/",
        "https://[fc00::1]:8443/",
        "https://[2001:db8::1]/",
        "https://[ff02::1]/",
        "https://[2002:7f00:1::1]/",
    ] {
        assert_eq!(
            p.check_url(url).unwrap_err(),
            EgressRefusal::PrivateAddress,
            "{url}"
        );
    }
    // a public address is a host like another, and is kept in its normal form
    let target = p.check_url("https://8.8.8.8/mcp").unwrap();
    assert_eq!(target.host(), &EgressHost::Ip(ip("8.8.8.8")));
    let target = p.check_url("https://134744072/mcp").unwrap();
    assert_eq!(target.url(), "https://8.8.8.8/mcp");
    let target = p
        .check_url("https://[2606:4700:4700::1111]:8443/mcp")
        .unwrap();
    assert_eq!(target.host(), &EgressHost::Ip(ip("2606:4700:4700::1111")));
    assert_eq!(target.port(), 8443);
}

#[test]
fn an_address_that_does_not_parse_as_one_is_not_an_address() {
    let p = policy(&[], &[]);
    // out of range or malformed: a URL parser refuses them, and so does the guard
    for url in [
        "https://256.0.0.1/",
        "https://1.2.3.4.5/",
        "https://999999999999/",
        "https://[::1/",
        "https://[1:2:3:4:5:6:7:8:9]/",
        "https://0x/",
    ] {
        assert!(p.check_url(url).is_err(), "{url}");
    }
}

#[test]
fn a_percent_encoded_dot_is_decoded_before_the_host_is_judged() {
    let p = policy(&[], &[]);
    for url in [
        "https://127%2e0%2e0%2e1/",
        "https://127%2E0%2E0%2E1/",
        "https://%31%32%37.0.0.1/",
        "https://169%2e254%2e169%2e254/",
        "https://localhost%2e/",
        "https://10%2e0.0%2e1:8443/mcp",
    ] {
        let refused = p.check_url(url).unwrap_err();
        assert!(
            matches!(refused, EgressRefusal::PrivateAddress | EgressRefusal::Host),
            "{url}: {refused:?}"
        );
    }
    assert_eq!(
        p.check_url("https://127%2e0%2e0%2e1/").unwrap_err(),
        EgressRefusal::PrivateAddress
    );
    // the decoded public address is that address, in its normal form
    assert_eq!(
        p.check_url("https://8%2e8%2e8%2e8/mcp").unwrap().url(),
        "https://8.8.8.8/mcp"
    );
}

#[test]
fn an_ipv6_literal_with_a_zone_id_is_refused() {
    let p = policy(&[], &[]);
    for url in [
        "https://[fe80::1%25eth0]/",
        "https://[fe80::1%eth0]/",
        "https://[2606:4700:4700::1111%25eth0]/",
        "https://[::1%251]/",
    ] {
        assert!(p.check_url(url).is_err(), "{url}");
    }
}

#[test]
fn localhost_names_are_private_unless_named_by_the_allow_list() {
    let p = policy(&[], &[]);
    for url in [
        "https://localhost/",
        "https://LOCALHOST:8443/",
        "https://localhost./",
        "https://app.localhost/",
        "https://a.b.localhost/",
    ] {
        assert_eq!(
            p.check_url(url).unwrap_err(),
            EgressRefusal::PrivateAddress,
            "{url}"
        );
    }
    // a name that only ends in the word is a name like another
    assert!(p.check_url("https://notlocalhost.example/").is_ok());
    assert!(p.check_url("https://localhost.example/").is_ok());
    // the operator can name it
    let dev = policy(&["localhost"], &[]).allowing_http();
    assert!(dev.check_url("http://localhost:8080/mcp").is_ok());
}

// ------------------------------------------------------------------------------- the lists

#[test]
fn a_host_pattern_is_an_exact_host_or_a_suffix_or_an_address() {
    let exact: HostPattern = "mcp.example.com".parse().unwrap();
    let suffix: HostPattern = "*.example.com".parse().unwrap();
    let address: HostPattern = "203.0.113.7".parse().unwrap();
    let v6: HostPattern = "[2001:db8::7]".parse().unwrap();
    let domain = |d: &str| EgressHost::Domain(d.to_owned());

    assert!(exact.matches(&domain("mcp.example.com")));
    assert!(!exact.matches(&domain("other.example.com")));
    assert!(!exact.matches(&domain("a.mcp.example.com")));
    assert!(!exact.matches(&domain("example.com")));

    assert!(suffix.matches(&domain("mcp.example.com")));
    assert!(suffix.matches(&domain("a.b.example.com")));
    // the suffix is subdomains: not the apex, not a host that only ends in the same letters
    assert!(!suffix.matches(&domain("example.com")));
    assert!(!suffix.matches(&domain("badexample.com")));
    assert!(!suffix.matches(&domain("example.com.evil.net")));

    assert!(address.matches(&EgressHost::Ip(ip("203.0.113.7"))));
    assert!(!address.matches(&EgressHost::Ip(ip("203.0.113.8"))));
    assert!(!address.matches(&domain("203.0.113.7")));
    assert!(v6.matches(&EgressHost::Ip(ip("2001:db8:0:0:0:0:0:7"))));
}

#[test]
fn a_pattern_is_read_in_its_normal_form() {
    let upper: HostPattern = "MCP.Example.COM.".parse().unwrap();
    assert!(upper.matches(&EgressHost::Domain("mcp.example.com".into())));
    assert_eq!(upper.to_string(), "mcp.example.com");
    let suffix: HostPattern = "*.Example.com".parse().unwrap();
    assert_eq!(suffix.to_string(), "*.example.com");
    let idn: HostPattern = "bücher.example".parse().unwrap();
    assert!(idn.matches(&EgressHost::Domain("xn--bcher-kva.example".into())));
}

#[test]
fn a_numeric_pattern_is_an_address_written_in_full_or_nothing() {
    // the URL parser reads these as addresses (`010.0.0.1` is 8.0.0.1, octal), so a pattern written
    // so would name an address nobody meant: a pattern takes the standard library's strict forms
    for pattern in [
        "010.0.0.1",
        "0177.0.0.1",
        "0x7f.0.0.1",
        "0x7f.1",
        "127.1",
        "127.0.1",
        "2130706433",
        "017700000001",
        "0x7f000001",
        "１２７.０.０.１",
        "127.0.0.1.",
        "1.2.3.4.5",
        "[::1%eth0]",
        "fe80::1%eth0",
        "[fe80::1%25eth0]",
        "::ffff:1.2.3.4.5",
    ] {
        assert!(pattern.parse::<HostPattern>().is_err(), "{pattern:?}");
    }
    let decimal: HostPattern = "10.0.0.1".parse().unwrap();
    assert!(decimal.matches(&EgressHost::Ip(ip("10.0.0.1"))));
    assert!(!decimal.matches(&EgressHost::Ip(ip("8.0.0.1"))));
    // and a name that starts with digits is a name
    assert!("123.example".parse::<HostPattern>().is_ok());
    assert!("1a.2b.example".parse::<HostPattern>().is_ok());
}

/// A name is at most 253 bytes and a label at most 63 (RFC 1035), both sides of each limit pinned: a
/// rule one byte too strict would refuse a name a person may own, one too loose would let the
/// guard compare a name no resolver reads.
#[test]
fn a_name_may_be_253_bytes_and_a_label_63_and_no_more() {
    let label = |n: usize| "a".repeat(n);
    let name = |last: usize| format!("{0}.{0}.{0}.{1}", label(63), label(last));
    assert_eq!(name(61).len(), 253);
    assert_eq!(name(62).len(), 254);
    for pattern in [
        format!("{}.com", label(63)),
        name(61),
        format!("{}.", name(61)),
        format!("*.{}", name(61)),
        format!("*.{}.com", label(63)),
    ] {
        assert!(pattern.parse::<HostPattern>().is_ok(), "{pattern:?}");
    }
    for pattern in [
        format!("{}.com", label(64)),
        name(62),
        format!("{}.", name(62)),
        format!("*.{}", name(62)),
        format!("*.{}.com", label(64)),
    ] {
        assert!(pattern.parse::<HostPattern>().is_err(), "{pattern:?}");
    }
    // the same limits for the host of a URL
    let url = |host: &str| format!("https://{host}/mcp");
    assert!(EgressPolicy::new().check_url(&url(&name(61))).is_ok());
    assert!(EgressPolicy::new().check_url(&url(&name(62))).is_err());
    assert!(
        EgressPolicy::new()
            .check_url(&url(&format!("{}.com", label(63))))
            .is_ok()
    );
    assert!(
        EgressPolicy::new()
            .check_url(&url(&format!("{}.com", label(64))))
            .is_err()
    );
}

#[test]
fn a_pattern_that_is_none_of_the_three_is_refused() {
    for pattern in [
        "",
        " ",
        "*",
        "*.",
        "**.example.com",
        "*example.com",
        "a.*.example.com",
        "mcp.*",
        "*.*.example.com",
        "https://mcp.example.com",
        "mcp.example.com/mcp",
        "mcp.example.com:443",
        "user@mcp.example.com",
        "mcp example.com",
        "mcp.example.com\n",
        "a..b",
        ".example.com",
        "*..example.com",
        "*.10.0.0.1",
        "*.[::1]",
        "[::1",
    ] {
        assert!(pattern.parse::<HostPattern>().is_err(), "{pattern:?}");
    }
}

#[test]
fn the_deny_list_refuses_whatever_else() {
    let p = policy(
        &["*.example.com"],
        &["evil.example.com", "*.internal.example.com"],
    );
    assert_eq!(
        p.check_url("https://evil.example.com/").unwrap_err(),
        EgressRefusal::Denied
    );
    assert_eq!(
        p.check_url("https://db.internal.example.com/").unwrap_err(),
        EgressRefusal::Denied
    );
    // denied even though the allow list names it, and even when the allow list is empty
    assert_eq!(
        policy(&[], &["mcp.example.com"])
            .check_url("https://mcp.example.com/")
            .unwrap_err(),
        EgressRefusal::Denied
    );
    // a public host the lists do not mention is fine when the allow list is empty
    assert!(
        policy(&[], &["evil.example.com"])
            .check_url("https://mcp.example.net/")
            .is_ok()
    );
    // the deny list can name an address too
    assert_eq!(
        policy(&[], &["8.8.8.8"])
            .check_url("https://8.8.8.8/")
            .unwrap_err(),
        EgressRefusal::Denied
    );
}

#[test]
fn a_non_empty_allow_list_refuses_every_host_it_does_not_match() {
    let p = policy(&["mcp.example.com", "*.trusted.example.net"], &[]);
    assert!(p.check_url("https://mcp.example.com/mcp").is_ok());
    assert!(p.check_url("https://a.trusted.example.net/mcp").is_ok());
    for url in [
        "https://other.example.com/",
        "https://example.com/",
        "https://trusted.example.net/",
        "https://8.8.8.8/",
        "https://[2606:4700:4700::1111]/",
    ] {
        assert_eq!(
            p.check_url(url).unwrap_err(),
            EgressRefusal::NotAllowed,
            "{url}"
        );
    }
    // an empty allow list is every public host
    assert!(
        policy(&[], &[])
            .check_url("https://anything.example.org/")
            .is_ok()
    );
}

#[test]
fn a_host_named_by_the_allow_list_may_be_private_but_never_special() {
    let p = policy(
        &[
            "mcp.corp.example",
            "10.1.2.3",
            "169.254.169.254",
            "metadata.example",
        ],
        &[],
    );

    // a private-use address written in the URL, when an entry names it
    assert!(p.check_url("https://10.1.2.3/mcp").is_ok());
    // ...but only that one
    assert_eq!(
        p.check_url("https://10.1.2.4/mcp").unwrap_err(),
        EgressRefusal::NotAllowed
    );
    // the cloud metadata address and the other special ranges are no one's choice
    assert_eq!(
        p.check_url("https://169.254.169.254/").unwrap_err(),
        EgressRefusal::PrivateAddress
    );

    // a name resolves to what the connection is made to
    let target = p.check_url("https://mcp.corp.example/mcp").unwrap();
    let ok = p
        .check_resolved(
            &target,
            &[sock("10.0.0.7"), sock("192.168.1.9"), sock("127.0.0.1")],
        )
        .unwrap();
    assert_eq!(ok.len(), 3);
    for special in [
        "169.254.169.254",
        "fe80::1",
        "0.0.0.0",
        "224.0.0.1",
        "::ffff:169.254.169.254",
    ] {
        assert_eq!(
            p.check_resolved(&target, &[sock("10.0.0.7"), sock(special)])
                .unwrap_err(),
            EgressRefusal::PrivateAddress,
            "{special}"
        );
    }
    let metadata = p.check_url("https://metadata.example/").unwrap();
    assert_eq!(
        p.check_resolved(&metadata, &[sock("169.254.169.254")])
            .unwrap_err(),
        EgressRefusal::PrivateAddress
    );
}

#[test]
fn a_suffix_entry_admits_hosts_but_never_lifts_the_private_ban() {
    // `*.vercel.app` names every project of a hosting service, and anybody can make one that
    // resolves to 10.0.0.1: the pattern says which hosts may be asked, not which networks
    let p = policy(&["*.vercel.app"], &[]);
    let target = p.check_url("https://evil.vercel.app/mcp").unwrap();
    assert!(
        p.check_resolved(&target, &[sock("93.184.216.34")]).is_ok(),
        "a public address is admitted"
    );
    for private in [
        "10.0.0.1",
        "127.0.0.1",
        "192.168.1.1",
        "100.64.0.1",
        "fd00::1",
        "::1",
        "::ffff:10.0.0.1",
    ] {
        assert_eq!(
            p.check_resolved(&target, &[sock(private)]).unwrap_err(),
            EgressRefusal::PrivateAddress,
            "{private}"
        );
        assert_eq!(
            p.check_resolved(&target, &[sock("93.184.216.34"), sock(private)])
                .unwrap_err(),
            EgressRefusal::PrivateAddress,
            "{private}, after a public one"
        );
    }

    // an exact host entry is the operator naming that host: it may be private
    let exact = policy(&["intranet.vercel.app", "*.vercel.app"], &[]);
    let intranet = exact.check_url("https://intranet.vercel.app/mcp").unwrap();
    assert!(
        exact
            .check_resolved(&intranet, &[sock("10.0.0.1"), sock("192.168.1.1")])
            .is_ok()
    );
    // ...and the suffix next to it still does not
    let other = exact.check_url("https://evil.vercel.app/mcp").unwrap();
    assert_eq!(
        exact
            .check_resolved(&other, &[sock("10.0.0.1")])
            .unwrap_err(),
        EgressRefusal::PrivateAddress
    );
}

#[test]
fn a_suffix_entry_does_not_lift_the_ban_on_localhost_names_either() {
    let dev = policy(&["*.localhost"], &[]).allowing_http();
    assert_eq!(
        dev.check_url("http://app.localhost:8080/mcp").unwrap_err(),
        EgressRefusal::PrivateAddress
    );
    let named = policy(&["app.localhost"], &[]).allowing_http();
    assert!(named.check_url("http://app.localhost:8080/mcp").is_ok());
}

#[test]
fn an_address_entry_lifts_the_private_ban_for_that_address_only() {
    let p = policy(&["*.example.com", "10.0.0.5", "2001:4860:4860::8888"], &[]);
    let target = p.check_url("https://mcp.example.com/mcp").unwrap();
    assert!(p.check_resolved(&target, &[sock("10.0.0.5")]).is_ok());
    // in another spelling of the same address
    assert!(
        p.check_resolved(&target, &[sock("::ffff:10.0.0.5")])
            .is_ok()
    );
    for other in ["10.0.0.6", "10.0.0.4", "127.0.0.1"] {
        assert_eq!(
            p.check_resolved(&target, &[sock("10.0.0.5"), sock(other)])
                .unwrap_err(),
            EgressRefusal::PrivateAddress,
            "{other}"
        );
    }
    // an address entry never lifts a special range
    let metadata = policy(&["*.example.com", "169.254.169.254"], &[]);
    let target = metadata.check_url("https://mcp.example.com/").unwrap();
    assert_eq!(
        metadata
            .check_resolved(&target, &[sock("169.254.169.254")])
            .unwrap_err(),
        EgressRefusal::PrivateAddress
    );
}

// --------------------------------------------------------------------- the connection

#[test]
fn a_name_is_checked_on_what_it_resolves_to() {
    let p = policy(&[], &[]);
    let target = p.check_url("https://mcp.example.com/mcp").unwrap();

    let ok = p
        .check_resolved(
            &target,
            &[
                sock("93.184.216.34"),
                sock("2606:2800:220:1:248:1893:25c8:1946"),
            ],
        )
        .unwrap();
    assert_eq!(ok.as_slice().len(), 2);
    assert_eq!(ok.as_slice()[0], sock("93.184.216.34"));

    for private in [
        "127.0.0.1",
        "10.0.0.1",
        "172.16.5.5",
        "192.168.0.10",
        "100.100.100.100",
        "169.254.169.254",
        "0.0.0.0",
        "224.0.0.251",
        "255.255.255.255",
        "::1",
        "fd00::1",
        "fe80::1",
        "ff02::1",
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        "::127.0.0.1",
        "64:ff9b::7f00:1",
        "2002:a00:1::1",
        "2001:db8::1",
    ] {
        assert_eq!(
            p.check_resolved(&target, &[sock(private)]).unwrap_err(),
            EgressRefusal::PrivateAddress,
            "{private}"
        );
    }
}

#[test]
fn one_bad_address_among_good_ones_refuses_the_whole_answer() {
    // a resolver that answers a public and a private address is how rebinding is tried
    let p = policy(&[], &[]);
    let target = p.check_url("https://mcp.example.com/mcp").unwrap();
    for answer in [
        vec![sock("93.184.216.34"), sock("127.0.0.1")],
        vec![sock("127.0.0.1"), sock("93.184.216.34")],
        vec![sock("93.184.216.34"), sock("93.184.216.35"), sock("::1")],
    ] {
        assert_eq!(
            p.check_resolved(&target, &answer).unwrap_err(),
            EgressRefusal::PrivateAddress
        );
    }
}

#[test]
fn no_address_is_no_connection() {
    let p = policy(&[], &[]);
    let target = p.check_url("https://mcp.example.com/mcp").unwrap();
    assert_eq!(
        p.check_resolved(&target, &[]).unwrap_err(),
        EgressRefusal::NoAddress
    );
}

#[test]
fn the_lists_are_applied_again_at_the_connection() {
    // a definition stored under one policy is connected to under the current one
    let target = policy(&[], &[])
        .check_url("https://mcp.example.com/mcp")
        .unwrap();
    let tightened = policy(&[], &["mcp.example.com"]);
    assert_eq!(
        tightened
            .check_resolved(&target, &[sock("93.184.216.34")])
            .unwrap_err(),
        EgressRefusal::Denied
    );
    let narrowed = policy(&["other.example.com"], &[]);
    assert_eq!(
        narrowed
            .check_resolved(&target, &[sock("93.184.216.34")])
            .unwrap_err(),
        EgressRefusal::NotAllowed
    );
}

#[test]
fn an_address_written_in_the_url_is_checked_as_the_address_it_connects_to() {
    let p = policy(&[], &[]);
    let target = p.check_url("https://8.8.8.8:8443/mcp").unwrap();
    assert_eq!(
        target.literal_addr(),
        Some(SocketAddr::new(ip("8.8.8.8"), 8443))
    );
    let literal = target.literal_addr().unwrap();
    assert!(p.check_resolved(&target, &[literal]).is_ok());
    // the name of a target has no literal
    let named = p.check_url("https://mcp.example.com/").unwrap();
    assert_eq!(named.literal_addr(), None);
}

#[test]
fn a_refusal_names_the_rule_and_never_an_address_or_the_url() {
    let p = policy(&[], &["denied.example"]);
    let target = p.check_url("https://mcp.example.com/secret-path").unwrap();
    let mut refusals = vec![
        p.check_resolved(&target, &[sock("169.254.169.254")])
            .unwrap_err(),
        p.check_resolved(&target, &[]).unwrap_err(),
        p.check_url("https://10.9.8.7/secret-path").unwrap_err(),
        p.check_url("https://denied.example/secret-path")
            .unwrap_err(),
        p.check_url("http://mcp.example.com/secret-path")
            .unwrap_err(),
        p.check_url("https://user:hunter2@mcp.example.com/secret-path")
            .unwrap_err(),
        p.check_url("https://mcp.example.com/secret-path?token=hunter2")
            .unwrap_err(),
        p.check_url("https://mcp.example.com/secret-path#hunter2")
            .unwrap_err(),
        p.check_url("https://mcp.example.com:0/secret-path")
            .unwrap_err(),
        policy(&["other.example"], &[])
            .check_url("https://mcp.example.com/secret-path")
            .unwrap_err(),
    ];
    refusals.push(p.check_url("not a url hunter2").unwrap_err());
    for refusal in refusals {
        let said = format!("{refusal} {refusal:?} {}", refusal.rule());
        for forbidden in [
            "169.254",
            "10.9.8.7",
            "hunter2",
            "secret-path",
            "mcp.example.com",
        ] {
            assert!(!said.contains(forbidden), "{forbidden} is in: {said}");
        }
        assert!(!refusal.rule().is_empty());
        assert_eq!(
            refusal.class(),
            if refusal == EgressRefusal::NoAddress {
                ErrorClass::Transient
            } else {
                ErrorClass::Invalid
            }
        );
    }
}

#[test]
fn what_a_person_is_told_does_not_tell_a_private_address_from_no_address() {
    // the precise rule is for the operator's log; the person gets a reason that is the same whether
    // a name resolves to an internal address or to nothing, so the form cannot map the network
    let internal = EgressRefusal::PrivateAddress;
    let nothing = EgressRefusal::NoAddress;
    assert_ne!(internal.rule(), nothing.rule());
    assert_eq!(internal.public_reason(), nothing.public_reason());
    assert_eq!(internal.public_rule(), nothing.public_rule());
    // the class too: a status chosen from it must not tell them apart either
    assert_ne!(internal.class(), nothing.class());
    assert_eq!(internal.public_class(), nothing.public_class());
    for refusal in [internal, nothing] {
        let said = format!("{} {}", refusal.public_reason(), refusal.public_rule());
        for forbidden in ["private", "reserved", "loopback", "resolve", "address"] {
            assert!(
                !said.to_lowercase().contains(forbidden),
                "{forbidden} is in: {said}"
            );
        }
    }
    // every other refusal is told as it is, with a reason and a code of its own
    let all = [
        EgressRefusal::TooLong,
        EgressRefusal::NotAUrl,
        EgressRefusal::Scheme,
        EgressRefusal::Host,
        EgressRefusal::UserInfo,
        EgressRefusal::Query,
        EgressRefusal::Fragment,
        EgressRefusal::Port,
        EgressRefusal::Denied,
        EgressRefusal::NotAllowed,
    ];
    for refusal in all {
        assert_eq!(refusal.public_rule(), refusal.rule());
        assert_eq!(refusal.public_class(), refusal.class());
        assert_eq!(refusal.public_reason(), refusal.to_string());
    }
    let mut codes: Vec<&str> = all.iter().map(|r| r.public_rule()).collect();
    codes.push(internal.public_rule());
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), all.len() + 1);
}

#[test]
fn a_target_prints_its_scheme_host_and_port_and_never_its_path() {
    // a path can carry a secret (a webhook-style URL, a token in a segment)
    let p = policy(&[], &[]);
    let target = p
        .check_url("https://mcp.example.com:8443/hook/s3cr3t-segment/mcp")
        .unwrap();
    let said = format!("{target:?} {target:#?}");
    assert!(
        !said.contains("s3cr3t-segment") && !said.contains("/hook"),
        "{said}"
    );
    assert!(
        said.contains("https") && said.contains("mcp.example.com") && said.contains("8443"),
        "{said}"
    );
    // and still the whole normal form where a caller asks for it
    assert_eq!(
        target.url(),
        "https://mcp.example.com:8443/hook/s3cr3t-segment/mcp"
    );
    let dev = EgressPolicy::new().allowing_http();
    let said = format!("{:?}", dev.check_url("http://8.8.8.8/s3cr3t").unwrap());
    assert!(said.contains("http") && said.contains("8.8.8.8") && !said.contains("s3cr3t"));
}

#[test]
fn each_refusal_has_a_rule_of_its_own() {
    let all = [
        EgressRefusal::TooLong,
        EgressRefusal::NotAUrl,
        EgressRefusal::Scheme,
        EgressRefusal::Host,
        EgressRefusal::UserInfo,
        EgressRefusal::Query,
        EgressRefusal::Fragment,
        EgressRefusal::Port,
        EgressRefusal::Denied,
        EgressRefusal::NotAllowed,
        EgressRefusal::PrivateAddress,
        EgressRefusal::NoAddress,
    ];
    let mut rules: Vec<&str> = all.iter().map(|r| r.rule()).collect();
    rules.sort_unstable();
    rules.dedup();
    assert_eq!(rules.len(), all.len());
}

#[test]
fn the_default_policy_is_https_only_public_hosts_only() {
    let p = EgressPolicy::default();
    assert!(p.check_url("https://mcp.example.com/").is_ok());
    assert_eq!(
        p.check_url("http://mcp.example.com/").unwrap_err(),
        EgressRefusal::Scheme
    );
    assert_eq!(
        p.check_url("https://10.0.0.1/").unwrap_err(),
        EgressRefusal::PrivateAddress
    );
}

#[test]
fn a_list_with_a_bad_pattern_is_not_a_policy() {
    assert!(EgressPolicy::from_lists(["*"], std::iter::empty::<&str>()).is_err());
    assert!(EgressPolicy::from_lists(std::iter::empty::<&str>(), ["https://x.example"]).is_err());
}

// ----------------------------------------------------- the lists, judged on addresses too

/// Every way to write 8.8.8.8 as an IPv6 address that a connection to it can come as.
const EIGHT_EIGHT_IN_V6: [&str; 6] = [
    "::ffff:8.8.8.8",
    "::ffff:808:808",
    "::8.8.8.8",
    "::ffff:0:808:808",
    "64:ff9b::808:808",
    "2002:808:808::1",
];

#[test]
fn a_name_that_resolves_to_a_denied_address_is_refused() {
    let p = policy(&[], &["8.8.8.8"]);
    // the name is not on the list; the address it resolved to is
    let target = p.check_url("https://dns.example.com/mcp").unwrap();
    assert_eq!(
        p.check_resolved(&target, &[sock("8.8.8.8")]).unwrap_err(),
        EgressRefusal::Denied
    );
    assert!(p.check_resolved(&target, &[sock("8.8.4.4")]).is_ok());
    // one denied address among others refuses the answer, and deny wins over any other rule
    for answer in [
        vec![sock("93.184.216.34"), sock("8.8.8.8")],
        vec![sock("10.0.0.1"), sock("8.8.8.8")],
        vec![sock("8.8.8.8"), sock("10.0.0.1")],
    ] {
        assert_eq!(
            p.check_resolved(&target, &answer).unwrap_err(),
            EgressRefusal::Denied,
            "{answer:?}"
        );
    }
}

#[test]
fn a_denied_address_is_denied_in_every_spelling() {
    let p = policy(&[], &["8.8.8.8"]);
    let target = p.check_url("https://dns.example.com/mcp").unwrap();
    for spelling in EIGHT_EIGHT_IN_V6 {
        assert_eq!(
            p.check_resolved(&target, &[sock(spelling)]).unwrap_err(),
            EgressRefusal::Denied,
            "resolved {spelling}"
        );
        assert_eq!(
            p.check_url(&format!("https://[{spelling}]/mcp"))
                .unwrap_err(),
            EgressRefusal::Denied,
            "written [{spelling}]"
        );
    }
    // the ways the URL parser reads the same address
    for url in [
        "https://8.8.8.8/",
        "https://134744072/",
        "https://0x8.8.8.8/",
        "https://8.8.8.8./",
        "https://8.526344/",
    ] {
        assert_eq!(
            p.check_url(url).unwrap_err(),
            EgressRefusal::Denied,
            "{url}"
        );
    }

    // an entry written as an IPv6 address that carries the IPv4 one denies the IPv4 one too
    let v6 = policy(&[], &["[::ffff:808:808]"]);
    assert_eq!(
        v6.check_url("https://8.8.8.8/").unwrap_err(),
        EgressRefusal::Denied
    );
    assert_eq!(
        v6.check_resolved(&target, &[sock("8.8.8.8")]).unwrap_err(),
        EgressRefusal::Denied
    );
    // and an entry for an address is for that address, not for its neighbours
    assert!(p.check_url("https://8.8.4.4/").is_ok());
    assert!(p.check_url("https://[2606:4700:4700::1111]/").is_ok());
}

#[test]
fn a_denied_private_address_is_denied_even_where_the_allow_list_lifts_the_ban() {
    let p = policy(&["intranet.example"], &["10.0.0.9"]);
    let target = p.check_url("https://intranet.example/mcp").unwrap();
    assert!(p.check_resolved(&target, &[sock("10.0.0.8")]).is_ok());
    assert_eq!(
        p.check_resolved(&target, &[sock("10.0.0.8"), sock("10.0.0.9")])
            .unwrap_err(),
        EgressRefusal::Denied
    );
    assert_eq!(
        p.check_resolved(&target, &[sock("::ffff:10.0.0.9")])
            .unwrap_err(),
        EgressRefusal::Denied
    );
}

#[test]
fn an_allow_entry_for_an_address_matches_what_a_name_resolved_to() {
    // the list names addresses only: a name cannot be judged until it is resolved
    let p = policy(&["93.184.216.34"], &[]);
    let target = p.check_url("https://mcp.example.com/mcp").unwrap();
    assert!(p.check_resolved(&target, &[sock("93.184.216.34")]).is_ok());
    assert!(
        p.check_resolved(&target, &[sock("::ffff:93.184.216.34")])
            .is_ok(),
        "in another spelling"
    );
    // a name that resolves anywhere else is not on the list; so is an answer that is only partly on it
    assert_eq!(
        p.check_resolved(&target, &[sock("93.184.216.35")])
            .unwrap_err(),
        EgressRefusal::NotAllowed
    );
    assert_eq!(
        p.check_resolved(&target, &[sock("93.184.216.34"), sock("93.184.216.35")])
            .unwrap_err(),
        EgressRefusal::NotAllowed
    );
    // an address written in the URL is matched by the same entry, in any spelling
    assert!(p.check_url("https://93.184.216.34/").is_ok());
    assert!(p.check_url("https://[::ffff:5db8:d822]/").is_ok());
    assert_eq!(
        p.check_url("https://93.184.216.35/").unwrap_err(),
        EgressRefusal::NotAllowed
    );

    // with only names on the list, a name that is not on it is refused before it is resolved
    let names = policy(&["mcp.other.example"], &[]);
    assert_eq!(
        names.check_url("https://mcp.example.com/").unwrap_err(),
        EgressRefusal::NotAllowed
    );
}

#[test]
fn a_name_on_the_allow_list_is_not_also_asked_to_resolve_to_a_listed_address() {
    let p = policy(&["mcp.example.com", "93.184.216.34"], &[]);
    let target = p.check_url("https://mcp.example.com/mcp").unwrap();
    assert!(p.check_resolved(&target, &[sock("1.1.1.1")]).is_ok());
}

// ------------------------------------------------------------- against the standard library

#[test]
fn no_ipv4_address_the_standard_library_calls_special_is_public() {
    // every /16, at five places inside it: 330 thousand addresses, a few milliseconds
    let tails = [(0, 0), (0, 1), (128, 128), (1, 1), (255, 255)];
    for a in 0..=255u8 {
        for b in 0..=255u8 {
            for (c, d) in tails {
                let address = std::net::Ipv4Addr::new(a, b, c, d);
                let std_says_not_public = address.is_private()
                    || address.is_loopback()
                    || address.is_link_local()
                    || address.is_multicast()
                    || address.is_broadcast()
                    || address.is_documentation()
                    || address.is_unspecified();
                if std_says_not_public {
                    assert!(!is_public_address(IpAddr::V4(address)), "{address}");
                }
                // and the other way for the ranges the standard library does not know: the
                // carrier-grade NAT block, the reserved block and benchmarking
                if (a == 100 && (64..=127).contains(&b))
                    || a >= 240
                    || (a == 198 && (18..=19).contains(&b))
                {
                    assert!(!is_public_address(IpAddr::V4(address)), "{address}");
                }
            }
        }
    }
}

#[test]
fn no_ipv6_address_the_standard_library_calls_special_is_public() {
    let tails = [
        [0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 1],
        [0, 0, 0, 0, 0xffff, 0x7f00, 1],
        [1, 2, 3, 4, 5, 6, 7],
        [0xffff; 7],
    ];
    for first in 0..=0xffffu16 {
        for tail in tails {
            let address = std::net::Ipv6Addr::new(
                first, tail[0], tail[1], tail[2], tail[3], tail[4], tail[5], tail[6],
            );
            if address.is_unspecified()
                || address.is_loopback()
                || address.is_multicast()
                || address.is_unique_local()
                || address.is_unicast_link_local()
            {
                assert!(!is_public_address(IpAddr::V6(address)), "{address}");
            }
        }
    }
}

#[test]
fn an_address_is_judged_the_same_in_every_form_it_is_given() {
    // IPv4 as itself, mapped, compatible, translated, NAT64 and 6to4: one verdict for all of them
    for text in [
        "8.8.8.8",
        "1.1.1.1",
        "10.1.2.3",
        "127.0.0.1",
        "169.254.169.254",
        "172.20.0.9",
        "192.168.5.5",
        "100.64.9.9",
        "198.18.0.1",
        "224.9.9.9",
        "0.1.2.3",
    ] {
        let v4: std::net::Ipv4Addr = text.parse().unwrap();
        let o = v4.octets();
        let hi = u16::from_be_bytes([o[0], o[1]]);
        let lo = u16::from_be_bytes([o[2], o[3]]);
        let forms = [
            std::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, hi, lo),
            std::net::Ipv6Addr::new(0, 0, 0, 0, 0xffff, 0, hi, lo),
            std::net::Ipv6Addr::new(0x64, 0xff9b, 0, 0, 0, 0, hi, lo),
            std::net::Ipv6Addr::new(0x2002, hi, lo, 0, 0, 0, 0, 1),
            std::net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, hi, lo),
        ];
        let want = classify_address(IpAddr::V4(v4));
        for form in forms {
            // `::a.b.c.d` for a small address is `::1`-like: those are special either way
            if form == std::net::Ipv6Addr::LOCALHOST {
                continue;
            }
            assert_eq!(
                classify_address(IpAddr::V6(form)),
                want,
                "{form} for {text}"
            );
        }
    }
}

// ------------------------------------------------------------------- a resolver hook's verdict

#[test]
fn a_connection_is_judged_on_the_addresses_the_host_resolved_to() {
    let policy = EgressPolicy::new();
    let ok = policy
        .check_connection("mcp.example.com", &[ip("93.184.216.34")])
        .unwrap();
    assert_eq!(ok.as_slice(), [SocketAddr::new(ip("93.184.216.34"), 0)]);

    // a public name that answers a private address, the metadata address or loopback is refused,
    // and so is an answer that mixes a public address with one that is not (rebinding)
    for private in ["10.0.0.5", "127.0.0.1", "169.254.169.254", "::1", "fd00::7"] {
        assert_eq!(
            policy.check_connection("mcp.example.com", &[ip(private)]),
            Err(EgressRefusal::PrivateAddress),
            "{private}"
        );
        assert_eq!(
            policy.check_connection("mcp.example.com", &[ip("93.184.216.34"), ip(private)]),
            Err(EgressRefusal::PrivateAddress),
            "a mixed answer with {private}"
        );
    }
    assert_eq!(
        policy.check_connection("mcp.example.com", &[]),
        Err(EgressRefusal::NoAddress)
    );
}

#[test]
fn a_connection_to_an_address_is_judged_with_or_without_brackets() {
    let policy = EgressPolicy::new();
    for host in ["::1", "[::1]"] {
        assert_eq!(
            policy.check_connection(host, &[ip("::1")]),
            Err(EgressRefusal::PrivateAddress),
            "{host}"
        );
    }
    assert!(
        policy
            .check_connection("[2606:4700:4700::1111]", &[ip("2606:4700:4700::1111")])
            .is_ok()
    );
    assert_eq!(
        policy.check_connection("127.0.0.1", &[ip("127.0.0.1")]),
        Err(EgressRefusal::PrivateAddress)
    );
}

#[test]
fn the_lists_decide_a_connection_as_they_decide_a_url() {
    // an exact entry lifts the ban for that name's private addresses, and only for it
    let allowed = policy(&["intranet.example.com"], &[]);
    assert!(
        allowed
            .check_connection("intranet.example.com", &[ip("10.1.2.3")])
            .is_ok()
    );
    assert_eq!(
        allowed.check_connection("other.example.com", &[ip("93.184.216.34")]),
        Err(EgressRefusal::NotAllowed)
    );
    // a suffix entry admits and never lifts
    let suffix = policy(&["*.example.com"], &[]);
    assert!(
        suffix
            .check_connection("a.example.com", &[ip("93.184.216.34")])
            .is_ok()
    );
    assert_eq!(
        suffix.check_connection("a.example.com", &[ip("10.1.2.3")]),
        Err(EgressRefusal::PrivateAddress)
    );
    // deny wins, for the name and for an address in any spelling
    let denied = policy(&[], &["bad.example.com", "8.8.8.8"]);
    assert_eq!(
        denied.check_connection("bad.example.com", &[ip("93.184.216.34")]),
        Err(EgressRefusal::Denied)
    );
    assert_eq!(
        denied.check_connection("good.example.com", &[ip("::ffff:808:808")]),
        Err(EgressRefusal::Denied)
    );
    // `localhost` is always loopback, unless the operator names it exactly
    assert_eq!(
        EgressPolicy::new().check_connection("localhost", &[ip("127.0.0.1")]),
        Err(EgressRefusal::PrivateAddress)
    );
    assert!(
        policy(&["localhost"], &[])
            .check_connection("localhost", &[ip("127.0.0.1")])
            .is_ok()
    );
}

#[test]
fn a_host_the_guard_cannot_read_is_refused() {
    assert_eq!(
        EgressPolicy::new().check_connection("", &[ip("93.184.216.34")]),
        Err(EgressRefusal::NotAUrl)
    );
    assert!(
        EgressPolicy::new()
            .check_connection("a b", &[ip("93.184.216.34")])
            .is_err()
    );
}

// ----------------------------------------------------------------------------- the lookup verdict

#[test]
fn a_lookup_that_failed_is_not_resolved_and_one_the_guard_refuses_carries_the_rule() {
    let open = policy(&[], &[]);
    // the answer passes: the hook hands the client these addresses
    let passed = judge_lookup(&open, "mcp.example.test", Ok(vec![ip("93.184.216.34")])).unwrap();
    assert_eq!(passed.as_slice(), [SocketAddr::new(ip("93.184.216.34"), 0)]);
    // a mixed answer is refused whole, with the rule that refused it
    assert_eq!(
        judge_lookup(
            &open,
            "mcp.example.test",
            Ok(vec![ip("93.184.216.34"), ip("10.0.0.7")])
        ),
        Err(LookupFailure::Refused(EgressRefusal::PrivateAddress))
    );
    // an empty answer is the guard's `NoAddress`; a failed lookup is not a refusal at all
    assert_eq!(
        judge_lookup(&open, "mcp.example.test", Ok(Vec::new())),
        Err(LookupFailure::Refused(EgressRefusal::NoAddress))
    );
    assert_eq!(
        judge_lookup(
            &open,
            "mcp.example.test",
            Err(std::io::Error::other("no resolver"))
        ),
        Err(LookupFailure::NotResolved)
    );
    // an exact allow entry lifts the ban on the private address its name resolves to
    let lifted = policy(&["mcp-search"], &[]);
    assert!(judge_lookup(&lifted, "mcp-search", Ok(vec![ip("10.1.2.3")])).is_ok());
    // the failure's words name no host and no address
    for failure in [
        LookupFailure::NotResolved,
        LookupFailure::TimedOut,
        LookupFailure::Refused(EgressRefusal::PrivateAddress),
    ] {
        let shown = format!("{failure} {failure:?}");
        assert!(
            !shown.contains("example") && !shown.contains("10."),
            "{shown}"
        );
    }
}

#[test]
fn the_lookup_timeout_is_shorter_than_the_clients_connect_timeout() {
    // the HTTP clients connect within 10 s: a name that does not answer is told apart from a server
    // that does not accept
    assert!(LOOKUP_TIMEOUT < std::time::Duration::from_secs(10));
}
