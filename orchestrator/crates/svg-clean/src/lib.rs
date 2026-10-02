//! A pure, allow-list sanitizer for SVG (ADR 0032, decision 9).
//!
//! An agent can hand a person an SVG, and the API may serve it inline. A browser runs script that
//! an SVG document holds when the document is opened as a page, so before the API serves one inline
//! it is rewritten by [`clean`]: read as XML (`quick-xml`), and written again from **what is on the
//! allow-list and nothing else**. The web draws a file only as `<img src>`, where an SVG runs no
//! script and loads no subresource, and the response carries a `Content-Security-Policy: sandbox`;
//! this is the layer under both (a download is the original bytes, as an attachment).
//!
//! What is kept:
//!
//! - **Elements** of the list in [`ELEMENTS`] (shapes, text, gradients, patterns, clipping and masks,
//!   markers, the filter primitives that need no resource). Anything else is dropped **with its whole
//!   subtree**: `script`, `foreignObject`, `iframe`, `a`, `image`, the animation elements (a `set`
//!   or an `animate` can write an `href` that `javascript:` fills), `feImage`, `style` that holds a
//!   reference to a resource, and every element of another namespace or with a prefix.
//! - **Attributes** of the list in [`ATTRIBUTES`] (geometry, presentation, the gradient and filter
//!   parameters, `id`, `class`, `xml:space`). Every `on*` attribute is not on it. `href` and
//!   `xlink:href` stay only when they start with `#` (after the entities in them are read, so
//!   `&#106;avascript:` is no way round), and `use` without one is dropped. A `style` attribute is
//!   dropped when it holds `url(`, `@import`, `expression(`, `javascript:`, `behavior` or `-moz-binding`
//!   or a backslash (a CSS escape can spell any of them), and any other attribute whose value has a
//!   `url(` keeps it only if it points at `#...` of the same document (`fill="url(#gradient)"`).
//!   `xmlns` and `xmlns:xlink` stay only with their own namespaces.
//! - **Text**, escaped again by this crate (never copied), inside the elements that keep it.
//!
//! What is never kept: comments, processing instructions, the XML declaration, a DOCTYPE (so no
//! entity of the document is defined, and a reference to one is dropped), a CDATA section (its
//! content is kept as escaped text). The output is a plain XML document of well-formed UTF-8 that
//! this crate reads back unchanged: `clean(clean(x)) == clean(x)`.
//!
//! An input that is not well-formed XML, is not UTF-8, nests deeper than [`MAX_DEPTH`] or whose root
//! is not an `svg` after cleaning is an [`SvgError`]: the caller does not serve it inline.

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};

/// The deepest nesting read. A deeper document is refused, not truncated.
pub const MAX_DEPTH: usize = 64;

/// The namespace of SVG.
const SVG_NS: &str = "http://www.w3.org/2000/svg";
/// The namespace of `xlink:href`.
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

/// The elements that are kept, by local name (an SVG name is case sensitive).
pub const ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "title",
    "desc",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "textPath",
    "linearGradient",
    "radialGradient",
    "stop",
    "pattern",
    "clipPath",
    "mask",
    "marker",
    "style",
    "filter",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feConvolveMatrix",
    "feDiffuseLighting",
    "feDisplacementMap",
    "feDistantLight",
    "feDropShadow",
    "feFlood",
    "feFuncA",
    "feFuncB",
    "feFuncG",
    "feFuncR",
    "feGaussianBlur",
    "feMerge",
    "feMergeNode",
    "feMorphology",
    "feOffset",
    "fePointLight",
    "feSpecularLighting",
    "feSpotLight",
    "feTile",
    "feTurbulence",
];

/// The elements that keep their text. (Whitespace between the others is kept as well.)
const TEXT_ELEMENTS: &[&str] = &["text", "tspan", "textPath", "title", "desc", "style"];

