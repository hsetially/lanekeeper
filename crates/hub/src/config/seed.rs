//! The settings of `hub seed` (plan 18, T7, G24; S6, S7, S10): the application role's credential, the CA that signed the server, and a
//! limit on how long the whole run may take.
//!
//! These are not `HubSettings`. A seed Job carries the database credential and the CA file and nothing else of the hub's, and the
//! serving hub's pod does not carry `LK_SEED_TIMEOUT_SECS` (`tests/hardening.rs` holds the pod to that).
//!
//! **Which credential.** The one the hub itself uses, `lanekeeper_app`, read the same way (`LK_DATABASE_URL_FILE`, a mounted file,
//! or `LK_DATABASE_URL` for local development) and checked to be that role before and after connecting (`crate::seed`): the seed
//! writes the rows the app role may write and nothing else, and it is refused as the migrator or the superuser, whose reach is wider.

use std::path::PathBuf;
use std::time::Duration;

use domain::Secret;

use super::ConfigError;
use super::database::DatabaseSettings;
use super::reader::Reader;

/// How long a run may take unless `LK_SEED_TIMEOUT_SECS` says otherwise: a file of a few hundred rows is a few statements and one
/// audit batch, so two minutes is a generous bound that still ends a stuck run.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// The variable only the seed Job sets. The hub pod must not carry it (`tests/hardening.rs`). The credential and the CA are the hub's
/// own variables and are read the same way, so they are not listed: the Job mounts what the pod mounts.
pub const SEED_JOB_ONLY_VARIABLES: [&str; 1] = ["LK_SEED_TIMEOUT_SECS"];

/// What `hub seed` needs.
#[derive(Debug)]
pub struct SeedSettings {
    /// The connection URL of `lanekeeper_app`. Holds the password: redacted in `Debug`, zeroized on drop.
    pub url: Secret<String>,
    /// The CA that signed the database server's certificate. Required: there is no "trust the system" mode.
    pub ca_file: PathBuf,
    /// The longest the whole run (connecting, waiting for locks, reading, writing) may take.
    pub timeout: Duration,
}

