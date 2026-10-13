//! T7 (S13, S14b, S21): the value-based secret scan. It looks at values, never at key names: private key headers and bodies,
//! known token prefixes, JWT-shaped strings and long random-looking strings. A match is a line, a column and a length, never
//! the value, so a caller that redacts has the place and the size and nothing to leak.
//!
//! Every sample is built at run time from pieces, so that this file holds nothing that looks like a secret to a scanner.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use engine::{Limits, ScanConfig, ScanError, SecretMatch, SecretRule, secret_scan};
use proptest::prelude::*;

fn scan(text: &str) -> Vec<SecretMatch> {
    secret_scan(text, &ScanConfig::default(), &Limits::default()).unwrap()
}

/// `line:column:length:rule` of each match.
fn shape(matches: &[SecretMatch]) -> Vec<String> {
    matches
        .iter()
        .map(|m| format!("{}:{}:{}:{}", m.line, m.column, m.length, m.rule))
        .collect()
}

/// A deterministic run of letters and digits that looks random (a fixed pseudo-random generator, not a real key).
fn random_token(len: usize, alphabet: &[u8], seed: u64) -> String {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            alphabet[usize::try_from(x % u64::try_from(alphabet.len()).unwrap()).unwrap()] as char
        })
        .collect()
}

const BASE62: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn begin(label: &str) -> String {
    format!("{}BEGIN {label}{}", "-".repeat(5), "-".repeat(5))
}

fn end(label: &str) -> String {
    format!("{}END {label}{}", "-".repeat(5), "-".repeat(5))
}

#[test]
fn private_key_header_flagged() {
    let body = random_token(64, BASE64, 7);
    let text = format!(
        "key: |\n  {}\n  {body}\n  {body}\n  {}\nnext: 1\n",
        begin("RSA PRIVATE KEY"),
        end("RSA PRIVATE KEY")
    );
    let found = scan(&text);
    // The header, then every line of the body (so that redaction hides the whole key), and not the end line.
    assert_eq!(
        shape(&found),
        [
            format!("2:3:{}:private_key", begin("RSA PRIVATE KEY").len()),
            "3:3:64:private_key".to_owned(),
            "4:3:64:private_key".to_owned(),
        ]
    );
    // The other label forms.
    for label in [
        "PRIVATE KEY",
        "EC PRIVATE KEY",
        "ENCRYPTED PRIVATE KEY",
        "OPENSSH PRIVATE KEY",
        "PGP PRIVATE KEY BLOCK",
    ] {
        let one = format!("{}\n{body}\n{}\n", begin(label), end(label));
        let found = scan(&one);
        assert_eq!(found.len(), 2, "{label}: {found:?}");
        assert!(found.iter().all(|m| m.rule == SecretRule::PrivateKey));
    }
    // A header with no end marker covers the rest of the text.
    let open = format!("{}\n{body}\n{body}\n", begin("PRIVATE KEY"));
    assert_eq!(scan(&open).len(), 3);
    // A whole key on one line, as JSON writes it.
    let inline = format!(
        "\"k\": \"{}\\n{body}\\n{}\"",
        begin("PRIVATE KEY"),
        end("PRIVATE KEY")
    );
    let found = scan(&inline);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].rule, SecretRule::PrivateKey);
    assert_eq!(found[0].column, 7);
    let span: String = inline.chars().skip(6).take(found[0].length as usize).collect();
    assert!(
        span.starts_with(&begin("PRIVATE KEY")) && span.ends_with(&end("PRIVATE KEY")),
        "{span}"
    );
}