/// The attributes that are kept, by name as written. `href` and `xlink:href` are checked on top of
/// this, `style` and the `xmlns` ones have their own rules, and a value with `url(` is checked.
pub const ATTRIBUTES: &[&str] = &[
    // identity and structure
    "id",
    "class",
    "style",
    "version",
    "xml:space",
    "xml:lang",
    "lang",
    "role",
    "tabindex",
    "aria-label",
    "aria-hidden",
    "aria-labelledby",
    "aria-describedby",
    "focusable",
    // geometry
    "width",
    "height",
    "viewBox",
    "preserveAspectRatio",
    "x",
    "y",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "x1",
    "y1",
    "x2",
    "y2",
    "fx",
    "fy",
    "fr",
    "d",
    "points",
    "pathLength",
    "transform",
    "dx",
    "dy",
    "rotate",
    "textLength",
    "lengthAdjust",
    "startOffset",
    "method",
    "spacing",
    "side",
    "refX",
    "refY",
    "markerWidth",
    "markerHeight",
    "markerUnits",
    "orient",
    // presentation
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "opacity",
    "color",
    "display",
    "visibility",
    "overflow",
    "clip-path",
    "clip-rule",
    "mask",
    "filter",
    "marker-start",
    "marker-mid",
    "marker-end",
    "font-family",
    "font-size",
    "font-style",
    "font-variant",
    "font-weight",
    "font-stretch",
    "letter-spacing",
    "word-spacing",
    "text-anchor",
    "text-decoration",
    "dominant-baseline",
    "alignment-baseline",
    "baseline-shift",
    "writing-mode",
    "direction",
    "unicode-bidi",
    "shape-rendering",
    "text-rendering",
    "image-rendering",
    "color-interpolation",
    "color-interpolation-filters",
    "vector-effect",
    "paint-order",
    "mix-blend-mode",
    "isolation",
    "stop-color",
    "stop-opacity",
    "flood-color",
    "flood-opacity",
    "lighting-color",
    // gradients, patterns, clipping, masks
    "offset",
    "gradientUnits",
    "gradientTransform",
    "spreadMethod",
    "patternUnits",
    "patternContentUnits",
    "patternTransform",
    "clipPathUnits",
    "maskUnits",
    "maskContentUnits",
    // filters
    "filterUnits",
    "primitiveUnits",
    "in",
    "in2",
    "result",
    "mode",
    "type",
    "values",
    "operator",
    "k1",
    "k2",
    "k3",
    "k4",
    "stdDeviation",
    "edgeMode",
    "radius",
    "baseFrequency",
    "numOctaves",
    "seed",
    "stitchTiles",
    "order",
    "kernelMatrix",
    "divisor",
    "bias",
    "targetX",
    "targetY",
    "preserveAlpha",
    "tableValues",
    "slope",
    "intercept",
    "amplitude",
    "exponent",
    "scale",
    "xChannelSelector",
    "yChannelSelector",
    "surfaceScale",
    "diffuseConstant",
    "specularConstant",
    "specularExponent",
    "kernelUnitLength",
    "azimuth",
    "elevation",
    "z",
    "pointsAtX",
    "pointsAtY",
    "pointsAtZ",
    "limitingConeAngle",
];

/// Why an SVG was not cleaned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SvgError {
    /// Not well-formed XML, or not UTF-8.
    #[error("the SVG is not well-formed XML")]
    Malformed,
    /// Nested deeper than [`MAX_DEPTH`].
    #[error("the SVG nests too deeply")]
    TooDeep,
    /// There is no `svg` root: not an SVG, or nothing of it is allowed.
    #[error("the document has no svg element that can be kept")]
    NotSvg,
}

