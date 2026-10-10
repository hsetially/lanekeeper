//! The properties the agent's fuzz targets assert (S11, S17, S22).
//!
//! This file is included, with `#[path]`, by the fuzz targets (`fuzz_targets/*.rs`) and by
//! `crates/agent/tests/corpus_replay.rs`, so the nightly fuzzer and the stable seed replay (which `just verify` runs)
//! check exactly the same things. A check returns `Err(reason)` for a violation; the fuzz targets turn that into a
//! panic, which libFuzzer reports as a crash.
//!
//! Inputs that a parser rightly rejects are not violations: the checks look at what was accepted, and at what an
//! operation did to the world around the root.
//!
//! # The targets
//!
//! | Target | Input | What must hold |
//! |---|---|---|
//! | `agent_path` | one byte (the operation), then a path | whatever the path is, an operation on a root with planted symlinks changes nothing outside the root, leaves every symlink as it was, leaves no temporary file, and a success never went through a symlink |
//! | `agent_hub_message` | a protobuf `HubMessage` | decoding never panics; a configuration is clamped into its ranges and its globs compile or fail cleanly; a file command gets exactly one answer, of bounded size, and touches nothing outside the root |
//! | `agent_cert_chain` | length-prefixed DER blobs | random bytes are never accepted as the agent's certificate |
//! | `agent_pem` | one byte (key, CA, or damage to a generated key), then PEM text | a key that is accepted survives a PEM round trip; an undamaged generated key is always accepted; neither parser panics |
//! | `agent_id_token` | text | a string accepted as a token is three non-empty base64url segments within the size limit |

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic
)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use agent::config::Tunables;
use agent::dispatch::{CommandHandler, Dispatcher, OpLimits};
use agent::fileops::{FileOps, TreeEdits};
use agent::identity::idtoken::looks_like_a_jwt;
use agent::identity::{ClientIdentity, KeyMaterial};
use agent::root::NfsRoot;
use agent::transport::tls::HubRoots;
use agent::transport::wire::{self, Decoded};
use agent::tree::{FileLeaf, WalkConfig};
use async_trait::async_trait;
use bytes::Bytes;
use domain::{ContentHash, Expected, HubCommand, NfsPath, SwimlaneId, Timestamp};
use prost::Message;
use proto::convert::{FromAgent, ToAgent};

/// A check over one fuzz input.
pub type Check = fn(&[u8]) -> Result<(), String>;

/// Every fuzz target: its name (also the file name in `fuzz_targets` and the corpus directory) and its check.
pub const TARGETS: [(&str, Check); 5] = [
    ("agent_path", agent_path),
    ("agent_hub_message", agent_hub_message),
    ("agent_cert_chain", agent_cert_chain),
    ("agent_pem", agent_pem),
    ("agent_id_token", agent_id_token),
];

/// Every seed corpus must hold at least this many inputs, so deleting the seeds cannot go unnoticed.
pub const MIN_SEEDS: usize = 8;

/// The most bytes of a path the file-operation target looks at. A path is at most 1,024 bytes anyway.
const MAX_PATH_INPUT: usize = 1100;

fn ensure(cond: bool, why: impl FnOnce() -> String) -> Result<(), String> {
    if cond { Ok(()) } else { Err(why()) }
}

// ------------------------------------------------------------------------------------------------ the world

/// What the file-operation targets run against: a root with files, directories and symlinks of every awkward kind, and
/// next to it things the agent must never touch.
///
/// ```text
/// outer/
///   sentinel.txt            "sentinel"
///   outside/deep.txt        "deep"
///   root/                   <- the NFS root
///     a.yml                 "alpha"
///     svc/b.yml             "beta"
///     svc/sub/
///     up                  -> ..
///     link                -> ../sentinel.txt
///     abs                 -> <outer>           (absolute)
///     dangling            -> nowhere
///     svc/escape          -> ../..
///     svc/alias           -> sub
///     svc/b-link.yml      -> b.yml
/// ```
struct World {
    outer: tempfile::TempDir,
    root: PathBuf,
}

