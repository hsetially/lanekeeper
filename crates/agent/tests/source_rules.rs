//! Rules about the source itself (S17, S21), checked by reading `src/`.
//!
//! These are cheap guards that fail the moment someone reaches for the wrong API: a test that only covers today's
//! code would miss the next file.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

/// Files that may open something with ambient authority. Each is a startup reader that runs before any hub command:
/// the NFS root (`root.rs`), the pinned hub CA file (`tls.rs`, read once, with a size limit, through a cap-std `Dir`
/// opened on its directory) and the spool directory (`volume.rs`, opened once from `LK_SPOOL_DIR`; the spool is a
/// mounted volume of its own and cannot be reached through the NFS root's handle; everything after that goes through the
/// `Dir`, and `spool_file_names_are_made_by_the_spool` checks that no name comes from anywhere else).
const AMBIENT_ALLOWED: &[&str] = &["root.rs", "tls.rs", "volume.rs"];

/// APIs that reach the filesystem without a cap-std `Dir`.
const FORBIDDEN_ALWAYS: &[&str] = &[
    "std::fs",
    "tokio::fs",
    "File::open",
    "File::create",
    "File::options",
];

/// APIs that create a capability from ambient authority.
const AMBIENT_ONLY_IN_ALLOWED: &[&str] = &[
    "ambient_authority",
    "open_ambient",
    "from_std_file",
    "from_raw_fd",
];

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `dir`, as (path relative to `src/`, contents).
fn sources(dir: &Path) -> Vec<(String, String)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// True when `token` appears in `line` as a whole path, so `std::fs` matches but `cap_std::fs` does not.
fn has_token(line: &str, token: &str) -> bool {
    line.match_indices(token).any(|(at, _)| {
        line[..at]
            .chars()
            .next_back()
            .is_none_or(|before| !(before.is_alphanumeric() || before == '_'))
    })
}

/// Lines of code that break the file-access rule. Comment lines are skipped.
fn file_access_violations(rel: &str, source: &str, ambient_allowed: &[&str]) -> Vec<String> {
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    let may_open_ambient = ambient_allowed.contains(&file_name);
    let mut found = Vec::new();
    for (n, line) in source.lines().enumerate() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        let uses_std_fs_in_a_group = line.trim_start().starts_with("use std::{") && has_token(line, "fs");
        let forbidden = FORBIDDEN_ALWAYS.iter().any(|t| has_token(line, t)) || uses_std_fs_in_a_group;
        let ambient = !may_open_ambient && AMBIENT_ONLY_IN_ALLOWED.iter().any(|t| has_token(line, t));
        if forbidden || ambient {
            found.push(format!("{rel}:{}: {}", n + 1, line.trim()));
        }
    }
    found
}

#[test]
fn all_file_access_via_cap_std() {
    let files = sources(&src_dir());
    assert!(files.len() >= 4, "the scan found only {} files", files.len());
    let violations: Vec<String> = files
        .iter()
        .flat_map(|(rel, source)| file_access_violations(rel, source, AMBIENT_ALLOWED))
        .collect();
    assert!(
        violations.is_empty(),
        "file access outside cap-std:\n{}",
        violations.join("\n")
    );
    // The allow-list is not a dead entry: root.rs really is the one place the root is opened, and spool/volume.rs the one
    // place the spool directory is.
    let root = files.iter().find(|(rel, _)| rel == "root.rs").unwrap();
    assert!(root.1.contains("open_ambient_dir"));
    let volume = files.iter().find(|(rel, _)| rel == "spool/volume.rs").unwrap();
    assert!(volume.1.contains("open_ambient_dir"));
    assert!(
        !files.iter().any(|(rel, source)| rel.starts_with("spool/")
            && rel != "spool/volume.rs"
            && source.contains("ambient_authority")),
        "only spool/volume.rs may open the spool directory with ambient authority"
    );
}

/// The calls that name a file inside the spool directory.
const SPOOL_NAMING_CALLS: &[&str] = &[".open(", ".open_with(", ".remove_file(", ".rename("];

/// What a name may start with: made here (`segment::name(id)`, the state file's constants) or "the directory itself".
const SPOOL_NAME_ORIGINS: &[&str] = &[
    "segment::name(",
    "name(",
    "STATE_FILE",
    "STATE_TMP",
    "\".\"",
    "&name",
    "name",
];

