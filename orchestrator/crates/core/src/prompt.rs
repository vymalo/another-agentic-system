//! A person's prompt library, the pure rules (ADR 0070, `docs/api/prompts-v1.md`).
//!
//! A prompt is a name, a title, a description, a body with `{{id}}` references, variables that are
//! questions of the UI catalog (version 8, without `show` and without the type `images`), and an
//! audience of roles. Everything a client and a server must agree on is here and nowhere else:
//! what a reference is ([`parse_body`]), which saves are refused and where ([`validate`]), which
//! answers fit the variables ([`validate_answers`]) and the exact text a filled prompt becomes
//! ([`render`]). No I/O, no clock, no async: the vectors of `docs/api/examples/prompts/` and the
//! UI catalog page's answer blocks are read by this module's tests, and `web/mock/prompts.ts` is the
//! second implementation of the same page.
//!
//! # Units
//!
//! A length is counted in Unicode scalar values (`chars().count()`), except the body and the
//! rendered text, which are counted in UTF-8 bytes; an offset in an error is counted in UTF-16 code
//! units. **Nothing is trimmed here.**

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde_json::{Map, Value};

use crate::error::{Classify, ErrorClass};

/// The most prompts one person holds.
pub const MAX_PROMPTS: usize = 100;
/// The longest name, in characters.
pub const MAX_NAME: usize = 32;
/// The longest title, in Unicode scalar values.
pub const MAX_TITLE: usize = 120;
/// The longest description, in Unicode scalar values.
pub const MAX_DESCRIPTION: usize = 500;
/// The longest body, in UTF-8 bytes.
pub const MAX_BODY_BYTES: usize = 16384;
/// The most variables of a prompt.
pub const MAX_VARIABLES: usize = 8;
/// The most roles a prompt is shared with.
pub const MAX_AUDIENCE: usize = 32;
/// The longest role value, in bytes.
pub const MAX_AUDIENCE_VALUE: usize = 64;
/// The longest rendered text, in UTF-8 bytes.
pub const MAX_RENDERED_BYTES: usize = 32768;
/// The largest request body, in bytes.
pub const MAX_REQUEST_BYTES: usize = 65536;
/// The most prompts one listing holds.
pub const PAGE: usize = 100;

// ------------------------------------------------------------------------------------------ ids

/// Not `p-` and 12 lowercase letters or digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a prompt id: `p-` and 12 lowercase letters or digits")]
pub struct BadPromptId;

impl Classify for BadPromptId {
    fn class(&self) -> ErrorClass {
        ErrorClass::Invalid
    }
}

/// The id of a prompt: `p-` and 12 lowercase base-36 characters. Global: the id alone names a
/// prompt, so a person in the audience fetches a shared prompt without knowing who wrote it. It is
/// no capability: without the audience it is a 404.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PromptId(String);

impl PromptId {
    const PREFIX: &'static str = "p-";
    const SUFFIX_LEN: usize = 12;

    /// Reads an id.
    ///
    /// # Errors
    /// [`BadPromptId`] for anything but `p-` and 12 characters of `[a-z0-9]`.
    pub fn parse(text: &str) -> Result<Self, BadPromptId> {
        let suffix = text.strip_prefix(Self::PREFIX).ok_or(BadPromptId)?;
        let ok = suffix.len() == Self::SUFFIX_LEN
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase());
        if ok {
            Ok(PromptId(text.to_owned()))
        } else {
            Err(BadPromptId)
        }
    }

    /// An id made of 16 bytes of entropy: the same bytes make the same id. The 128 bits are read
    /// as a number and cut to 36^12 values: the bias is 2^-76.
    pub fn from_entropy(entropy: [u8; 16]) -> Self {
        const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut n = u128::from_be_bytes(entropy) % 36u128.pow(12);
        let mut suffix = [b'0'; PromptId::SUFFIX_LEN];
        for slot in suffix.iter_mut().rev() {
            *slot = DIGITS[usize::try_from(n % 36).unwrap_or(0)];
            n /= 36;
        }
        let suffix: String = suffix.iter().map(|b| char::from(*b)).collect();
        PromptId(format!("{}{suffix}", Self::PREFIX))
    }

    /// The text of the id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PromptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for PromptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PromptId({})", self.0)
    }
}

impl FromStr for PromptId {
    type Err = BadPromptId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

// ----------------------------------------------------------------------------------- the errors

/// A save the rules refuse: which rule (the `code` of the 400), where (`at`) and, for a reference,
/// the UTF-16 offset of the `{{` concerned. It never holds a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptError {
    /// `bad_name`, `bad_title`, `bad_description`, `bad_body`, `bad_reference`, `bad_variable`,
    /// `undeclared_variable`, `unused_variable` or `bad_audience`.
    pub code: &'static str,
    /// `name`, `title`, `description`, `body`, `variables`, `variables.<i>`,
    /// `variables.<i>.<member>`, `variables.<i>.options.<j>.<member>` or `audience`.
    pub at: String,
    /// The UTF-16 offset in the body, for `bad_reference` and `undeclared_variable`.
    pub offset: Option<u32>,
}

impl fmt::Display for PromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let words = match self.code {
            "bad_name" => {
                "the name is not 1 to 32 of a-z, 0-9 and -, starting with a letter or a digit"
            }
            "bad_title" => "the title is not 1 to 120 characters without controls or edge spaces",
            "bad_description" => "the description is not at most 500 characters without controls",
            "bad_body" => "the body is not 1 to 16384 bytes of text without controls",
            "bad_reference" => "the body has a {{ that is not a reference",
            "bad_variable" => "a variable is not a question the library takes",
            "undeclared_variable" => "the body names a variable that is not declared",
            "unused_variable" => "a declared variable is not used by the body",
            _ => "the audience is not up to 32 roles, or * alone",
        };
        f.write_str(words)
    }
}

