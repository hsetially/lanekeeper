//! S21: `Secret<T>` redacts itself, never serialises and zeroizes on drop.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use domain::Secret;
use serde::Serialize;
use static_assertions::{assert_impl_all, assert_not_impl_any};
use zeroize::Zeroize;

assert_not_impl_any!(Secret<String>: Serialize, std::fmt::Display, Clone, PartialEq);
assert_not_impl_any!(Secret<[u8; 32]>: Serialize, std::fmt::Display, Clone, PartialEq);
assert_impl_all!(Secret<String>: Send, Sync, std::fmt::Debug);
assert_impl_all!(Secret<[u8; 32]>: Send, Sync, std::fmt::Debug);

#[test]
fn debug_is_redacted() {
    let s = Secret::new(String::from("hunter2-very-secret"));
    assert_eq!(format!("{s:?}"), "[redacted]");
    assert_eq!(format!("{s:#?}"), "[redacted]");
    let wrapped = Some(Secret::new(String::from("hunter2-very-secret")));
    assert!(!format!("{wrapped:?}").contains("hunter2"));
    let key = Secret::new([0xAB_u8; 32]);
    assert_eq!(format!("{key:?}"), "[redacted]");
}

#[test]
fn display_is_not_implemented() {
    // Compile-time proof is the `assert_not_impl_any!` above; this checks the runtime
    // consequence for a struct that derives Debug around a secret.
    #[derive(Debug)]
    struct Holder {
        #[allow(dead_code)]
        token: Secret<String>,
    }
    let h = Holder {
        token: Secret::new("tok-123".into()),
    };
    assert!(!format!("{h:?}").contains("tok-123"));
}

#[test]
fn serialize_is_not_implemented() {
    // Compile-time: see `assert_not_impl_any!(Secret<String>: Serialize)` at the top.
    // Deserialize exists for Secret<String> only (config loading).
    let s: Secret<String> = serde_json::from_str("\"abc\"").unwrap();
    assert_eq!(s.expose(), "abc");
}

#[test]
fn compare_is_constant_time_api() {
    let a = Secret::new(String::from("same-value"));
    let b = Secret::new(String::from("same-value"));
    let c = Secret::new(String::from("other-value"));
    let short = Secret::new(String::from("same"));
    assert!(bool::from(a.ct_eq(&b)));
    assert!(!bool::from(a.ct_eq(&c)));
    assert!(!bool::from(a.ct_eq(&short)));
    assert!(a.ct_eq_bytes(b"same-value").unwrap_u8() == 1);
    assert!(a.ct_eq_bytes(b"nope").unwrap_u8() == 0);
    let k1 = Secret::new([1_u8; 32]);
    let k2 = Secret::new([1_u8; 32]);
    let k3 = Secret::new([2_u8; 32]);
    assert!(bool::from(k1.ct_eq(&k2)));
    assert!(!bool::from(k1.ct_eq(&k3)));
}

struct Probe(Arc<AtomicBool>);

impl Zeroize for Probe {
    fn zeroize(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[test]
fn zeroize_on_drop() {
    let flag = Arc::new(AtomicBool::new(false));
    let secret = Secret::new(Probe(Arc::clone(&flag)));
    assert!(!flag.load(Ordering::SeqCst));
    drop(secret);
    assert!(
        flag.load(Ordering::SeqCst),
        "inner value must be zeroized when the secret drops"
    );
}

#[test]
fn expose_gives_the_value_only_on_request() {
    let s = Secret::new(String::from("v"));
    assert_eq!(s.expose(), "v");
    let k = Secret::new([9_u8; 32]);
    assert_eq!(k.expose(), &[9_u8; 32]);
}