fn spool_naming_violations(rel: &str, source: &str) -> Vec<String> {
    if !rel.starts_with("spool/") {
        return Vec::new();
    }
    source
        .lines()
        .enumerate()
        .take_while(|(_, line)| line.trim() != "#[cfg(test)]")
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter_map(|(n, line)| {
            SPOOL_NAMING_CALLS.iter().find_map(|call| {
                let at = line.find(call)?;
                let argument = line[at + call.len()..].trim_start();
                let ok = SPOOL_NAME_ORIGINS
                    .iter()
                    .any(|origin| argument.starts_with(origin));
                (!ok).then(|| format!("{rel}:{}: {}", n + 1, line.trim()))
            })
        })
        .collect()
}

/// S17: a file name in the spool is made by the spool (`seg-<hex>.lks`, `state`), never taken from a hub message, a path
/// in a delta or a file's content.
#[test]
fn spool_file_names_are_made_by_the_spool() {
    let files = sources(&src_dir());
    let violations: Vec<String> = files
        .iter()
        .flat_map(|(rel, source)| spool_naming_violations(rel, source))
        .collect();
    assert!(
        violations.is_empty(),
        "spool files named from elsewhere:\n{}",
        violations.join("\n")
    );
    // The scan is not blind: the spool does name files this way.
    let core = files.iter().find(|(rel, _)| rel == "spool/core.rs").unwrap();
    assert!(core.1.contains("segment::name("));
    // And it catches a planted name.
    for planted in [
        "dir.open(delta.path.as_str())?;",
        "dir.remove_file(&entry.path)?;",
        "dir.open_with(format!(\"seg-{}\", p), &o)?;",
        "dir.rename(from, dir, to)?;",
    ] {
        assert_eq!(
            spool_naming_violations("spool/core.rs", planted).len(),
            1,
            "{planted}"
        );
    }
    assert!(spool_naming_violations("spool/core.rs", "dir.open(segment::name(id))?;").is_empty());
    assert!(spool_naming_violations("fileops.rs", "dir.open(path)?;").is_empty());
}

#[test]
fn file_access_scan_rejects_planted_violations() {
    let planted = [
        "let b = std::fs::read(p)?;",
        "let f = tokio::fs::File::open(p).await?;",
        "let f = File::open(p)?;",
        "use std::{fs, io};",
        "let d = Dir::open_ambient_dir(p, ambient_authority())?;",
    ];
    for line in planted {
        assert_eq!(
            file_access_violations("tree/walk.rs", line, AMBIENT_ALLOWED).len(),
            1,
            "{line}"
        );
    }
    // The ambient open is allowed in an allow-listed file, and comments are not code.
    let ambient = "let d = Dir::open_ambient_dir(p, ambient_authority())?;";
    assert!(file_access_violations("root.rs", ambient, AMBIENT_ALLOWED).is_empty());
    assert!(
        file_access_violations("tree/walk.rs", "// std::fs::read is forbidden", AMBIENT_ALLOWED).is_empty()
    );
    assert!(file_access_violations("tree/walk.rs", "/// see `std::fs`", AMBIENT_ALLOWED).is_empty());
    // A cap-std path that merely contains `fs::` is fine.
    assert!(file_access_violations("tree/walk.rs", "use cap_std::fs::Dir;", AMBIENT_ALLOWED).is_empty());
}

#[test]
fn forbid_unsafe_present() {
    let src = src_dir();
    for file in ["lib.rs", "main.rs"] {
        let text = fs::read_to_string(src.join(file)).unwrap();
        assert!(
            text.contains("#![forbid(unsafe_code)]"),
            "{file} lacks #![forbid(unsafe_code)]"
        );
    }
}

#[test]
fn lints_inherit_the_workspace_policy() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let lints = manifest.split("[lints]").nth(1).expect("a [lints] table");
    let first = lints.lines().find(|l| !l.trim().is_empty()).unwrap();
    assert_eq!(first.trim(), "workspace = true");
}