/// Rewrites `svg` keeping only what the lists allow (see the module documentation).
///
/// # Errors
/// [`SvgError`]: the input is not a well-formed UTF-8 XML document of an `svg` root, or it nests
/// deeper than [`MAX_DEPTH`].
pub fn clean(svg: &[u8]) -> Result<Vec<u8>, SvgError> {
    // `from_reader` over a slice reads it as it is; the input is checked to be UTF-8 first, so
    // that no byte of another encoding is ever read as markup.
    std::str::from_utf8(svg).map_err(|_| SvgError::Malformed)?;
    let mut reader = Reader::from_reader(svg);
    reader.config_mut().check_end_names = true;
    reader.config_mut().expand_empty_elements = false;

    let mut out = String::with_capacity(svg.len());
    // The elements that are open and kept, by name.
    let mut open: Vec<&'static str> = Vec::new();
    // While > 0 the reader is inside an element that was dropped, and this many of its levels are
    // still open.
    let mut skipping: usize = 0;
    let mut depth: usize = 0;
    // The text of the open `style` element, held until it ends.
    let mut css: Option<String> = None;
    let mut root_kept = false;
    let mut root_closed = false;
    let mut buf = Vec::new();

    loop {
        buf.clear();
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|_| SvgError::Malformed)?;
        match event {
            Event::Eof => break,
            Event::Start(start) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(SvgError::TooDeep);
                }
                if skipping > 0 {
                    skipping += 1;
                    continue;
                }
                // a `style` element holds text only: an element inside it is dropped
                if css.is_some() {
                    skipping = 1;
                    continue;
                }
                match keep_element(&start, open.is_empty(), root_closed)? {
                    Some((name, tag)) => {
                        if open.is_empty() {
                            root_kept = true;
                        }
                        if name == "style" {
                            css = Some(String::new());
                        }
                        out.push_str(&tag);
                        open.push(name);
                    }
                    None => skipping = 1,
                }
            }
            Event::Empty(start) => {
                if depth + 1 > MAX_DEPTH {
                    return Err(SvgError::TooDeep);
                }
                if skipping > 0 || css.is_some() {
                    continue;
                }
                if let Some((name, tag)) = keep_element(&start, open.is_empty(), root_closed)? {
                    if open.is_empty() {
                        root_kept = true;
                        root_closed = true;
                    }
                    // `<tag .../>` written as an empty element
                    out.push_str(tag.trim_end_matches('>'));
                    out.push_str("/>");
                    let _ = name;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if skipping > 0 {
                    skipping -= 1;
                    continue;
                }
                let Some(name) = open.pop() else {
                    return Err(SvgError::Malformed);
                };
                if name == "style" {
                    let text = css.take().unwrap_or_default();
                    if style_is_safe(&text) {
                        escape_into(&mut out, &text);
                    } else {
                        // Drop the element: its start tag is the last thing written for it.
                        drop_last_start(&mut out, "style");
                        continue;
                    }
                }
                out.push_str("</");
                out.push_str(name);
                out.push('>');
                if open.is_empty() {
                    root_closed = true;
                }
            }
            Event::Text(text) => {
                if skipping > 0 {
                    continue;
                }
                let text = text.decode().map_err(|_| SvgError::Malformed)?;
                keep_text(&mut out, &mut css, &open, &text);
            }
            Event::CData(cdata) => {
                if skipping > 0 {
                    continue;
                }
                let text = cdata.decode().map_err(|_| SvgError::Malformed)?;
                keep_text(&mut out, &mut css, &open, &text);
            }
            Event::GeneralRef(reference) => {
                if skipping > 0 {
                    continue;
                }
                let resolved = if reference.is_char_ref() {
                    reference
                        .resolve_char_ref()
                        .ok()
                        .flatten()
                        .map(String::from)
                } else {
                    // only the five entities every XML parser knows; an entity the document would
                    // have defined in a DOCTYPE is not kept (there is no DOCTYPE)
                    reference
                        .decode()
                        .ok()
                        .and_then(|name| predefined(&name))
                        .map(String::from)
                };
                if let Some(text) = resolved {
                    keep_text(&mut out, &mut css, &open, &text);
                }
            }
            // comments, processing instructions, the declaration and the DOCTYPE are not kept
            Event::Comment(_) | Event::PI(_) | Event::Decl(_) | Event::DocType(_) => {}
        }
    }
    if !root_kept || !open.is_empty() {
        return Err(SvgError::NotSvg);
    }
    Ok(out.into_bytes())
}