impl std::error::Error for PromptError {}

impl Classify for PromptError {
    fn class(&self) -> ErrorClass {
        ErrorClass::Invalid
    }
}

fn fail(code: &'static str, at: impl Into<String>) -> PromptError {
    PromptError {
        code,
        at: at.into(),
        offset: None,
    }
}

/// An answer the variables refuse: which question (`None` is the whole answer, `*`) and which
/// rule. It never holds a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerRefusal {
    /// The id of the variable, or `None` for the whole answer.
    pub question: Option<String>,
    /// `unknown`, `unexpected`, `required`, `too_many`, `too_few`, `not_offered`, `repeated`,
    /// `out_of_order`, `too_long`, `grammar`, `below_min`, `above_max`, `off_step` or
    /// `not_a_permutation`.
    pub rule: &'static str,
}

impl AnswerRefusal {
    /// The `question` of the problem: the variable's id, or `*`.
    pub fn question_text(&self) -> &str {
        self.question.as_deref().unwrap_or("*")
    }
}

impl fmt::Display for AnswerRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the answer to this question was not used")
    }
}

impl std::error::Error for AnswerRefusal {}

impl Classify for AnswerRefusal {
    fn class(&self) -> ErrorClass {
        ErrorClass::Rejected
    }
}

/// A render that cannot be done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    /// The text would be over [`MAX_RENDERED_BYTES`]; nothing was built.
    #[error("the text would be over {MAX_RENDERED_BYTES} bytes")]
    TooLong,
    /// The body is not one the rules read (a stored prompt is always one).
    #[error("the body has a malformed reference")]
    BadBody,
}

impl Classify for RenderError {
    fn class(&self) -> ErrorClass {
        match self {
            RenderError::TooLong => ErrorClass::Rejected,
            RenderError::BadBody => ErrorClass::Corrupt,
        }
    }
}

// ------------------------------------------------------------------------------- the audience

/// Who a prompt is shared with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAudience {
    /// Nobody: the owner's own.
    Private,
    /// Everybody (`"*"`).
    Everyone,
    /// The people whose roles claim holds one of these values, byte for byte.
    Roles(BTreeSet<String>),
}

impl PromptAudience {
    /// Reads role values: each once, `"*"` alone, at most [`MAX_AUDIENCE`] of 1 to
    /// [`MAX_AUDIENCE_VALUE`] bytes in `0x21` to `0x7E`.
    ///
    /// # Errors
    /// `bad_audience` at `audience`.
    pub fn from_values(values: &[String]) -> Result<Self, PromptError> {
        let set: BTreeSet<String> = values.iter().cloned().collect();
        if set.len() > MAX_AUDIENCE
            || !set.iter().all(|v| Self::is_role_value(v))
            || (set.contains("*") && set.len() > 1)
        {
            return Err(fail("bad_audience", "audience"));
        }
        Ok(match set.len() {
            0 => PromptAudience::Private,
            _ if set.contains("*") => PromptAudience::Everyone,
            _ => PromptAudience::Roles(set),
        })
    }

    /// Whether `value` can be a role value of an audience: 1 to [`MAX_AUDIENCE_VALUE`] bytes, every
    /// byte in `0x21` to `0x7E`. A role of a caller that is not one is in no audience, and is not
    /// worth asking a store about.
    pub fn is_role_value(value: &str) -> bool {
        (1..=MAX_AUDIENCE_VALUE).contains(&value.len())
            && value.bytes().all(|b| (0x21..=0x7e).contains(&b))
    }

    /// The values, sorted by bytes (empty for private).
    pub fn values(&self) -> Vec<String> {
        match self {
            PromptAudience::Private => Vec::new(),
            PromptAudience::Everyone => vec!["*".to_owned()],
            PromptAudience::Roles(roles) => roles.iter().cloned().collect(),
        }
    }

    /// Whether nobody sees it but its owner.
    pub fn is_private(&self) -> bool {
        matches!(self, PromptAudience::Private)
    }

    /// Whether a caller whose roles claim is `roles` sees it: `"*"`, or a role of theirs, byte for
    /// byte. There is no administrator exception (ADR 0039).
    pub fn admits(&self, roles: &BTreeSet<String>) -> bool {
        match self {
            PromptAudience::Private => false,
            PromptAudience::Everyone => true,
            PromptAudience::Roles(wanted) => wanted.iter().any(|r| roles.contains(r)),
        }
    }
}

// ------------------------------------------------------------------------------- the definition

/// The name of a prompt: `^[a-z0-9][a-z0-9-]{0,31}$`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PromptName(String);

impl PromptName {
    /// Reads a name.
    ///
    /// # Errors
    /// `bad_name` at `name`.
    pub fn parse(text: &str) -> Result<Self, PromptError> {
        let mut bytes = text.bytes();
        let first = bytes
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        let rest = text.len() <= MAX_NAME
            && text
                .bytes()
                .skip(1)
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if first && rest {
            Ok(PromptName(text.to_owned()))
        } else {
            Err(fail("bad_name", "name"))
        }
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The variables of a prompt: questions that [`validate`] accepted.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Variables(Vec<Value>);

impl Variables {
    /// The questions, as given.
    pub fn as_slice(&self) -> &[Value] {
        &self.0
    }

    /// How many.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The questions as one JSON array (how a store keeps them).
    pub fn to_value(&self) -> Value {
        Value::Array(self.0.clone())
    }
}

/// What a save holds before the rules have read it.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptInput {
    /// The name.
    pub name: String,
    /// The title.
    pub title: String,
    /// The description (empty when absent).
    pub description: String,
    /// The body.
    pub body: String,
    /// The questions (empty when absent).
    pub variables: Vec<Value>,
    /// The role values (empty when absent).
    pub audience: Vec<String>,
}

/// A prompt the rules accepted. It holds no owner: ownership is the store's, and no type of the
/// port carries one.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptDefinition {
    id: PromptId,
    name: PromptName,
    title: String,
    description: String,
    body: String,
    variables: Variables,
    audience: PromptAudience,
}

