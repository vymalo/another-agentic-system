//! Who may call a thread's tools (`thread-tools/v1`, ADR 0023, `docs/api/thread-tools-v1.md`).
//!
//! The orchestrator gives an agent that lists the extension one MCP endpoint for the thread it
//! is working on. What authorises a call is a token, minted by the A2A adapter at the moment it
//! sends a message. This module holds the part of that which is **not a secret** and which
//! travels inside the orchestrator: [`ToolsGrant`], who the token is for, and [`Caller`], which
//! side of the thread the call is made on. Both are on the send request; the token is not, and it
//! is never written to the event log, to the outbox or to a log line.
//!
//! An asked agent's grant also carries what the person said in the thread ([`PersonSaid`], ADR
//! 0026 amended 2026-10-17): the person's own words, bounded, which the agent applies its own rule
//! to (adam-coder reads which repositories were named). The orchestrator writes it, never a model.

use std::collections::VecDeque;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ids::{AgentId, ThreadId};
use crate::tools::AttachedServer;

/// Who is calling the thread's tools: a closed set (ADR 0004).
///
/// On the wire, in the token's `caller` claim: `main` or `ask:<n>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Caller {
    /// The agent the thread is addressed to, working on the job.
    Main,
    /// The n-th agent the addressed agent asked in this job (`ask_agent`, ADR 0026), n from 1.
    Ask(u32),
}

/// A string that is not `main` or `ask:<n>`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a caller is `main` or `ask:<n>` with n from 1, written without leading zeros")]
pub struct CallerError;

impl Caller {
    /// Whether this is the thread's addressed agent.
    pub const fn is_main(self) -> bool {
        matches!(self, Caller::Main)
    }
}

impl fmt::Display for Caller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Caller::Main => f.write_str("main"),
            Caller::Ask(n) => write!(f, "ask:{n}"),
        }
    }
}

impl FromStr for Caller {
    type Err = CallerError;

    /// Strict: one spelling per caller, so a claim that reads back and prints again is the same
    /// bytes.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "main" {
            return Ok(Caller::Main);
        }
        let digits = s.strip_prefix("ask:").ok_or(CallerError)?;
        if digits.is_empty()
            || digits.starts_with('0')
            || !digits.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(CallerError);
        }
        digits.parse().map(Caller::Ask).map_err(|_| CallerError)
    }
}

impl Serialize for Caller {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Caller {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// What the endpoint's token is minted for, without the token: the thread, the job of the thread
/// the message belongs to, the agent it is sent to, who calls, how deep in a chain of asks, and the
/// MCP servers attached to the thread that the agent may use (ADR 0024).
///
/// It rides on the send request (`orch_ports::SendRequest`); only the A2A adapter turns it into a
/// token, when it sends. Nothing in it is a secret: the attached servers are named by id, name and
/// description, never by a URL or a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolsGrant {
    /// The one thread the token opens.
    pub thread: ThreadId,
    /// The number of the thread's job the message belongs to (ADR 0020), from 1.
    pub job: u32,
    /// The agent the message is sent to.
    pub agent: AgentId,
    /// Who the agent is: the addressed agent, or one it asked.
    pub caller: Caller,
    /// 0 for [`Caller::Main`]; for an asked agent, its depth in the chain of asks.
    pub depth: u8,
    /// The servers attached to the thread, at the moment of the send, that `agent` may use
    /// (`attached` of the message metadata): sorted by id, at most
    /// [`MAX_ATTACHED_SERVERS`](crate::MAX_ATTACHED_SERVERS). Empty when there are none, and the
    /// metadata then has no `attached`.
    pub attached: Vec<AttachedServer>,
    /// What the deployment offers for large files (`uploads/v1`, ADR 0058): its limits, **told to
    /// the agent in the message** when the adapter activates the extension. `None` when the
    /// deployment offers none (no limit configured, or a store that cannot presign), and for every
    /// caller but [`Caller::Main`]. The adapter activates the extension only when this is `Some` and
    /// the card it read for the very message lists `uploads/v1` and `thread-tools/v1`.
    pub uploads: Option<UploadOffer>,
    /// What the person said in the thread, **told to an asked agent only** (`person` of the
    /// message metadata, ADR 0026 amended 2026-10-17): the texts of the person's messages, oldest
    /// first, within [`MAX_PERSON_SAID_BYTES`]. `None` for [`Caller::Main`], which has the messages
    /// themselves, and the adapter never mints a grant that says otherwise (`is_consistent`).
    pub person: Option<PersonSaid>,
}

/// Most bytes of one entry of [`PersonSaid::said`]: a longer message is cut there, on a character
/// boundary (the same number as [`MAX_HISTORY_ENTRY_BYTES`](crate::MAX_HISTORY_ENTRY_BYTES)).
pub const MAX_PERSON_SAID_ENTRY_BYTES: usize = 4 * 1024;
/// Most bytes of [`PersonSaid::said`], every entry together: beyond it the oldest entries are left
/// out whole, and [`PersonSaid::omitted`] counts them.
pub const MAX_PERSON_SAID_BYTES: usize = 64 * 1024;

/// What the person said in a thread, as an asked agent is told it (`person` of the `thread-tools/v1`
/// metadata, ADR 0026 amended 2026-10-17): the `text` of every `user_message` of the thread whose
/// origin is a person's ([`Origin::is_person`](crate::Origin::is_person)), every job of the thread,
/// oldest first, as written, a steered message among them. Whitespace-only texts are skipped,
/// credential-shaped text is redacted as a step's text is ([`redact_text`](crate::redact_text)),
/// each entry is cut to its first [`MAX_PERSON_SAID_ENTRY_BYTES`] on a character boundary, and
/// when the entries together would exceed [`MAX_PERSON_SAID_BYTES`] the oldest are left out whole
/// and counted in `omitted`.
///
/// It is the person's words and not a list of repositories, because the rule of what names one is
/// the agent's; the orchestrator knows nothing of repositories. Built by [`PersonSaidBuilder`] or
/// [`PersonSaid::from_texts`]; the A2A adapter writes it into the message (`{said, omitted?}`,
/// `omitted` only above 0). `Debug` prints counts and bytes, never the text, so no log line of a
/// send can carry what the person wrote.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct PersonSaid {
    /// The kept texts, oldest first.
    pub said: Vec<String>,
    /// How many older texts were left out for [`MAX_PERSON_SAID_BYTES`].
    pub omitted: u32,
}

