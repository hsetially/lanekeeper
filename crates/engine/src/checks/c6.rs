//! C6 `environment_host_mismatch` (domain-model, "Consistency checks", D82).
//!
//! In the real data no config mentions a swimlane or a branch name; what differs between environments is the external
//! hosts. An admin configures how a host name tells its tier (`-dev` in the name, `.oktapreview.com` against `.okta.com`),
//! the swimlane has a tier, and a setting whose value names a host of another tier is a finding.
//!
//! # What counts as a host
//!
//! Hosts are looked for in the places a host name is written:
//!
//! - the authority of a URL, `scheme://[user[:password]@]host[:port]/...`, wherever it is in the value (a value can hold
//!   several, separated by commas or spaces). User and password are never read;
//! - the whole value, when it is a bare `host` or `host:port`.
//!
//! A host must have a dot: a single label is a cluster-local service name (`core-adapter-symxchange`) and is never
//! external. An IP address is not a name and has no tier. Host names are compared in lower case.
//!
//! Only string values are read, never keys, and only values of settings: the check is about what a service will connect to.

use domain::{FindingKind, LineRange, SettingLocation, ShortText};

use super::{CheckDetail, CheckFile, CheckResult, MismatchedHost, count, message};
use crate::error::HostRulesError;
use crate::limits::Limits;

/// How a rule recognises a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostMatcher {
    /// The host ends with this text: `.oktapreview.com`.
    Suffix(String),
    /// The host contains this text: `-dev`.
    Contains(String),
}

/// A host rule: hosts that match belong to `tier`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRule {
    pub tier: ShortText,
    pub matcher: HostMatcher,
}

/// The admin's host rules, in order: the first rule that matches a host names its tier. The default is no rules, and then
/// C6 reports nothing (plan decision A14).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct HostRules {
    rules: Vec<HostRule>,
}

impl HostRules {
    /// The most rules `new` takes.
    pub const MAX_RULES: usize = 1_000;
    /// The longest text a rule matches on: the longest host name there is.
    pub const MAX_NEEDLE_BYTES: usize = 253;

    /// Rules in order of precedence. The text of a rule is kept in lower case.
    ///
    /// # Errors
    ///
    /// `TooManyRules`, `EmptyNeedle`, `NeedleTooLong`, or `InvalidNeedle` for a character that is not an ASCII letter,
    /// digit, `.`, `-` or `_`. The error does not contain the rule.
    pub fn new(rules: Vec<HostRule>) -> Result<Self, HostRulesError> {
        if rules.len() > Self::MAX_RULES {
            return Err(HostRulesError::TooManyRules);
        }
        let mut kept = Vec::with_capacity(rules.len());
        for rule in rules {
            let (needle, rebuild): (String, fn(String) -> HostMatcher) = match rule.matcher {
                HostMatcher::Suffix(n) => (n, HostMatcher::Suffix),
                HostMatcher::Contains(n) => (n, HostMatcher::Contains),
            };
            if needle.is_empty() {
                return Err(HostRulesError::EmptyNeedle);
            }
            if needle.len() > Self::MAX_NEEDLE_BYTES {
                return Err(HostRulesError::NeedleTooLong);
            }
            if !needle.bytes().all(is_host_byte) {
                return Err(HostRulesError::InvalidNeedle);
            }
            kept.push(HostRule {
                tier: rule.tier,
                matcher: rebuild(needle.to_ascii_lowercase()),
            });
        }
        Ok(Self { rules: kept })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The tier of a host name: the tier of the first rule that matches it, or `None`.
    pub fn tier_of(&self, host: &str) -> Option<&ShortText> {
        let host = host.to_ascii_lowercase();
        self.rules
            .iter()
            .find(|rule| match &rule.matcher {
                HostMatcher::Suffix(s) => host.ends_with(s.as_str()),
                HostMatcher::Contains(s) => host.contains(s.as_str()),
            })
            .map(|rule| &rule.tier)
    }
}

impl std::fmt::Debug for HostRules {
    // The rules are admin configuration; they are not printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostRules")
            .field("rules", &self.rules.len())
            .finish_non_exhaustive()
    }
}

fn is_host_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')
}

