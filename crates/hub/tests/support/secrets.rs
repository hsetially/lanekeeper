//! Secrets a test can change while the hub runs: what Secret Manager is to the real hub, in memory.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use domain::Secret;
use ports::{SecretError, SecretSource, is_valid_secret_name};

#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<String, String>>);

impl std::fmt::Debug for MemorySecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Names only; values are secrets.
        f.debug_struct("MemorySecrets").finish_non_exhaustive()
    }
}

impl MemorySecrets {
    pub fn put(&self, name: &str, value: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_owned(), value.to_owned());
    }

    pub fn remove(&self, name: &str) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).remove(name);
    }
}

#[async_trait]
impl SecretSource for MemorySecrets {
    async fn get(&self, name: &str) -> Result<Secret<String>, SecretError> {
        if !is_valid_secret_name(name) {
            return Err(SecretError::InvalidName);
        }
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .map(|v| Secret::new(v.clone()))
            .ok_or(SecretError::NotFound)
    }
}
