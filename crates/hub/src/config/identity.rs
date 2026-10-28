//! The identity settings (plan 18, T4, G26; S1, S2, S7, S9): the Entra app registration the users sign in through, who may be
//! the first admins, and where the audit chain's checkpoints go.
//!
//! The settings hold names, never secrets. The Entra client secret and the key that seals the login cookie are Secret Manager
//! secrets named by `*_REF` variables (the hub reads them at start), so the pod has no variable or volume for them. The federated
//! credential of Q25 (a projected service-account token) would need a fifth volume that the pod contract does not allow, so it is
//! not offered here: the client-secret mode is the one that exists until that question is answered.
//!
//! The group is all or nothing, as the platform's is: a pod that sets some of it is a half-written deployment and is refused by
//! name. A process built from the library's defaults has none of it, and `require_serving` refuses that.

use std::collections::BTreeSet;

use domain::Guid;
use hub_identity::config::{ClientAuth, EntraConfig, IdentityConfig, MAX_BOOTSTRAP_ADMINS};
use hub_platform::gcp::wire::CryptoKeyVersionName;
use ports::KeyRef;

use super::ConfigError;
use super::reader::Reader;

/// The public Entra authority, unless `LK_ENTRA_AUTHORITY_HOST` names another (a sovereign cloud).
pub const DEFAULT_AUTHORITY_HOST: &str = hub_identity::config::DEFAULT_AUTHORITY_HOST;
/// Where Entra sends the browser back to, on the public origin, unless `LK_ENTRA_REDIRECT_PATH` says otherwise.
pub const DEFAULT_REDIRECT_PATH: &str = "/auth/callback";
/// The most older key versions that may be named for verifying checkpoints.
const MAX_VERIFY_KEYS: usize = 8;

/// Where the audit chain's checkpoints are signed and kept (S9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointSettings {
    /// The Cloud Storage bucket (with a retention lock, outside this hub's control) that takes one object per checkpoint.
    pub bucket: String,
    /// The Cloud KMS key version that signs each checkpoint's digest. The hub signs with it and never holds a key.
    pub key: KeyRef,
    /// Older versions of the same key, whose signatures the nightly verification must still accept after a rotation.
    pub older_keys: Vec<KeyRef>,
}

/// Everything the identity module is made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentitySettings {
    /// Entra sign-in and the people who start as admins. Validated: a value that fails is a refused setting, by name.
    pub config: IdentityConfig,
    pub checkpoint: CheckpointSettings,
}

impl IdentitySettings {
    /// The host the hub's Entra HTTPS client is allowed to reach: the authority and nothing else (S18).
    #[must_use]
    pub fn authority_host(&self) -> &str {
        &self.config.entra.authority_host
    }

    /// `https://host[:port]`, the one origin a state-changing request may name in `Origin` (S3).
    #[must_use]
    pub fn public_origin(&self) -> String {
        self.config.entra.origin()
    }

    /// `None` when none of the group is set.
    pub(super) fn read(reader: &Reader<'_>) -> Result<Option<Self>, ConfigError> {
        let guid = |name: &'static str| -> Result<Option<Guid>, ConfigError> {
            reader
                .parse(name)?
                .map(|text| Guid::parse(&text).map_err(|_| ConfigError::Invalid(name)))
                .transpose()
        };
        let tenant_id = guid("LK_ENTRA_TENANT_ID")?;
        let client_id = guid("LK_ENTRA_CLIENT_ID")?;
        let public_origin = reader.parse("LK_PUBLIC_ORIGIN")?;
        let client_secret_ref = reader.secret_ref("LK_ENTRA_CLIENT_AUTH_REF")?;
        let cookie_key_ref = reader.secret_ref("LK_ENTRA_COOKIE_KEY_REF")?;
        let bucket = reader.parse("LK_AUDIT_CHECKPOINT_BUCKET")?;
        let key = kms_ref(reader, "LK_AUDIT_CHECKPOINT_KMS_REF")?;

        // Read whether or not the group is present, so that a malformed optional value is refused in a half-written pod too.
        let authority_host = reader
            .parse("LK_ENTRA_AUTHORITY_HOST")?
            .unwrap_or_else(|| DEFAULT_AUTHORITY_HOST.to_owned());
        let redirect_path = reader
            .parse("LK_ENTRA_REDIRECT_PATH")?
            .unwrap_or_else(|| DEFAULT_REDIRECT_PATH.to_owned());
        let audiences = reader.parse("LK_ENTRA_API_AUDIENCES")?;
        let bootstrap_admins = bootstrap_admins(reader)?;
        let older_keys = older_keys(reader)?;

