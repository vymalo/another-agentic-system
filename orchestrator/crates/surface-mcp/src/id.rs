//! The thread id of a `start_job` with a `client_request_id` (ADR 0019).

use orch_core::{ThreadId, UserId};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The most bytes a `client_request_id` may have.
pub const MAX_CLIENT_REQUEST_ID_BYTES: usize = 256;

/// The domain of the hash: a different use of the same digest would need a different tag.
const DOMAIN: &[u8] = b"another-agentic-system/mcp/start_job/v1";

/// The id of the job that `start_job` creates for `(user, client_request_id)`: the same pair
/// always gives the same id, on any replica, so a retried call finds the thread it created.
///
/// It is the first 16 bytes of SHA-256 over a tag, the length-prefixed user and the request id,
/// with the version and variant bits of a UUID set (version 8, "custom"). The user is part of the
/// hash, so two users who pick the same `client_request_id` get different jobs; and the id is
/// not a secret, because a thread is only ever served to its owner.
pub fn job_id_for(user: &UserId, client_request_id: &str) -> ThreadId {
    let mut hash = Sha256::new();
    hash.update(DOMAIN);
    // Length prefixes, so ("ab", "c") and ("a", "bc") cannot collide.
    hash.update((user.as_str().len() as u64).to_be_bytes());
    hash.update(user.as_str().as_bytes());
    hash.update((client_request_id.len() as u64).to_be_bytes());
    hash.update(client_request_id.as_bytes());
    let digest = hash.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80; // version 8
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant RFC 9562
    ThreadId(Uuid::from_bytes(bytes))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_same_user_and_request_id_give_the_same_job() {
        let alice = UserId::new("alice@example.com");
        assert_eq!(job_id_for(&alice, "r-1"), job_id_for(&alice, "r-1"));
        // The user is normalised before it is hashed, like everywhere else.
        assert_eq!(
            job_id_for(&UserId::new(" Alice@Example.com "), "r-1"),
            job_id_for(&alice, "r-1")
        );
    }

    #[test]
    fn a_different_user_or_request_id_gives_a_different_job() {
        let alice = UserId::new("alice@example.com");
        let bob = UserId::new("bob@example.com");
        assert_ne!(job_id_for(&alice, "r-1"), job_id_for(&alice, "r-2"));
        assert_ne!(job_id_for(&alice, "r-1"), job_id_for(&bob, "r-1"));
        // The boundary between the two parts is part of the hash.
        assert_ne!(
            job_id_for(&UserId::new("ab"), "c"),
            job_id_for(&UserId::new("a"), "bc")
        );
    }

    #[test]
    fn the_id_is_a_well_formed_uuid_that_parses_back() {
        let id = job_id_for(&UserId::new("alice@example.com"), "r-1");
        assert_eq!(id.0.get_version_num(), 8);
        assert_eq!(id.to_string().parse::<ThreadId>().unwrap(), id);
    }
}
