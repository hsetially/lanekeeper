use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use domain::Secret;

use crate::{SecretError, SecretSource, is_valid_secret_name};

/// A fixed set of named secrets.
#[derive(Clone, Default)]
pub struct FakeSecretSource {
    secrets: Arc<HashMap<String, String>>,
}

impl std::fmt::Debug for FakeSecretSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Names only; values are secrets.
        let mut names: Vec<_> = self.secrets.keys().collect();
        names.sort();
        f.debug_struct("FakeSecretSource").field("names", &names).finish()
    }
}

impl FakeSecretSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a secret. Builder style, for test setup.
    #[must_use]
    pub fn with(self, name: &str, value: &str) -> Self {
        let mut map = (*self.secrets).clone();
        map.insert(name.to_owned(), value.to_owned());
        Self {
            secrets: Arc::new(map),
        }
    }
}

#[async_trait]
impl SecretSource for FakeSecretSource {
    async fn get(&self, name: &str) -> Result<Secret<String>, SecretError> {
        if !is_valid_secret_name(name) {
            return Err(SecretError::InvalidName);
        }
        self.secrets
            .get(name)
            .map(|v| Secret::new(v.clone()))
            .ok_or(SecretError::NotFound)
    }
}