impl PromptDefinition {
    /// A definition of `input` under `id`, if the rules accept it.
    ///
    /// # Errors
    /// The first rule the save breaks, as [`validate`] says.
    pub fn new(id: PromptId, input: PromptInput) -> Result<Self, PromptError> {
        validate(&input)?;
        Ok(PromptDefinition {
            id,
            name: PromptName::parse(&input.name)?,
            title: input.title,
            description: input.description,
            body: input.body,
            variables: Variables(input.variables),
            audience: PromptAudience::from_values(&input.audience)?,
        })
    }

    /// A definition a store read back: the same rules, so that a row that is not a prompt is an
    /// error (the adapter says `Corrupt`).
    ///
    /// # Errors
    /// The first rule the row breaks; `variables` that are not a JSON array are `bad_variable`.
    pub fn from_stored(
        id: PromptId,
        name: &str,
        title: &str,
        description: &str,
        body: &str,
        variables: Value,
        audience: &[String],
    ) -> Result<Self, PromptError> {
        let Value::Array(variables) = variables else {
            return Err(fail("bad_variable", "variables"));
        };
        Self::new(
            id,
            PromptInput {
                name: name.to_owned(),
                title: title.to_owned(),
                description: description.to_owned(),
                body: body.to_owned(),
                variables,
                audience: audience.to_vec(),
            },
        )
    }

    /// The id.
    pub fn id(&self) -> &PromptId {
        &self.id
    }

    /// The name.
    pub fn name(&self) -> &PromptName {
        &self.name
    }

    /// The title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The description (possibly empty).
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The body.
    pub fn body(&self) -> &str {
        &self.body
    }

    /// The variables.
    pub fn variables(&self) -> &Variables {
        &self.variables
    }

    /// Who it is shared with.
    pub fn audience(&self) -> &PromptAudience {
        &self.audience
    }

    /// The same prompt with another audience.
    #[must_use]
    pub fn with_audience(mut self, audience: PromptAudience) -> Self {
        self.audience = audience;
        self
    }

    /// The save this definition would be made from (for an edit that changes some members).
    pub fn to_input(&self) -> PromptInput {
        PromptInput {
            name: self.name.0.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            body: self.body.clone(),
            variables: self.variables.0.clone(),
            audience: self.audience.values(),
        }
    }
}

// --------------------------------------------------------------------------------- references

/// A piece of a body: text with the escapes resolved, or a reference to a variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Literal text.
    Literal(String),
    /// `{{id}}`.
    Ref(String),
}

/// A `{{` that is not a reference, at this UTF-16 offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a {{{{ that is not a reference, at offset {offset}")]
pub struct BadReference {
    /// The UTF-16 code unit offset of the `{{`.
    pub offset: u32,
}

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-')
}

fn starts_with(chars: &[char], at: usize, text: &str) -> bool {
    text.chars()
        .enumerate()
        .all(|(k, c)| chars.get(at + k) == Some(&c))
}

/// A body into its segments. `\{{` writes `{{`; any other `\` is itself; an unescaped `{{` must be
/// followed by an id (`[A-Za-z0-9_.:-]{1,64}`) and exactly `}}`; a lone `}}`, `{x}` and `{` are
/// text.
///
/// # Errors
/// [`BadReference`] with the UTF-16 offset of the first `{{` that is not a reference.
pub fn parse_body(body: &str) -> Result<Vec<Segment>, BadReference> {
    let chars: Vec<char> = body.chars().collect();
    let mut segments = Vec::new();
    let mut literal = String::new();
    let mut units: u32 = 0;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && starts_with(&chars, i + 1, "{{") {
            literal.push_str("{{");
            units += 3;
            i += 3;
        } else if c == '{' && starts_with(&chars, i, "{{") {
            let mut j = i + 2;
            let mut id = String::new();
            while j < chars.len() && id.chars().count() < 64 && is_id_char(chars[j]) {
                id.push(chars[j]);
                j += 1;
            }
            if id.is_empty() || !starts_with(&chars, j, "}}") {
                return Err(BadReference { offset: units });
            }
            if !literal.is_empty() {
                segments.push(Segment::Literal(std::mem::take(&mut literal)));
            }
            let consumed = u32::try_from(j + 2 - i).unwrap_or(u32::MAX);
            segments.push(Segment::Ref(id));
            units += consumed;
            i = j + 2;
        } else {
            literal.push(c);
            units += u32::try_from(c.len_utf16()).unwrap_or(1);
            i += 1;
        }
    }
    if !literal.is_empty() {
        segments.push(Segment::Literal(literal));
    }
    Ok(segments)
}

/// The UTF-16 offset of the `{{` of the first reference of `body` whose id is not in `declared`.
fn offset_of_undeclared(body: &str, declared: &BTreeSet<&str>) -> u32 {
    let chars: Vec<char> = body.chars().collect();
    let mut units: u32 = 0;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && starts_with(&chars, i + 1, "{{") {
            units += 3;
            i += 3;
        } else if c == '{' && starts_with(&chars, i, "{{") {
            let mut j = i + 2;
            let mut id = String::new();
            while j < chars.len() && is_id_char(chars[j]) {
                id.push(chars[j]);
                j += 1;
            }
            if !declared.contains(id.as_str()) {
                return units;
            }
            units += u32::try_from(j + 2 - i).unwrap_or(u32::MAX);
            i = j + 2;
        } else {
            units += u32::try_from(c.len_utf16()).unwrap_or(1);
            i += 1;
        }
    }
    0
}

