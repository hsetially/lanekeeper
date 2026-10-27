//! The command line (plan 18, T1 to T3 and T7): `hub` and `hub serve` run the hub, `hub migrate` applies the schema, `hub seed`
//! registers swimlanes from a file, `hub ca-init` makes a CA certificate, `hub help` prints the usage.
//!
//! A hand parser, because there are only a few words and a parser crate would be a dependency on a trust boundary for nothing.
//! The arguments are untrusted text (rule 10): an error is a fixed message that never repeats the argument, so an argument
//! that is a pasted secret does not end up in a log. The only text an error names is one of the option names of `ca-init` or
//! `<file>` of `seed`, which are constants.

use std::path::PathBuf;

use hub_platform::gcp::wire::CryptoKeyVersionName;
use ports::KeyRef;

/// The longest a CA's common name may be.
pub const MAX_COMMON_NAME: usize = 64;
/// The longest a CA certificate may be asked to live: ten years.
pub const MAX_VALID_DAYS: u32 = 3650;

/// What the process was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run the hub until told to stop. The default, so `hub` with no arguments is what the container's entry point runs.
    Serve,
    /// Apply the database migrations as `lanekeeper_migrator` and exit (T2, G4). Run as a Job before the hub rolls: the
    /// serving hub never migrates at start, so the migrator credential is never in its pod.
    Migrate,
    /// Register the projects, swimlanes, tenants and sentinels that a file lists (T7, G24). Run by a person or a Job, as the application
    /// role; it creates and updates, never deletes, and a second run with the same file changes nothing.
    Seed(SeedArgs),
    /// Make the self-signed certificate of a CA whose key is in Cloud KMS, and print it (T3, G15). Run once per CA by a person,
    /// before the first start; the certificate is public.
    CaInit(CaInitArgs),
    /// Print [`USAGE`] and exit 0.
    Help,
}

/// What `hub seed` was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedArgs {
    /// The seed file (YAML or JSON). A path, not a secret: it is never repeated in an error, but it is a file the operator chose.
    pub file: PathBuf,
    /// Print what would change and write nothing (no row and no audit event).
    pub dry_run: bool,
}

/// What `hub ca-init` was asked for. All three are required: a CA's name and life are decisions, not defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaInitArgs {
    /// The KMS key version that will sign for the CA (`projects/P/.../cryptoKeys/K/cryptoKeyVersions/N`).
    pub kms_ref: KeyRef,
    /// The certificate's common name, for people reading it.
    pub common_name: String,
    /// How long the certificate is valid from now.
    pub valid_days: u32,
}

/// Why the arguments were refused. The text is fixed; it never contains the argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CliError {
    #[error("unknown command")]
    UnknownCommand,
    #[error("unexpected argument")]
    UnexpectedArgument,
    /// An option of `ca-init`, or the `<file>` of `seed`, that was not given. The name is one of the constants below.
    #[error("{0} is required")]
    MissingOption(&'static str),
    /// An option of `ca-init`, or the `<file>` of `seed`, whose value is missing or not acceptable. The value is never shown.
    #[error("{0} is not valid")]
    InvalidOption(&'static str),
}

/// The text `hub help` prints and a usage error points at.
pub const USAGE: &str = "usage: hub [serve]\n       hub migrate\n       hub seed [--dry-run] <file>\n       hub ca-init --kms-ref <key version> --common-name <name> --valid-days <days>\n       hub help\n\nserve    run the hub until SIGTERM or SIGINT (the default); settings are the LK_* environment variables\nmigrate  apply the database migrations as lanekeeper_migrator and exit; needs LK_MIGRATOR_URL_FILE and LK_DB_CA_FILE\nseed     register the projects, swimlanes, tenants and sentinels that a YAML or JSON file lists, as lanekeeper_app; it creates and\n         updates and never deletes, and a second run with the same file changes nothing; --dry-run prints what would change and\n         writes nothing; needs LK_DATABASE_URL_FILE and LK_DB_CA_FILE\nca-init  print the self-signed certificate (PEM) of a CA whose key is the given Cloud KMS key version; the key signs it and never\n         leaves KMS. Run once for each CA, then store the output in the Secret Manager secret that LK_AGENT_CA_CERT_REF or\n         LK_REPLICA_CA_CERT_REF names\nhelp     print this text\n";

const KMS_REF: &str = "--kms-ref";
const COMMON_NAME: &str = "--common-name";
const VALID_DAYS: &str = "--valid-days";
const DRY_RUN: &str = "--dry-run";
/// How `seed` names its positional argument in an error: a constant, never the path that was given.
const FILE: &str = "<file>";
/// The longest seed file path accepted: Linux's `PATH_MAX`.
const MAX_PATH: usize = 4096;

/// Parse the arguments after the program name.
pub fn parse<I, S>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Command::Serve);
    };
    let command = match first.as_ref() {
        "serve" => Command::Serve,
        "migrate" => Command::Migrate,
        "seed" => return parse_seed(args),
        "ca-init" => return parse_ca_init(args),
        "help" | "--help" | "-h" => Command::Help,
        _ => return Err(CliError::UnknownCommand),
    };
    if args.next().is_some() {
        return Err(CliError::UnexpectedArgument);
    }
    Ok(command)
}

