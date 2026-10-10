//! Which environment variables may have their values reported (S17, D88, A19).
//!
//! The hub sends an allowlist of names in `AgentConfig`. Only those names get a value in a `ClusterReport`; every other
//! name is reported as a name. On top of the hub's list the agent keeps its own rules, because a bad value would make
//! the hub reject the whole report and a secret must not travel even if someone allowlists it by mistake:
//!
//! - a name that looks like it holds a secret (`SECRET`, `PASSWORD`, `TOKEN`, `KEY`, `CREDENTIAL`, in any case) never
//!   gets a value;
//! - a value over 256 bytes or with a control character is dropped, and its name is reported as a name.
//!
//! The guard is applied twice: when a Deployment is trimmed on ingest, so that values of other variables are never kept
//! in memory, and again when the report is built, so that a list that has just narrowed takes effect at once.

use std::collections::HashSet;
use std::fmt;
use std::sync::{Arc, RwLock};

use domain::ShortText;

/// Name fragments that mark a variable as holding a secret. Compared in upper case.
const SECRET_MARKERS: [&str; 5] = ["SECRET", "PASSWORD", "TOKEN", "KEY", "CREDENTIAL"];

/// True when the name looks like it holds a secret.
pub fn looks_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_MARKERS.iter().any(|marker| upper.contains(marker))
}

/// The hub's allowlist, shared between the watchers (which trim) and the report builder.
#[derive(Clone, Default)]
pub struct EnvGuard {
    allowed: Arc<RwLock<Arc<HashSet<String>>>>,
}

impl fmt::Debug for EnvGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnvGuard")
            .field("allowed_names", &self.snapshot().len())
            .finish()
    }
}

impl EnvGuard {
    /// A guard that allows nothing: until the hub's `AgentConfig` arrives, no value is reported.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_allowlist(names: &[ShortText]) -> Self {
        let guard = Self::new();
        guard.set_allowlist(names);
        guard
    }

    fn snapshot(&self) -> Arc<HashSet<String>> {
        // A poisoned lock only means a thread panicked while holding it; the set is still whole.
        Arc::clone(
            &self
                .allowed
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Replace the list. True when it differs from the one before, which is when the watchers must list again.
    pub fn set_allowlist(&self, names: &[ShortText]) -> bool {
        let next: HashSet<String> = names.iter().map(|n| n.as_str().to_owned()).collect();
        let mut slot = self
            .allowed
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if **slot == next {
            return false;
        }
        *slot = Arc::new(next);
        true
    }

    /// May a value of this variable be reported at all? The name must be on the list (exact, case-sensitive) and must
    /// not look like a secret.
    pub fn permits_value(&self, name: &str) -> bool {
        !looks_secret(name) && self.snapshot().contains(name)
    }

    /// The value to report for this variable: `Some` only if the name is permitted and the value is something the hub
    /// will accept.
    pub fn value_for(&self, name: &str, value: &str) -> Option<ShortText> {
        if !self.permits_value(name) {
            return None;
        }
        ShortText::parse(value).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> Vec<ShortText> {
        items.iter().map(|s| ShortText::parse(s).unwrap()).collect()
    }

    #[test]
    fn guard_allows_nothing_until_the_hub_sends_a_list() {
        let guard = EnvGuard::new();
        assert!(!guard.permits_value("CONFIG_CLIENT_CACHE_TTL"));
        assert_eq!(guard.value_for("CONFIG_CLIENT_CACHE_TTL", "20m"), None);
    }

    #[test]
    fn guard_matches_exact_names_only() {
        let guard = EnvGuard::with_allowlist(&names(&["CONFIG_CLIENT_CACHE_TTL"]));
        assert!(guard.permits_value("CONFIG_CLIENT_CACHE_TTL"));
        for other in [
            "config_client_cache_ttl",
            "CONFIG_CLIENT_CACHE_TTL ",
            "CONFIG_CLIENT_CACHE",
            "X_CONFIG_CLIENT_CACHE_TTL",
            "",
        ] {
            assert!(!guard.permits_value(other), "{other:?}");
        }
    }

    #[test]
    fn guard_never_gives_a_value_to_a_secret_looking_name() {
        let list = names(&[
            "DB_PASSWORD",
            "api_key",
            "Auth_Token",
            "MY_SECRET",
            "SvcCredential",
            "OK_NAME",
        ]);
        let guard = EnvGuard::with_allowlist(&list);
        for name in [
            "DB_PASSWORD",
            "api_key",
            "Auth_Token",
            "MY_SECRET",
            "SvcCredential",
        ] {
            assert!(looks_secret(name), "{name}");
            assert_eq!(guard.value_for(name, "x"), None, "{name}");
        }
        assert_eq!(guard.value_for("OK_NAME", "x").unwrap().as_str(), "x");
    }

    #[test]
    fn guard_drops_values_the_hub_would_reject() {
        let guard = EnvGuard::with_allowlist(&names(&["V"]));
        assert!(guard.value_for("V", &"a".repeat(256)).is_some());
        assert_eq!(guard.value_for("V", &"a".repeat(257)), None);
        for bad in ["a\nb", "a\u{0}b", "tab\there", "bell\u{7}", "\u{1b}[31m"] {
            assert_eq!(guard.value_for("V", bad), None, "{bad:?}");
        }
        // An empty value is a value.
        assert_eq!(guard.value_for("V", "").unwrap().as_str(), "");
    }

    #[test]
    fn guard_reports_whether_a_new_list_differs() {
        let guard = EnvGuard::new();
        assert!(!guard.set_allowlist(&[]), "empty over empty is no change");
        assert!(guard.set_allowlist(&names(&["A", "B"])));
        assert!(!guard.set_allowlist(&names(&["B", "A"])), "order does not matter");
        assert!(guard.set_allowlist(&names(&["A"])));
        assert!(!guard.permits_value("B"), "a narrowed list takes effect at once");
    }

    #[test]
    fn guard_debug_shows_a_count_and_no_names() {
        let guard = EnvGuard::with_allowlist(&names(&["VISIBLE_IN_DEBUG_NEVER"]));
        let shown = format!("{guard:?}");
        assert!(shown.contains('1'));
        assert!(!shown.contains("VISIBLE_IN_DEBUG_NEVER"));
    }
}