#[test]
fn private_key_body_before_an_end_marker_on_the_same_line_is_flagged() {
    // Found by the fuzzer: a line that holds the end marker is not all marker. What comes before it, in a private block, is key.
    let body = random_token(64, BASE64, 17);
    let tail = random_token(40, BASE62, 18);
    let text = format!(
        "{}\n{body}\n  {body}{}ghp_{}\nafter: 1\n",
        begin("PRIVATE KEY"),
        end("PRIVATE KEY"),
        &tail[..36]
    );
    let found = scan(&text);
    assert_eq!(
        shape(&found),
        [
            format!("1:1:{}:private_key", begin("PRIVATE KEY").len()),
            "2:1:64:private_key".to_owned(),
            "3:3:64:private_key".to_owned(),
            // And what follows the end marker is an ordinary line again.
            format!("3:{}:40:github_token", 3 + 64 + end("PRIVATE KEY").len()),
        ]
    );
    // A public block is the same the other way round: the marker ends it, and the rest of its line is looked at.
    let public = format!(
        "{}\n{body}\n{}ghp_{}\n",
        begin("CERTIFICATE"),
        end("CERTIFICATE"),
        &tail[..36]
    );
    assert_eq!(
        shape(&scan(&public)),
        [format!("3:{}:40:github_token", end("CERTIFICATE").len() + 1)]
    );
}

#[test]
fn a_marker_does_not_fuse_with_the_token_after_it() {
    // Found by the fuzzer: the dashes of an end marker are token characters. A token written right after one is a token of
    // its own, whether or not the scan was inside the block when it reached the marker; redacting a key must not change it.
    let short = "ghp_aB3dE5gH7jK9mN1pQ3sT5v---";
    let inside = format!(
        "{}\nAbC1d+E/f{}{short}\n",
        begin("PRIVATE KEY"),
        end("PRIVATE KEY")
    );
    assert_eq!(scan(&inside).len(), 2, "{:?}", scan(&inside));
    let stray = format!("[REDACTED: 9 chars, private_key]{}{short}\n", end("PRIVATE KEY"));
    assert!(scan(&stray).is_empty(), "{:?}", scan(&stray));
    // Two blocks in one line (JSON can do that) are both found.
    let body = random_token(64, BASE64, 19);
    let two = format!(
        "{}{body}{} {}{body}{}\n",
        begin("PRIVATE KEY"),
        end("PRIVATE KEY"),
        begin("PRIVATE KEY"),
        end("PRIVATE KEY")
    );
    assert_eq!(scan(&two).len(), 2);
}

#[test]
fn github_pat_flagged() {
    let classic = format!("ghp_{}", random_token(36, BASE62, 11));
    let fine = format!("github_pat_{}", random_token(40, BASE62, 12));
    let app = format!("ghs_{}", random_token(36, BASE62, 13));
    let text = format!("token: {classic}\nother: \"{fine}\"\napp={app}\n");
    let found = scan(&text);
    assert_eq!(
        shape(&found),
        [
            "1:8:40:github_token".to_owned(),
            "2:9:51:github_token".to_owned(),
            "3:5:40:github_token".to_owned(),
        ]
    );
    // A prefix with too little after it is a word, not a token; a prefix inside a word is not a prefix.
    assert!(scan("ghp_short and github_pat_nope\n").is_empty());
    assert!(
        scan(&format!("xghp_{}\n", random_token(36, BASE62, 14)))
            .iter()
            .all(|m| m.rule != SecretRule::GithubToken)
    );
}

