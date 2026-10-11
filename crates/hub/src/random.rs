//! Random identifiers from the operating system's CSPRNG through aws-lc-rs, the stack's one crypto provider.
//!
//! Request ids and CSP nonces come from here. A nonce must be unguessable (S12), so `fastrand` is not used.

use aws_lc_rs::rand::{SecureRandom, SystemRandom};

/// The random source failed. It does not happen on a healthy system, and the caller answers with a 500 when it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the system random source failed")]
pub struct RandomError;

/// `bytes` random bytes, as lower-case hex (so `2 * bytes` characters).
pub fn random_hex(bytes: usize) -> Result<String, RandomError> {
    let mut buffer = vec![0u8; bytes];
    SystemRandom::new().fill(&mut buffer).map_err(|_| RandomError)?;
    Ok(hex::encode(buffer))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn hex_has_the_requested_length_and_alphabet() {
        let value = random_hex(16).unwrap_or_default();
        assert_eq!(value.len(), 32);
        assert!(value.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn values_do_not_repeat() {
        let seen: HashSet<String> = (0..1000).filter_map(|_| random_hex(16).ok()).collect();
        assert_eq!(seen.len(), 1000);
    }
}
