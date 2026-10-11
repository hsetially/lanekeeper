//! T7 (Q17, A12, S7): the read-only service credential. It is read before each fetch (so a rotated token needs no restart), it is
//! trimmed and checked, it is redacted wherever it could be printed, and a failure names neither the token nor where it lives.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use domain::Secret;
use hub_platform::git::credentials::{
    CredentialError, DEFAULT_USERNAME, GitCredential, GitCredentials, MountedFileCredentials, NoCredentials,
    SecretManagerCredentials,
};
use ports::fakes::FakeSecretSource;
use ports::{SecretError, SecretSource};

const TOKEN: &str = "ghp_PlantedTokenForTheCredentialTest0123456789";

#[tokio::test]
async fn no_credentials_means_anonymous_access() {
    assert!(NoCredentials.credential().await.unwrap().is_none());
}

#[tokio::test]
async fn secret_manager_credentials_read_the_named_secret_trimmed_with_the_documented_user_name() {
    let source = Arc::new(FakeSecretSource::new().with("git-token", &format!("  {TOKEN}\r\n")));
    let credential = SecretManagerCredentials::new(source, "git-token")
        .credential()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(credential.username, DEFAULT_USERNAME);
    assert_eq!(credential.token.expose(), TOKEN);
}

#[tokio::test]
async fn the_user_name_can_be_chosen() {
    let source = Arc::new(FakeSecretSource::new().with("git-token", TOKEN));
    let credential = SecretManagerCredentials::new(source, "git-token")
        .with_username("service-account")
        .credential()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(credential.username, "service-account");
}

/// A secret source whose value changes between reads, as a rotated secret does.
struct Rotating(Mutex<Vec<String>>);

#[async_trait]
impl SecretSource for Rotating {
    async fn get(&self, _name: &str) -> Result<Secret<String>, SecretError> {
        let mut values = self.0.lock().unwrap();
        let value = if values.len() > 1 {
            values.remove(0)
        } else {
            values[0].clone()
        };
        Ok(Secret::new(value))
    }
}

#[tokio::test]
async fn the_credential_is_read_again_for_each_fetch_so_a_rotated_token_needs_no_restart() {
    let source = Arc::new(Rotating(Mutex::new(vec![
        "ghp_old".to_owned(),
        "ghp_new".to_owned(),
    ])));
    let credentials = SecretManagerCredentials::new(source, "git-token");
    assert_eq!(
        credentials.credential().await.unwrap().unwrap().token.expose(),
        "ghp_old"
    );
    assert_eq!(
        credentials.credential().await.unwrap().unwrap().token.expose(),
        "ghp_new"
    );
}

/// A secret source that is down.
struct Down;

#[async_trait]
impl SecretSource for Down {
    async fn get(&self, _name: &str) -> Result<Secret<String>, SecretError> {
        Err(SecretError::Unavailable)
    }
}

#[tokio::test]
async fn a_credential_that_is_missing_empty_or_malformed_is_an_error_that_says_nothing_about_it() {
    for (value, expected) in [
        ("", CredentialError::Invalid),
        ("   \n", CredentialError::Invalid),
        ("tok\u{0}en", CredentialError::Invalid),
        ("to\nken", CredentialError::Invalid),
        (&"x".repeat(5000), CredentialError::Invalid),
    ] {
        let source = Arc::new(FakeSecretSource::new().with("git-token", value));
        let error = SecretManagerCredentials::new(source, "git-token")
            .credential()
            .await
            .unwrap_err();
        assert_eq!(error, expected, "{value:?}");
    }
    let absent = SecretManagerCredentials::new(Arc::new(FakeSecretSource::new()), "git-token")
        .credential()
        .await
        .unwrap_err();
    assert_eq!(absent, CredentialError::Invalid);
    let text = format!("{absent} {absent:?}");
    assert!(
        !text.contains("git-token"),
        "the error does not name the secret: {text}"
    );

    let down = SecretManagerCredentials::new(Arc::new(Down), "git-token")
        .credential()
        .await
        .unwrap_err();
    assert_eq!(
        down,
        CredentialError::Unavailable,
        "an outage is not a bad credential"
    );
}

#[tokio::test]
async fn mounted_file_credentials_read_the_file_trimmed_and_read_it_again_each_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    std::fs::write(&path, format!("{TOKEN}\n")).unwrap();
    let credentials = MountedFileCredentials::new(&path);
    assert_eq!(
        credentials.credential().await.unwrap().unwrap().token.expose(),
        TOKEN
    );
    std::fs::write(&path, "ghp_rotated\n").unwrap();
    assert_eq!(
        credentials.credential().await.unwrap().unwrap().token.expose(),
        "ghp_rotated"
    );
}

#[tokio::test]
async fn mounted_file_credentials_refuse_a_missing_huge_empty_or_binary_file_without_naming_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let missing = MountedFileCredentials::new(dir.path().join("secret-path-name"))
        .credential()
        .await
        .unwrap_err();
    assert_eq!(missing, CredentialError::Unavailable);
    assert!(!format!("{missing} {missing:?}").contains("secret-path-name"));
    for (name, bytes) in [
        ("huge", vec![b'a'; 100_000]),
        ("empty", Vec::new()),
        ("binary", vec![0xff, 0xfe, 0x00, 0x01]),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(
            MountedFileCredentials::new(&path).credential().await.unwrap_err(),
            CredentialError::Invalid,
            "{name}"
        );
    }
}

#[test]
fn a_credential_never_prints_its_token() {
    let credential = GitCredential::new("x-access-token", Secret::new(TOKEN.to_owned())).unwrap();
    let text = format!("{credential:?} {credential:#?}");
    assert!(!text.contains(TOKEN) && !text.contains("ghp_"), "{text}");
    let source = Arc::new(FakeSecretSource::new().with("git-token", TOKEN));
    let credentials = SecretManagerCredentials::new(source, "git-token");
    assert!(!format!("{credentials:?}").contains(TOKEN));
    assert!(
        GitCredential::new("", Secret::new("t".to_owned())).is_none(),
        "an empty user name"
    );
    assert!(
        GitCredential::new("a:b", Secret::new("t".to_owned())).is_none(),
        "a colon would split the basic-auth pair"
    );
}