impl SeedSettings {
    /// Read the settings through `lookup`, which is `std::env::var` in `main` and a map in tests. The file named by
    /// `LK_DATABASE_URL_FILE` is read from disk.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        Self::from_parts(lookup, |path| std::fs::read_to_string(path))
    }

    /// Like [`SeedSettings::from_lookup`], with the file reader given (a map in tests).
    pub fn from_parts(
        lookup: impl Fn(&str) -> Option<String>,
        read_file: impl Fn(&std::path::Path) -> std::io::Result<String>,
    ) -> Result<Self, ConfigError> {
        let reader = Reader::new(&lookup, &read_file);
        Self::read(&reader)
    }

    /// Every variable is checked before the credential file is read, so a Job that is missing two things hears about the first.
    fn read(reader: &Reader<'_>) -> Result<Self, ConfigError> {
        let timeout =
            Duration::from_secs(reader.number("LK_SEED_TIMEOUT_SECS", DEFAULT_TIMEOUT_SECS, 1, 3600)?);
        // The same reader of the same two variables as the hub's pool, so the Job and the hub cannot disagree about what a
        // database setting means. `None` is "no URL": the Job cannot run without one.
        let database = DatabaseSettings::read(reader)?.ok_or(ConfigError::Missing("LK_DATABASE_URL_FILE"))?;
        Ok(Self {
            url: database.url,
            ca_file: database.ca_file,
            timeout,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io;
    use std::path::Path;

    use super::*;

    fn read(env: &[(&str, &str)], files: &[(&str, &str)]) -> Result<SeedSettings, ConfigError> {
        let env: BTreeMap<String, String> = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let files: BTreeMap<String, String> = files
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        SeedSettings::from_parts(
            |name| env.get(name).cloned(),
            |path: &Path| {
                files
                    .get(path.to_string_lossy().as_ref())
                    .cloned()
                    .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
            },
        )
    }

    const URL: &str = "postgres://lanekeeper_app:planted-password-4711@db.internal/lanekeeper";

    #[test]
    fn the_job_needs_the_credential_file_and_the_ca_and_nothing_else() {
        let ok = read(
            &[
                ("LK_DATABASE_URL_FILE", "/run/db/uri"),
                ("LK_DB_CA_FILE", "/run/ca/ca.pem"),
            ],
            &[("/run/db/uri", URL)],
        )
        .unwrap();
        assert_eq!(ok.url.expose(), URL);
        assert_eq!(ok.ca_file, PathBuf::from("/run/ca/ca.pem"));
        assert_eq!(ok.timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECS));

        assert_eq!(
            read(&[("LK_DB_CA_FILE", "/ca")], &[]).unwrap_err(),
            ConfigError::Missing("LK_DATABASE_URL_FILE")
        );
        assert_eq!(
            read(&[("LK_DATABASE_URL_FILE", "/u")], &[("/u", URL)]).unwrap_err(),
            ConfigError::Missing("LK_DB_CA_FILE")
        );
        // The plain variable exists for local development, as it does for the hub, and is refused beside the file one.
        assert!(read(&[("LK_DATABASE_URL", URL), ("LK_DB_CA_FILE", "/ca")], &[]).is_ok());
        assert_eq!(
            read(
                &[
                    ("LK_DATABASE_URL", URL),
                    ("LK_DATABASE_URL_FILE", "/u"),
                    ("LK_DB_CA_FILE", "/ca")
                ],
                &[("/u", URL)]
            )
            .unwrap_err(),
            ConfigError::Invalid("LK_DATABASE_URL")
        );
    }

    #[test]
    fn the_time_limit_is_one_second_to_an_hour() {
        let with = |value: &str| {
            read(
                &[
                    ("LK_DATABASE_URL", URL),
                    ("LK_DB_CA_FILE", "/ca"),
                    ("LK_SEED_TIMEOUT_SECS", value),
                ],
                &[],
            )
        };
        assert_eq!(with("1").unwrap().timeout, Duration::from_secs(1));
        assert_eq!(with("3600").unwrap().timeout, Duration::from_secs(3600));
        for bad in ["0", "3601", "soon", "-1", "1.5", " 5"] {
            assert_eq!(
                with(bad).unwrap_err(),
                ConfigError::Invalid("LK_SEED_TIMEOUT_SECS"),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_refusal_names_the_variable_and_never_the_value_or_the_password() {
        let error = read(
            &[("LK_DATABASE_URL_FILE", "/u"), ("LK_DB_CA_FILE", "/ca")],
            &[("/u", "planted-password-4711\nsecond line")],
        )
        .unwrap_err();
        assert_eq!(error, ConfigError::Invalid("LK_DATABASE_URL_FILE"));
        let shown = format!("{error} {error:?}");
        assert!(!shown.contains("planted"), "{shown}");
        let settings = read(&[("LK_DATABASE_URL", URL), ("LK_DB_CA_FILE", "/ca")], &[]).unwrap();
        assert!(
            !format!("{settings:?}").contains("planted-password"),
            "the URL is redacted in debug output"
        );
    }

    #[test]
    fn the_job_only_variable_is_read_here_and_not_by_the_serving_settings() {
        let production = include_str!("seed.rs").split("#[cfg(test)]").next().unwrap();
        let code = production.split("impl SeedSettings").nth(1).unwrap();
        let serving = include_str!("../config.rs").split("#[cfg(test)]").next().unwrap();
        for name in SEED_JOB_ONLY_VARIABLES {
            assert!(code.contains(&format!("\"{name}\"")), "{name} is not read");
            assert!(
                !serving.contains(&format!("\"{name}\"")),
                "{name} is read by the hub"
            );
        }
    }
}
