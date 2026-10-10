//! Where the key and certificate are kept between restarts: the certificate Secret (S5, S17).
//!
//! The Secret is created by the chart and named in `LK_CERT_SECRET`. The agent may `get` and `update` exactly that
//! Secret and nothing else (A2), so it never creates it, never lists, and never touches another one. It stores PEM under
//! the conventional TLS entry names, so an operator can inspect it with `kubectl` and `openssl`.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use async_trait::async_trait;
use bytes::Bytes;
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::Secret;
use kube::Client;
use kube::api::{Api, PostParams};
use zeroize::Zeroize;

use super::cert::ClientIdentity;
use super::error::{KubeError, StoreError};
use super::key::KeyMaterial;
use super::kubecall::kube_call;
use crate::config::KubeName;

/// The Secret entries, as in a `kubernetes.io/tls` Secret.
const KEY_ENTRY: &str = "tls.key";
const CERT_ENTRY: &str = "tls.crt";
const CERT_LABEL: &str = "CERTIFICATE";

const READ: &str = "read the certificate Secret";
const UPDATE: &str = "update the certificate Secret";
/// A save is retried this many times when someone else wrote the Secret in between.
const SAVE_ATTEMPTS: usize = 3;

/// What a store holds: the key and the certificate chain (DER, leaf first). It has not been checked yet; run it through
/// [`ClientIdentity::verify`].
#[derive(Debug)]
pub struct StoredIdentity {
    pub key: KeyMaterial,
    pub chain_der: Vec<Bytes>,
}

#[async_trait]
pub trait CertStore: Send + Sync + fmt::Debug {
    /// The stored identity, or `None` when the store is empty. A store whose contents cannot be read is an error.
    async fn load(&self) -> Result<Option<StoredIdentity>, StoreError>;

    /// Replace the stored identity.
    async fn save(&self, identity: &ClientIdentity) -> Result<(), StoreError>;

    /// Check that [`CertStore::save`] can work, before a single-use credential is spent on a join.
    async fn probe(&self) -> Result<(), StoreError>;
}

/// The certificate Secret in the agent's own namespace.
pub struct KubeCertStore {
    api: Api<Secret>,
    name: String,
}

impl KubeCertStore {
    /// `client` is namespaced to the pod's own namespace, which is where the chart creates the Secret.
    pub fn new(client: Client, name: &KubeName) -> Self {
        Self {
            api: Api::default_namespaced(client),
            name: name.as_str().to_owned(),
        }
    }

    async fn read(&self) -> Result<Secret, StoreError> {
        kube_call(READ, self.api.get_opt(&self.name))
            .await?
            .ok_or(StoreError::SecretMissing)
    }

    async fn write(&self, secret: &Secret) -> Result<(), KubeError> {
        kube_call(
            UPDATE,
            self.api.replace(&self.name, &PostParams::default(), secret),
        )
        .await
        .map(drop)
    }
}

impl fmt::Debug for KubeCertStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KubeCertStore")
            .field("secret", &self.name)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl CertStore for KubeCertStore {
    async fn load(&self) -> Result<Option<StoredIdentity>, StoreError> {
        let secret = self.read().await?;
        let mut data = secret.data.unwrap_or_default();
        let parsed = parse_entries(&data);
        // The PEM text of the key was deserialized into the object; wipe our copy now that it has been parsed.
        if let Some(entry) = data.get_mut(KEY_ENTRY) {
            entry.0.zeroize();
        }
        parsed
    }

    async fn save(&self, identity: &ClientIdentity) -> Result<(), StoreError> {
        let mut key_pem = identity.key().to_pem().expose().clone().into_bytes();
        let chain_pem = encode_chain(identity.chain_der());
        let mut outcome = Err(StoreError::SecretMissing);
        for _ in 0..SAVE_ATTEMPTS {
            // Read first, so the update carries the current resourceVersion and every entry we do not own.
            let mut secret = match self.read().await {
                Ok(secret) => secret,
                Err(e) => {
                    outcome = Err(e);
                    break;
                }
            };
            let data = secret.data.get_or_insert_with(Default::default);
            // The key being replaced is a private key too.
            if let Some(mut old) = data.insert(KEY_ENTRY.to_owned(), ByteString(key_pem.clone())) {
                old.0.zeroize();
            }
            data.insert(CERT_ENTRY.to_owned(), ByteString(chain_pem.clone()));
            let written = self.write(&secret).await;
            // The request body has been sent; wipe our copy of the key from the object before it is dropped.
            if let Some(entry) = secret.data.as_mut().and_then(|d| d.get_mut(KEY_ENTRY)) {
                entry.0.zeroize();
            }
            match written {
                Ok(()) => {
                    outcome = Ok(());
                    break;
                }
                Err(KubeError::Status { status: 409, op }) => {
                    outcome = Err(StoreError::Api(KubeError::Status { op, status: 409 }));
                }
                Err(e) => {
                    outcome = Err(StoreError::Api(e));
                    break;
                }
            }
        }
        key_pem.zeroize();
        outcome
    }

    async fn probe(&self) -> Result<(), StoreError> {
        // Writing the Secret back unchanged proves that `update` is allowed without altering anything.
        let secret = self.read().await?;
        self.write(&secret).await?;
        Ok(())
    }
}