#[test]
fn aws_key_prefix_flagged() {
    // The key id that the AWS documentation uses as its example.
    let id = format!("AKIA{}", "IOSFODNN7EXAMPLE");
    let temporary = format!("ASIA{}", random_token(16, b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567", 5));
    let found = scan(&format!("aws_access_key_id: {id}\nsession = {temporary}\n"));
    assert_eq!(
        shape(&found),
        [
            "1:20:20:aws_access_key".to_owned(),
            "2:11:20:aws_access_key".to_owned()
        ]
    );
    // Too short, too long (it is another token then), or lower case.
    assert!(scan("AKIAIOSFODNN7EXAM\n").is_empty());
    assert!(scan("akiaiosfodnn7example\n").is_empty());
    assert!(scan("AKIAIOSFODNN7EXAMPLEEXTRA\n").is_empty());
}

#[test]
fn other_known_prefixes_are_flagged() {
    let slack = format!("xoxb-{}", random_token(30, b"0123456789abcdefABCDEF-", 3));
    let gitlab = format!("glpat-{}", random_token(20, BASE62, 4));
    let google = format!("AIza{}", random_token(35, BASE62, 5));
    let stripe = format!("sk_live_{}", random_token(24, BASE62, 6));
    let text = format!("a: {slack}\nb: {gitlab}\nc: {google}\nd: {stripe}\n");
    let rules: Vec<SecretRule> = scan(&text).iter().map(|m| m.rule).collect();
    assert_eq!(
        rules,
        [
            SecretRule::SlackToken,
            SecretRule::GitlabToken,
            SecretRule::GoogleApiKey,
            SecretRule::StripeKey
        ]
    );
}

#[test]
fn jwt_shape_flagged() {
    // {"alg":"HS256","typ":"JWT"} . {"sub":"1234567890"} . a signature
    let jwt = format!(
        "{}.{}.{}",
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
        "eyJzdWIiOiIxMjM0NTY3ODkwIn0",
        random_token(
            43,
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_",
            9
        )
    );
    let found = scan(&format!("Authorization: Bearer {jwt}\n"));
    assert_eq!(shape(&found), [format!("1:23:{}:jwt", jwt.len())]);
    // Three segments are needed, and the second has to be an object too.
    assert!(scan("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0\n").is_empty());
    assert!(scan("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.notanobject.signaturesignature\n").is_empty());
    // A trailing full stop is a sentence's, not the token's.
    let found = scan(&format!("see {jwt}.\n"));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].length as usize, jwt.len());
}