// ------------------------------------------------------------------------------- the variables

const KINDS: [&str; 10] = [
    "single", "multiple", "select", "text", "longText", "confirm", "number", "scale", "date",
    "rank",
];
/// Every member a question of the catalog's schema has (`show` too: a prompt refuses it).
const CATALOG_MEMBERS: [&str; 24] = [
    "id",
    "question",
    "help",
    "show",
    "type",
    "options",
    "multiple",
    "allowOther",
    "required",
    "default",
    "min",
    "max",
    "yesLabel",
    "noLabel",
    "minValue",
    "maxValue",
    "step",
    "unit",
    "from",
    "to",
    "fromLabel",
    "toLabel",
    "minDate",
    "maxDate",
];
const COMMON: [&str; 5] = ["id", "question", "help", "type", "required"];
const OPTION_MEMBERS: [&str; 5] = ["value", "label", "description", "icon", "recommended"];
const ICONS: [&str; 15] = [
    "agent", "read", "edit", "delete", "move", "search", "execute", "think", "fetch", "web", "git",
    "test", "file", "tool", "opencode",
];

fn kind_members(kind: &str) -> Vec<&'static str> {
    let extra: &[&'static str] = match kind {
        "single" => &["options", "allowOther", "default", "multiple"],
        "multiple" | "select" => &["options", "allowOther", "default", "multiple", "min", "max"],
        "text" | "longText" => &["default"],
        "confirm" => &["yesLabel", "noLabel", "default"],
        "number" => &["minValue", "maxValue", "step", "unit", "default"],
        "scale" => &["from", "to", "fromLabel", "toLabel", "default"],
        "date" => &["minDate", "maxDate", "default"],
        _ => &["options", "default"],
    };
    COMMON.iter().chain(extra.iter()).copied().collect()
}

fn scalars(s: &str) -> usize {
    s.chars().count()
}

/// Whether `s` has a control character (U+0000 to U+001F, U+007F to U+009F) other than `allowed`.
fn has_control(s: &str, allowed: &[char]) -> bool {
    s.chars().any(|c| {
        let n = u32::from(c);
        (n <= 0x1f || (0x7f..=0x9f).contains(&n)) && !allowed.contains(&c)
    })
}

fn is_id(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.chars().all(is_id_char)
}

/// Whether `c` is a bidirectional embedding, override or isolate control, or a line or paragraph
/// separator: U+202A to U+202E, U+2066 to U+2069, U+2028 and U+2029. (The joiners and the left-to-right
/// and right-to-left marks stay allowed: scripts and emoji need them.)
fn is_bidi(c: char) -> bool {
    matches!(u32::from(c), 0x202a..=0x202e | 0x2066..=0x2069 | 0x2028 | 0x2029)
}

/// Whether `s` is not a text a person may read as one line of a shared prompt: a control character
/// other than `allowed`, a bidirectional control or a line or paragraph separator.
fn is_unsafe_text(s: &str, allowed: &[char]) -> bool {
    has_control(s, allowed) || s.chars().any(is_bidi)
}

/// A text of a variable that a person reads (a question, a help, a label, a unit): `lo` to `hi`
/// scalar values and nothing [`is_unsafe_text`] refuses.
fn shown_in(v: Option<&Value>, lo: usize, hi: usize) -> bool {
    v.and_then(Value::as_str)
        .is_some_and(|s| (lo..=hi).contains(&scalars(s)) && !is_unsafe_text(s, &[]))
}

/// The integer a JSON number is, if its value is one in range: `5`, `5.0` and `5e0` are 5 (a client
/// that writes a whole double as `5.0` is not refused for it); `5.5`, a string, a number out of the
/// range of `i64` and anything that is not a number are not.
fn as_int(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    let f = v.as_f64()?;
    // every integer of this range is a float exactly: the cast loses nothing
    (f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0).then_some(f as i64)
}

fn int_in(v: &Value, lo: i64, hi: i64) -> bool {
    as_int(v).is_some_and(|n| (lo..=hi).contains(&n))
}

/// A decimal string as millionths: `-?(0|[1-9][0-9]{0,14})(\.[0-9]{1,6})?`, and zero has no sign.
fn decimal(v: Option<&Value>) -> Option<i128> {
    let text = v?.as_str()?;
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = match rest.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (rest, None),
    };
    let whole_ok = !whole.is_empty()
        && whole.len() <= 15
        && whole.bytes().all(|b| b.is_ascii_digit())
        && (whole == "0" || !whole.starts_with('0'));
    let fraction_ok = fraction
        .is_none_or(|f| (1..=6).contains(&f.len()) && f.bytes().all(|b| b.is_ascii_digit()));
    if !whole_ok || !fraction_ok {
        return None;
    }
    let mut value = whole.parse::<i128>().ok()? * 1_000_000;
    if let Some(f) = fraction {
        let padded = format!("{f:0<6}");
        value += padded.parse::<i128>().ok()?;
    }
    if value == 0 && negative {
        return None;
    }
    Some(if negative { -value } else { value })
}