/// The identity in the Secret's entries, or `None` for the chart's placeholder.
fn parse_entries(data: &BTreeMap<String, ByteString>) -> Result<Option<StoredIdentity>, StoreError> {
    let entry = |name: &str| data.get(name).map(|v| v.0.as_slice()).filter(|v| !v.is_empty());
    // The placeholder has the entries empty or absent. Half an identity is the same as none.
    let (Some(key), Some(chain)) = (entry(KEY_ENTRY), entry(CERT_ENTRY)) else {
        return Ok(None);
    };
    let key = KeyMaterial::from_pem(key).map_err(|_| StoreError::Malformed)?;
    let chain_der = parse_chain(chain)?;
    Ok(Some(StoredIdentity { key, chain_der }))
}

/// PEM text of a chain, leaf first.
fn encode_chain(chain: &[Bytes]) -> Vec<u8> {
    let blocks: Vec<pem::Pem> = chain
        .iter()
        .map(|der| pem::Pem::new(CERT_LABEL, der.to_vec()))
        .collect();
    pem::encode_many_config(
        &blocks,
        pem::EncodeConfig::new().set_line_ending(pem::LineEnding::LF),
    )
    .into_bytes()
}

/// The certificates in a PEM chain, as DER, at most the contract's limit. Anything else in the text is a fault.
fn parse_chain(text: &[u8]) -> Result<Vec<Bytes>, StoreError> {
    let blocks = pem::parse_many(text).map_err(|_| StoreError::Malformed)?;
    let in_range = !blocks.is_empty() && blocks.len() <= proto::limits::MAX_CERT_CHAIN;
    if !in_range
        || blocks
            .iter()
            .any(|b| b.tag() != CERT_LABEL || b.contents().is_empty())
    {
        return Err(StoreError::Malformed);
    }
    Ok(blocks
        .into_iter()
        .map(|b| Bytes::from(b.into_contents()))
        .collect())
}

/// A store that keeps the identity in memory. For tests, and for a development run with no cluster.
#[derive(Debug, Default)]
pub struct MemoryCertStore {
    inner: Mutex<MemoryState>,
}

#[derive(Debug, Default)]
struct MemoryState {
    stored: Option<(KeyMaterial, Vec<Bytes>)>,
    saves: usize,
}

impl MemoryCertStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A store that already holds an identity (not yet checked), as after a restart.
    pub fn with(key: KeyMaterial, chain_der: Vec<Bytes>) -> Self {
        Self {
            inner: Mutex::new(MemoryState {
                stored: Some((key, chain_der)),
                saves: 0,
            }),
        }
    }

    /// How many times [`CertStore::save`] has been called.
    pub fn saves(&self) -> usize {
        self.lock().saves
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryState> {
        // A poisoned lock only means a test thread panicked while holding it; the data is still consistent.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl CertStore for MemoryCertStore {
    async fn load(&self) -> Result<Option<StoredIdentity>, StoreError> {
        let state = self.lock();
        let Some((key, chain)) = &state.stored else {
            return Ok(None);
        };
        // A key is not cloneable, so the copy goes through its PKCS#8 bytes.
        let key = KeyMaterial::from_pkcs8_der(key.pkcs8_der().to_vec()).map_err(|_| StoreError::Malformed)?;
        Ok(Some(StoredIdentity {
            key,
            chain_der: chain.clone(),
        }))
    }

    async fn save(&self, identity: &ClientIdentity) -> Result<(), StoreError> {
        let key = KeyMaterial::from_pkcs8_der(identity.key().pkcs8_der().to_vec())
            .map_err(|_| StoreError::Malformed)?;
        let mut state = self.lock();
        state.stored = Some((key, identity.chain_der().to_vec()));
        state.saves += 1;
        Ok(())
    }

    async fn probe(&self) -> Result<(), StoreError> {
        Ok(())
    }
}
