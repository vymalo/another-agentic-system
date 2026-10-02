//! A corpus of hostile and ordinary SVG (the payloads of the OWASP XSS filter evasion lists, the
//! SVG ones, and the usual XML attacks): what the sanitizer drops, what it keeps, and the
//! properties that hold for every one of them.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_svg_clean::{ATTRIBUTES, ELEMENTS, SvgError, clean};
use quick_xml::Reader;
use quick_xml::events::Event;

const NS: &str = r#"xmlns="http://www.w3.org/2000/svg""#;

fn svg(inner: &str) -> String {
    format!(
        r#"<svg {NS} xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20">{inner}</svg>"#
    )
}

fn cleaned(input: &str) -> String {
    String::from_utf8(clean(input.as_bytes()).unwrap()).unwrap()
}

/// What no output of the sanitizer may contain, whatever the input was (lower case).
const NEVER: &[&str] = &[
    "<script",
    "javascript:",
    "onload",
    "onclick",
    "onerror",
    "onmouseover",
    "onbegin",
    "onfocus",
    "foreignobject",
    "<iframe",
    "<embed",
    "<object",
    "@import",
    "evil.example",
    "<!doctype",
    "<!entity",
    "<!--",
    "<?",
    "cdata",
    "<animate",
    "<set ",
    "<image",
    "<a ",
    "data:text/html",
    "data:image/svg",
];

/// Reads `output` back and checks it only has what the lists allow.
fn assert_allowed(output: &str) {
    let mut reader = Reader::from_reader(output.as_bytes());
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader
            .read_event_into(&mut buf)
            .expect("the output is well-formed XML")
        {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let name = std::str::from_utf8(e.name().as_ref()).unwrap().to_owned();
                assert!(
                    ELEMENTS.contains(&name.as_str()),
                    "element {name} in {output}"
                );
                for attr in e.attributes() {
                    let attr = attr.unwrap();
                    let key = std::str::from_utf8(attr.key.as_ref()).unwrap();
                    let value = attr
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .unwrap();
                    match key {
                        "href" | "xlink:href" => {
                            assert!(value.starts_with('#'), "{key}={value} in {output}");
                        }
                        "xmlns" | "xmlns:xlink" => {}
                        "style" => {
                            let v = value.to_ascii_lowercase();
                            assert!(!v.contains("url(") && !v.contains("@import"), "{output}");
                        }
                        _ => assert!(ATTRIBUTES.contains(&key), "attribute {key} in {output}"),
                    }
                }
            }
            Event::Text(_) | Event::GeneralRef(_) | Event::End(_) => {}
            other => panic!("{other:?} in {output}"),
        }
    }
}

