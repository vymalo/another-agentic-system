//! Opaque string identifiers of the wire protocol.
//!
//! Every id is a distinct type so a `runId` cannot be passed where a `threadId` is expected.
//! On the wire each is a plain JSON string; AG-UI puts no structure on any of them.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! wire_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wraps any string-like value.
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            /// The raw id.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Takes the raw id.
            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(id: String) -> Self {
                Self(id)
            }
        }

        impl From<&str> for $name {
            fn from(id: &str) -> Self {
                Self(id.to_owned())
            }
        }
    };
}

wire_id! {
    /// The conversation a run belongs to (`threadId`). The application mints it.
    ThreadId
}
wire_id! {
    /// One run on a thread (`runId`). Never reused on a thread.
    RunId
}
wire_id! {
    /// A message, activity or reasoning span (`messageId`, message `id`).
    MessageId
}
wire_id! {
    /// A tool call (`toolCallId`).
    ToolCallId
}
wire_id! {
    /// One subagent invocation (`subagentRunId`): not a reusable subagent name.
    SubagentRunId
}
wire_id! {
    /// An interrupt a run stopped on (`Interrupt.id`, `ResumeEntry.interruptId`).
    InterruptId
}