/// Whether `text` is `YYYY-MM-DD`, year 0001 to 9999, and a day that exists.
fn real_day(text: &str) -> bool {
    let b = text.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<i64> {
        let part = &text[r];
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse::<i64>().ok())
            .flatten()
    };
    let (Some(y), Some(m), Some(d)) = (digits(0..4), digits(5..7), digits(8..10)) else {
        return false;
    };
    if y < 1 {
        return false;
    }
    match (i16::try_from(y), i8::try_from(m), i8::try_from(d)) {
        (Ok(y), Ok(m), Ok(d)) => jiff::civil::Date::new(y, m, d).is_ok(),
        _ => false,
    }
}

fn kind_of(q: &Map<String, Value>) -> &str {
    q.get("type").and_then(Value::as_str).unwrap_or("single")
}

fn is_choice(kind: &str) -> bool {
    matches!(kind, "single" | "multiple" | "select")
}

fn takes_several(q: &Map<String, Value>) -> bool {
    let kind = kind_of(q);
    kind == "multiple" || (kind == "select" && q.get("multiple") == Some(&Value::Bool(true)))
}

fn options_of(q: &Map<String, Value>) -> Vec<&Map<String, Value>> {
    q.get("options")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

fn sorted_keys(o: &Map<String, Value>) -> Vec<&String> {
    let mut keys: Vec<&String> = o.keys().collect();
    keys.sort();
    keys
}

/// The first rule a variable breaks, or `None`.
fn check_variable(i: usize, value: &Value, seen: &mut BTreeSet<String>) -> Option<PromptError> {
    let here = format!("variables.{i}");
    let bad = |at: String| Some(fail("bad_variable", at));
    let Some(q) = value.as_object() else {
        return bad(here);
    };
    // (2) members that no question has, and show
    for k in sorted_keys(q) {
        if !CATALOG_MEMBERS.contains(&k.as_str()) || k == "show" {
            return bad(format!("{here}.{k}"));
        }
    }
    // (3) the type
    if let Some(t) = q.get("type")
        && !t.as_str().is_some_and(|t| KINDS.contains(&t))
    {
        return bad(format!("{here}.type"));
    }
    let kind = kind_of(q);
    // (4) members this kind does not take
    let takes = kind_members(kind);
    let one_choice = is_choice(kind) && !takes_several(q);
    for k in sorted_keys(q) {
        if !takes.contains(&k.as_str()) || (one_choice && (k == "min" || k == "max")) {
            return bad(format!("{here}.{k}"));
        }
    }
    // (5) each member's own type and bounds
    match q.get("id").and_then(Value::as_str) {
        Some(id) if is_id(id) && !seen.contains(id) => {
            seen.insert(id.to_owned());
        }
        _ => return bad(format!("{here}.id")),
    }
    if !shown_in(q.get("question"), 1, 300) {
        return bad(format!("{here}.question"));
    }
    if q.get("help").is_some() && !shown_in(q.get("help"), 0, 300) {
        return bad(format!("{here}.help"));
    }
    if q.get("required").is_some_and(|v| !v.is_boolean()) {
        return bad(format!("{here}.required"));
    }
    let has_options = kind == "rank" || is_choice(kind);
    if has_options {
        let Some(options) = q.get("options").and_then(Value::as_array) else {
            return bad(format!("{here}.options"));
        };
        for (j, o) in options.iter().enumerate() {
            let at = format!("{here}.options.{j}");
            let Some(o) = o.as_object() else {
                return bad(at);
            };
            for k in sorted_keys(o) {
                if !OPTION_MEMBERS.contains(&k.as_str()) {
                    return bad(format!("{at}.{k}"));
                }
            }
            if !o.get("value").and_then(Value::as_str).is_some_and(is_id) {
                return bad(format!("{at}.value"));
            }
            if !shown_in(o.get("label"), 1, 120) {
                return bad(format!("{at}.label"));
            }
            if o.get("description").is_some() && !shown_in(o.get("description"), 0, 300) {
                return bad(format!("{at}.description"));
            }
            if o.get("icon")
                .is_some_and(|v| !v.as_str().is_some_and(|s| ICONS.contains(&s)))
            {
                return bad(format!("{at}.icon"));
            }
            if o.get("recommended").is_some_and(|v| !v.is_boolean()) {
                return bad(format!("{at}.recommended"));
            }
        }
    }
    for k in ["multiple", "allowOther"] {
        if q.get(k).is_some_and(|v| !v.is_boolean()) {
            return bad(format!("{here}.{k}"));
        }
    }
    if let Some(d) = q.get("default") {
        let list_ok = has_options && d.as_array().is_some_and(|a| a.iter().all(Value::is_string));
        if !d.is_string() && !list_ok {
            return bad(format!("{here}.default"));
        }
    }
    if q.get("min").is_some_and(|v| !int_in(v, 0, 50)) {
        return bad(format!("{here}.min"));
    }
    if q.get("max").is_some_and(|v| !int_in(v, 1, 50)) {
        return bad(format!("{here}.max"));
    }
    for k in ["yesLabel", "noLabel"] {
        if q.get(k).is_some() && !shown_in(q.get(k), 1, 40) {
            return bad(format!("{here}.{k}"));
        }
    }
    for k in ["minValue", "maxValue", "step"] {
        if q.get(k).is_some() && decimal(q.get(k)).is_none() {
            return bad(format!("{here}.{k}"));
        }
    }
    if q.get("unit").is_some() && !shown_in(q.get("unit"), 1, 20) {
        return bad(format!("{here}.unit"));
    }
    if q.get("from").is_some_and(|v| !int_in(v, 0, 1)) {
        return bad(format!("{here}.from"));
    }
    if kind == "scale" && !q.get("to").is_some_and(|v| int_in(v, 2, 10)) {
        return bad(format!("{here}.to"));
    }
    for k in ["fromLabel", "toLabel"] {
        if q.get(k).is_some() && !shown_in(q.get(k), 1, 60) {
            return bad(format!("{here}.{k}"));
        }
    }
    for k in ["minDate", "maxDate"] {
        if q.get(k).is_some() && !q.get(k).and_then(Value::as_str).is_some_and(real_day) {
            return bad(format!("{here}.{k}"));
        }
    }
    // (6) the rules between members
    if kind == "single" && q.get("multiple") == Some(&Value::Bool(true)) {
        return bad(format!("{here}.multiple"));
    }
    if kind == "multiple" && q.get("multiple") == Some(&Value::Bool(false)) {
        return bad(format!("{here}.multiple"));
    }
    if has_options {
        let options = options_of(q);
        let (lo, hi) = if kind == "select" { (2, 50) } else { (2, 12) };
        if !(lo..=hi).contains(&options.len()) {
            return bad(format!("{here}.options"));
        }
        let mut values = BTreeSet::new();
        for (j, o) in options.iter().enumerate() {
            let v = o.get("value").and_then(Value::as_str).unwrap_or_default();
            if !values.insert(v) {
                return bad(format!("{here}.options.{j}.value"));
            }
        }
        if takes_several(q) {
            let offered = i64::try_from(options.len()).unwrap_or(i64::MAX)
                + i64::from(q.get("allowOther") == Some(&Value::Bool(true)));
            let min = q.get("min").and_then(as_int);
            let max = q.get("max").and_then(as_int);
            if let (Some(min), Some(max)) = (min, max)
                && min > max
            {
                return bad(format!("{here}.min"));
            }
            if min.is_some_and(|m| m > offered) {
                return bad(format!("{here}.min"));
            }
            if max.is_some_and(|m| m > offered) {
                return bad(format!("{here}.max"));
            }
        }
    }
    let (min_v, max_v, step) = (
        decimal(q.get("minValue")),
        decimal(q.get("maxValue")),
        decimal(q.get("step")),
    );
    if let (Some(a), Some(b)) = (min_v, max_v)
        && a > b
    {
        return bad(format!("{here}.minValue"));
    }
    if step.is_some_and(|s| s <= 0) {
        return bad(format!("{here}.step"));
    }
    if let (Some(a), Some(b)) = (
        q.get("minDate").and_then(Value::as_str),
        q.get("maxDate").and_then(Value::as_str),
    ) && a > b
    {
        return bad(format!("{here}.minDate"));
    }
    if kind == "scale" {
        let from = q.get("from").and_then(as_int).unwrap_or(1);
        if q.get("to").and_then(as_int).unwrap_or(0) <= from {
            return bad(format!("{here}.to"));
        }
    }
    if q.get("default").is_some() && !default_accepted(q) {
        return bad(format!("{here}.default"));
    }
    None
}

/// A default is an answer the kind accepts (never an empty rank).
fn default_accepted(q: &Map<String, Value>) -> bool {
    let kind = kind_of(q);
    let Some(d) = q.get("default") else {
        return true;
    };
    let mut loose = q.clone();
    loose.insert("required".to_owned(), Value::Bool(false));
    let mut entry = Map::new();
    entry.insert("id".to_owned(), Value::String("d".to_owned()));
    if kind == "text" || kind == "longText" {
        let Some(text) = d.as_str().filter(|t| !t.chars().any(is_bidi)) else {
            return false;
        };
        entry.insert("values".to_owned(), Value::Array(Vec::new()));
        entry.insert("text".to_owned(), Value::String(text.to_owned()));
        return refusal_of_entry(&loose, &entry).is_none();
    }
    let values: Vec<Value> = match d {
        Value::Array(a) => a.clone(),
        other => vec![other.clone()],
    };
    if kind == "rank" && values.is_empty() {
        return false;
    }
    if (kind == "single" || (kind == "select" && !takes_several(q))) && values.len() > 1 {
        return false;
    }
    entry.insert("values".to_owned(), Value::Array(values));
    refusal_of_entry(&loose, &entry).is_none()
}

// ---------------------------------------------------------------------------------- the save

/// The first rule a save breaks, in the order of the page: name, title, description, body (its size
/// and characters), the grammar of every reference, the variables (each in order), a reference
/// that no variable declares, a variable that no reference uses, the audience.
///
/// # Errors
/// [`PromptError`], with the rule's `code`, where, and for a reference its UTF-16 offset.
pub fn validate(input: &PromptInput) -> Result<(), PromptError> {
    PromptName::parse(&input.name)?;
    let title = &input.title;
    let edge = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r');
    if !(1..=MAX_TITLE).contains(&scalars(title))
        || is_unsafe_text(title, &[])
        || title.starts_with(edge)
        || title.ends_with(edge)
    {
        return Err(fail("bad_title", "title"));
    }
    if scalars(&input.description) > MAX_DESCRIPTION
        || is_unsafe_text(&input.description, &['\t', '\n'])
    {
        return Err(fail("bad_description", "description"));
    }
    let body = &input.body;
    if body.is_empty()
        || body.len() > MAX_BODY_BYTES
        || has_control(body, &['\t', '\n', '\r'])
        || body.chars().all(edge)
    {
        return Err(fail("bad_body", "body"));
    }
    let segments = parse_body(body).map_err(|e| PromptError {
        code: "bad_reference",
        at: "body".to_owned(),
        offset: Some(e.offset),
    })?;
    if input.variables.len() > MAX_VARIABLES {
        return Err(fail("bad_variable", "variables"));
    }
    let mut seen = BTreeSet::new();
    for (i, q) in input.variables.iter().enumerate() {
        if let Some(error) = check_variable(i, q, &mut seen) {
            return Err(error);
        }
    }
    let declared: BTreeSet<&str> = seen.iter().map(String::as_str).collect();
    let mut used = BTreeSet::new();
    for segment in &segments {
        if let Segment::Ref(id) = segment {
            if !declared.contains(id.as_str()) {
                return Err(PromptError {
                    code: "undeclared_variable",
                    at: "body".to_owned(),
                    offset: Some(offset_of_undeclared(body, &declared)),
                });
            }
            used.insert(id.as_str());
        }
    }
    for (i, q) in input.variables.iter().enumerate() {
        let id = q.get("id").and_then(Value::as_str).unwrap_or_default();
        if !used.contains(id) {
            return Err(fail("unused_variable", format!("variables.{i}")));
        }
    }
    PromptAudience::from_values(&input.audience)?;
    Ok(())
}

// -------------------------------------------------------------------------------- the answers

/// Answers that fit a prompt's variables (one entry per variable, in order): what [`render`] takes.
#[derive(Debug, Clone, PartialEq)]
pub struct Answers(Vec<Value>);

/// The first rule that one entry breaks for one question, or `None`.
fn refusal_of_entry(q: &Map<String, Value>, entry: &Map<String, Value>) -> Option<&'static str> {
    let kind = kind_of(q);
    let required = q.get("required") != Some(&Value::Bool(false));
    let Some(values) = entry.get("values").and_then(Value::as_array) else {
        return Some("unknown");
    };
    let mut allowed = vec!["id", "values"];
    if kind == "text" || kind == "longText" {
        allowed.push("text");
    } else if is_choice(kind) && q.get("allowOther") == Some(&Value::Bool(true)) {
        allowed.push("other");
    }
    if entry.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Some("unexpected");
    }
    match kind {
        "text" | "longText" => {
            if !values.is_empty() {
                return Some("not_offered");
            }
            let Some(text) = entry.get("text") else {
                return required.then_some("required");
            };
            let Some(text) = text.as_str().filter(|t| !t.is_empty()) else {
                return Some("grammar");
            };
            let allowed: &[char] = if kind == "text" {
                &['\t']
            } else {
                &['\t', '\n']
            };
            if has_control(text, allowed) {
                return Some("grammar");
            }
            if scalars(text) > if kind == "text" { 500 } else { 2000 } {
                return Some("too_long");
            }
            None
        }
        "single" | "multiple" | "select" => refusal_of_choice(q, entry, values, required),
        _ => refusal_of_kind(q, kind, values, required),
    }
}

