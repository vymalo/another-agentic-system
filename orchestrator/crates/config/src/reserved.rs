//! The keys that are reserved: named in the contract, built by a later PR, and refused until then
//! so that a file never holds a setting that silently does nothing.
//!
//! The table is the "reserved" rows of [`docs/api/config.md`](../../../../docs/api/config.md#every-key).
//! A reserved key is reported as reserved, naming the change that brings it, not as unknown.

/// A reserved key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reserved {
    /// The key path in dotted form. A reserved key reserves everything under it.
    pub path: &'static str,
    /// The change that brings it.
    pub by: &'static str,
}

/// Every reserved key.
pub const RESERVED: &[Reserved] = &[
    Reserved {
        path: "tasks.turnSummary",
        by: "a later task of ADR 0035 that has no PR yet",
    },
    Reserved {
        path: "tasks.stepLabel",
        by: "a later task of ADR 0035 that has no PR yet",
    },
    Reserved {
        path: "auth.defaultRole",
        by: "PR S15 (ADR 0033, roles and permissions)",
    },
    Reserved {
        path: "auth.roles",
        by: "PR S15 (ADR 0033, roles and permissions)",
    },
];

/// The reservation that covers `path` (dotted, such as `tasks.title.system`), if any.
pub fn reserved(path: &str) -> Option<&'static Reserved> {
    RESERVED.iter().find(|r| {
        path == r.path
            || path
                .strip_prefix(r.path)
                .is_some_and(|rest| rest.starts_with(['.', '[']))
    })
}