/// `seed`: one file and, optionally, `--dry-run`, in either order. A word that starts with a dash is an option, and the only option is
/// `--dry-run`, so a file whose name starts with a dash is written `./-name`.
fn parse_seed<I, S>(args: I) -> Result<Command, CliError>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let (mut file, mut dry_run) = (None, false);
    for arg in args {
        let arg = arg.as_ref();
        if arg == DRY_RUN {
            if dry_run {
                return Err(CliError::UnexpectedArgument);
            }
            dry_run = true;
        } else if arg.starts_with('-') || file.is_some() {
            return Err(CliError::UnexpectedArgument);
        } else if arg.is_empty() || arg.len() > MAX_PATH {
            return Err(CliError::InvalidOption(FILE));
        } else {
            file = Some(PathBuf::from(arg));
        }
    }
    Ok(Command::Seed(SeedArgs {
        file: file.ok_or(CliError::MissingOption(FILE))?,
        dry_run,
    }))
}

/// `ca-init` and its three options, in any order, each once, each followed by its value.
fn parse_ca_init<I, S>(mut args: I) -> Result<Command, CliError>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let (mut kms_ref, mut common_name, mut valid_days) = (None, None, None);
    while let Some(option) = args.next() {
        // The option is one of three constants, and a value is read for it only. Anything else is "unexpected", and an
        // option given twice is too: the second value would silently win.
        let (name, taken) = match option.as_ref() {
            KMS_REF => (KMS_REF, kms_ref.is_some()),
            COMMON_NAME => (COMMON_NAME, common_name.is_some()),
            VALID_DAYS => (VALID_DAYS, valid_days.is_some()),
            _ => return Err(CliError::UnexpectedArgument),
        };
        if taken {
            return Err(CliError::UnexpectedArgument);
        }
        let value = args.next().ok_or(CliError::InvalidOption(name))?;
        let value = value.as_ref();
        match name {
            KMS_REF => kms_ref = Some(kms_ref_value(value).ok_or(CliError::InvalidOption(KMS_REF))?),
            COMMON_NAME => {
                common_name = Some(common_name_value(value).ok_or(CliError::InvalidOption(COMMON_NAME))?);
            }
            _ => valid_days = Some(valid_days_value(value).ok_or(CliError::InvalidOption(VALID_DAYS))?),
        }
    }
    Ok(Command::CaInit(CaInitArgs {
        kms_ref: kms_ref.ok_or(CliError::MissingOption(KMS_REF))?,
        common_name: common_name.ok_or(CliError::MissingOption(COMMON_NAME))?,
        valid_days: valid_days.ok_or(CliError::MissingOption(VALID_DAYS))?,
    }))
}

/// A KMS key version, not a key: a signature is made by one version, and the certificate belongs to it.
fn kms_ref_value(value: &str) -> Option<KeyRef> {
    let key = KeyRef::parse(value).ok()?;
    CryptoKeyVersionName::parse(&key)?;
    Some(key)
}

/// Letters, digits, spaces and `. _ -`, starting with a letter or digit, at most [`MAX_COMMON_NAME`] bytes. It goes into a
/// certificate's subject, which people read: nothing that could be mistaken for markup or another field.
fn common_name_value(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let first_ok = bytes.first().is_some_and(u8::is_ascii_alphanumeric);
    let rest_ok = bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'.' | b'_' | b'-'));
    (first_ok && rest_ok && bytes.len() <= MAX_COMMON_NAME).then(|| value.to_owned())
}