fn refusal_of_choice(
    q: &Map<String, Value>,
    entry: &Map<String, Value>,
    values: &[Value],
    required: bool,
) -> Option<&'static str> {
    let other = entry.get("other");
    let count = values.len() + usize::from(other.is_some());
    if count == 0 {
        return required.then_some("required");
    }
    let several = takes_several(q);
    if !several && count > 1 {
        return Some("too_many");
    }
    let options: Vec<&str> = options_of(q)
        .iter()
        .filter_map(|o| o.get("value").and_then(Value::as_str))
        .collect();
    let mut places = Vec::with_capacity(values.len());
    for v in values {
        match v
            .as_str()
            .and_then(|s| options.iter().position(|o| *o == s))
        {
            Some(p) => places.push(p),
            None => return Some("not_offered"),
        }
    }
    let distinct: BTreeSet<usize> = places.iter().copied().collect();
    if distinct.len() != places.len() {
        return Some("repeated");
    }
    if places.windows(2).any(|w| w[1] < w[0]) {
        return Some("out_of_order");
    }
    if let Some(other) = other {
        let Some(other) = other
            .as_str()
            .filter(|t| !t.is_empty() && !has_control(t, &['\t']))
        else {
            return Some("grammar");
        };
        if scalars(other) > 500 {
            return Some("too_long");
        }
    }
    if several {
        let count = i64::try_from(count).unwrap_or(i64::MAX);
        if q.get("max").and_then(as_int).is_some_and(|m| count > m) {
            return Some("too_many");
        }
        if q.get("min").and_then(as_int).is_some_and(|m| count < m) {
            return Some("too_few");
        }
    }
    None
}