impl World {
    fn new() -> Self {
        let outer = tempfile::TempDir::new().unwrap();
        let root = outer.path().join("root");
        fs::create_dir_all(root.join("svc/sub")).unwrap();
        fs::create_dir_all(outer.path().join("outside")).unwrap();
        fs::write(outer.path().join("sentinel.txt"), b"sentinel").unwrap();
        fs::write(outer.path().join("outside/deep.txt"), b"deep").unwrap();
        fs::write(root.join("a.yml"), b"alpha").unwrap();
        fs::write(root.join("svc/b.yml"), b"beta").unwrap();
        symlink("..", root.join("up")).unwrap();
        symlink("../sentinel.txt", root.join("link")).unwrap();
        symlink(outer.path(), root.join("abs")).unwrap();
        symlink("nowhere", root.join("dangling")).unwrap();
        symlink("../..", root.join("svc/escape")).unwrap();
        symlink("sub", root.join("svc/alias")).unwrap();
        symlink("b.yml", root.join("svc/b-link.yml")).unwrap();
        Self { outer, root }
    }

    /// Everything that must not change: all of `outer` except the contents of `root`, and every symlink in `root`.
    fn guarded(&self) -> BTreeMap<String, String> {
        let mut seen = BTreeMap::new();
        walk(self.outer.path(), self.outer.path(), &self.root, &mut seen);
        seen
    }

    /// Names of the agent's temporary files anywhere under the root.
    fn temp_files(&self) -> Vec<String> {
        let mut found = Vec::new();
        find_temp(&self.root, &mut found);
        found
    }
}

fn walk(base: &Path, dir: &Path, root: &Path, seen: &mut BTreeMap<String, String>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let rel = path.strip_prefix(base).unwrap().to_string_lossy().into_owned();
        let md = fs::symlink_metadata(&path).unwrap();
        if md.file_type().is_symlink() {
            seen.insert(rel, format!("link:{}", fs::read_link(&path).unwrap().display()));
        } else if md.is_dir() {
            if path != root {
                seen.insert(rel, "dir".to_owned());
            }
            // Inside the root only the symlinks are guarded: the operations are allowed to change plain files there.
            walk(base, &path, root, seen);
        } else if !path.starts_with(root) {
            seen.insert(
                rel,
                format!("file:{}", String::from_utf8_lossy(&fs::read(&path).unwrap())),
            );
        }
    }
}

fn find_temp(dir: &Path, found: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(".lanekeeper-tmp-") {
            found.push(name);
        }
        if entry.file_type().unwrap().is_dir() {
            find_temp(&entry.path(), found);
        }
    }
}

/// The tree is not under test here.
#[derive(Debug)]
struct NoEdits;

#[async_trait]
impl TreeEdits for NoEdits {
    async fn written(&self, _path: &NfsPath, _leaf: FileLeaf) {}
    async fn removed(&self, _path: &NfsPath) {}
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
    })
}

fn sha(content: &[u8]) -> ContentHash {
    agent::tree::hash::hash_bytes(content)
}

fn dispatcher(world: &World) -> Dispatcher {
    Dispatcher::new(FileOps::new(
        NfsRoot::open(&world.root).unwrap(),
        Arc::new(NoEdits),
    ))
}

const LIMITS: OpLimits = OpLimits {
    max_file_bytes: 2 * 1024 * 1024,
};

/// After an operation: nothing outside the root moved, no symlink was changed, no temporary file is left behind.
fn world_unharmed(world: &World, before: &BTreeMap<String, String>, what: &str) -> Result<(), String> {
    let after = world.guarded();
    ensure(&after == before, || {
        let changed: Vec<_> = before
            .iter()
            .filter(|(k, v)| after.get(*k) != Some(*v))
            .map(|(k, _)| k.clone())
            .chain(after.keys().filter(|k| !before.contains_key(*k)).cloned())
            .collect();
        format!("{what} changed what must never change: {changed:?}")
    })?;
    let temps = world.temp_files();
    ensure(temps.is_empty(), || {
        format!("{what} left temporary files: {temps:?}")
    })
}

/// A path the operation succeeded on must not have gone through a symlink.
fn no_symlink_on_the_way(world: &World, path: &NfsPath) -> Result<(), String> {
    let mut walked = world.root.clone();
    for component in path.as_str().split('/') {
        walked.push(component);
        // A delete removed the last component, so there may be nothing there any more.
        if let Ok(md) = fs::symlink_metadata(&walked) {
            ensure(!md.file_type().is_symlink(), || {
                format!(
                    "{:?} succeeded through the symlink at {component:?}",
                    path.as_str()
                )
            })?;
        }
    }
    Ok(())
}