impl PersonSaid {
    /// The person's words from `texts`, oldest first, under the bounds above.
    pub fn from_texts<I, S>(texts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut builder = PersonSaidBuilder::new();
        for text in texts {
            builder.push(text.as_ref());
        }
        builder.finish()
    }

    /// The bytes of every kept text together.
    pub fn bytes(&self) -> usize {
        self.said.iter().map(String::len).sum()
    }
}

impl fmt::Debug for PersonSaid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersonSaid")
            .field("entries", &self.said.len())
            .field("bytes", &self.bytes())
            .field("omitted", &self.omitted)
            .finish()
    }
}

/// Builds a [`PersonSaid`] from texts given oldest first, keeping the bounds as it goes: a reader
/// that pages a long log forward never holds more than [`MAX_PERSON_SAID_BYTES`] of it.
#[derive(Default)]
pub struct PersonSaidBuilder {
    kept: VecDeque<String>,
    bytes: usize,
    omitted: u32,
}

impl PersonSaidBuilder {
    /// Nothing said yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The next text the person wrote, newer than every one pushed before. A whitespace-only text
    /// is skipped; credential-shaped text is redacted as a step's text is
    /// ([`redact_text`](crate::redact_text): a pasted token never travels to an asked agent, a
    /// repository name or URL survives); a long one is then cut to
    /// [`MAX_PERSON_SAID_ENTRY_BYTES`]; and the oldest kept texts go, whole, while the total is
    /// over [`MAX_PERSON_SAID_BYTES`].
    pub fn push(&mut self, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let redacted = crate::redact::redact_text(text);
        let entry = cut_at_char_boundary(&redacted, MAX_PERSON_SAID_ENTRY_BYTES).to_owned();
        self.bytes += entry.len();
        self.kept.push_back(entry);
        while self.bytes > MAX_PERSON_SAID_BYTES {
            let Some(oldest) = self.kept.pop_front() else {
                break;
            };
            self.bytes -= oldest.len();
            self.omitted = self.omitted.saturating_add(1);
        }
    }

    /// What was kept.
    pub fn finish(self) -> PersonSaid {
        PersonSaid {
            said: self.kept.into(),
            omitted: self.omitted,
        }
    }
}

impl fmt::Debug for PersonSaidBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersonSaidBuilder")
            .field("entries", &self.kept.len())
            .field("bytes", &self.bytes)
            .field("omitted", &self.omitted)
            .finish()
    }
}