/// The SAN prefix is written once, in `identity/cert.rs`, and everything else refers to the constant (S5, decision A5).
#[test]
fn agent_san_defined_in_one_constant() {
    const PREFIX: &str = "spiffe://lanekeeper/swimlane/";
    const OLD_FORM: &str = "spiffe://lanekeeper/agent/";
    let files = sources(&src_dir());
    let mut spelled_out = Vec::new();
    for (rel, source) in &files {
        for (n, line) in source.lines().enumerate() {
            // Comments may describe the form; only code can define it.
            if line.trim_start().starts_with("//") {
                continue;
            }
            assert!(!line.contains(OLD_FORM), "{rel}:{}: the old SAN form (A5)", n + 1);
            if line.contains(PREFIX) {
                spelled_out.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert_eq!(
        spelled_out.len(),
        1,
        "the SAN prefix must be defined once:\n{}",
        spelled_out.join("\n")
    );
    assert!(
        spelled_out[0].starts_with("identity/cert.rs:") && spelled_out[0].contains("AGENT_SAN_PREFIX"),
        "{}",
        spelled_out[0]
    );
    // And the one place that checks a certificate uses the constant.
    let cert = files.iter().find(|(rel, _)| rel == "identity/cert.rs").unwrap();
    assert!(cert.1.contains("strip_prefix(AGENT_SAN_PREFIX)"));
}

/// Lines that name the generated wire types outside the transport directory. Comments are skipped.
fn generated_type_uses(rel: &str, source: &str) -> Vec<String> {
    if rel.starts_with("transport/") {
        return Vec::new();
    }
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(_, line)| has_token(line, "proto::pb") || has_token(line, "pb::"))
        .map(|(n, line)| format!("{rel}:{}: {}", n + 1, line.trim()))
        .collect()
}

/// S11: everything the hub sends is validated by `proto::convert` before the agent acts on it. The generated messages
/// are used in `transport/` only, so no other module can read an unvalidated path, hash or length.
#[test]
fn generated_wire_types_stay_in_the_transport_directory() {
    let files = sources(&src_dir());
    let violations: Vec<String> = files
        .iter()
        .flat_map(|(rel, source)| generated_type_uses(rel, source))
        .collect();
    assert!(
        violations.is_empty(),
        "generated wire types outside transport/:\n{}",
        violations.join("\n")
    );
    // The scan is not blind: the transport does use them.
    assert!(
        files
            .iter()
            .any(|(rel, source)| rel.starts_with("transport/") && has_token(source, "pb::HubMessage"))
    );
    // And it catches a planted use.
    assert_eq!(
        generated_type_uses("scan.rs", "fn f(m: proto::pb::HubMessage) {}").len(),
        1
    );
    assert_eq!(
        generated_type_uses("dispatch.rs", "use proto::pb;\nlet m: pb::ReadFile = x;").len(),
        2
    );
    assert!(generated_type_uses("transport/wire.rs", "use proto::pb;").is_empty());
}

/// Rule 5: every channel is bounded.
#[test]
fn no_unbounded_channels() {
    for (rel, source) in sources(&src_dir()) {
        for (n, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            assert!(
                !line.contains("unbounded_channel") && !line.contains("UnboundedSender"),
                "{rel}:{}: an unbounded channel (code rule 5)",
                n + 1
            );
        }
    }
}

/// Decision A1 / D89: the walker is the agent's own, over cap-std. The `ignore` crate walks by path and would follow a
/// directory swapped for a symlink, so it must not come back as a dependency.
#[test]
fn the_agent_does_not_depend_on_the_ignore_crate() {
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for line in manifest.lines().filter(|l| !l.trim_start().starts_with('#')) {
        let name = line.split(['=', '.']).next().unwrap_or("").trim();
        assert_ne!(
            name, "ignore",
            "the agent walks with its own cap-std walker (D89): {line}"
        );
    }
    for (rel, source) in sources(&src_dir()) {
        for (n, line) in source.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            assert!(
                !line.contains("WalkParallel") && !line.contains("ignore::"),
                "{rel}:{}: the `ignore` crate's walker (D89)",
                n + 1
            );
        }
    }
}

/// APIs that start another program. The agent never runs a shell or any other process (S17): everything it does is a
/// file operation through cap-std or a call on the Kubernetes API. `std::process::ExitCode` (the exit status of
/// `main`) is not one of them.
const PROCESS_APIS: &[&str] = &[
    "tokio::process",
    "std::process::Command",
    "process::Command",
    "Command::new",
    "CommandExt",
    "std::os::unix::process",
    "libc::system",
    "libc::execv",
    "libc::execve",
    "libc::fork",
    "posix_spawn",
];

/// Lines of code that start a process. Comment lines are skipped.
fn process_violations(rel: &str, source: &str) -> Vec<String> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(_, line)| {
            let in_use_group =
                line.trim_start().starts_with("use std::process::{") && has_token(line, "Command");
            in_use_group || PROCESS_APIS.iter().any(|api| has_token(line, api))
        })
        .map(|(n, line)| format!("{rel}:{}: {}", n + 1, line.trim()))
        .collect()
}

