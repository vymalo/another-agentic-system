//! What a server's SQLSTATE says to do, for both PostgreSQL adapters (ADR 0107 decision 5): the
//! codes of PostgreSQL itself that waiting can cure, the two a pool tries again at connect, and the
//! class `OR`, the codes the orchestrator's own functions raise. Each adapter builds its port's
//! error from [`class`]; neither keeps a table of its own.
//!
//! Meanings of PostgreSQL's codes: *verified 2026-11-27* against postgresql.org, Appendix A, 18
//! documentation (class `08` connection exception, `40001` serialization failure, `40P01` deadlock
//! detected, class `53` insufficient resources, `57P01`..`57P03` admin shutdown, crash shutdown,
//! cannot connect now). It has no class `OR`, and a code of one's own is five digits or upper-case
//! letters that should not end in `000` (Errors and Messages, 18 documentation).

/// What to do about a server's error, by its SQLSTATE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlStateClass {
    /// Waiting can cure it: the connection, resources, a shutdown, a lost serialization.
    Transient,
    /// The stored data breaks a rule a function of the schema checks: [`RuleCode::Corrupt`].
    Corrupt,
    /// Retrying repeats it: a rejected statement, a constraint, an argument a function refuses, an
    /// `OR` code this build does not know.
    Internal,
}

/// The codes of the class `OR` that the orchestrator's functions and procedures raise. Closed on
/// purpose: a code added is a variant, its row in ADR 0107 and in the crate's README, and its
/// class here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleCode {
    /// `OR001`: stored data breaks an invariant the function checks (a thread's `last_seq` is not
    /// its greatest event's; a stored revision or version is not a positive number).
    Corrupt,
    /// `OR002`: a function refuses its arguments (arrays of different lengths, a negative limit, an
    /// empty owner): a caller's bug, raised before any write.
    BadArguments,
}

impl RuleCode {
    /// Every code, in the order of their numbers.
    pub const ALL: [RuleCode; 2] = [RuleCode::Corrupt, RuleCode::BadArguments];

    /// The class every code of the orchestrator's starts with.
    pub const CLASS: &'static str = "OR";

    /// Its SQLSTATE, as a function's `RAISE … USING ERRCODE` names it.
    pub const fn code(self) -> &'static str {
        match self {
            RuleCode::Corrupt => "OR001",
            RuleCode::BadArguments => "OR002",
        }
    }

    /// The code of `sqlstate`, when it is one this build knows.
    pub fn parse(sqlstate: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.code() == sqlstate)
    }

    /// What to do about it.
    pub const fn class(self) -> SqlStateClass {
        match self {
            RuleCode::Corrupt => SqlStateClass::Corrupt,
            RuleCode::BadArguments => SqlStateClass::Internal,
        }
    }
}

/// Whether PostgreSQL's code says "try again later".
pub fn is_transient(sqlstate: &str) -> bool {
    sqlstate.starts_with("08")
        || sqlstate.starts_with("53")
        || matches!(sqlstate, "40001" | "40P01" | "57P01" | "57P02" | "57P03")
}

/// Whether a connection refused with this code is one a database that is starting refuses, which
/// sqlx's pool tried again: `53300` too many connections, `57P03` cannot connect now.
pub fn is_retried_at_connect(sqlstate: &str) -> bool {
    matches!(sqlstate, "53300" | "57P03")
}

/// The class of a server's error: an `OR` code by [`RuleCode`] (one this build does not know is
/// [`SqlStateClass::Internal`]), then PostgreSQL's transient codes; everything else is internal.
pub fn class(sqlstate: &str) -> SqlStateClass {
    if let Some(rest) = sqlstate.strip_prefix(RuleCode::CLASS)
        && rest.len() == 3
    {
        return RuleCode::parse(sqlstate).map_or(SqlStateClass::Internal, RuleCode::class);
    }
    if is_transient(sqlstate) {
        SqlStateClass::Transient
    } else {
        SqlStateClass::Internal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_orchestrator_s_codes_have_their_classes() {
        assert_eq!(class("OR001"), SqlStateClass::Corrupt);
        assert_eq!(class("OR002"), SqlStateClass::Internal);
        // a code of a later release, and adam-rs's class: never retried, never read as corrupt
        assert_eq!(class("OR999"), SqlStateClass::Internal);
        assert_eq!(class("AD001"), SqlStateClass::Internal);
        for code in RuleCode::ALL {
            assert_eq!(RuleCode::parse(code.code()), Some(code));
            assert!(code.code().starts_with(RuleCode::CLASS));
            assert_eq!(code.code().len(), 5);
            assert!(!code.code().ends_with("000"), "{code:?}");
        }
        assert_eq!(RuleCode::parse("OR000"), None);
    }

    #[test]
    fn postgresql_s_codes_are_classified_by_what_retrying_can_do() {
        for code in [
            "08000", "08006", "40001", "40P01", "53100", "53300", "57P01", "57P02", "57P03",
        ] {
            assert_eq!(class(code), SqlStateClass::Transient, "{code}");
        }
        for code in [
            "42601", "42P01", "23505", "23514", "22012", "55P03", "57014", "P0001",
        ] {
            assert_eq!(class(code), SqlStateClass::Internal, "{code}");
        }
        assert!(is_retried_at_connect("53300") && is_retried_at_connect("57P03"));
        assert!(!is_retried_at_connect("08006") && !is_retried_at_connect("57P01"));
    }
}