/// The first `max` bytes of `text` at most, cut on a character boundary.
fn cut_at_char_boundary(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// What a deployment offers an agent for large files (`uploads/v1`, `docs/api/uploads-v1.md`
/// section 2): the metadata of the message that activates the extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadOffer {
    /// The most bytes one file may be (`maxBytes`), at least 1.
    pub max_bytes: u64,
    /// How long a slot's URL works (`ttlSecs`), 30 to 3600.
    pub ttl_secs: u64,
}

impl UploadOffer {
    /// The shortest a slot's URL may live, in seconds.
    pub const MIN_TTL_SECS: u64 = 30;
    /// The longest a slot's URL may live, in seconds: an hour.
    pub const MAX_TTL_SECS: u64 = 3600;

    /// Whether the numbers are usable: at least one byte, a lifetime from 30 s to an hour.
    pub const fn is_valid(&self) -> bool {
        self.max_bytes >= 1
            && self.ttl_secs >= Self::MIN_TTL_SECS
            && self.ttl_secs <= Self::MAX_TTL_SECS
    }
}

impl ToolsGrant {
    /// The grant of the thread's addressed agent working on `job`.
    pub fn main(thread: ThreadId, job: u32, agent: AgentId) -> Self {
        ToolsGrant {
            thread,
            job,
            agent,
            caller: Caller::Main,
            depth: 0,
            attached: Vec::new(),
            uploads: None,
            person: None,
        }
    }

    /// The same grant, told what the person said in the thread (`None` tells nothing): only an
    /// asked agent's grant is told, and one of the addressed agent's that is becomes inconsistent.
    #[must_use]
    pub fn with_person(self, person: Option<PersonSaid>) -> Self {
        ToolsGrant { person, ..self }
    }

    /// The same grant, told of these attached servers.
    #[must_use]
    pub fn with_attached(self, attached: Vec<AttachedServer>) -> Self {
        ToolsGrant { attached, ..self }
    }

    /// The same grant, told what the deployment offers for large files (`None` offers none).
    #[must_use]
    pub fn with_uploads(self, uploads: Option<UploadOffer>) -> Self {
        ToolsGrant { uploads, ..self }
    }