/// The rules of the five newer kinds (the catalog's table).
fn refusal_of_kind(
    q: &Map<String, Value>,
    kind: &str,
    values: &[Value],
    required: bool,
) -> Option<&'static str> {
    if values.is_empty() {
        return required.then_some("required");
    }
    if kind == "rank" {
        let options: Vec<&str> = options_of(q)
            .iter()
            .filter_map(|o| o.get("value").and_then(Value::as_str))
            .collect();
        if values
            .iter()
            .any(|v| !v.as_str().is_some_and(|s| options.contains(&s)))
        {
            return Some("not_offered");
        }
        let distinct: BTreeSet<&str> = values.iter().filter_map(Value::as_str).collect();
        if distinct.len() != values.len() || values.len() != options.len() {
            return Some("not_a_permutation");
        }
        return None;
    }
    if values.len() > 1 {
        return Some("too_many");
    }
    let Some(value) = values[0].as_str() else {
        return Some(if kind == "confirm" {
            "not_offered"
        } else {
            "grammar"
        });
    };
    match kind {
        "confirm" => (value != "yes" && value != "no").then_some("not_offered"),
        "number" => {
            let Some(n) = decimal(Some(&values[0])) else {
                return Some("grammar");
            };
            let (min, max, step) = (
                decimal(q.get("minValue")),
                decimal(q.get("maxValue")),
                decimal(q.get("step")),
            );
            if min.is_some_and(|m| n < m) {
                return Some("below_min");
            }
            if max.is_some_and(|m| n > m) {
                return Some("above_max");
            }
            if step.is_some_and(|s| s > 0 && (n - min.unwrap_or(0)) % s != 0) {
                return Some("off_step");
            }
            None
        }
        "scale" => {
            let grammar = value == "0"
                || (!value.starts_with('0')
                    && (1..=2).contains(&value.len())
                    && value.bytes().all(|b| b.is_ascii_digit()));
            if !grammar {
                return Some("grammar");
            }
            let n: i64 = value.parse().ok()?;
            let from = q.get("from").and_then(as_int).unwrap_or(1);
            let to = q.get("to").and_then(as_int).unwrap_or(from);
            if n < from {
                return Some("below_min");
            }
            (n > to).then_some("above_max")
        }
        "date" => {
            if !real_day(value) {
                return Some("grammar");
            }
            if q.get("minDate")
                .and_then(Value::as_str)
                .is_some_and(|m| value < m)
            {
                return Some("below_min");
            }
            q.get("maxDate")
                .and_then(Value::as_str)
                .is_some_and(|m| value > m)
                .then_some("above_max")
        }
        _ => None,
    }
}