#[test]
fn random_long_string_flagged() {
    // The secret key that the AWS documentation uses as its example, and random-looking strings of the usual lengths.
    let aws = format!("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLE{}", "KEY");
    let found = scan(&format!("aws_secret_access_key: {aws}\n"));
    assert_eq!(shape(&found), ["1:24:40:high_entropy".to_owned()]);
    for (len, seed) in [(32, 1), (40, 2), (44, 3), (64, 4), (88, 5)] {
        let token = random_token(len, BASE64, seed);
        let found = scan(&format!("secret: \"{token}\"\n"));
        assert_eq!(found.len(), 1, "{len}: {token}");
        assert_eq!((found[0].column, found[0].length as usize), (10, len));
    }
    // Base64 padding is part of the value.
    let padded = format!("{}==", random_token(42, BASE62, 8));
    let found = scan(&format!("k={padded}\n"));
    assert_eq!(shape(&found), ["1:3:44:high_entropy".to_owned()]);
    // Almost every random string of the usual lengths is found.
    let mut missed = 0;
    for seed in 100..400 {
        let token = random_token(32 + usize::try_from(seed % 33).unwrap(), BASE62, seed);
        if scan(&format!("v: {token}\n")).is_empty() {
            missed += 1;
        }
    }
    assert!(missed <= 3, "{missed} of 300 random strings were not flagged");
    // A short value is not secret enough to judge, however random.
    assert!(scan(&format!("v: {}\n", random_token(31, BASE62, 77))).is_empty());
    // The threshold is configuration.
    let config = ScanConfig {
        min_random_len: 16,
        min_entropy_millibits: 3_500,
        ..ScanConfig::default()
    };
    let short = random_token(20, BASE62, 78);
    assert_eq!(
        secret_scan(&format!("v: {short}\n"), &config, &Limits::default())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn things_that_only_look_long_are_not_flagged() {
    let hex40 = random_token(40, b"0123456789abcdef", 1);
    let hex64 = random_token(64, b"0123456789abcdef", 2);
    let uuid = "123e4567-e89b-12d3-a456-426614174000";
    let samples = [
        // Digests are not secrets here (A18); a setting can turn this on.
        format!("sha: {hex40}\nsha256: {hex64}\n"),
        format!("id: {uuid}\n"),
        // Words, paths, class names, URLs and host names.
        "path: Reference-Data/Catalog-Big-01/tx-infinity-core-sit1/settings.yml\n".to_owned(),
        "class: com.example.configuration.SomeVeryLongClassNameForTheThing2\n".to_owned(),
        "name: ThisIsALongCamelCasePropertyNameWithDigits12345\n".to_owned(),
        "url: https://auth.example.com/oauth2/v1/authorize?client=web&scope=openid\n".to_owned(),
        "host: tx-infinity-core-sit1-account-sorting-config.internal.example.com\n".to_owned(),
        "text: the quick brown fox jumps over the lazy dog again and again and again\n".to_owned(),
        // A long run of one letter or of digits.
        format!("pad: {}\nnum: {}\n", "A".repeat(80), "1234567890".repeat(6)),
    ];
    for text in samples {
        let found = scan(&text);
        assert!(found.is_empty(), "{text}: {found:?}");
    }
    // Unless the setting says that hex is random.
    let config = ScanConfig {
        hex_is_random: true,
        ..ScanConfig::default()
    };
    assert_eq!(
        secret_scan(&format!("k: {hex64}\n"), &config, &Limits::default())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn key_name_alone_not_flagged() {
    // Only values are judged. A name that says "secret" or "password", with nothing after it or a short value, is nothing.
    let samples = [
        "password:\n",
        "api_key: \n",
        "private_key:\n",
        "client-secret: changeme\n",
        "AWS_SECRET_ACCESS_KEY=\n",
        "db.password=${DB_PASSWORD}\n",
        "token: null\n",
        "secret_access_key_for_the_production_account_in_the_main_region: ~\n",
    ];
    for text in samples {
        assert!(scan(text).is_empty(), "{text}");
    }
    // A random-looking key (a name) at the start of a line is a name; its value is judged on its own.
    let name = random_token(40, BASE62, 21);
    assert!(scan(&format!("{name}: 1\n")).is_empty());
    assert!(scan(&format!("  - {name}: 1\n")).is_empty());
    assert!(scan(&format!("{name}=1\n")).is_empty());
    assert_eq!(scan(&format!("value: {name}\n")).len(), 1);
}

#[test]
fn placeholder_not_flagged() {
    let name = random_token(40, BASE62, 22);
    let samples = [
        format!("password: ${{{name}}}\n"),
        "password: ${DB_PASSWORD_FOR_THE_PRODUCTION_SYSTEM_ACCOUNT_VALUE}\n".to_owned(),
        format!("url: jdbc://x?pw=${{{name}}}&u=1\n"),
    ];
    for text in samples {
        assert!(scan(&text).is_empty(), "{text}");
    }
    // The text around a placeholder is judged as usual.
    let token = random_token(40, BASE62, 23);
    let found = scan(&format!("a: ${{X}}{token}\n"));
    assert_eq!(found.len(), 1);
}

#[test]
fn public_key_and_certificate_blocks_not_flagged() {
    let body = random_token(64, BASE64, 31);
    for label in [
        "PUBLIC KEY",
        "RSA PUBLIC KEY",
        "CERTIFICATE",
        "TRUSTED CERTIFICATE",
        "CERTIFICATE REQUEST",
        "X509 CRL",
    ] {
        let text = format!(
            "pem: |\n  {}\n  {body}\n  {body}\n  {}\nafter: 1\n",
            begin(label),
            end(label)
        );
        assert!(scan(&text).is_empty(), "{label}");
    }
    // A public block does not hide what follows it, and a private block after one is found.
    let key = random_token(64, BASE64, 32);
    let text = format!(
        "{}\n{body}\n{}\n{}\n{key}\n{}\ntail: {}\n",
        begin("CERTIFICATE"),
        end("CERTIFICATE"),
        begin("PRIVATE KEY"),
        end("PRIVATE KEY"),
        random_token(40, BASE62, 33)
    );
    let rules: Vec<SecretRule> = scan(&text).iter().map(|m| m.rule).collect();
    assert_eq!(
        rules,
        [
            SecretRule::PrivateKey,
            SecretRule::PrivateKey,
            SecretRule::HighEntropy
        ]
    );
    // An end marker that never comes does not make the rest of the file a certificate that nobody looks at... but it is
    // one: a block with no end is a block to its end. The scan says so by staying quiet, and a following private key
    // (which needs its own begin) is not read either.
    let open = format!("{}\n{body}\n", begin("CERTIFICATE"));
    assert!(scan(&open).is_empty());
}

#[test]
fn match_never_contains_value() {
    // The planted values appear in no output of a match: not in `Debug`, not in `Display`, not in the marker.
    let secrets = [
        format!("ghp_{}", random_token(36, BASE62, 41)),
        random_token(40, BASE62, 42),
        format!("AKIA{}", "IOSFODNN7EXAMPLE"),
    ];
    let text = format!("a: {}\nb: {}\nc: {}\n", secrets[0], secrets[1], secrets[2]);
    let found = scan(&text);
    assert_eq!(found.len(), 3);
    for m in &found {
        let outputs = [
            format!("{m:?}"),
            format!("{m:#?}"),
            m.marker(),
            format!("{}", m.rule),
        ];
        for secret in &secrets {
            for out in &outputs {
                // Not the whole value, and not any run of eight characters of it.
                for window in secret.as_bytes().windows(8) {
                    let piece = std::str::from_utf8(window).unwrap();
                    assert!(!out.contains(piece), "{out} holds {piece}");
                }
            }
        }
    }
    // The match is only numbers and a rule: this destructuring stops compiling the day a value is added to it.
    let SecretMatch {
        line,
        column,
        length,
        rule,
    } = found[0];
    assert_eq!((line, column, length, rule), (1, 4, 40, SecretRule::GithubToken));
    // The error values hold nothing either.
    let tiny = Limits {
        input_bytes: 8,
        ..Limits::default()
    };
    let err = secret_scan(&text, &ScanConfig::default(), &tiny).unwrap_err();
    assert!(!format!("{err} {err:?}").contains("ghp_"));
}

#[test]
fn marker_format() {
    let token = format!("ghp_{}", random_token(36, BASE62, 51));
    let found = scan(&format!("t: {token}\n"));
    assert_eq!(found[0].marker(), "[REDACTED: 40 chars, github_token]");
    let m = |rule, length| SecretMatch {
        line: 1,
        column: 1,
        length,
        rule,
    };
    assert_eq!(
        m(SecretRule::PrivateKey, 1).marker(),
        "[REDACTED: 1 chars, private_key]"
    );
    assert_eq!(
        m(SecretRule::HighEntropy, 123).marker(),
        "[REDACTED: 123 chars, high_entropy]"
    );
    // The rule names are fixed: they are in answers that other components and people read.
    let names: Vec<String> = [
        SecretRule::PrivateKey,
        SecretRule::GithubToken,
        SecretRule::AwsAccessKey,
        SecretRule::SlackToken,
        SecretRule::GitlabToken,
        SecretRule::GoogleApiKey,
        SecretRule::StripeKey,
        SecretRule::Jwt,
        SecretRule::HighEntropy,
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(
        names,
        [
            "private_key",
            "github_token",
            "aws_access_key",
            "slack_token",
            "gitlab_token",
            "google_api_key",
            "stripe_key",
            "jwt",
            "high_entropy"
        ]
    );
}

#[test]
fn columns_and_lengths_count_characters_in_every_kind_of_line() {
    let token = format!("ghp_{}", random_token(36, BASE62, 61));
    // Non-ASCII text before the token moves the column by characters, not bytes; CRLF, LF and CR all end a line.
    let text = format!("ünï: ✓ {token}\r\nplain\r\n  x: {token}\nlast\r{token}");
    let found = scan(&text);
    assert_eq!(
        shape(&found),
        [
            "1:8:40:github_token".to_owned(),
            "3:6:40:github_token".to_owned(),
            "5:1:40:github_token".to_owned(),
        ]
    );
    // Text with a mark and nothing else.
    assert!(scan("\u{feff}").is_empty());
    assert!(scan("").is_empty());
}

#[test]
fn input_and_match_counts_are_bounded() {
    let limits = Limits {
        input_bytes: 100,
        ..Limits::default()
    };
    assert!(secret_scan(&"a".repeat(100), &ScanConfig::default(), &limits).is_ok());
    assert_eq!(
        secret_scan(&"a".repeat(101), &ScanConfig::default(), &limits).unwrap_err(),
        ScanError::TooLarge
    );
    // More matches than the cap is an error, never a short list that leaves the rest unredacted.
    let config = ScanConfig {
        max_matches: 3,
        ..ScanConfig::default()
    };
    let one = format!("ghp_{}\n", random_token(36, BASE62, 71));
    assert_eq!(
        secret_scan(&one.repeat(3), &config, &Limits::default())
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        secret_scan(&one.repeat(4), &config, &Limits::default()).unwrap_err(),
        ScanError::TooManyMatches
    );
    // One-character lines in a private key body are the cheapest way to many matches.
    let body = format!("{}\n{}", begin("PRIVATE KEY"), "x\n".repeat(10));
    assert_eq!(
        secret_scan(&body, &config, &Limits::default()).unwrap_err(),
        ScanError::TooManyMatches
    );
}

#[test]
fn the_scan_is_deterministic() {
    let token = random_token(40, BASE62, 81);
    let text = format!(
        "a: {token}\nb: ghp_{}\n{}\nx\n{}\n",
        random_token(36, BASE62, 82),
        begin("PRIVATE KEY"),
        end("PRIVATE KEY")
    );
    assert_eq!(scan(&text), scan(&text));
}

proptest! {
    /// Whatever the text, the scan finishes, and every match is in order, inside the text and not empty.
    #[test]
    fn matches_are_ordered_non_overlapping_and_inside_the_text(text in "\\PC{0,400}|[ -~\\n\\r]{0,400}") {
        let found = scan(&text);
        let lines: Vec<&str> = text.split_inclusive(['\n', '\r']).collect();
        let mut previous: Option<(u32, u32)> = None;
        for m in &found {
            prop_assert!(m.line >= 1 && m.column >= 1 && m.length >= 1);
            if let Some((line, end)) = previous {
                prop_assert!(m.line > line || m.column >= end, "overlap at {:?}", m);
            }
            previous = Some((m.line, m.column + m.length));
        }
        prop_assert!(found.len() <= lines.len() * 4 + 4);
    }

    /// A text with a planted token in it always has that token found, wherever it sits between ordinary words.
    #[test]
    fn a_planted_token_is_found(before in "[a-z ]{0,30}", after in "[a-z ]{0,30}", seed in 1_u64..u64::MAX) {
        let token = format!("ghp_{}", random_token(36, BASE62, seed));
        let text = format!("{before} {token} {after}\n");
        let found = scan(&text);
        prop_assert_eq!(found.len(), 1, "{}", text);
        prop_assert_eq!(found[0].length, 40);
        prop_assert_eq!(found[0].rule, SecretRule::GithubToken);
    }
}

#[test]
fn hostile_lines_are_walked_in_linear_time() {
    // A file can be one line of markers, or one line that starts with a million spaces. A scan that looked for the next marker
    // from scratch, or measured the indentation again for each token, would do work in the square of the length here, and
    // these inputs would not finish.
    let limits = Limits::default();
    let config = ScanConfig {
        max_matches: usize::MAX,
        ..ScanConfig::default()
    };
    let size = limits.input_bytes;
    for unit in [
        format!("{} ", end("CERTIFICATE")),
        format!("{} ", begin("CERTIFICATE")),
        format!("{}x {} ", begin("PRIVATE KEY"), end("PRIVATE KEY")),
        begin("PRIVATE KEY"),
        "-----END ".to_owned(),
        "-----BEGIN -----".to_owned(),
    ] {
        let text = unit.repeat(size / unit.len());
        assert!(secret_scan(&text, &config, &limits).is_ok(), "{unit:?}");
    }
    let indented = format!("{}{}", " ".repeat(size / 2), "ab ".repeat(size / 6));
    assert!(secret_scan(&indented, &config, &limits).is_ok());
    let non_ascii = "é ghp_x ".repeat(size / 16);
    assert!(secret_scan(&non_ascii, &config, &limits).is_ok());
}
