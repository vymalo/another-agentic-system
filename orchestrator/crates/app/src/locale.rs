//! The interface locale of a message (ADR 0085, [`docs/api/locale-v1.md`](../../../../docs/api/locale-v1.md)):
//! what a run names, and what is checked about it before anything is written.
//!
//! The orchestrator decides nothing with it. It reads `forwardedProps["vymalo.locale"]`, checks that
//! it is a BCP 47 tag ([`parse`], **400**), and carries it on the message: the core puts it on the
//! delegation (or the steer, or the text a Stop & send holds), the outbox row keeps it, and the
//! adapter sends it to an agent whose live card lists `locale/v1`. **Nothing of it is written to the
//! log**, so there is nothing to project, nothing to export and nothing to inherit: a message that
//! names no locale carries none, and a redelivery of a message carries none either.

use orch_core::LocaleHint;
use serde_json::Value;

use crate::AppError;

/// The key of a run's `forwardedProps` that carries the message's interface locale (`locale/v1`): a
/// BCP 47 language tag, for example `en-GB`.
pub const THREAD_LOCALE_KEY: &str = "vymalo.locale";

/// Why a locale was refused before it could be carried: **400**.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LocaleRequestError(String);

impl From<LocaleRequestError> for AppError {
    fn from(error: LocaleRequestError) -> Self {
        AppError::Invalid(error.0)
    }
}

/// Reads `forwardedProps["vymalo.locale"]`: a well-formed BCP 47 tag, or `null` for none.
///
/// # Errors
/// [`LocaleRequestError`] for anything else: a number, an object, an empty string, a tag that is not
/// one (`en_CM`, `en CM`, `en-`). The refusal names the key and an example, and never repeats what
/// the client sent.
pub fn parse(value: &Value) -> Result<Option<LocaleHint>, LocaleRequestError> {
    match value {
        Value::Null => Ok(None),
        Value::String(tag) => LocaleHint::parse(tag).map(Some).map_err(|_| not_a_locale()),
        _ => Err(not_a_locale()),
    }
}

fn not_a_locale() -> LocaleRequestError {
    LocaleRequestError(format!(
        "{THREAD_LOCALE_KEY} must be a BCP 47 language tag, for example en-GB"
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_tag_is_read_and_none_is_none() {
        assert_eq!(
            parse(&json!("fr-FR"))
                .unwrap()
                .map(|l| l.as_str().to_owned()),
            Some("fr-FR".to_owned())
        );
        assert_eq!(parse(&json!(null)).unwrap(), None);
    }

    #[test]
    fn anything_that_is_not_a_tag_is_refused_and_names_the_key() {
        for bad in [
            json!(""),
            json!("en_CM"),
            json!("en CM"),
            json!("en-"),
            json!("klingon please"),
            json!(3),
            json!(["en-GB"]),
            json!({"locale": "en-GB"}),
        ] {
            let error = parse(&bad).unwrap_err();
            assert!(
                error
                    .to_string()
                    .starts_with("vymalo.locale must be a BCP 47"),
                "{bad}: {error}"
            );
        }
        // the refusal never repeats what the client sent
        let error = parse(&json!("<script>")).unwrap_err();
        assert!(!error.to_string().contains("script"), "{error}");
    }
}