/// Every payload: cleaned, and then the properties of every output hold.
fn hostile() -> Vec<(&'static str, String)> {
    let drop = |inner: &str| svg(inner);
    vec![
        ("script element", drop("<script>alert(1)</script>")),
        (
            "script with a data href",
            drop(r#"<script xlink:href="data:text/javascript,alert(1)"/>"#),
        ),
        (
            "script in CDATA",
            drop("<script><![CDATA[alert(1)]]></script>"),
        ),
        ("upper case script", drop("<SCRIPT>alert(1)</SCRIPT>")),
        (
            "prefixed script",
            drop(r#"<svg:script xmlns:svg="http://www.w3.org/2000/svg">alert(1)</svg:script>"#),
        ),
        (
            "xhtml script",
            drop(
                r#"<html:script xmlns:html="http://www.w3.org/1999/xhtml">alert(1)</html:script>"#,
            ),
        ),
        (
            "onload on the root",
            format!(r#"<svg {NS} onload="alert(1)"><rect width="1" height="1"/></svg>"#),
        ),
        (
            "onclick on a child",
            drop(r#"<rect width="9" height="9" onclick="alert(1)"/>"#),
        ),
        (
            "onmouseover, upper case",
            drop(r#"<rect width="9" height="9" ONMOUSEOVER="alert(1)"/>"#),
        ),
        (
            "onerror on a use",
            drop(r##"<use href="#a" onerror="alert(1)"/>"##),
        ),
        (
            "foreignObject with an iframe",
            drop(
                r#"<foreignObject width="9" height="9"><iframe srcdoc="&lt;script&gt;alert(1)&lt;/script&gt;"/></foreignObject>"#,
            ),
        ),
        (
            "foreignObject with a body",
            drop(
                r#"<foreignObject><body xmlns="http://www.w3.org/1999/xhtml" onload="alert(1)"/></foreignObject>"#,
            ),
        ),
        (
            "a javascript link",
            drop(r#"<a xlink:href="javascript:alert(1)"><text>x</text></a>"#),
        ),
        (
            "an entity-encoded javascript link",
            drop(r#"<a href="&#106;avascript:alert(1)"><text>x</text></a>"#),
        ),
        (
            "a javascript link with a tab",
            drop("<a href=\"java&#9;script:alert(1)\"><text>x</text></a>"),
        ),
        (
            "a use of a remote document",
            drop(r#"<use href="http://evil.example/x.svg#a"/>"#),
        ),
        (
            "a use of a data document",
            drop(r#"<use xlink:href="data:image/svg+xml;base64,PHN2Zz48L3N2Zz4="/>"#),
        ),
        ("a use with no reference", drop("<use/>")),
        (
            "animate writing an href",
            drop(
                r#"<a><animate attributeName="href" values="javascript:alert(1)"/><text>x</text></a>"#,
            ),
        ),
        (
            "set writing a handler",
            drop(
                r#"<rect width="9" height="9"><set attributeName="onmouseover" to="alert(1)"/></rect>"#,
            ),
        ),
        (
            "animateTransform with onbegin",
            drop(r#"<g><animateTransform onbegin="alert(1)" attributeName="transform"/></g>"#),
        ),
        (
            "an image of a remote file",
            drop(r#"<image href="http://evil.example/x.png" width="9" height="9"/>"#),
        ),
        (
            "an image of a data URI",
            drop(r#"<image href="data:image/svg+xml;base64,AAAA" width="9" height="9"/>"#),
        ),
        (
            "a style attribute with url",
            drop(r#"<rect width="9" height="9" style="fill:url(http://evil.example/x)"/>"#),
        ),
        (
            "a style attribute with javascript",
            drop(r#"<rect width="9" height="9" style="background:url(javascript:alert(1))"/>"#),
        ),
        (
            "a style attribute with an expression",
            drop(r#"<rect width="9" height="9" style="width:expression(alert(1))"/>"#),
        ),
        (
            "a style attribute with an escape",
            drop(r#"<rect width="9" height="9" style="fill:\75rl(http://evil.example)"/>"#),
        ),
        (
            "a style with an import",
            drop("<style>@import url(http://evil.example/x.css);</style>"),
        ),
        (
            "a style with a url",
            drop("<style>rect{fill:url(http://evil.example/x)}</style>"),
        ),
        (
            "a style with an import hidden by a comment",
            drop("<style>@im/**/port 'x'; rect{fill:u/**/rl(http://evil.example)}</style>"),
        ),
        (
            "a style with a binding",
            drop("<style>rect{-moz-binding:url(http://evil.example/x.xml#a)}</style>"),
        ),
        (
            "an element inside a style",
            drop("<style><script>alert(1)</script></style>"),
        ),
        (
            "a fill that is a remote url",
            drop(r#"<rect width="9" height="9" fill="url(http://evil.example/x#a)"/>"#),
        ),
        (
            "a filter that is a remote url",
            drop(r#"<rect width="9" height="9" filter="url('http://evil.example/x.svg#f')"/>"#),
        ),
        (
            "a comment that closes early",
            drop("<!--><script>alert(1)</script>-->"),
        ),
        (
            "a comment with a script",
            drop("<!-- <script>alert(1)</script> -->"),
        ),
        (
            "a processing instruction",
            drop(r#"<?xml-stylesheet href="http://evil.example/x.css"?>"#),
        ),
        (
            "a feImage of a remote file",
            drop(r#"<filter id="f"><feImage href="http://evil.example/x.png"/></filter>"#),
        ),
        (
            "an unknown element and its children",
            drop("<widget><rect width=\"9\" height=\"9\"/></widget>"),
        ),
        (
            "text that looks like markup",
            drop("<text>&lt;script&gt;alert(1)&lt;/script&gt;</text>"),
        ),
        (
            "a CDATA of a script in a title",
            drop("<title><![CDATA[<script>alert(1)</script>]]></title>"),
        ),
        (
            "an external entity",
            format!(
                r#"<?xml version="1.0"?><!DOCTYPE svg [<!ENTITY xxe SYSTEM "file:///etc/passwd">]><svg {NS}><text>&xxe;</text></svg>"#
            ),
        ),
        (
            "an entity that expands",
            format!(
                r#"<!DOCTYPE svg [<!ENTITY a "<script>alert(1)</script>">]><svg {NS}>&a;</svg>"#
            ),
        ),
        (
            "a billion laughs",
            format!(
                r#"<!DOCTYPE svg [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;">]><svg {NS}><text>&c;</text></svg>"#
            ),
        ),
        (
            "a second root",
            format!(r#"<svg {NS}/><script>alert(1)</script>"#),
        ),
        (
            "another namespace on the root's child",
            drop(r#"<x:y xmlns:x="http://example.com/"><rect width="9" height="9"/></x:y>"#),
        ),
        (
            "an xlink show",
            drop(r#"<rect width="9" height="9" xlink:show="new" xlink:actuate="onLoad"/>"#),
        ),
        (
            "another xmlns",
            format!(r#"<svg {NS} xmlns:ev="http://www.w3.org/2001/xml-events" ev:event="load"/>"#),
        ),
    ]
}

#[test]
fn nothing_hostile_survives_and_every_output_has_only_what_the_lists_allow() {
    for (name, input) in hostile() {
        let out = match clean(input.as_bytes()) {
            Ok(out) => String::from_utf8(out).unwrap(),
            // a refusal is as good: nothing is served
            Err(_) => continue,
        };
        let lower = out.to_ascii_lowercase();
        for never in NEVER {
            assert!(
                !lower.contains(never),
                "{name}: {never:?} survived in {out}"
            );
        }
        assert_allowed(&out);
        // and cleaning is stable: what is left is already clean
        assert_eq!(cleaned(&out), out, "{name}");
    }
}

#[test]
fn specific_payloads_are_gone_and_the_rest_of_the_drawing_stays() {
    let out = cleaned(&svg(
        r#"<circle cx="5" cy="5" r="4" fill="teal" onclick="alert(1)"/><script>alert(1)</script><rect width="3" height="3"/>"#,
    ));
    assert!(
        out.contains(r#"<circle cx="5" cy="5" r="4" fill="teal"/>"#),
        "{out}"
    );
    assert!(out.contains(r#"<rect width="3" height="3"/>"#), "{out}");
    assert!(!out.contains("script") && !out.contains("onclick"), "{out}");

    // a dropped element takes its subtree, text and all
    let out = cleaned(&svg(
        "<foreignObject><div>secret text</div></foreignObject><rect width=\"1\" height=\"1\"/>",
    ));
    assert!(!out.contains("secret") && out.contains("<rect"), "{out}");

    // text that looks like markup stays text
    let out = cleaned(&svg("<text>&lt;script&gt;alert(1)&lt;/script&gt;</text>"));
    assert!(
        out.contains("&lt;script&gt;alert(1)&lt;/script&gt;") && !out.contains("<script"),
        "{out}"
    );

    // an unknown entity is dropped, never expanded
    let out = cleaned(&format!(
        r#"<!DOCTYPE svg [<!ENTITY xxe SYSTEM "file:///etc/passwd">]><svg {NS}><text>a&xxe;b</text></svg>"#
    ));
    assert!(out.contains("<text>ab</text>"), "{out}");
}

#[test]
fn ordinary_drawings_are_kept() {
    let kept = [
        r##"<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#fff"/><stop offset="1" stop-color="#000"/></linearGradient></defs><rect width="9" height="9" fill="url(#g)"/>"##,
        r##"<defs><circle id="c" r="2"/></defs><use href="#c" x="4" y="4"/><use xlink:href="#c" x="8" y="8"/>"##,
        r##"<g transform="translate(2 2) rotate(45)"><path d="M0 0 L9 0 L9 9 Z" stroke="black" stroke-width="2" fill="none"/></g>"##,
        r##"<text x="1" y="9" font-family="serif" font-size="8" text-anchor="middle">Q1 &amp; Q2 <tspan font-weight="bold">sales</tspan></text>"##,
        r##"<style>rect{fill:red} .a > circle{stroke:blue}</style><rect class="a" width="3" height="3"/>"##,
        r##"<rect width="3" height="3" style="fill:red;stroke:#00f"/>"##,
        r##"<defs><filter id="f"><feGaussianBlur stdDeviation="2"/></filter></defs><rect width="9" height="9" filter="url(#f)"/>"##,
        r##"<title>Chart</title><desc>A chart</desc><polygon points="0,0 9,0 9,9"/>"##,
    ];
    for inner in kept {
        let input = svg(inner);
        let out = cleaned(&input);
        assert_allowed(&out);
        // nothing of these was dropped: the same drawing comes back (the one with a `>` in a
        // stylesheet has it escaped again)
        if !inner.contains("<style>") {
            assert_eq!(out, input);
        }
        assert_eq!(cleaned(&out), out);
    }
    let out = cleaned(&svg(r##"<rect width="9" height="9" fill="url( '#g' )"/>"##));
    assert!(out.contains("fill=\"url( '#g' )\""), "{out}");
    let out = cleaned(&svg("<style>rect{fill:red}</style>"));
    assert!(out.contains("<style>rect{fill:red}</style>"), "{out}");
}

#[test]
fn an_exact_roundtrip_of_a_small_drawing() {
    let input = svg(r##"<g id="a"><circle cx="1" cy="1" r="1"/><text>a &lt; b</text></g>"##);
    assert_eq!(cleaned(&input), input);
}

#[test]
fn what_is_not_an_svg_is_an_error() {
    let not_xml: [&[u8]; 3] = [
        b"<svg",
        b"<svg><g></svg>",
        b"<svg width=\"1\" width=\"2\"/>",
    ];
    for input in not_xml {
        assert_eq!(
            clean(input),
            Err(SvgError::Malformed),
            "{:?}",
            String::from_utf8_lossy(input)
        );
    }
    // not UTF-8: never read as markup
    assert_eq!(clean(b"<svg \xff/>"), Err(SvgError::Malformed));
    assert_eq!(
        clean(b"\xff\xfe<\0s\0v\0g\0/\0>\0"),
        Err(SvgError::Malformed)
    );
    // well-formed, but nothing of it is an svg
    for input in [
        "",
        "hello",
        "<html><script>alert(1)</script></html>",
        "<g/>",
        "<SVG/>",
        "<?xml version=\"1.0\"?>",
    ] {
        assert_eq!(clean(input.as_bytes()), Err(SvgError::NotSvg), "{input}");
    }
}

#[test]
fn nesting_is_bounded() {
    let deep = |n: usize| format!("{}{}", "<g>".repeat(n), "</g>".repeat(n));
    // 1 (svg) + 62 groups is inside the bound, 1 + 64 is not
    assert!(clean(svg(&deep(62)).as_bytes()).is_ok());
    assert_eq!(clean(svg(&deep(64)).as_bytes()), Err(SvgError::TooDeep));
    // a bomb of unknown elements is as deep as any other
    let bomb = format!(
        "<svg {NS}>{}{}</svg>",
        "<x>".repeat(200),
        "</x>".repeat(200)
    );
    assert_eq!(clean(bomb.as_bytes()), Err(SvgError::TooDeep));
}

#[test]
fn a_big_flat_drawing_is_cleaned() {
    let rects = r#"<rect width="1" height="1"/>"#.repeat(20_000);
    let out = cleaned(&svg(&rects));
    assert_eq!(out.matches("<rect").count(), 20_000);
}