        if tenant_id.is_none()
            && client_id.is_none()
            && public_origin.is_none()
            && client_secret_ref.is_none()
            && cookie_key_ref.is_none()
            && bucket.is_none()
            && key.is_none()
        {
            return Ok(None);
        }
        let tenant_id = tenant_id.ok_or(ConfigError::Missing("LK_ENTRA_TENANT_ID"))?;
        let client_id = client_id.ok_or(ConfigError::Missing("LK_ENTRA_CLIENT_ID"))?;
        let public_origin = public_origin.ok_or(ConfigError::Missing("LK_PUBLIC_ORIGIN"))?;
        let client_secret_ref = client_secret_ref.ok_or(ConfigError::Missing("LK_ENTRA_CLIENT_AUTH_REF"))?;
        let cookie_key_ref = cookie_key_ref.ok_or(ConfigError::Missing("LK_ENTRA_COOKIE_KEY_REF"))?;
        let bucket = bucket.ok_or(ConfigError::Missing("LK_AUDIT_CHECKPOINT_BUCKET"))?;
        let key = key.ok_or(ConfigError::Missing("LK_AUDIT_CHECKPOINT_KMS_REF"))?;
        if !valid_bucket_name(&bucket) {
            return Err(ConfigError::Invalid("LK_AUDIT_CHECKPOINT_BUCKET"));
        }

        // Entra's convention for the API's audiences: the application id URI (`api://<client id>`) and the client id itself.
        // A deployment that registered another URI names it.
        let accepted_api_audiences = match audiences {
            Some(list) => list.split(',').map(|a| a.trim().to_owned()).collect(),
            None => vec![format!("api://{client_id}"), client_id.to_string()],
        };
        let config = IdentityConfig {
            entra: EntraConfig {
                tenant_id,
                client_id,
                authority_host,
                public_origin,
                redirect_path,
                accepted_api_audiences,
                client_auth: ClientAuth::Secret {
                    name: client_secret_ref,
                },
                login_cookie_key_secret: cookie_key_ref,
            },
            bootstrap_admins,
        };
        config
            .validate()
            .map_err(|error| ConfigError::Invalid(variable_of(error)))?;
        Ok(Some(Self {
            config,
            checkpoint: CheckpointSettings {
                bucket,
                key,
                older_keys,
            },
        }))
    }
}

/// The variable behind an identity configuration error, so that the message the operator reads names what to fix.
fn variable_of(error: hub_identity::config::ConfigError) -> &'static str {
    use hub_identity::config::ConfigError as E;
    match error {
        E::AuthorityHost => "LK_ENTRA_AUTHORITY_HOST",
        E::PublicOrigin => "LK_PUBLIC_ORIGIN",
        E::RedirectPath => "LK_ENTRA_REDIRECT_PATH",
        E::SecretName | E::TokenFile => "LK_ENTRA_CLIENT_AUTH_REF",
        E::Audiences => "LK_ENTRA_API_AUDIENCES",
        E::BootstrapAdmins => "LK_BOOTSTRAP_ADMINS",
    }
}

/// A Cloud KMS crypto key version: the version, not the key, because a signature is made by one version.
fn kms_ref(reader: &Reader<'_>, name: &'static str) -> Result<Option<KeyRef>, ConfigError> {
    let Some(text) = reader.parse(name)? else {
        return Ok(None);
    };
    let key = KeyRef::parse(&text).map_err(|_| ConfigError::Invalid(name))?;
    CryptoKeyVersionName::parse(&key).ok_or(ConfigError::Invalid(name))?;
    Ok(Some(key))
}

/// Entra object ids that become active admins at their first sign-in (S1): a handful of people who set the hub up. A stray comma
/// is a typo to refuse, not an entry to skip.
fn bootstrap_admins(reader: &Reader<'_>) -> Result<Vec<Guid>, ConfigError> {
    let Some(text) = reader.parse("LK_BOOTSTRAP_ADMINS")? else {
        return Ok(Vec::new());
    };
    let invalid = || ConfigError::Invalid("LK_BOOTSTRAP_ADMINS");
    let ids: Vec<Guid> = text
        .split(',')
        .map(|id| Guid::parse(id.trim()).map_err(|_| invalid()))
        .collect::<Result<_, _>>()?;
    let distinct: BTreeSet<&Guid> = ids.iter().collect();
    if ids.len() > MAX_BOOTSTRAP_ADMINS || distinct.len() != ids.len() {
        return Err(invalid());
    }
    Ok(ids)
}

