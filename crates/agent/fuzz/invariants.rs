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
//! | `agent_path` | one byte (the operation), then a path | whatever the path is, an operation on a root with planted symlinks changes nothing outside the root, leaves every symlink as it was, leaves no temporary file, and a success never went through a symlink; a path the deny list denies is never read, written, created or deleted, and its file is never changed (D79) |
//! | `agent_hub_message` | a protobuf `HubMessage` | decoding never panics; a configuration is clamped into its ranges and its deny globs are applied or dropped cleanly, whatever they say, and never remove a built-in glob; a file command gets exactly one answer, of bounded size, and touches nothing outside the root |
//! | `agent_cert_chain` | length-prefixed DER blobs | random bytes are never accepted as the agent's certificate |
//! | `agent_pem` | one byte (key, CA, or damage to a generated key), then PEM text | a key that is accepted survives a PEM round trip; an undamaged generated key is always accepted; neither parser panics |
//! | `agent_id_token` | text | a string accepted as a token is three non-empty base64url segments within the size limit |
//! | `agent_spool_record` | one byte (raw segment bytes, or message bodies the target wraps in valid frames), then the data | opening a spool over any bytes never panics and never reads or allocates beyond one record; whatever it replays is a delta the hub's own conversion accepts, in strictly rising order, within the bounds; damage is cut off so that opening again finds the same spool |

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