/// A way to hurt the world, for the self-test below.
type Harm = fn(&World);

/// Proof that the world check is not blind: each of the harms it exists to find, planted by hand, is found. Run by
/// `corpus_replay.rs`; not a fuzz target.
pub fn planted_harm_is_detected() -> Result<(), String> {
    let harms: [(&str, Harm); 5] = [
        ("a changed sentinel", |w| {
            fs::write(w.outer.path().join("sentinel.txt"), b"changed").unwrap()
        }),
        ("a file planted outside the root", |w| {
            fs::write(w.outer.path().join("outside/planted.txt"), b"x").unwrap();
        }),
        ("a deleted outside file", |w| {
            fs::remove_file(w.outer.path().join("outside/deep.txt")).unwrap()
        }),
        ("a symlink replaced by a file", |w| {
            fs::remove_file(w.root.join("link")).unwrap();
            fs::write(w.root.join("link"), b"not a link any more").unwrap();
        }),
        ("a temporary file left behind", |w| {
            fs::write(w.root.join("svc/.lanekeeper-tmp-00"), b"x").unwrap();
        }),
    ];
    for (name, harm) in harms {
        let world = World::new();
        let before = world.guarded();
        world_unharmed(&world, &before, "nothing")?;
        harm(&world);
        ensure(world_unharmed(&world, &before, name).is_err(), || {
            format!("{name} went unnoticed")
        })?;
    }
    // And a success that went through a symlink is noticed.
    let world = World::new();
    ensure(
        no_symlink_on_the_way(&world, &NfsPath::parse("svc/alias/x").unwrap()).is_err(),
        || "a path through a symlink directory was not noticed".to_owned(),
    )?;
    ensure(
        no_symlink_on_the_way(&world, &NfsPath::parse("svc/b.yml").unwrap()).is_ok(),
        || "a plain path was flagged".to_owned(),
    )
}

// ------------------------------------------------------------------------------------------------ agent_path