/// Older versions of the checkpoint key, comma separated.
fn older_keys(reader: &Reader<'_>) -> Result<Vec<KeyRef>, ConfigError> {
    const NAME: &str = "LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS";
    let Some(text) = reader.parse(NAME)? else {
        return Ok(Vec::new());
    };
    let keys: Vec<KeyRef> = text
        .split(',')
        .map(|part| {
            let key = KeyRef::parse(part.trim()).map_err(|_| ConfigError::Invalid(NAME))?;
            CryptoKeyVersionName::parse(&key).ok_or(ConfigError::Invalid(NAME))?;
            Ok(key)
        })
        .collect::<Result<_, ConfigError>>()?;
    if keys.len() > MAX_VERIFY_KEYS {
        return Err(ConfigError::Invalid(NAME));
    }
    Ok(keys)
}

/// A Cloud Storage bucket name: 3 to 63 characters of lower-case letters, digits, `-`, `_` and `.`, starting and ending with a
/// letter or digit. The same rule the bucket client applies, so that a name it would refuse is refused here, by variable.
fn valid_bucket_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (3..=63).contains(&bytes.len())
        && bytes
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'_' | b'.'))
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const TENANT: &str = "11111111-1111-4111-8111-111111111111";
    const CLIENT: &str = "22222222-2222-4222-8222-222222222222";
    const KEY: &str = "projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/audit-checkpoint/cryptoKeyVersions/1";

    fn env<'a>(extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut env = vec![
            ("LK_ENTRA_TENANT_ID", TENANT),
            ("LK_ENTRA_CLIENT_ID", CLIENT),
            ("LK_PUBLIC_ORIGIN", "https://lanekeeper.example.com"),
            ("LK_ENTRA_CLIENT_AUTH_REF", "lanekeeper-entra-client"),
            ("LK_ENTRA_COOKIE_KEY_REF", "lanekeeper-login-cookie"),
            ("LK_AUDIT_CHECKPOINT_BUCKET", "lanekeeper-audit-checkpoints"),
            ("LK_AUDIT_CHECKPOINT_KMS_REF", KEY),
        ];
        for (name, value) in extra {
            env.retain(|(n, _)| n != name);
            env.push((name, value));
        }
        env
    }

    fn read(pairs: &[(&str, &str)]) -> Result<Option<IdentitySettings>, ConfigError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let lookup = move |name: &str| map.get(name).cloned();
        let read_file = |_: &std::path::Path| Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        IdentitySettings::read(&Reader::new(&lookup, &read_file))
    }

    #[test]
    fn nothing_set_is_no_group_and_the_required_ones_make_the_group() {
        assert_eq!(read(&[]).unwrap(), None);
        let settings = read(&env(&[])).unwrap().unwrap();
        assert_eq!(settings.public_origin(), "https://lanekeeper.example.com");
        assert_eq!(settings.authority_host(), "login.microsoftonline.com");
        assert_eq!(settings.config.entra.redirect_path, "/auth/callback");
        assert_eq!(
            settings.config.entra.client_auth,
            ClientAuth::Secret {
                name: "lanekeeper-entra-client".to_owned()
            }
        );
        assert_eq!(
            settings.config.entra.login_cookie_key_secret,
            "lanekeeper-login-cookie"
        );
        assert!(settings.config.bootstrap_admins.is_empty());
        assert_eq!(settings.checkpoint.bucket, "lanekeeper-audit-checkpoints");
        assert!(settings.checkpoint.older_keys.is_empty());
    }

    #[test]
    fn the_audiences_default_to_the_application_id_uri_and_the_client_id() {
        let settings = read(&env(&[])).unwrap().unwrap();
        assert_eq!(
            settings.config.entra.accepted_api_audiences,
            vec![format!("api://{CLIENT}"), CLIENT.to_owned()]
        );
        let named = read(&env(&[("LK_ENTRA_API_AUDIENCES", "api://hub, api://other")]))
            .unwrap()
            .unwrap();
        assert_eq!(
            named.config.entra.accepted_api_audiences,
            vec!["api://hub".to_owned(), "api://other".to_owned()]
        );
    }

    #[test]
    fn a_half_written_group_is_refused_by_the_variable_that_is_missing() {
        for name in [
            "LK_ENTRA_TENANT_ID",
            "LK_ENTRA_CLIENT_ID",
            "LK_PUBLIC_ORIGIN",
            "LK_ENTRA_CLIENT_AUTH_REF",
            "LK_ENTRA_COOKIE_KEY_REF",
            "LK_AUDIT_CHECKPOINT_BUCKET",
            "LK_AUDIT_CHECKPOINT_KMS_REF",
        ] {
            let mut pairs = env(&[]);
            pairs.retain(|(n, _)| *n != name);
            assert_eq!(read(&pairs).unwrap_err(), ConfigError::Missing(name), "{name}");
        }
    }

    #[test]
    fn a_bad_value_is_refused_by_name_and_the_value_is_never_printed() {
        let planted = "planted-bad-value";
        for (name, value) in [
            ("LK_ENTRA_TENANT_ID", planted),
            ("LK_ENTRA_CLIENT_ID", "1234"),
            ("LK_PUBLIC_ORIGIN", "http://lanekeeper.example.com"),
            ("LK_PUBLIC_ORIGIN", "https://lanekeeper.example.com/app"),
            ("LK_ENTRA_CLIENT_AUTH_REF", "projects/p/secrets/s"),
            ("LK_ENTRA_COOKIE_KEY_REF", "has space"),
            ("LK_AUDIT_CHECKPOINT_BUCKET", "UPPER"),
            ("LK_AUDIT_CHECKPOINT_BUCKET", "ab"),
            ("LK_AUDIT_CHECKPOINT_KMS_REF", planted),
            // A key, not a key version: a signature is made by one version.
            (
                "LK_AUDIT_CHECKPOINT_KMS_REF",
                "projects/lk/locations/l/keyRings/r/cryptoKeys/audit-checkpoint",
            ),
            ("LK_ENTRA_AUTHORITY_HOST", "Login.Example"),
            ("LK_ENTRA_REDIRECT_PATH", "//evil.example"),
            ("LK_ENTRA_API_AUDIENCES", ","),
            ("LK_BOOTSTRAP_ADMINS", planted),
            ("LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS", planted),
        ] {
            let error = read(&env(&[(name, value)])).unwrap_err();
            assert_eq!(error, ConfigError::Invalid(name), "{name}={value}");
            assert!(!format!("{error} {error:?}").contains(planted));
        }
    }

    #[test]
    fn bootstrap_admins_are_a_short_list_of_distinct_object_ids() {
        let a = "33333333-3333-4333-8333-333333333333";
        let b = "44444444-4444-4444-8444-444444444444";
        let list = format!("{a}, {b}");
        let settings = read(&env(&[("LK_BOOTSTRAP_ADMINS", &list)])).unwrap().unwrap();
        assert_eq!(settings.config.bootstrap_admins.len(), 2);
        for bad in [
            format!("{a},{a}"),
            format!("{a},"),
            format!(",{a}"),
            String::from("x"),
        ] {
            assert_eq!(
                read(&env(&[("LK_BOOTSTRAP_ADMINS", &bad)])).unwrap_err(),
                ConfigError::Invalid("LK_BOOTSTRAP_ADMINS"),
                "{bad}"
            );
        }
        let too_many: Vec<String> = (0..=MAX_BOOTSTRAP_ADMINS)
            .map(|n| format!("00000000-0000-4000-8000-{n:012x}"))
            .collect();
        assert_eq!(
            read(&env(&[("LK_BOOTSTRAP_ADMINS", &too_many.join(","))])).unwrap_err(),
            ConfigError::Invalid("LK_BOOTSTRAP_ADMINS")
        );
    }

    #[test]
    fn older_checkpoint_key_versions_are_accepted_for_verification() {
        let older = KEY.replace("cryptoKeyVersions/1", "cryptoKeyVersions/0");
        let settings = read(&env(&[("LK_AUDIT_CHECKPOINT_VERIFY_KMS_REFS", &older)]))
            .unwrap()
            .unwrap();
        assert_eq!(settings.checkpoint.older_keys.len(), 1);
        assert_eq!(settings.checkpoint.older_keys[0].as_str(), older);
    }

    #[test]
    fn a_malformed_optional_value_is_refused_even_when_the_required_ones_are_absent() {
        assert_eq!(
            read(&[("LK_BOOTSTRAP_ADMINS", "x")]).unwrap_err(),
            ConfigError::Invalid("LK_BOOTSTRAP_ADMINS")
        );
    }
}