/// The five predefined XML entities.
fn predefined(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

/// Writes `text` where the open element keeps text (escaped again), or collects it for a `style`
/// element, or drops it.
fn keep_text(out: &mut String, css: &mut Option<String>, open: &[&'static str], text: &str) {
    if let Some(css) = css.as_mut() {
        css.push_str(text);
        return;
    }
    let keeps = match open.last() {
        Some(name) => TEXT_ELEMENTS.contains(name) || text.trim().is_empty(),
        None => false,
    };
    if keeps {
        escape_into(out, text);
    }
}

/// Removes the start tag of the `style` element that was just written and has no end yet.
fn drop_last_start(out: &mut String, name: &str) {
    if let Some(at) = out.rfind(&format!("<{name}")) {
        out.truncate(at);
    }
}

/// Escapes `text` for element content and attribute values.
fn escape_into(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // characters XML cannot hold at all are dropped
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
}

/// The start tag to write for `start`, with its name, or `None` when the element is dropped (with
/// its subtree). `at_root` is whether no element is open yet.
fn keep_element(
    start: &BytesStart<'_>,
    at_root: bool,
    root_closed: bool,
) -> Result<Option<(&'static str, String)>, SvgError> {
    let full = std::str::from_utf8(start.name().as_ref())
        .map_err(|_| SvgError::Malformed)?
        .to_owned();
    // a second root, or an element that has a prefix (another namespace), is not kept
    if (at_root && root_closed) || full.contains(':') {
        return Ok(None);
    }
    let Some(name) = ELEMENTS.iter().copied().find(|e| *e == full) else {
        return Ok(None);
    };
    if at_root && name != "svg" {
        return Ok(None);
    }
    let mut tag = String::from("<");
    tag.push_str(name);
    for attr in start.attributes().with_checks(true) {
        let Ok(attr) = attr else {
            return Err(SvgError::Malformed);
        };
        let Ok(key) = std::str::from_utf8(attr.key.as_ref()) else {
            return Err(SvgError::Malformed);
        };
        let Ok(value) = attr.normalized_value(XmlVersion::Implicit1_0) else {
            // an entity this document does not define: the attribute is not kept
            continue;
        };
        match attribute_verdict(name, key, &value) {
            Verdict::Keep => {
                tag.push(' ');
                tag.push_str(key);
                tag.push_str("=\"");
                escape_into(&mut tag, &value);
                tag.push('"');
            }
            Verdict::Skip => {}
            Verdict::DropElement => return Ok(None),
        }
    }
    // `use` is kept only with a reference inside the document
    if name == "use" && !has_local_href(&tag) {
        return Ok(None);
    }
    tag.push('>');
    Ok(Some((name, tag)))
}

/// Whether the start tag written so far has an `href` or `xlink:href` (it was kept, so it is `#...`).
fn has_local_href(tag: &str) -> bool {
    tag.contains(" href=\"#") || tag.contains(" xlink:href=\"#")
}

enum Verdict {
    Keep,
    Skip,
    DropElement,
}

/// What to do with one attribute.
fn attribute_verdict(element: &str, key: &str, value: &str) -> Verdict {
    match key {
        "xmlns" => return only_if(value == SVG_NS),
        "xmlns:xlink" => return only_if(value == XLINK_NS),
        "href" | "xlink:href" => {
            // a reference to something in this document, and nothing else
            return if element == "use"
                || [
                    "linearGradient",
                    "radialGradient",
                    "pattern",
                    "textPath",
                    "filter",
                ]
                .contains(&element)
            {
                if value.starts_with('#') {
                    Verdict::Keep
                } else if element == "use" {
                    Verdict::DropElement
                } else {
                    Verdict::Skip
                }
            } else {
                Verdict::Skip
            };
        }
        "style" => return only_if(style_is_safe(value)),
        _ => {}
    }
    if !ATTRIBUTES.contains(&key) {
        return Verdict::Skip;
    }
    only_if(references_are_local(value) && !value.to_ascii_lowercase().contains("javascript:"))
}

fn only_if(keep: bool) -> Verdict {
    if keep { Verdict::Keep } else { Verdict::Skip }
}

/// Every `url(` of `value` points into this document (`url(#id)`, with optional quotes and
/// blanks); a value without one passes.
fn references_are_local(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if lower.contains('\\') {
        return false;
    }
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("url(") {
        let after = rest[at + 4..].trim_start_matches([' ', '\t', '\n', '\r', '"', '\'']);
        if !after.starts_with('#') {
            return false;
        }
        rest = &rest[at + 4..];
    }
    true
}

/// Whether a stylesheet or a `style` attribute holds nothing that loads or runs anything.
fn style_is_safe(css: &str) -> bool {
    // Comments are removed before looking, and a backslash is refused: CSS escapes can spell any
    // keyword (`\75rl(`).
    let mut plain = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(at) = rest.find("/*") {
        plain.push_str(&rest[..at]);
        rest = match rest[at + 2..].find("*/") {
            Some(end) => &rest[at + 2 + end + 2..],
            None => "",
        };
    }
    plain.push_str(rest);
    let lower = plain.to_ascii_lowercase();
    !(lower.contains('\\')
        || lower.contains("url(")
        || lower.contains("@import")
        || lower.contains("expression(")
        || lower.contains("javascript:")
        || lower.contains("behavior")
        || lower.contains("-moz-binding"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_drawing_is_kept() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><circle cx="5" cy="5" r="4" fill="teal"/></svg>"##;
        assert_eq!(
            String::from_utf8(clean(svg.as_bytes()).unwrap()).unwrap(),
            svg
        );
    }
}