/// The first rule that a render request's answers break against the variables.
///
/// `answers` is the request's member (`None` when absent) and `note` says the request had a
/// `note`, which a prompt never takes (it has no free box). The whole answer is judged first
/// (`answers` missing or not a list: `unknown`; a note: `unexpected`; the entries are not the
/// variables once each in order: `unknown`), then each variable in order.
///
/// # Errors
/// [`AnswerRefusal`] with the variable's id (or the whole answer) and the rule.
pub fn validate_answers(
    variables: &Variables,
    answers: Option<&Value>,
    note: bool,
) -> Result<Answers, AnswerRefusal> {
    let whole = |rule| AnswerRefusal {
        question: None,
        rule,
    };
    let Some(Value::Array(answers)) = answers else {
        return Err(whole("unknown"));
    };
    if note {
        return Err(whole("unexpected"));
    }
    let id_of = |v: &Value| v.get("id").and_then(Value::as_str).map(str::to_owned);
    let matches = answers.len() == variables.len()
        && answers
            .iter()
            .zip(variables.as_slice())
            .all(|(a, q)| a.is_object() && id_of(a).is_some() && id_of(a) == id_of(q));
    if !matches {
        return Err(whole("unknown"));
    }
    for (q, a) in variables.as_slice().iter().zip(answers) {
        let (Some(q), Some(a)) = (q.as_object(), a.as_object()) else {
            return Err(whole("unknown"));
        };
        if let Some(rule) = refusal_of_entry(q, a) {
            return Err(AnswerRefusal {
                question: q.get("id").and_then(Value::as_str).map(str::to_owned),
                rule,
            });
        }
    }
    Ok(Answers(answers.clone()))
}

// ---------------------------------------------------------------------------------- render

fn label_of(q: &Map<String, Value>, value: &Value) -> String {
    options_of(q)
        .iter()
        .find(|o| o.get("value") == Some(value))
        .and_then(|o| o.get("label").and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned()
}

/// What one validated answer renders as.
fn value_text(q: &Map<String, Value>, entry: &Map<String, Value>) -> String {
    let values: &[Value] = entry
        .get("values")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice);
    match kind_of(q) {
        "text" | "longText" => entry
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        "single" | "multiple" | "select" => {
            let mut parts: Vec<String> = values.iter().map(|v| label_of(q, v)).collect();
            if let Some(other) = entry.get("other").and_then(Value::as_str) {
                parts.push(other.to_owned());
            }
            parts.join(", ")
        }
        "rank" => values
            .iter()
            .enumerate()
            .map(|(i, v)| format!("{}. {}", i + 1, label_of(q, v)))
            .collect::<Vec<_>>()
            .join("; "),
        _ => values
            .first()
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    }
}

/// The text of a prompt for validated answers: the body's literal text, and for each reference the
/// answer written by its kind (a choice as its label, several as labels in option order joined by
/// `", "`, a text verbatim, `yes` or `no`, a number, a scale point and a date as sent, a rank as
/// `1. A; 2. B`). An optional variable left alone is the empty string and its default is never
/// used; nothing is trimmed and a value is never read again for references. The sizes are added up
/// first: a text over [`MAX_RENDERED_BYTES`] is refused before anything is built.
///
/// # Errors
/// [`RenderError::TooLong`]; [`RenderError::BadBody`] for a body the rules do not read (a stored
/// prompt always is one).
pub fn render(body: &str, variables: &Variables, answers: &Answers) -> Result<String, RenderError> {
    let segments = parse_body(body).map_err(|_| RenderError::BadBody)?;
    let mut text = std::collections::BTreeMap::new();
    for (q, a) in variables.as_slice().iter().zip(&answers.0) {
        if let (Some(q), Some(a)) = (q.as_object(), a.as_object()) {
            let id = q.get("id").and_then(Value::as_str).unwrap_or_default();
            text.insert(id.to_owned(), value_text(q, a));
        }
    }
    let part = |segment: &Segment| -> String {
        match segment {
            Segment::Literal(s) => s.clone(),
            Segment::Ref(id) => text.get(id).cloned().unwrap_or_default(),
        }
    };
    let size: usize = segments
        .iter()
        .map(|s| match s {
            Segment::Literal(s) => s.len(),
            Segment::Ref(id) => text.get(id).map_or(0, String::len),
        })
        .sum();
    if size > MAX_RENDERED_BYTES {
        return Err(RenderError::TooLong);
    }
    let mut out = String::with_capacity(size);
    for segment in &segments {
        out.push_str(&part(segment));
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