use agent::clock::Clock;
use agent::config::Tunables;
use agent::deny::{DEFAULT_DENY_GLOBS, DenyList};
use agent::dispatch::{CommandHandler, Dispatcher, OpLimits};
use agent::fileops::{FileOps, TreeEdits};
use agent::identity::idtoken::looks_like_a_jwt;
use agent::identity::{ClientIdentity, KeyMaterial};
use agent::root::NfsRoot;
use agent::spool::record::{FRAME_OVERHEAD, Frame, Meta, SEGMENT_MAGIC};
use agent::spool::{IoMode, Spool, SpoolLimits, SpoolOptions, SpoolVolume};
use agent::transport::outbox::{self, OutboxLimits};
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
pub const TARGETS: [(&str, Check); 6] = [
    ("agent_path", agent_path),
    ("agent_hub_message", agent_hub_message),
    ("agent_cert_chain", agent_cert_chain),
    ("agent_pem", agent_pem),
    ("agent_id_token", agent_id_token),
    ("agent_spool_record", agent_spool_record),
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
///     svc/server.pem        "pem-body"         (a denied file: nothing may read, change or delete it)
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
        fs::write(root.join("svc/server.pem"), b"pem-body").unwrap();
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

    /// Every denied file in the root, with its content: none may appear, vanish or change (D79).
    fn denied_files(&self) -> BTreeMap<String, String> {
        let mut seen = BTreeMap::new();
        find_denied(&self.root, &self.root, &DenyList::default(), &mut seen);
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

fn find_denied(root: &Path, dir: &Path, deny: &DenyList, seen: &mut BTreeMap<String, String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let md = fs::symlink_metadata(&path).unwrap();
        if md.is_dir() {
            find_denied(root, &path, deny, seen);
        } else if md.is_file() {
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if deny.is_denied(&rel) {
                seen.insert(
                    rel,
                    String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned(),
                );
            }
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
    let denied = world.denied_files();
    ensure(
        denied.len() == 1 && denied.get("svc/server.pem").map(String::as_str) == Some("pem-body"),
        || format!("{what} changed a denied file or made one: {denied:?}"),
    )?;
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
    let harms: [(&str, Harm); 7] = [
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
        ("a denied file rewritten", |w| {
            fs::write(w.root.join("svc/server.pem"), b"planted").unwrap();
        }),
        ("a denied file created", |w| {
            fs::write(w.root.join("svc/created.KEY"), b"planted").unwrap();
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
    if DenyList::default().is_denied(path.as_str()) {
        // D79: nothing succeeds on a denied path, and the refusal is DENIED, whatever else is wrong with the request.
        ensure(!succeeded, || {
            format!("an operation succeeded on the denied path {:?}", path.as_str())
        })?;
        let code = match &reply {
            domain::AgentReply::Op(result) => result.error,
            _ => None,
        };
        ensure(code == Some(domain::OpError::Denied), || {
            format!(
                "a denied path {:?} was refused with {code:?}, not DENIED",
                path.as_str()
            )
        })?;
        if let domain::AgentReply::Op(result) = &reply {
            ensure(result.current_hash.is_none(), || {
                "a refusal of a denied path carried a hash".to_owned()
            })?;
        }
    }
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
            // Applying the hub's globs must work or drop what does not, and never take unbounded time or panic.
            let globs: Vec<&str> = tunables
                .deny_globs
                .iter()
                .map(domain::ShortText::as_str)
                .collect();
            let _ = WalkConfig::new(&agent::tree::walk::BUILT_IN_IGNORE, &globs);
            let deny = DenyList::new(&globs);
            ensure(deny.rejected() <= globs.len(), || {
                "more globs rejected than were sent".to_owned()
            })?;
            // Whatever the hub sent, it cannot have removed a built-in glob (D79): a name for each is still denied.
            for sample in [
                "a.jks",
                "a.p12",
                "a.pfx",
                "a.pem",
                "a.key",
                "a.keystore",
                "x/private.yml",
                "X/A.PEM",
            ] {
                ensure(deny.is_denied(sample), || {
                    format!("the hub's globs {globs:?} un-denied {sample}")
                })?;
            }
            deny.set_hub_globs(&[]);
            for glob in DEFAULT_DENY_GLOBS {
                let sample = glob.replace('*', "name");
                ensure(deny.is_denied(&sample), || {
                    format!("taking the hub's globs back un-denied {sample}")
                })?;
            }
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

// ------------------------------------------------------------------------------------------------ agent_spool_record

/// The most bytes of a segment the spool target looks at.
const MAX_SPOOL_INPUT: usize = 1 << 20;
/// The most message bodies the second input mode wraps.
const MAX_BODIES: usize = 32;
const SPOOL_SEGMENT: &str = "seg-0000000000000001.lks";

#[derive(Debug)]
struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(1_800_000_000_000)
    }

    fn instant(&self) -> tokio::time::Instant {
        tokio::time::Instant::now()
    }
}

/// What the wire conversion says about one message the spool replayed.
type Replay = Result<Option<FromAgent>, proto::convert::ConvertError>;

/// What opening the spool over some bytes and replaying it showed.
struct Replayed {
    seqs: Vec<u64>,
    damaged_segments: usize,
}

/// A directory for one input: on a RAM disk when there is one, because opening a spool syncs its state file.
fn scratch_dir() -> tempfile::TempDir {
    let ram = Path::new("/dev/shm");
    let built = if ram.is_dir() {
        tempfile::Builder::new().prefix("lk-spool-fuzz-").tempdir_in(ram)
    } else {
        tempfile::Builder::new().prefix("lk-spool-fuzz-").tempdir()
    };
    built.unwrap()
}

/// The frame a delta is stored as: the same call the spool makes.
fn frame_of(delta: &domain::ScanDelta) -> Vec<u8> {
    let (mut first, mut last) = (i64::MAX, i64::MIN);
    for e in &delta.entries {
        first = first.min(e.observed_at.unix_millis());
        last = last.max(e.observed_at.unix_millis());
    }
    if first > last {
        (first, last) = (0, 0);
    }
    let meta = Meta {
        seq: delta.seq,
        part: delta.part,
        more: delta.more,
        entries: u32::try_from(delta.entries.len() + delta.removed.len() + delta.skipped.len())
            .unwrap_or(u32::MAX),
        first_ms: first,
        last_ms: last,
    };
    let mut buf = Frame::begin(0);
    agent::transport::wire::encode_delta_into(delta.clone(), &mut buf);
    Frame::seal(buf, &meta).unwrap().as_bytes().to_vec()
}

/// A segment made of `bodies`, each wrapped in a frame with a valid checksum, so that what the fuzzer mutates reaches
/// the decoder instead of stopping at the first checksum. A body that is a delta gets the metadata its delta implies;
/// one that is not gets metadata that does not match it, which the spool must notice when it reads it back.
fn framed_bodies(data: &[u8]) -> Vec<u8> {
    let mut segment = SEGMENT_MAGIC.to_vec();
    let mut rest = data;
    let mut last_seq = 0;
    for index in 0..MAX_BODIES {
        let Some((len, tail)) = rest.split_first_chunk::<2>() else {
            break;
        };
        let len = usize::from(u16::from_le_bytes(*len)).min(tail.len());
        let (body, tail) = tail.split_at(len);
        rest = tail;
        let decoded = agent::transport::wire::decode_delta(Bytes::copy_from_slice(body));
        let meta = match &decoded {
            Ok(d) if d.seq > last_seq => {
                last_seq = d.seq;
                Meta {
                    seq: d.seq,
                    part: d.part,
                    more: d.more,
                    entries: u32::try_from(d.entries.len() + d.removed.len() + d.skipped.len())
                        .unwrap_or(u32::MAX),
                    first_ms: 0,
                    last_ms: 0,
                }
            }
            _ => {
                last_seq += 1;
                Meta {
                    seq: last_seq + index as u64,
                    part: 0,
                    more: false,
                    entries: 1,
                    first_ms: 0,
                    last_ms: 0,
                }
            }
        };
        let mut buf = Frame::begin(body.len());
        buf.extend_from_slice(body);
        if let Ok(frame) = Frame::seal(buf, &meta) {
            segment.extend_from_slice(frame.as_bytes());
        }
    }
    segment
}

fn open_and_replay(dir: &Path, file_len: usize) -> Result<Replayed, String> {
    let volume = SpoolVolume::open(dir).map_err(|e| format!("the volume would not open: {e}"))?;
    let limits = SpoolLimits::new(64 << 20, 10_000).with_segment_bytes(1 << 20);
    let options = SpoolOptions::new(limits).with_io(IoMode::Inline);
    let (spool, recovery) = Spool::open(volume, options, Arc::new(FixedClock))
        .map_err(|e| format!("opening the spool failed: {e}"))?;
    ensure(recovery.records <= file_len / FRAME_OVERHEAD + 1, || {
        format!("{} records from {file_len} bytes", recovery.records)
    })?;
    let stats = spool.stats();
    ensure(stats.entries <= 10_000, || {
        format!("{} entries held, bound 10,000", stats.entries)
    })?;
    ensure(stats.disk_bytes <= file_len as u64, || {
        format!("{} bytes counted, the file has {file_len}", stats.disk_bytes)
    })?;

    // The runtime of this call alone: a shared one would let another thread's `block_on` run this pump's task, and the
    // stable replay runs the targets in parallel.
    let own = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let messages = own.block_on(async {
        let (outbox, mut rx) = outbox::channel(OutboxLimits {
            messages: 1024,
            bytes: 64 << 20,
        });
        let pump = tokio::spawn(spool.attach(outbox).run());
        // Inline file operations: the pump sends everything it can before it first has to wait.
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        let mut got = Vec::new();
        while let Some(queued) = rx.try_recv() {
            let (message, _permit) = queued.into_parts();
            got.push(FromAgent::from_proto(message));
        }
        pump.abort();
        got
    });
    let seqs = check_replayed(messages)?;
    Ok(Replayed {
        seqs,
        damaged_segments: recovery.damaged_segments,
    })
}

/// What a replay must look like: deltas only, strictly rising, each one the hub's own conversion takes unchanged, a denied
/// entry without bytes, and a gap only on a first part and never running backwards. Returns the sequence numbers.
fn check_replayed(
    messages: Vec<Result<Option<FromAgent>, proto::convert::ConvertError>>,
) -> Result<Vec<u64>, String> {
    let mut seqs = Vec::new();
    let mut previous = 0;
    for message in messages {
        let Ok(Some(FromAgent::Delta(d))) = message else {
            return Err("the spool replayed something that is not a delta".to_owned());
        };
        ensure(d.seq > previous, || format!("seq {} after {previous}", d.seq))?;
        previous = d.seq;
        // The hub's own conversion accepts exactly this.
        let again = FromAgent::from_proto(FromAgent::Delta(d.clone()).into_proto());
        ensure(matches!(&again, Ok(Some(FromAgent::Delta(x))) if *x == d), || {
            format!("seq {} does not survive the wire conversion", d.seq)
        })?;
        ensure(d.entries.iter().all(|e| !e.denied || e.bytes.is_none()), || {
            "a denied entry came back with bytes".to_owned()
        })?;
        if let Some(gap) = d.gap {
            ensure(gap.from <= gap.to && d.part == 0, || {
                format!("gap {gap:?} on part {}", d.part)
            })?;
        }
        seqs.push(d.seq);
    }
    Ok(seqs)
}

/// The first byte picks how the rest is read: odd, message bodies that are wrapped in valid frames; even, the bytes of a
/// segment file. Whatever the file holds, opening the spool over it is safe, and it opens the same way again (S22).
pub fn agent_spool_record(data: &[u8]) -> Result<(), String> {
    let Some((&mode, rest)) = data.split_first() else {
        return Ok(());
    };
    let rest = &rest[..rest.len().min(MAX_SPOOL_INPUT)];
    let file = if mode % 2 == 0 {
        rest.to_vec()
    } else {
        framed_bodies(rest)
    };
    let dir = scratch_dir();
    fs::write(dir.path().join(SPOOL_SEGMENT), &file).unwrap();
    let first = open_and_replay(dir.path(), file.len())?;
    // Damage was cut off and what was lost was written down, so the next start finds a spool it can read whole.
    let second = open_and_replay(dir.path(), file.len())?;
    ensure(second.seqs == first.seqs, || {
        format!(
            "a second open replays {:?}, the first {:?}",
            second.seqs, first.seqs
        )
    })?;
    ensure(second.damaged_segments == 0, || {
        format!(
            "{} segments are still damaged after being repaired",
            second.damaged_segments
        )
    })
}

/// The seeds for `agent_spool_record`: (file name, bytes). `corpus_replay.rs` checks the files in `corpus/` are exactly
/// these, and can rewrite them (`UPDATE_SPOOL_SEEDS=1`), so the seeds stay honest when the record format changes.
pub fn spool_seeds() -> Vec<(&'static str, Vec<u8>)> {
    use domain::{ScanDelta, ScanEntry, SkippedEntry};

    let entry = |path: &str, content: &[u8], at: i64, denied: bool| ScanEntry {
        path: NfsPath::parse(path).unwrap(),
        hash: sha(content),
        size: content.len() as u64,
        mtime: Timestamp::from_unix_millis(at),
        observed_at: Timestamp::from_unix_millis(at),
        denied,
        bytes: (!denied).then(|| Bytes::copy_from_slice(content)),
    };
    let delta = |seq: u64, part: u32, more: bool, entries: Vec<ScanEntry>| ScanDelta {
        seq,
        base_root: Some(ContentHash::from_bytes([seq as u8; 32])),
        new_root: ContentHash::from_bytes([seq as u8 + 1; 32]),
        entries,
        removed: Vec::new(),
        skipped: Vec::new(),
        during_job: None,
        more,
        part,
        gap: None,
    };
    let segment = |frames: &[Vec<u8>]| {
        let mut bytes = SEGMENT_MAGIC.to_vec();
        for f in frames {
            bytes.extend_from_slice(f);
        }
        bytes
    };
    let raw = |bytes: Vec<u8>| [vec![0_u8], bytes].concat();
    let single = frame_of(&delta(
        1,
        0,
        false,
        vec![entry("svc/a.yml", b"a: 1\r\n", 1_000, false)],
    ));
    let second = frame_of(&delta(
        2,
        0,
        false,
        vec![entry("svc/a.yml", b"a: 2\r\n", 2_000, false)],
    ));
    let parts = [
        frame_of(&delta(3, 0, true, vec![entry("svc/b.yml", b"b", 3_000, false)])),
        frame_of(&delta(4, 1, true, vec![entry("svc/c.yml", b"c", 3_000, false)])),
        frame_of(&delta(5, 2, false, vec![entry("svc/d.yml", b"d", 3_000, false)])),
    ];
    let mut removal = delta(6, 0, false, Vec::new());
    removal.removed.push(NfsPath::parse("svc/gone.yml").unwrap());
    removal.skipped.push(SkippedEntry {
        path: NfsPath::parse("svc/big.bin").unwrap(),
        reason: domain::ShortText::parse("too_large").unwrap(),
    });
    let denied = frame_of(&delta(
        7,
        0,
        false,
        vec![entry("keys/site.pem", &[7; 300], 4_000, true)],
    ));
    let removal = frame_of(&removal);

    let mut flipped = segment(&[single.clone(), second.clone()]);
    let at = flipped.len() - 4;
    flipped[at] ^= 0xFF;
    let mut huge = segment(&[single.clone()]);
    huge.extend_from_slice(&[0xFF; 8]);
    huge.extend_from_slice(&[0_u8; 64]);
    let mut wrong_magic = segment(&[single.clone()]);
    wrong_magic[7] = b'9';
    let mut backwards = segment(&[second.clone(), single.clone()]);
    backwards.extend_from_slice(&removal);
    let whole = segment(&[
        single.clone(),
        second.clone(),
        parts[0].clone(),
        parts[1].clone(),
        parts[2].clone(),
        removal.clone(),
        denied.clone(),
    ]);
    let mut cut = whole.clone();
    cut.truncate(cut.len() - 20);

    let body_of = |d: &ScanDelta| {
        let mut buf = Vec::new();
        agent::transport::wire::encode_delta_into(d.clone(), &mut buf);
        buf
    };
    let chunk = |body: &[u8]| [(body.len() as u16).to_le_bytes().to_vec(), body.to_vec()].concat();
    let good_body = body_of(&delta(10, 0, false, vec![entry("x.yml", b"x", 5_000, false)]));
    let mut bodies = chunk(&good_body);
    bodies.extend(chunk(&body_of(&delta(
        11,
        0,
        false,
        vec![entry("y.yml", b"y", 6_000, false)],
    ))));
    bodies.extend(chunk(b"\xff\xff\xff\xffnot a message"));

    vec![
        ("valid_single_delta", raw(segment(&[single.clone()]))),
        (
            "valid_delta_and_its_successor",
            raw(segment(&[single.clone(), second.clone()])),
        ),
        ("valid_three_part_delta", raw(segment(&parts))),
        ("valid_mixed", raw(whole.clone())),
        ("denied_entry_without_bytes", raw(segment(&[denied]))),
        ("removal_and_skip", raw(segment(&[removal]))),
        ("header_only", raw(SEGMENT_MAGIC.to_vec())),
        ("empty_file", vec![0_u8]),
        ("truncated_inside_a_frame", raw(cut)),
        ("flipped_checksummed_byte", raw(flipped)),
        ("length_field_of_four_gigabytes", raw(huge)),
        ("wrong_magic", raw(wrong_magic)),
        ("seq_goes_backwards", raw(backwards)),
        (
            "only_a_middle_part",
            raw(segment(&[parts[1].clone(), parts[2].clone()])),
        ),
        (
            "garbage",
            raw((0..200_u32).map(|i| (i * 37 + 11) as u8).collect()),
        ),
        ("bodies_valid_then_invalid", [vec![1_u8], bodies].concat()),
        ("bodies_empty", vec![1_u8]),
    ]
}

/// The spool target is not blind: each violation of what a replay must look like is reported when planted, and a clean
/// replay is not. Run by `corpus_replay.rs`.
pub fn planted_spool_harm_is_detected() -> Result<(), String> {
    use domain::{ScanDelta, ScanEntry, SpoolGap};

    let plain = |seq: u64, part: u32| ScanDelta {
        seq,
        base_root: None,
        new_root: ContentHash::from_bytes([1; 32]),
        entries: Vec::new(),
        removed: Vec::new(),
        skipped: Vec::new(),
        during_job: None,
        more: false,
        part,
        gap: None,
    };
    let message = |d: ScanDelta| Ok(Some(FromAgent::Delta(d)));
    ensure(
        check_replayed(vec![message(plain(1, 0)), message(plain(2, 0))]) == Ok(vec![1, 2]),
        || "a clean replay was refused".to_owned(),
    )?;
    let mut denied_with_bytes = plain(3, 0);
    denied_with_bytes.entries.push(ScanEntry {
        path: NfsPath::parse("keys/a.pem").unwrap(),
        hash: sha(b"k"),
        size: 1,
        mtime: Timestamp::from_unix_millis(1),
        observed_at: Timestamp::from_unix_millis(1),
        denied: true,
        bytes: Some(Bytes::from_static(b"k")),
    });
    let mut backwards_gap = plain(4, 0);
    backwards_gap.gap = Some(SpoolGap {
        from: Timestamp::from_unix_millis(5),
        to: Timestamp::from_unix_millis(1),
        lost_entries: 1,
    });
    let mut gap_on_a_later_part = plain(5, 1);
    gap_on_a_later_part.gap = Some(SpoolGap {
        from: Timestamp::from_unix_millis(1),
        to: Timestamp::from_unix_millis(2),
        lost_entries: 1,
    });
    let harms: [(&str, Vec<Replay>); 6] = [
        (
            "a seq that goes backwards",
            vec![message(plain(2, 0)), message(plain(1, 0))],
        ),
        (
            "the same seq twice",
            vec![message(plain(2, 0)), message(plain(2, 0))],
        ),
        (
            "a message that is not a delta",
            vec![Ok(Some(FromAgent::CertRenewal {
                csr_der: Bytes::from_static(b"x"),
            }))],
        ),
        ("a denied entry with bytes", vec![message(denied_with_bytes)]),
        ("a gap that runs backwards", vec![message(backwards_gap)]),
        ("a gap on a later part", vec![message(gap_on_a_later_part)]),
    ];
    for (name, replay) in harms {
        ensure(check_replayed(replay).is_err(), || {
            format!("{name} went unnoticed")
        })?;
    }
    Ok(())
}