/// The first byte picks the operation; the rest is the path. Whatever it is, the root's walls hold (S17).
pub fn agent_path(data: &[u8]) -> Result<(), String> {
    let Some((op, rest)) = data.split_first() else {
        return Ok(());
    };
    let rest = &rest[..rest.len().min(MAX_PATH_INPUT)];
    let text = String::from_utf8_lossy(rest);
    let Ok(path) = NfsPath::parse(&text) else {
        return Ok(());
    };
    let world = World::new();
    let before = world.guarded();
    let dispatcher = dispatcher(&world);
    let id = domain::RequestId::parse("fuzz").unwrap();
    let command = match op % 6 {
        0 => HubCommand::ReadFile {
            request_id: id,
            path: path.clone(),
        },
        1 => HubCommand::WriteFile {
            request_id: id,
            path: path.clone(),
            expected: Expected::Absent,
            bytes: Bytes::from_static(b"planted"),
        },
        2 => HubCommand::WriteFile {
            request_id: id,
            path: path.clone(),
            expected: Expected::Hash { hash: sha(b"alpha") },
            bytes: Bytes::from_static(b"planted"),
        },
        3 => HubCommand::WriteFile {
            request_id: id,
            path: path.clone(),
            expected: Expected::Hash { hash: sha(b"beta") },
            bytes: Bytes::from_static(b"planted"),
        },
        4 => HubCommand::DeleteFile {
            request_id: id,
            path: path.clone(),
            expected: sha(b"alpha"),
        },
        _ => HubCommand::DeleteFile {
            request_id: id,
            path: path.clone(),
            expected: sha(b"sentinel"),
        },
    };
    let is_change = !matches!(command, HubCommand::ReadFile { .. });
    let reply = runtime().block_on(dispatcher.handle(command, LIMITS));
    let Some(FromAgent::Reply(reply)) = reply else {
        return Err("a file command got no answer".to_owned());
    };
    let succeeded = match &reply {
        domain::AgentReply::Op(result) => result.ok,
        domain::AgentReply::File { .. } => true,
        _ => return Err("a file command got a reply of another kind".to_owned()),
    };
    world_unharmed(&world, &before, &format!("{:?}", path.as_str()))?;
    if succeeded {
        no_symlink_on_the_way(&world, &path)?;
        if let domain::AgentReply::File { bytes, hash, .. } = &reply {
            // What a read returns is what is in a plain file inside the root, and its hash.
            ensure(*hash == sha(bytes), || {
                "a read returned bytes that do not match their hash".to_owned()
            })?;
            ensure(matches!(&bytes[..], b"alpha" | b"beta" | b"planted"), || {
                format!(
                    "a read returned bytes that are in no file of the root: {:?}",
                    String::from_utf8_lossy(bytes)
                )
            })?;
        }
    }
    // A change that was refused must have left the plain files alone too.
    if is_change && !succeeded {
        ensure(fs::read(world.root.join("a.yml")).unwrap() == b"alpha", || {
            "a refused change touched a.yml".to_owned()
        })?;
        ensure(fs::read(world.root.join("svc/b.yml")).unwrap() == b"beta", || {
            "a refused change touched svc/b.yml".to_owned()
        })?;
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ agent_hub_message

/// A message from the hub is decoded, validated, and, if it is a file command, carried out against the world.
pub fn agent_hub_message(data: &[u8]) -> Result<(), String> {
    let Ok(message) = proto::pb::HubMessage::decode(data) else {
        return Ok(());
    };
    match wire::decode(message) {
        Decoded::Unknown => Ok(()),
        Decoded::Invalid { request_id, .. } => {
            if let Some(id) = request_id {
                let encoded = wire::encode(wire::refusal(id));
                ensure(encoded.encoded_len() < 512, || {
                    "a refusal grew with its input".to_owned()
                })?;
            }
            Ok(())
        }
        Decoded::Message(ToAgent::Config(config)) => {
            let tunables = Tunables::from_hub(&config);
            let secs = |d: std::time::Duration| d.as_secs();
            ensure((5..=300).contains(&secs(tunables.scan_interval)), || {
                format!("scan interval {:?} is out of range", tunables.scan_interval)
            })?;
            ensure((10..=300).contains(&secs(tunables.heartbeat_interval)), || {
                format!(
                    "heartbeat interval {:?} is out of range",
                    tunables.heartbeat_interval
                )
            })?;
            ensure(
                tunables.max_file_bytes > 0
                    && tunables.max_file_bytes <= agent::config::limits::MAX_FILE_BYTES,
                || format!("max file bytes {} is out of range", tunables.max_file_bytes),
            )?;
            ensure(
                tunables.deny_globs.len() <= proto::limits::MAX_CONFIG_ITEMS,
                || "more deny globs than the contract allows".to_owned(),
            )?;
            // Compiling the hub's globs must either work or fail cleanly, and never take unbounded time or panic.
            let globs: Vec<&str> = tunables
                .deny_globs
                .iter()
                .map(domain::ShortText::as_str)
                .collect();
            let _ = WalkConfig::new(&agent::tree::walk::BUILT_IN_IGNORE, &globs);
            Ok(())
        }
        Decoded::Message(ToAgent::Command(command)) => {
            let world = World::new();
            let before = world.guarded();
            let dispatcher = dispatcher(&world);
            let wants_reply = !matches!(
                command,
                HubCommand::RequestDelta { .. } | HubCommand::RequestFullScan
            );
            let reply = runtime().block_on(dispatcher.handle(command, LIMITS));
            ensure(reply.is_some() == wants_reply, || {
                "a command got the wrong number of answers".to_owned()
            })?;
            if let Some(reply) = reply {
                let encoded = wire::encode(reply);
                ensure(encoded.encoded_len() <= proto::limits::MAX_MESSAGE_BYTES, || {
                    "an answer is bigger than a message may be".to_owned()
                })?;
            }
            world_unharmed(&world, &before, "a command from the hub")
        }
        Decoded::Message(ToAgent::Ack(_) | ToAgent::CertRenewal(_)) => Ok(()),
    }
}

// ------------------------------------------------------------------------------------------------ agent_cert_chain

/// DER blobs, each with a two-byte big-endian length in front, at most eight of them.
fn framed_blobs(mut data: &[u8]) -> Vec<Bytes> {
    let mut blobs = Vec::new();
    while data.len() >= 2 && blobs.len() < 8 {
        let len = usize::from(u16::from_be_bytes([data[0], data[1]]));
        let take = len.min(data.len() - 2);
        blobs.push(Bytes::copy_from_slice(&data[2..2 + take]));
        data = &data[2 + take..];
    }
    blobs
}

/// A chain made of arbitrary bytes is never the agent's certificate, whatever it parses as.
pub fn agent_cert_chain(data: &[u8]) -> Result<(), String> {
    let key = KeyMaterial::generate().map_err(|_| "no key".to_owned())?;
    let chain = framed_blobs(data);
    let swimlane = SwimlaneId::parse("sit1").unwrap();
    let verified = ClientIdentity::verify(
        key,
        chain,
        &swimlane,
        Timestamp::from_unix_millis(1_800_000_000_000),
    );
    ensure(verified.is_err(), || {
        "random bytes were accepted as the agent's certificate".to_owned()
    })
}

// ------------------------------------------------------------------------------------------------ agent_pem

/// The first byte picks what is parsed:
///
/// - `0`: the rest is the PEM text of the private key (`tls.key` in the certificate Secret);
/// - `1`: the rest is the hub CA file;
/// - `2`: the rest says how to damage a *freshly generated* valid key's PEM, as (position, xor) pairs, so the accepting
///   path of the key parser is fuzzed without any key in the repository. `0xff 0xfe` first means: cut it in half.
pub fn agent_pem(data: &[u8]) -> Result<(), String> {
    let Some((which, text)) = data.split_first() else {
        return Ok(());
    };
    match which % 3 {
        0 => {
            if let Ok(key) = KeyMaterial::from_pem(text) {
                pem_round_trips(&key)?;
            }
        }
        1 => {
            if HubRoots::from_pem(text).is_ok() {
                ensure(text.windows(11).any(|w| w == b"CERTIFICATE"), || {
                    "a CA file without a certificate block was accepted".to_owned()
                })?;
            }
        }
        _ => {
            let original = KeyMaterial::generate().map_err(|_| "no key".to_owned())?;
            let mut pem = original.to_pem().expose().clone().into_bytes();
            let mut damaged = false;
            if text.starts_with(&[0xff, 0xfe]) {
                pem.truncate(pem.len() / 2);
                damaged = true;
            } else {
                for pair in text.chunks_exact(2) {
                    let at = usize::from(pair[0]) * 3 % pem.len();
                    pem[at] ^= pair[1];
                    damaged |= pair[1] != 0;
                }
            }
            match KeyMaterial::from_pem(&pem) {
                Ok(key) => {
                    pem_round_trips(&key)?;
                    // Only damage that left no trace may leave the key the same; a different key is fine too (the
                    // base64 of the secret scalar can change and still be a valid key).
                    if !damaged {
                        ensure(key.spki_der() == original.spki_der(), || {
                            "an undamaged key PEM gave another key".to_owned()
                        })?;
                    }
                }
                Err(_) => ensure(damaged, || "an undamaged key PEM was refused".to_owned())?,
            }
        }
    }
    Ok(())
}

/// A key that was accepted survives its own PEM.
fn pem_round_trips(key: &KeyMaterial) -> Result<(), String> {
    let again = KeyMaterial::from_pem(key.to_pem().expose().as_bytes())
        .map_err(|_| "a key that was accepted did not survive its own PEM".to_owned())?;
    ensure(again.spki_der() == key.spki_der(), || {
        "the key changed in a PEM round trip".to_owned()
    })
}

// ------------------------------------------------------------------------------------------------ agent_id_token

/// A string accepted as a Google ID token is three non-empty base64url segments and within the size limit.
pub fn agent_id_token(data: &[u8]) -> Result<(), String> {
    let Ok(text) = std::str::from_utf8(data) else {
        return Ok(());
    };
    if !looks_like_a_jwt(text) {
        return Ok(());
    }
    ensure(text.len() <= proto::limits::MAX_TOKEN_BYTES, || {
        "an over-long token was accepted".to_owned()
    })?;
    let segments: Vec<&str> = text.split('.').collect();
    ensure(
        segments.len() == 3 && segments.iter().all(|s| !s.is_empty()),
        || format!("{} segments were accepted", segments.len()),
    )?;
    ensure(
        text.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
        || "a token with a character outside base64url was accepted".to_owned(),
    )
}