#[test]
fn no_shell_or_exec_in_source() {
    let files = sources(&src_dir());
    let violations: Vec<String> = files
        .iter()
        .flat_map(|(rel, source)| process_violations(rel, source))
        .collect();
    assert!(
        violations.is_empty(),
        "the agent starts a process (S17):\n{}",
        violations.join("\n")
    );
    // The manifest pulls in nothing that exists to run programs.
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    for line in manifest.lines().filter(|l| !l.trim_start().starts_with('#')) {
        let name = line.split(['=', '.']).next().unwrap_or("").trim();
        assert!(
            !matches!(
                name,
                "duct" | "subprocess" | "xshell" | "cmd_lib" | "tokio-process" | "async-process"
            ),
            "a process-spawning dependency: {line}"
        );
    }
}

#[test]
fn process_scan_rejects_planted_violations() {
    let planted = [
        "let out = std::process::Command::new(\"sh\").output()?;",
        "let child = tokio::process::Command::new(\"ls\");",
        "use std::process::{Command, ExitCode};",
        "use std::os::unix::process::CommandExt;",
        "let c = Command::new(\"cat\");",
        "unsafe { libc::system(cmd) };",
    ];
    for line in planted {
        assert_eq!(process_violations("fileops.rs", line).len(), 1, "{line}");
    }
    // The exit status of `main` is fine, and so are comments.
    assert!(process_violations("main.rs", "use std::process::ExitCode;").is_empty());
    assert!(process_violations("main.rs", "ExitCode::from(2)").is_empty());
    assert!(process_violations("fileops.rs", "// never std::process::Command").is_empty());
    assert!(process_violations("dispatch.rs", "pub enum HubCommand { }").is_empty());
}

/// Calls that change the file system. Through cap-std these are methods on a `Dir` or an `OpenOptions`; they are listed
/// by name because a `Dir` is the only way the agent reaches a file at all (`all_file_access_via_cap_std`).
const FS_WRITE_APIS: &[&str] = &[
    "create_new(",
    "OpenOptions::new",
    ".rename(",
    ".remove_file(",
    ".remove_dir",
    ".create_dir",
    ".hard_link(",
    ".symlink(",
    ".set_permissions(",
    ".write_all(",
    ".create(",
];

/// Where those calls may be (S16): the file operations on the NFS export, and (from T9) the spool. The temp directory is
/// not written by the agent today. `tree/testfs.rs` builds fixtures for unit tests and is not part of the binary.
const FS_WRITERS: &[&str] = &["fileops.rs", "spool/", "tree/testfs.rs"];

/// Lines of non-test code that change the file system, outside the files allowed to. Everything from the first
/// `#[cfg(test)]` on is test code (the convention in this crate), so a helper that builds a fixture is not a writer.
fn fs_write_violations(rel: &str, source: &str, writers: &[&str]) -> Vec<String> {
    if writers
        .iter()
        .any(|w| rel == *w || (w.ends_with('/') && rel.starts_with(w)))
    {
        return Vec::new();
    }
    source
        .lines()
        .enumerate()
        .take_while(|(_, line)| line.trim() != "#[cfg(test)]")
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(_, line)| FS_WRITE_APIS.iter().any(|api| line.contains(api)))
        .map(|(n, line)| format!("{rel}:{}: {}", n + 1, line.trim()))
        .collect()
}

/// S16: the root file system is read-only, and the three places the pod may write are the NFS export (file operations),
/// the spool, and the temp directory. Nothing else in the source changes a file.
#[test]
fn no_fs_writes_outside_spool_and_tmp() {
    let files = sources(&src_dir());
    let violations: Vec<String> = files
        .iter()
        .flat_map(|(rel, source)| fs_write_violations(rel, source, FS_WRITERS))
        .collect();
    assert!(
        violations.is_empty(),
        "file-system writes outside the file operations and the spool (S16):\n{}",
        violations.join("\n")
    );
    // `tree/testfs.rs` is allowed because it is compiled for the crate's unit tests only.
    let tree_mod = files.iter().find(|(rel, _)| rel == "tree/mod.rs").unwrap();
    assert!(
        tree_mod.1.contains("#[cfg(test)]\nmod testfs;"),
        "tree/testfs.rs must be declared under #[cfg(test)]"
    );
    // The scan is not blind: the file operations do write, through these calls.
    let fileops = files.iter().find(|(rel, _)| rel == "fileops.rs").unwrap();
    assert!(
        FS_WRITE_APIS
            .iter()
            .filter(|api| fileops.1.contains(*api))
            .count()
            >= 4
    );
}

