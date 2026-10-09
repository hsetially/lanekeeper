use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;

use super::util::lock;
use crate::{LeaseBackend, LeaseError, LeaseGuard, Leases, check_ttl, is_valid_lease_name};

/// Most distinct lease names the fake tracks.
const MAX_LEASES: usize = 10_000;

#[derive(Default)]
struct State {
    next_token: u64,
    held: HashMap<String, (u64, Instant)>,
}

struct Inner {
    state: Mutex<State>,
}

/// Leases that expire on the tokio clock, so `tokio::time::pause` and `advance` control them.
#[derive(Clone)]
pub struct FakeLeases {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for FakeLeases {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeLeases").finish_non_exhaustive()
    }
}

impl FakeLeases {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State::default()),
            }),
        }
    }
}

impl Default for FakeLeases {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LeaseBackend for Inner {
    async fn renew(&self, name: &str, token: u64, ttl: Duration) -> Result<Instant, LeaseError> {
        let mut st = lock(&self.state);
        let now = Instant::now();
        match st.held.get_mut(name) {
            Some((t, exp)) if *t == token && now < *exp => {
                *exp = now + ttl;
                Ok(*exp)
            }
            _ => Err(LeaseError::Lost),
        }
    }

    async fn release(&self, name: &str, token: u64) -> Result<(), LeaseError> {
        let mut st = lock(&self.state);
        let now = Instant::now();
        match st.held.get(name) {
            Some((t, exp)) if *t == token && now < *exp => {
                st.held.remove(name);
                Ok(())
            }
            _ => Err(LeaseError::Lost),
        }
    }
}

#[async_trait]
impl Leases for FakeLeases {
    async fn try_acquire(&self, name: &str, ttl: Duration) -> Result<Option<LeaseGuard>, LeaseError> {
        if !is_valid_lease_name(name) {
            return Err(LeaseError::InvalidName);
        }
        check_ttl(ttl)?;
        let mut st = lock(&self.inner.state);
        let now = Instant::now();
        // Forget expired leases first so that the map stays bounded by live leases.
        st.held.retain(|_, (_, exp)| now < *exp);
        if st.held.contains_key(name) {
            return Ok(None);
        }
        if st.held.len() >= MAX_LEASES {
            return Err(LeaseError::Unavailable);
        }
        st.next_token += 1;
        let token = st.next_token;
        let expires_at = now + ttl;
        st.held.insert(name.to_owned(), (token, expires_at));
        Ok(Some(LeaseGuard::new(
            name.to_owned(),
            token,
            expires_at,
            self.inner.clone(),
        )))
    }
}
