//! The properties the three fuzz targets assert (S11, S22).
//!
//! This file is included, with `#[path]`, by the fuzz targets (`fuzz/fuzz_targets/*.rs`) and by
//! `crates/domain/tests/corpus_replay.rs`, so the nightly fuzzer and the stable seed replay check exactly the same
//! things. It depends only on `std` and `domain`. A check returns `Err(reason)` for a violation; the fuzz targets turn
//! that into a panic, which libFuzzer reports as a crash.
//!
//! Inputs that the parser rightly rejects are not violations: the checks only look at what was accepted.

#![allow(dead_code)]

use domain::{CompareRef, NfsPath, PathMappingRule, RepoPath, ReverseMapping, TenantId, TenantSet};

/// A check over one fuzz input.
pub type Check = fn(&[u8]) -> Result<(), String>;

/// Every fuzz target: its name (also the file name in `fuzz/fuzz_targets` and the corpus directory) and its check.
pub const TARGETS: [(&str, Check); 3] = [
    ("nfs_path", nfs_path),
    ("compare_ref", compare_ref),
    ("path_mapping_reverse", path_mapping_reverse),
];

/// Every seed corpus must hold at least this many inputs, so deleting the seeds cannot go unnoticed.
pub const MIN_SEEDS: usize = 8;

fn ensure(cond: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if cond { Ok(()) } else { Err(why()) }
}

/// An accepted `NfsPath` is normalised, bounded and stable: no `..`, no NUL or control character, no backslash, no
/// leading slash, no empty or `.` component, at most 1,024 bytes, and it parses back to itself.
pub fn nfs_path(data: &[u8]) -> Result<(), String> {
    let Ok(input) = std::str::from_utf8(data) else {
        return Ok(());
    };
    if let Ok(path) = NfsPath::parse(input) {
        check_accepted_path(&path, false, input)?;
    }
    if let Ok(prefix) = NfsPath::parse_prefix(input) {
        check_accepted_path(&prefix, true, input)?;
    }
    Ok(())
}

fn check_accepted_path(path: &NfsPath, allow_root: bool, input: &str) -> Result<(), String> {
    let s = path.as_str();
    ensure(s.len() <= 1024, || {
        format!("accepted {input:?}, which is {} bytes", s.len())
    })?;
    ensure(!s.contains('\0'), || {
        format!("accepted {input:?} with a NUL byte")
    })?;
    ensure(!s.chars().any(char::is_control), || {
        format!("accepted {input:?} with a control character")
    })?;
    ensure(!s.contains('\\'), || {
        format!("accepted {input:?} with a backslash")
    })?;
    ensure(!s.starts_with('/'), || {
        format!("accepted {input:?} as an absolute path")
    })?;
    if s.is_empty() {
        ensure(allow_root && path.is_root(), || {
            format!("accepted {input:?} as an empty file path")
        })?;
    } else {
        for component in s.split('/') {
            ensure(!matches!(component, "" | "." | ".."), || {
                format!("accepted {input:?} with the component {component:?}")
            })?;
        }
    }
    let again = if allow_root {
        NfsPath::parse_prefix(s)
    } else {
        NfsPath::parse(s)
    };
    ensure(again.as_ref() == Ok(path), || {
        format!("{input:?} -> {s:?} does not parse back to itself")
    })
}

/// An accepted `CompareRef` displays as a string that parses back to the same value.
pub fn compare_ref(data: &[u8]) -> Result<(), String> {
    let Ok(input) = std::str::from_utf8(data) else {
        return Ok(());
    };
    let Ok(parsed) = input.parse::<CompareRef>() else {
        return Ok(());
    };
    let shown = parsed.to_string();
    ensure(shown.parse::<CompareRef>().as_ref() == Ok(&parsed), || {
        format!("{input:?} displays as {shown:?}, which does not parse back to the same value")
    })
}

/// Reverse mapping never panics, and when it succeeds the forward mapping gives the NFS path back.
///
/// Input format (so seeds stay plain text): the first line is an NFS path, every further line is a tenant id (lines that
/// are not valid tenant ids are ignored). The default rule (`config/`, `data/config/`) is used.
pub fn path_mapping_reverse(data: &[u8]) -> Result<(), String> {
    let Ok(input) = std::str::from_utf8(data) else {
        return Ok(());
    };
    let mut lines = input.split('\n');
    let first = lines.next().unwrap_or("");
    let tenants: Vec<TenantId> = lines.filter_map(|l| TenantId::parse(l).ok()).collect();
    let Ok(tenant_set) = TenantSet::new(tenants.iter().cloned()) else {
        return Ok(());
    };
    let rule = PathMappingRule::default();

    if let Ok(nfs) = NfsPath::parse(first) {
        match rule.reverse(&nfs, &tenant_set) {
            Err(_) => {}
            Ok(ReverseMapping::Base { repo_path, logical }) => {
                ensure(logical.as_path() == &nfs, || {
                    format!("{nfs} reversed to the base file {logical}, which differs from the input")
                })?;
                ensure(rule.base_to_nfs(&repo_path).as_ref() == Ok(&nfs), || {
                    format!("{nfs} reversed to {repo_path}, which does not map forward to it")
                })?;
            }
            Ok(ReverseMapping::Tenant {
                tenant, repo_path, ..
            }) => {
                ensure(tenant_set.contains(&tenant), || {
                    format!("{nfs} reversed to the unknown tenant {tenant}")
                })?;
                ensure(
                    rule.tenant_to_nfs(&repo_path, &tenant).as_ref() == Ok(&nfs),
                    || format!("{nfs} reversed to {repo_path} on {tenant}, which does not map forward to it"),
                )?;
            }
        }
    }

    // The forward direction must also be panic free on any repo path, and a base file maps back to itself.
    if let Ok(repo_path) = RepoPath::parse(first) {
        if let Ok(nfs) = rule.base_to_nfs(&repo_path) {
            if let Ok(ReverseMapping::Base { repo_path: back, .. }) =
                rule.reverse(&nfs, &TenantSet::default())
            {
                ensure(back == repo_path, || {
                    format!("{repo_path} maps to {nfs}, which reverses to {back}")
                })?;
            }
        }
        for tenant in &tenants {
            let _ = rule.tenant_to_nfs(&repo_path, tenant);
        }
    }
    Ok(())
}