/// The hosts named in a setting value, lower-cased, in order, without repeats.
fn hosts_in(value: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut add = |host: String| {
        if !found.contains(&host) {
            found.push(host);
        }
    };

    let mut rest = value;
    let mut saw_url = false;
    while let Some(at) = rest.find("://") {
        saw_url = true;
        let after = &rest[at + 3..];
        let end = after
            .find(|c: char| {
                matches!(c, '/' | '?' | '#') || c.is_whitespace() || matches!(c, ',' | ';' | '"' | '\'')
            })
            .unwrap_or(after.len());
        if let Some(host) = host_of_authority(&after[..end]) {
            add(host);
        }
        rest = &after[end..];
    }
    if !saw_url {
        if let Some(host) = host_of_authority(value.trim()) {
            // A bare value must look like a name, not like a file name or a version: its last label is letters.
            if host
                .rsplit('.')
                .next()
                .is_some_and(|tld| tld.len() >= 2 && tld.bytes().all(|b| b.is_ascii_alphabetic()))
            {
                add(host);
            }
        }
    }
    found
}

/// The host of `[user[:password]@]host[:port]`, if it is a name with a dot.
fn host_of_authority(authority: &str) -> Option<String> {
    let host_port = authority.rsplit('@').next()?;
    // `[::1]:80` and the like are addresses.
    if host_port.starts_with('[') {
        return None;
    }
    let host = match host_port.split_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        Some(_) => return None,
        None => host_port,
    };
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty() || host.len() > HostRules::MAX_NEEDLE_BYTES || !host.bytes().all(is_host_byte) {
        return None;
    }
    // A single label is a cluster-local name; an address is not a name.
    if !host.contains('.') || host.split('.').any(str::is_empty) {
        return None;
    }
    if host.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// C6 for one structured file. `None` without a tier, without rules, for a file that is not structured, and when every
/// classified host is of the swimlane's tier.
pub fn c6_environment_host_mismatch(
    file: &CheckFile<'_>,
    tier: Option<&ShortText>,
    rules: &HostRules,
    limits: &Limits,
) -> Option<CheckResult> {
    let tier = tier?;
    if rules.is_empty() {
        return None;
    }
    let structured = file.structured.as_ref()?;

    // Each mismatch with the lines of the setting it is in.
    let mut found: Vec<(MismatchedHost, LineRange)> = Vec::new();
    for setting in structured.settings {
        if setting.value.ty != domain::ValueType::String {
            continue;
        }
        for host in hosts_in(&setting.value.text) {
            if let Some(host_tier) = rules.tier_of(&host).filter(|t| *t != tier) {
                found.push((
                    MismatchedHost {
                        path: setting.path.clone(),
                        host,
                        host_tier: host_tier.clone(),
                    },
                    setting.lines,
                ));
            }
        }
    }
    if found.is_empty() {
        return None;
    }
    found.sort_by(|a, b| (&a.0.path, &a.0.host).cmp(&(&b.0.path, &b.0.host)));

    let total = found.len();
    // One location per setting, whatever the number of hosts in it.
    let mut located: Vec<SettingLocation> = found
        .iter()
        .map(|(m, lines)| SettingLocation {
            path: m.path.clone(),
            lines: *lines,
        })
        .collect();
    located.dedup_by(|a, b| a.path == b.path);
    let settings = located.len();
    located.truncate(limits.locations_per_finding);

    let mut tiers: Vec<&str> = found.iter().map(|(m, _)| m.host_tier.as_str()).collect();
    tiers.sort_unstable();
    tiers.dedup();
    let text = format!(
        "{} point to hosts of another environment ({}); this swimlane is {tier}.",
        count(settings, "setting", "settings"),
        tiers.join(", ")
    );
    let hosts: Vec<MismatchedHost> = found
        .into_iter()
        .take(limits.locations_per_finding)
        .map(|(m, _)| m)
        .collect();
    let locations = located;

    Some(CheckResult {
        kind: FindingKind::EnvironmentHostMismatch,
        path: Some(file.path.clone()),
        message: message(&text)?,
        locations,
        detail: CheckDetail::EnvironmentHostMismatch {
            swimlane_tier: tier.clone(),
            hosts,
            total,
        },
    })
}
