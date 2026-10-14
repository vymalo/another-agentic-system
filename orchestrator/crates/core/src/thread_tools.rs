//! Who may call a thread's tools (`thread-tools/v1`, ADR 0023, `docs/api/thread-tools-v1.md`).
//!
//! The orchestrator gives an agent that lists the extension one MCP endpoint for the thread it
//! is working on. What authorises a call is a token, minted by the A2A adapter at the moment it
//! sends a message. This module holds the part of that which is **not a secret** and which
//! travels inside the orchestrator: [`ToolsGrant`], who the token is for, and [`Caller`], which
//! side of the thread the call is made on. Both are on the send request; the token is not, and it
//! is never written to the event log, to the outbox or to a log line.

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
        }
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
    /// [`Caller::Main`], and an upload offer only for the addressed agent, with usable numbers. A
    /// grant that does not is never minted.
    pub fn is_consistent(&self) -> bool {
        self.job >= 1
            && match self.caller {
                Caller::Main => self.depth == 0,
                Caller::Ask(n) => n >= 1 && self.depth >= 1,
            }
            && self
                .uploads
                .is_none_or(|offer| self.caller.is_main() && offer.is_valid())
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
