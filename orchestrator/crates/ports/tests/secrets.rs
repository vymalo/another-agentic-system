//! The secret material and the keys of the secret store (ADR 0046, the amendment of 2026-10-11),
//! through the public API.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::{Classify, ErrorClass};
use orch_ports::{
    DefinitionError, MAX_SECRET_BYTES, MaterialTooLarge, SecretError, SecretMaterial, SecretServer,
    SecretVersion, UserToolServerId,
};

const SENTINEL: &str = "sentinel-5d0e9b-do-not-print";

#[test]
fn material_is_at_most_sixteen_kib() {
    assert_eq!(MAX_SECRET_BYTES, 16 * 1024);
    assert!(SecretMaterial::new(Vec::new()).is_ok());
    assert!(SecretMaterial::new(vec![7; MAX_SECRET_BYTES]).is_ok());
    let error = SecretMaterial::new(vec![7; MAX_SECRET_BYTES + 1]).unwrap_err();
    assert_eq!(
        error,
        MaterialTooLarge {
            len: MAX_SECRET_BYTES + 1
        }
    );
    assert_eq!(error.class(), ErrorClass::Invalid);
    assert!(error.to_string().contains("16384"), "{error}");
    // from a slice as well as from a vector
    assert!(SecretMaterial::new(&[1_u8, 2, 3][..]).is_ok());
    assert!(SecretMaterial::new(&vec![0_u8; MAX_SECRET_BYTES + 1][..]).is_err());
}

#[test]
fn material_reads_back_as_it_was_given() {
    let bytes: Vec<u8> = (0..=255).collect();
    let material = SecretMaterial::new(bytes.clone()).unwrap();
    assert_eq!(material.expose(), bytes.as_slice());
    assert_eq!(material.len(), 256);
    assert!(!material.is_empty());
    assert!(SecretMaterial::new(Vec::new()).unwrap().is_empty());
    let copy = material.clone();
    assert_eq!(copy.expose(), material.expose());
}

#[test]
fn material_prints_nothing() {
    let material = SecretMaterial::new(SENTINEL.as_bytes().to_vec()).unwrap();
    assert_eq!(format!("{material:?}"), "<redacted>");
    assert_eq!(format!("{material:#?}"), "<redacted>");
    // inside what holds it
    let held = Some((SecretVersion::new(3), material.clone()));
    let text = format!("{held:?} {:?}", [&material]);
    assert!(!text.contains(SENTINEL), "{text}");
    assert!(text.contains("<redacted>"));
    // the error of a material that is too large does not carry it
    let error = SecretMaterial::new(vec![b'x'; MAX_SECRET_BYTES + 1]).unwrap_err();
    assert!(!format!("{error} {error:?}").contains("xxxx"));
}

#[test]
fn material_is_wiped_when_it_is_dropped() {
    // the type says so: this does not compile unless `SecretMaterial` is `ZeroizeOnDrop`
    fn wiped_on_drop<T: zeroize::ZeroizeOnDrop>() {}
    wiped_on_drop::<SecretMaterial>();
}

#[test]
fn a_secret_is_kept_under_a_server_or_under_its_pending_flow() {
    let id = UserToolServerId::parse("u-abcdefghij").unwrap();
    let credential = SecretServer::credential(&id);
    let pending = SecretServer::pending_flow(&id);
    assert_ne!(credential, pending);
    assert_eq!(credential.to_string(), "u-abcdefghij");
    assert_eq!(pending.to_string(), "u-abcdefghij#flow");
    assert_eq!(credential.server_id(), &id);
    assert_eq!(pending.server_id(), &id);
    assert!(!credential.is_pending_flow());
    assert!(pending.is_pending_flow());
    assert_eq!(SecretServer::parse("u-abcdefghij").unwrap(), credential);
    assert_eq!(SecretServer::parse("u-abcdefghij#flow").unwrap(), pending);
    assert_eq!(credential, SecretServer::credential(&id));

    for bad in [
        "",
        "u-abcdefghij#",
        "u-abcdefghij#flows",
        "u-abcdefghij#Flow",
        "u-abcdefghij#flow#flow",
        "u-abcdefghij flow",
        "#flow",
        "u-abc#flow",
        "../u-abcdefghij",
        "deployment-id",
    ] {
        assert_eq!(
            SecretServer::parse(bad).unwrap_err(),
            DefinitionError::BadId,
            "{bad:?}"
        );
    }
}

#[test]
fn versions_are_ordered_and_opaque_numbers() {
    assert!(SecretVersion::new(2) > SecretVersion::new(1));
    assert_eq!(SecretVersion::new(5).get(), 5);
    assert_eq!(SecretVersion::new(5).to_string(), "5");
    assert_eq!(SecretVersion::new(5), SecretVersion::new(5));
}

#[test]
fn each_secret_error_has_the_class_that_says_what_to_do() {
    let cases: [(SecretError, ErrorClass); 4] = [
        (SecretError::Conflict, ErrorClass::Conflict),
        (
            SecretError::unavailable("the database"),
            ErrorClass::Transient,
        ),
        // a row sealed under a key the deployment no longer holds: not retryable, and an operator
        // is told
        (SecretError::KeyUnknown, ErrorClass::Corrupt),
        (
            SecretError::corrupt("a row that does not open"),
            ErrorClass::Corrupt,
        ),
    ];
    for (error, class) in cases {
        assert_eq!(error.class(), class, "{error}");
        assert!(!error.to_string().is_empty());
    }
    assert!(SecretError::Conflict.is_retryable());
    assert!(SecretError::unavailable("x").is_retryable());
    assert!(!SecretError::KeyUnknown.is_retryable());
    assert!(SecretError::KeyUnknown.class().should_alert());
}

#[test]
fn an_unavailable_error_keeps_its_cause_and_no_other_variant_does() {
    let error = SecretError::unavailable("could not reach the database")
        .with_source(std::io::Error::other("connection refused"));
    assert!(std::error::Error::source(&error).is_some());
    let unchanged = SecretError::Conflict.with_source(std::io::Error::other("x"));
    assert!(matches!(unchanged, SecretError::Conflict));
    assert!(std::error::Error::source(&unchanged).is_none());
}