/// Digits only, from 1 to [`MAX_VALID_DAYS`]. No sign, no space, no leading plus.
fn valid_days_value(value: &str) -> Option<u32> {
    if value.is_empty() || value.len() > 5 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value
        .parse()
        .ok()
        .filter(|days| (1..=MAX_VALID_DAYS).contains(days))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(args: &[&str]) -> Result<Command, CliError> {
        parse(args.iter().copied())
    }

    #[test]
    fn no_arguments_serve() {
        assert_eq!(parse_all(&[]), Ok(Command::Serve));
        assert_eq!(parse_all(&["serve"]), Ok(Command::Serve));
    }

    #[test]
    fn migrate_is_a_command_of_its_own() {
        assert_eq!(parse_all(&["migrate"]), Ok(Command::Migrate));
        assert_eq!(parse_all(&["migrate", "now"]), Err(CliError::UnexpectedArgument));
        // Spelled exactly: the migration job is a fixed word in a manifest, not something to guess at.
        for word in ["Migrate", "--migrate", "migrations", "migrate "] {
            assert_eq!(parse_all(&[word]), Err(CliError::UnknownCommand), "{word:?}");
        }
    }

    #[test]
    fn help_has_three_spellings() {
        for word in ["help", "--help", "-h"] {
            assert_eq!(parse_all(&[word]), Ok(Command::Help), "{word}");
        }
    }

    #[test]
    fn an_unknown_word_is_refused() {
        for word in [
            "Seed", "SERVE", "--serve", "", "serve ", "-x", "--", "ca-init ", "Ca-Init", "ca_init", "cainit",
        ] {
            assert_eq!(parse_all(&[word]), Err(CliError::UnknownCommand), "{word:?}");
        }
    }

    #[test]
    fn extra_arguments_are_refused() {
        assert_eq!(parse_all(&["serve", "now"]), Err(CliError::UnexpectedArgument));
        assert_eq!(parse_all(&["help", "serve"]), Err(CliError::UnexpectedArgument));
        assert_eq!(parse_all(&["help", "migrate"]), Err(CliError::UnexpectedArgument));
    }

    #[test]
    fn an_error_never_repeats_the_argument() {
        let planted = "hunter2-planted-secret";
        for args in [vec![planted], vec!["serve", planted]] {
            let error = parse(args).unwrap_err();
            assert!(!format!("{error} {error:?}").contains("hunter2"));
        }
    }

    #[test]
    fn the_usage_text_is_the_golden_one() {
        insta::assert_snapshot!(USAGE, @r"
        usage: hub [serve]
               hub migrate
               hub seed [--dry-run] <file>
               hub ca-init --kms-ref <key version> --common-name <name> --valid-days <days>
               hub help

        serve    run the hub until SIGTERM or SIGINT (the default); settings are the LK_* environment variables
        migrate  apply the database migrations as lanekeeper_migrator and exit; needs LK_MIGRATOR_URL_FILE and LK_DB_CA_FILE
        seed     register the projects, swimlanes, tenants and sentinels that a YAML or JSON file lists, as lanekeeper_app; it creates and
                 updates and never deletes, and a second run with the same file changes nothing; --dry-run prints what would change and
                 writes nothing; needs LK_DATABASE_URL_FILE and LK_DB_CA_FILE
        ca-init  print the self-signed certificate (PEM) of a CA whose key is the given Cloud KMS key version; the key signs it and never
                 leaves KMS. Run once for each CA, then store the output in the Secret Manager secret that LK_AGENT_CA_CERT_REF or
                 LK_REPLICA_CA_CERT_REF names
        help     print this text
        ");
    }

    const VERSION: &str =
        "projects/lk/locations/europe-west2/keyRings/lanekeeper/cryptoKeys/agent-ca/cryptoKeyVersions/1";

    fn ca_init(args: &[&str]) -> Result<CaInitArgs, CliError> {
        let mut all = vec!["ca-init"];
        all.extend_from_slice(args);
        match parse_all(&all)? {
            Command::CaInit(parsed) => Ok(parsed),
            other => panic!("not ca-init: {other:?}"),
        }
    }

    #[test]
    fn ca_init_takes_its_three_options_in_any_order() {
        let want = CaInitArgs {
            kms_ref: KeyRef::parse(VERSION).unwrap(),
            common_name: "Lanekeeper agent CA".to_owned(),
            valid_days: 3650,
        };
        assert_eq!(
            ca_init(&[
                "--kms-ref",
                VERSION,
                "--common-name",
                "Lanekeeper agent CA",
                "--valid-days",
                "3650"
            ]),
            Ok(want.clone())
        );
        assert_eq!(
            ca_init(&[
                "--valid-days",
                "3650",
                "--common-name",
                "Lanekeeper agent CA",
                "--kms-ref",
                VERSION
            ]),
            Ok(want)
        );
    }

    #[test]
    fn ca_init_names_the_option_that_is_missing_and_never_a_value() {
        assert_eq!(ca_init(&[]), Err(CliError::MissingOption("--kms-ref")));
        assert_eq!(
            ca_init(&["--kms-ref", VERSION]),
            Err(CliError::MissingOption("--common-name"))
        );
        assert_eq!(
            ca_init(&["--kms-ref", VERSION, "--common-name", "CA"]),
            Err(CliError::MissingOption("--valid-days"))
        );
        assert_eq!(
            CliError::MissingOption("--kms-ref").to_string(),
            "--kms-ref is required"
        );
        assert_eq!(
            CliError::InvalidOption("--valid-days").to_string(),
            "--valid-days is not valid"
        );
    }

    #[test]
    fn ca_init_refuses_a_value_that_is_not_what_the_option_takes() {
        let ok = ["--kms-ref", VERSION, "--common-name", "CA", "--valid-days", "30"];
        let with = |option: &str, value: &str| -> Vec<String> {
            let mut args: Vec<String> = ok.iter().map(|s| (*s).to_owned()).collect();
            let at = args.iter().position(|a| a == option).unwrap();
            args[at + 1] = value.to_owned();
            args
        };
        let toolong = "a".repeat(65);
        for (option, bad) in [
            // A key, and not a version of it; a name that could break out of a field; a number that is not one.
            ("--kms-ref", "projects/lk/locations/l/keyRings/r/cryptoKeys/k"),
            ("--kms-ref", "fake/kms/keys/1"),
            ("--kms-ref", ""),
            (
                "--kms-ref",
                "projects/lk/../locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1",
            ),
            ("--common-name", ""),
            ("--common-name", " leading space"),
            ("--common-name", "a,b=c"),
            ("--common-name", "line\nbreak"),
            ("--common-name", "caf\u{e9}"),
            ("--common-name", toolong.as_str()),
            ("--valid-days", "0"),
            ("--valid-days", "3651"),
            ("--valid-days", "-1"),
            ("--valid-days", "+5"),
            ("--valid-days", "1e3"),
            ("--valid-days", " 5"),
            ("--valid-days", "99999999999"),
            ("--valid-days", ""),
        ] {
            let args = with(option, bad);
            let mut all = vec!["ca-init"];
            all.extend(args.iter().map(String::as_str));
            let got = parse_all(&all);
            let want: &'static str = match option {
                "--kms-ref" => "--kms-ref",
                "--common-name" => "--common-name",
                _ => "--valid-days",
            };
            assert_eq!(got, Err(CliError::InvalidOption(want)), "{option} {bad:?}");
            if bad.len() >= 3 {
                assert!(!format!("{got:?}").contains(bad), "{bad:?}");
            }
        }
        // The edges that are fine.
        assert_eq!(ca_init(&with_edges("1")).unwrap().valid_days, 1);
        assert_eq!(ca_init(&with_edges("3650")).unwrap().valid_days, 3650);
    }

    fn with_edges(days: &str) -> Vec<&str> {
        vec!["--kms-ref", VERSION, "--common-name", "a", "--valid-days", days]
    }

    #[test]
    fn ca_init_refuses_what_it_does_not_know_and_an_option_given_twice() {
        for args in [
            vec![
                "--kms-ref",
                VERSION,
                "--common-name",
                "a",
                "--valid-days",
                "1",
                "--force",
            ],
            vec![
                "--kms-ref",
                VERSION,
                "--common-name",
                "a",
                "--valid-days",
                "1",
                "extra",
            ],
            vec![
                "--kms-ref",
                VERSION,
                "--kms-ref",
                VERSION,
                "--common-name",
                "a",
                "--valid-days",
                "1",
            ],
            vec!["--common-name", "a", "--common-name", "b"],
            vec!["extra"],
            vec!["-kms-ref", VERSION],
            vec!["--kms-ref=projects/x"],
        ] {
            assert_eq!(ca_init(&args), Err(CliError::UnexpectedArgument), "{args:?}");
        }
        // An option with no value after it is an option whose value is not valid.
        assert_eq!(
            ca_init(&["--valid-days"]),
            Err(CliError::InvalidOption("--valid-days"))
        );
        // An option's value is read as a value: another option name in its place is not one.
        assert_eq!(
            ca_init(&["--kms-ref", "--common-name"]),
            Err(CliError::InvalidOption("--kms-ref"))
        );
    }

    #[test]
    fn ca_init_errors_never_repeat_a_pasted_secret() {
        // Not a valid value for any of the three options, so every case below is an error.
        let planted = "hunter2:planted/secret";
        for args in [
            vec!["ca-init", planted],
            vec!["ca-init", "--kms-ref", planted],
            vec![
                "ca-init",
                "--kms-ref",
                VERSION,
                "--common-name",
                planted,
                "--valid-days",
                "1",
            ],
            vec![
                "ca-init",
                "--kms-ref",
                VERSION,
                "--common-name",
                "a",
                "--valid-days",
                planted,
            ],
            vec!["ca-init", planted, planted],
        ] {
            let error = parse_all(&args).unwrap_err();
            assert!(!format!("{error} {error:?}").contains("hunter2"), "{args:?}");
        }
    }

    fn seed(args: &[&str]) -> Result<SeedArgs, CliError> {
        let mut all = vec!["seed"];
        all.extend_from_slice(args);
        match parse_all(&all)? {
            Command::Seed(parsed) => Ok(parsed),
            other => panic!("not seed: {other:?}"),
        }
    }

    #[test]
    fn seed_takes_a_file_and_an_optional_dry_run_in_either_order() {
        let apply = SeedArgs {
            file: PathBuf::from("/etc/lanekeeper/seed/seed.yaml"),
            dry_run: false,
        };
        let dry = SeedArgs {
            dry_run: true,
            ..apply.clone()
        };
        assert_eq!(seed(&["/etc/lanekeeper/seed/seed.yaml"]), Ok(apply));
        assert_eq!(
            seed(&["--dry-run", "/etc/lanekeeper/seed/seed.yaml"]),
            Ok(dry.clone())
        );
        assert_eq!(seed(&["/etc/lanekeeper/seed/seed.yaml", "--dry-run"]), Ok(dry));
        // A relative path is a path; a file with a dot or a space in its name is too.
        assert_eq!(
            seed(&["seed.dev.yaml"]).unwrap().file,
            PathBuf::from("seed.dev.yaml")
        );
        assert_eq!(
            seed(&["my seed.json"]).unwrap().file,
            PathBuf::from("my seed.json")
        );
    }

    #[test]
    fn seed_names_the_file_it_is_missing_and_never_a_value() {
        assert_eq!(seed(&[]), Err(CliError::MissingOption("<file>")));
        assert_eq!(seed(&["--dry-run"]), Err(CliError::MissingOption("<file>")));
        // An empty path is a path that is not valid, not one that is missing.
        assert_eq!(seed(&[""]), Err(CliError::InvalidOption("<file>")));
        assert_eq!(
            CliError::MissingOption("<file>").to_string(),
            "<file> is required"
        );
        assert_eq!(
            CliError::InvalidOption("<file>").to_string(),
            "<file> is not valid"
        );
    }

    #[test]
    fn seed_refuses_what_it_does_not_know_and_what_is_given_twice() {
        for args in [
            vec!["a.yaml", "b.yaml"],
            vec!["--dry-run", "--dry-run", "a.yaml"],
            vec!["a.yaml", "--dry-run", "--dry-run"],
            vec!["--force", "a.yaml"],
            vec!["a.yaml", "--dry-run=true"],
            // A path that starts with a dash is an option nobody defined; `./-name` is how to name such a file.
            vec!["-a.yaml"],
            vec!["-"],
            vec!["--"],
        ] {
            assert_eq!(seed(&args), Err(CliError::UnexpectedArgument), "{args:?}");
        }
        assert!(seed(&["./-a.yaml"]).is_ok());
    }

    #[test]
    fn seed_errors_never_repeat_a_path_or_a_pasted_secret() {
        let planted = "hunter2-planted-secret";
        for args in [
            vec!["seed", planted, planted],
            vec!["seed", "a.yaml", planted],
            vec!["seed", "--dry-run", "--dry-run", planted],
            vec!["seed", "-hunter2-planted-secret"],
        ] {
            let error = parse_all(&args).unwrap_err();
            assert!(!format!("{error} {error:?}").contains("hunter2"), "{args:?}");
        }
    }

    #[test]
    fn seed_is_a_word_of_its_own() {
        for word in ["Seed", "seeds", "--seed", "seed "] {
            assert_eq!(parse_all(&[word]), Err(CliError::UnknownCommand), "{word:?}");
        }
    }

    proptest::proptest! {
        /// Any argument list is either a command or one of the two errors, and never a panic.
        #[test]
        fn arbitrary_arguments_never_panic(args in proptest::collection::vec(".{0,24}", 0..4)) {
            let _ = parse(args.iter().map(String::as_str));
        }
    }
}
