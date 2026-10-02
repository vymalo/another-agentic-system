//! What a validated token says about the person: the user, the e-mail, the name and the roles.
//!
//! The signature, the issuer, the audience and the times are checked before this runs; this reads
//! the claims the deployment configured (ADR 0033: `auth.jwt.userClaim`, `auth.jwt.rolesClaim`).
//! A reason it gives for a refusal names a claim by its configured name and never shows a value.

use std::collections::BTreeSet;

use orch_core::{Timestamp, UserId};
use orch_ports::{AuthError, Principal, Role};
use serde_json::{Map, Value};

/// The claim of the e-mail address, which `email_verified` speaks about.
const EMAIL: &str = "email";

/// The claim of the display name.
const NAME: &str = "name";

/// The claim that says whether the issuer verified the e-mail address.
const EMAIL_VERIFIED: &str = "email_verified";

/// A claim by its name, then, when `path` has dots and there is no claim of that exact name (a
/// namespaced claim such as `https://example.com/roles` has dots in its name), by the path
/// through the objects: `realm_access.roles`.
fn lookup<'a>(claims: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    if let Some(value) = claims.get(path) {
        return Some(value);
    }
    let (head, rest) = path.split_once('.')?;
    let mut value = claims.get(head)?;
    for part in rest.split('.') {
        value = value.as_object()?.get(part)?;
    }
    Some(value)
}

fn text<'a>(claims: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    lookup(claims, name)?
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// The roles at `path`: the texts of a list (anything else in it is skipped), or a single text.
/// A claim that is absent or of another kind is no roles.
fn roles(claims: &Map<String, Value>, path: &str) -> BTreeSet<Role> {
    match lookup(claims, path) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .filter(|r| !r.is_empty())
            .map(Role::new)
            .collect(),
        Some(Value::String(role)) if !role.is_empty() => BTreeSet::from([Role::new(role.as_str())]),
        _ => BTreeSet::new(),
    }
}

/// Whether the token says its e-mail is not verified: `false`, or the text `false` that some
/// issuers send. An absent claim says nothing.
fn email_unverified(claims: &Map<String, Value>) -> bool {
    match claims.get(EMAIL_VERIFIED) {
        Some(Value::Bool(verified)) => !verified,
        Some(Value::String(text)) => text.trim().eq_ignore_ascii_case("false"),
        _ => false,
    }
}

/// The principal a token with `claims` names.
///
/// # Errors
/// `Invalid` (a bearer) when the e-mail is not verified, or when the configured user claim is
/// absent, is not text, or is empty.
pub(crate) fn principal(
    claims: &Map<String, Value>,
    user_claim: &str,
    roles_claim: Option<&str>,
) -> Result<Principal, AuthError> {
    if email_unverified(claims) {
        return Err(AuthError::invalid_bearer(
            "the token's e-mail is not verified",
        ));
    }
    let user = text(claims, user_claim).ok_or_else(|| {
        AuthError::invalid_bearer(format!("the token has no usable {user_claim:?} claim"))
    })?;
    Ok(Principal {
        user: UserId::new(user),
        email: text(claims, EMAIL).map(str::to_owned),
        name: text(claims, NAME).map(str::to_owned),
        roles: roles_claim
            .map(|path| roles(claims, path))
            .unwrap_or_default(),
        // The library has checked `exp` (it is required); a value that is not a time cannot be
        // here, and would only mean a stream that is not bounded by it.
        expires_at: claims
            .get("exp")
            .and_then(Value::as_i64)
            .and_then(|seconds| Timestamp::from_second(seconds).ok()),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claims(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn a_claim_is_found_by_its_exact_name_before_its_path() {
        let c = claims(json!({
            "https://example.com/roles": ["x"],
            "realm_access": {"roles": ["r"], "deep": {"er": ["d"]}},
            "a.b": "flat",
            "a": {"b": "nested"},
        }));
        assert_eq!(lookup(&c, "https://example.com/roles"), Some(&json!(["x"])));
        assert_eq!(lookup(&c, "realm_access.roles"), Some(&json!(["r"])));
        assert_eq!(lookup(&c, "realm_access.deep.er"), Some(&json!(["d"])));
        assert_eq!(lookup(&c, "a.b"), Some(&json!("flat")));
        assert_eq!(lookup(&c, "realm_access.nope"), None);
        assert_eq!(lookup(&c, "nope.roles"), None);
        assert_eq!(lookup(&c, "realm_access.roles.x"), None);
    }

    #[test]
    fn roles_are_the_texts_and_nothing_makes_them_fail() {
        let at = |v: Value| roles(&claims(json!({"g": v})), "g");
        let names = |s: BTreeSet<Role>| s.iter().map(|r| r.as_str().to_owned()).collect::<Vec<_>>();
        assert_eq!(names(at(json!(["b", "a", "b", "", 3, null]))), ["a", "b"]);
        assert_eq!(names(at(json!("solo"))), ["solo"]);
        assert!(at(json!("")).is_empty());
        assert!(at(json!(7)).is_empty());
        assert!(at(json!({"a": 1})).is_empty());
        assert!(roles(&claims(json!({})), "g").is_empty());
    }

    #[test]
    fn the_principal_is_the_user_claim_with_the_email_and_the_name() {
        let c = claims(
            json!({"email": " Alice@Example.com ", "preferred_username": "Al", "name": "Alice", "g": ["user"]}),
        );
        let p = principal(&c, "preferred_username", Some("g")).unwrap();
        assert_eq!(p.user.as_str(), "al");
        assert_eq!(p.email.as_deref(), Some("Alice@Example.com"));
        assert_eq!(p.name.as_deref(), Some("Alice"));
        assert_eq!(p.roles.len(), 1);
        let p = principal(&c, "email", None).unwrap();
        assert_eq!(p.user.as_str(), "alice@example.com");
        assert!(p.roles.is_empty());
    }

    #[test]
    fn the_principal_carries_the_expiry_of_the_token() {
        let at = |value: Value| {
            principal(
                &claims(json!({"email": "a@b.c", "exp": value})),
                "email",
                None,
            )
            .unwrap()
            .expires_at
            .map(|t| t.as_second())
        };
        assert_eq!(at(json!(1_800_000_000)), Some(1_800_000_000));
        // Something that is not a time bounds nothing (the library refuses a token with no `exp`).
        assert_eq!(at(json!("soon")), None);
        let none = principal(&claims(json!({"email": "a@b.c"})), "email", None).unwrap();
        assert_eq!(none.expires_at, None);
    }

    #[test]
    fn an_unusable_user_claim_refuses_the_token_without_showing_values() {
        for value in [
            json!({}),
            json!({"email": 5}),
            json!({"email": "  "}),
            json!({"email": ["a@b"]}),
        ] {
            let err = principal(&claims(value), "email", None).unwrap_err();
            let shown = err.to_string();
            assert!(shown.contains("\"email\""), "{shown}");
        }
    }

    #[test]
    fn an_unverified_email_refuses_the_token_whatever_the_user_claim_is() {
        for unverified in [json!(false), json!("false"), json!(" FALSE ")] {
            let c = claims(json!({"email": "a@b", "sub": "s", "email_verified": unverified}));
            assert!(principal(&c, "email", None).is_err());
            assert!(principal(&c, "sub", None).is_err());
        }
        for fine in [json!(true), json!("true"), json!(1), json!(null)] {
            let c = claims(json!({"email": "a@b", "email_verified": fine}));
            assert!(principal(&c, "email", None).is_ok());
        }
        assert!(principal(&claims(json!({"email": "a@b"})), "email", None).is_ok());
    }
}