#[test]
fn fs_write_scan_rejects_planted_violations() {
    let planted = [
        "dir.create(\"x\")?;",
        "let f = options.write(true).create_new(true);",
        "dir.rename(a, &dir, b)?;",
        "dir.remove_file(name)?;",
        "dir.create_dir_all(p)?;",
        "file.write_all(bytes)?;",
        "std::fs::OpenOptions::new()",
    ];
    for line in planted {
        assert_eq!(
            fs_write_violations("scan.rs", line, FS_WRITERS).len(),
            1,
            "{line}"
        );
    }
    // The allowed writers, comments, and code after `#[cfg(test)]` are not violations.
    assert!(fs_write_violations("fileops.rs", "dir.rename(a, &dir, b)?;", FS_WRITERS).is_empty());
    assert!(fs_write_violations("spool/segment.rs", "file.write_all(b)?;", FS_WRITERS).is_empty());
    assert!(fs_write_violations("scan.rs", "// dir.remove_file(name)", FS_WRITERS).is_empty());
    assert!(
        fs_write_violations(
            "tree/source.rs",
            "fn real() {}\n#[cfg(test)]\nmod tests { fn f() { dir.create_dir(x); } }",
            FS_WRITERS
        )
        .is_empty()
    );
    // But a writer that is not allowed is caught even when a test module follows.
    assert_eq!(
        fs_write_violations(
            "tree/source.rs",
            "fn real() { dir.create_dir(x); }\n#[cfg(test)]\nmod tests {}",
            FS_WRITERS
        )
        .len(),
        1
    );
}

// ------------------------------------------------------------------------------------------------ the config-server

/// Lines of code, outside `allowed` files, that name the config-server's unauthenticated refresh endpoint, make a client
/// for it, or call one of its two calls (Q37, S17). Comment lines are skipped. The calls may be made from the dispatcher
/// only, and the client is built in `app.rs` alone: nothing else in the agent can reach the config-server, so every call
/// is the answer to a command from the hub.
fn config_server_violations(rel: &str, source: &str) -> Vec<String> {
    const CONFINED: [(&str, &[&str]); 4] = [
        ("update-resources", &["configserver.rs"]),
        ("ConfigServerClient::new", &["configserver.rs", "app.rs"]),
        (".notify(", &["configserver.rs", "dispatch.rs"]),
        (".fetch_served(", &["configserver.rs", "dispatch.rs"]),
    ];
    // Everything from the first `#[cfg(test)]` on is test code (the convention in this crate).
    let code = source.split("#[cfg(test)]").next().unwrap_or(source);
    code.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(_, line)| {
            CONFINED
                .iter()
                .any(|(needle, allowed)| line.contains(needle) && !allowed.contains(&rel))
        })
        .map(|(n, line)| format!("{rel}:{}: {}", n + 1, line.trim()))
        .collect()
}

#[test]
fn config_server_calls_only_from_the_dispatcher() {
    let violations: Vec<String> = sources(&src_dir())
        .iter()
        .flat_map(|(rel, source)| config_server_violations(rel, source))
        .collect();
    assert!(
        violations.is_empty(),
        "something other than the dispatcher can reach the config-server (Q37):\n{}",
        violations.join("\n")
    );
    // And the rule is not vacuous: the dispatcher does call both.
    let files = sources(&src_dir());
    let dispatch = &files.iter().find(|(rel, _)| rel == "dispatch.rs").unwrap().1;
    assert!(dispatch.contains(".notify(") && dispatch.contains(".fetch_served("));
}

#[test]
fn config_server_scan_rejects_planted_violations() {
    for (rel, line) in [
        ("scan.rs", "client.notify(&paths).await"),
        ("scan.rs", "let r = client.fetch_served(&request).await;"),
        ("kube/watch.rs", "post(\"/update-resources\")"),
        (
            "kube/watch.rs",
            "let c = ConfigServerClient::new(base, deny, clock);",
        ),
        (
            "dispatch.rs",
            "let c = ConfigServerClient::new(base, deny, clock);",
        ),
    ] {
        assert_eq!(config_server_violations(rel, line).len(), 1, "{rel}: {line}");
    }
    for (rel, line) in [
        ("dispatch.rs", "client.notify(paths).await"),
        ("dispatch.rs", "client.fetch_served(request).await"),
        ("app.rs", "ConfigServerClient::new(base, deny, clock)"),
        (
            "configserver.rs",
            "pub const NOTIFY_PATH: &str = \"/update-resources\";",
        ),
        ("scan.rs", "// never call update-resources from here"),
    ] {
        assert!(config_server_violations(rel, line).is_empty(), "{rel}: {line}");
    }
    // Test code is not scanned.
    assert!(
        config_server_violations("scan.rs", "#[cfg(test)]\nmod t { fn f() { c.notify(&p); } }").is_empty()
    );
}