    /// Whether the fields agree: a job from 1, an `ask:<n>` with n from 1, depth 0 exactly for
    /// [`Caller::Main`], an upload offer only for the addressed agent, with usable numbers, and
    /// what the person said only for an asked agent. A grant that does not is never minted.
    pub fn is_consistent(&self) -> bool {
        self.job >= 1
            && match self.caller {
                Caller::Main => self.depth == 0,
                Caller::Ask(n) => n >= 1 && self.depth >= 1,
            }
            && self
                .uploads
                .is_none_or(|offer| self.caller.is_main() && offer.is_valid())
            && (self.person.is_none() || !self.caller.is_main())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_caller_has_one_spelling() {
        assert_eq!("main".parse(), Ok(Caller::Main));
        assert_eq!("ask:1".parse(), Ok(Caller::Ask(1)));
        assert_eq!("ask:4294967295".parse(), Ok(Caller::Ask(u32::MAX)));
        for bad in [
            "",
            "Main",
            "MAIN",
            " main",
            "main ",
            "ask",
            "ask:",
            "ask:0",
            "ask:01",
            "ask:-1",
            "ask:+1",
            "ask: 1",
            "ask:1 ",
            "ask:4294967296",
            "Ask:1",
            "ask:1.0",
            "asks:1",
            "main:1",
        ] {
            assert!(bad.parse::<Caller>().is_err(), "{bad:?}");
        }
        for caller in [Caller::Main, Caller::Ask(1), Caller::Ask(77)] {
            assert_eq!(caller.to_string().parse(), Ok(caller));
        }
    }

    #[test]
    fn a_caller_is_a_string_in_json() {
        assert_eq!(serde_json::to_string(&Caller::Main).unwrap(), r#""main""#);
        assert_eq!(
            serde_json::to_string(&Caller::Ask(3)).unwrap(),
            r#""ask:3""#
        );
        assert_eq!(
            serde_json::from_str::<Caller>(r#""ask:3""#).unwrap(),
            Caller::Ask(3)
        );
        assert!(serde_json::from_str::<Caller>(r#""ask:0""#).is_err());
        assert!(serde_json::from_str::<Caller>("3").is_err());
    }

    #[test]
    fn an_upload_offer_is_for_the_addressed_agent_with_usable_numbers() {
        let thread = ThreadId(uuid::Uuid::nil());
        let offer = UploadOffer {
            max_bytes: 1 << 30,
            ttl_secs: 300,
        };
        let main = ToolsGrant::main(thread, 1, AgentId::new("coder"));
        assert!(main.uploads.is_none(), "a grant offers nothing by default");
        let with = main.clone().with_uploads(Some(offer));
        assert!(with.is_consistent());
        assert_eq!(with.uploads, Some(offer));
        assert_eq!(with.with_uploads(None), main);
        // an asked agent is offered nothing: the orchestrator keeps no file of an asked agent
        let ask = ToolsGrant {
            caller: Caller::Ask(1),
            depth: 1,
            ..main.clone()
        };
        assert!(ask.clone().is_consistent());
        assert!(!ask.with_uploads(Some(offer)).is_consistent());
        // numbers a message could not carry
        for bad in [
            UploadOffer {
                max_bytes: 0,
                ..offer
            },
            UploadOffer {
                ttl_secs: 29,
                ..offer
            },
            UploadOffer {
                ttl_secs: 3601,
                ..offer
            },
        ] {
            assert!(!bad.is_valid(), "{bad:?}");
            assert!(
                !main.clone().with_uploads(Some(bad)).is_consistent(),
                "{bad:?}"
            );
        }
        for ok in [
            UploadOffer {
                max_bytes: 1,
                ttl_secs: 30,
            },
            UploadOffer {
                max_bytes: u64::MAX,
                ttl_secs: 3600,
            },
        ] {
            assert!(ok.is_valid(), "{ok:?}");
        }
    }

    fn said(texts: &[&str]) -> PersonSaid {
        PersonSaid::from_texts(texts)
    }

    #[test]
    fn what_the_person_said_is_kept_oldest_first_as_written_and_blank_texts_are_skipped() {
        let person = said(&[
            "Set up acme/widgets for me, branch main",
            "   ",
            "",
            "\n\t",
            "  yes, go ahead  ",
        ]);
        assert_eq!(
            person.said,
            [
                "Set up acme/widgets for me, branch main",
                "  yes, go ahead  "
            ]
        );
        assert_eq!(person.omitted, 0);
        assert_eq!(said(&[]), PersonSaid::default());
        assert_eq!(said(&["", "  "]), PersonSaid::default());
    }

    /// What a person pastes goes through the same net as a step's text (ADR 0030): a credential
    /// does not travel to an asked agent; what names a repository does.
    #[test]
    fn a_credential_in_what_the_person_said_does_not_travel() {
        let pasted = "use token=abc123 on https://github.com/acme/widgets.git";
        let clean = "use token=[redacted] on https://github.com/acme/widgets.git";
        assert_eq!(said(&[pasted]).said, [clean]);
        let mut builder = PersonSaidBuilder::new();
        builder.push(pasted);
        assert_eq!(builder.finish(), said(&[pasted]));
        // redacted before the cut: the bound applies to what travels
        let long = format!("{pasted} {}", "x".repeat(MAX_PERSON_SAID_ENTRY_BYTES));
        let entry = &said(&[&long]).said[0];
        assert!(entry.starts_with(clean), "{entry}");
        assert_eq!(entry.len(), MAX_PERSON_SAID_ENTRY_BYTES);
        // a plain repository name or URL is untouched
        assert_eq!(
            said(&["set up acme/widgets and git@github.com:acme/docs.git"]).said,
            ["set up acme/widgets and git@github.com:acme/docs.git"]
        );
    }

    #[test]
    fn a_long_text_is_cut_to_the_entry_bound_on_a_character_boundary() {
        // `é` is two bytes: one starts on the last byte before the bound and would be split
        let mut text = "a".repeat(MAX_PERSON_SAID_ENTRY_BYTES - 1);
        text.push_str("ééé");
        let person = said(&[&text]);
        assert_eq!(person.said.len(), 1);
        assert_eq!(person.said[0].len(), MAX_PERSON_SAID_ENTRY_BYTES - 1);
        assert_eq!(person.said[0], "a".repeat(MAX_PERSON_SAID_ENTRY_BYTES - 1));
        // a text that ends exactly on the bound is kept whole; one byte more is cut
        let exact = "b".repeat(MAX_PERSON_SAID_ENTRY_BYTES);
        assert_eq!(said(&[&exact]).said[0], exact);
        let over = "c".repeat(MAX_PERSON_SAID_ENTRY_BYTES + 1);
        assert_eq!(said(&[&over]).said[0].len(), MAX_PERSON_SAID_ENTRY_BYTES);
        let four = "é".repeat(MAX_PERSON_SAID_ENTRY_BYTES / 2 + 1);
        let cut = &said(&[&four]).said[0];
        assert_eq!(cut.len(), MAX_PERSON_SAID_ENTRY_BYTES);
        assert!(cut.chars().all(|c| c == 'é'));
    }

    #[test]
    fn over_the_total_the_oldest_texts_go_whole_and_are_counted() {
        // 20 texts of 4 KiB each: 80 KiB; 16 fit in 64 KiB exactly, so the oldest 4 go
        let texts: Vec<String> = (0..20)
            .map(|i| format!("{i:02}{}", "x".repeat(MAX_PERSON_SAID_ENTRY_BYTES - 2)))
            .collect();
        let person = PersonSaid::from_texts(&texts);
        assert_eq!(person.omitted, 4);
        assert_eq!(person.said.len(), 16);
        assert_eq!(person.bytes(), MAX_PERSON_SAID_BYTES);
        assert_eq!(&person.said[0][..2], "04", "the oldest kept is the fifth");
        assert_eq!(&person.said[15][..2], "19", "the newest is kept");
        assert_eq!(person.said, texts[4..]);
        // one more byte over the total and the next oldest goes whole: never a partial entry
        let mut more = texts.clone();
        more.push("y".to_owned());
        let person = PersonSaid::from_texts(&more);
        assert_eq!(person.omitted, 5);
        assert_eq!(person.said.len(), 16);
        assert_eq!(&person.said[0][..2], "05");
        assert_eq!(person.said[15], "y");
        // the builder gives the same as the constructor, pushed one at a time
        let mut builder = PersonSaidBuilder::new();
        for text in &more {
            builder.push(text);
        }
        assert_eq!(builder.finish(), person);
    }

    #[test]
    fn debug_of_what_the_person_said_shows_counts_and_never_the_text() {
        let person = said(&["acme/widgets is the repository", "yes"]);
        let shown = format!("{person:?}");
        assert_eq!(
            shown, "PersonSaid { entries: 2, bytes: 33, omitted: 0 }",
            "{shown}"
        );
        assert!(!shown.contains("acme"));
        let mut builder = PersonSaidBuilder::new();
        builder.push("acme/widgets is the repository");
        let shown = format!("{builder:?}");
        assert!(!shown.contains("acme"), "{shown}");
        assert!(shown.contains("entries: 1"), "{shown}");
        // and the grant that carries it shows no text either
        let grant = ToolsGrant {
            caller: Caller::Ask(1),
            depth: 1,
            ..ToolsGrant::main(ThreadId(uuid::Uuid::nil()), 1, AgentId::new("coder"))
        }
        .with_person(Some(person));
        assert!(!format!("{grant:?}").contains("acme"));
    }

    #[test]
    fn what_the_person_said_is_for_an_asked_agent_only() {
        let thread = ThreadId(uuid::Uuid::nil());
        let main = ToolsGrant::main(thread, 1, AgentId::new("coder"));
        assert!(
            main.person.is_none(),
            "the addressed agent has the messages"
        );
        let person = said(&["hello"]);
        assert!(
            !main
                .clone()
                .with_person(Some(person.clone()))
                .is_consistent(),
            "never on a main message"
        );
        let ask = ToolsGrant {
            caller: Caller::Ask(1),
            depth: 1,
            ..main
        };
        assert!(ask.clone().is_consistent());
        let told = ask.clone().with_person(Some(person.clone()));
        assert!(told.is_consistent());
        assert_eq!(told.person, Some(person));
        assert_eq!(told.with_person(None), ask);
    }

    #[test]
    fn a_grant_is_consistent_when_its_caller_and_depth_agree() {
        let thread = ThreadId(uuid::Uuid::nil());
        let main = ToolsGrant::main(thread, 1, AgentId::new("coder"));
        assert!(main.is_consistent());
        assert!(
            !ToolsGrant {
                depth: 1,
                ..main.clone()
            }
            .is_consistent()
        );
        assert!(
            !ToolsGrant {
                job: 0,
                ..main.clone()
            }
            .is_consistent()
        );
        let ask = ToolsGrant {
            caller: Caller::Ask(2),
            depth: 1,
            ..main.clone()
        };
        assert!(ask.is_consistent());
        assert!(
            !ToolsGrant {
                depth: 0,
                ..ask.clone()
            }
            .is_consistent()
        );
        assert!(
            !ToolsGrant {
                caller: Caller::Ask(0),
                ..ask
            }
            .is_consistent()
        );
    }
}
