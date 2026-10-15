use async_trait::async_trait;

use super::sample::{hash, nfs, request_id, swimlane, user_id};
use crate::{ToolDeleteEntry, ToolWriteEntry, ToolWriteError, ToolWrites, Tx, TxFactory};

/// What the tool-write conformance suites need to see.
#[async_trait]
pub trait ToolWritesProbe: ToolWrites {
    /// The committed registrations, oldest first. A bounded probe may have dropped the oldest.
    async fn registered(&self) -> Vec<ToolWriteEntry>;

    /// The committed delete registrations ([`ToolWrites::register_delete`]), oldest first (a bounded probe may have dropped
    /// the oldest), or `None` when this implementation does not register deletes.
    ///
    /// - `None`, the default, says that the implementation keeps the default `register_delete`, which registers nothing.
    ///   [`tool_writes`] then only checks that the default is harmless: it says `Ok`, and a delete never shows up as a hash
    ///   registration.
    /// - `Some(entries)` says that it registers deletes. [`tool_writes`] then holds it to every delete rule: atomic with
    ///   the transaction, a rollback leaves nothing, a repeat is a no-op, a delete registration is distinct from a hash
    ///   registration of the same path, a transaction of another store is refused. An implementation that opts in and
    ///   registers nothing fails, which is what a wrapper that forgot to forward `register_delete` looks like.
    ///
    /// **An implementation that overrides `register_delete` MUST make its probe return `Some(..)`, and run
    /// [`tool_writes_with_deletes`] (and [`tool_writes_bounded`]).** This is a convention and not something the types
    /// enforce: a probe left at the default `None` passes [`tool_writes`] with every delete rule unchecked, and only
    /// [`tool_writes_with_deletes`] fails for it. Do the same for the probe of a wrapper over such an implementation.
    async fn registered_deletes(&self) -> Option<Vec<ToolDeleteEntry>> {
        None
    }
}

fn entry(path: &str, content: &[u8], request: &str) -> ToolWriteEntry {
    ToolWriteEntry {
        swimlane: swimlane("sit1"),
        path: nfs(path),
        new_hash: hash(content),
        user: user_id(1),
        request_id: request_id(request),
    }
}

/// An entry for another swimlane and another user.
fn entry_of(sw: &str, user: u32, path: &str, content: &[u8], request: &str) -> ToolWriteEntry {
    ToolWriteEntry {
        swimlane: swimlane(sw),
        user: user_id(user),
        ..entry(path, content, request)
    }
}

fn delete_entry(path: &str, content: &[u8], request: &str) -> ToolDeleteEntry {
    ToolDeleteEntry {
        swimlane: swimlane("sit1"),
        path: nfs(path),
        deleted_hash: hash(content),
        user: user_id(1),
        request_id: request_id(request),
    }
}

/// A delete entry for another swimlane and another user.
fn delete_entry_of(sw: &str, user: u32, path: &str, content: &[u8], request: &str) -> ToolDeleteEntry {
    ToolDeleteEntry {
        swimlane: swimlane(sw),
        user: user_id(user),
        ..delete_entry(path, content, request)
    }
}

async fn register<T: ToolWritesProbe + ?Sized>(
    t: &T,
    tx: &mut Tx<'_>,
    e: &ToolWriteEntry,
) -> Result<(), ToolWriteError> {
    t.register(tx, &e.swimlane, &e.path, &e.new_hash, &e.user, &e.request_id)
        .await
}

async fn register_delete<T: ToolWritesProbe + ?Sized>(
    t: &T,
    tx: &mut Tx<'_>,
    e: &ToolDeleteEntry,
) -> Result<(), ToolWriteError> {
    t.register_delete(tx, &e.swimlane, &e.path, &e.deleted_hash, &e.user, &e.request_id)
        .await
}

/// The newest registrations are exactly `expected`, oldest of them first. A bounded probe may have dropped the oldest,
/// so this never counts on the history being complete.
fn ends_with<E: PartialEq>(all: &[E], expected: &[E]) -> bool {
    all.ends_with(expected)
}

/// How many times `e` is registered in what the probe shows.
fn times<E: PartialEq>(all: &[E], e: &E) -> usize {
    all.iter().filter(|x| *x == e).count()
}

/// What the probe shows of the delete registrations. Only called for a probe that opted in.
async fn deletes_of<T: ToolWritesProbe + ?Sized>(t: &T) -> Vec<ToolDeleteEntry> {
    t.registered_deletes()
        .await
        .expect("the probe opted in to delete registrations and then stopped showing them")
}

/// - A registration exists only if its transaction commits; a rolled-back one leaves nothing behind.
/// - Registrations keep their order, and carry the swimlane, path, hash, user and request they were made with.
/// - Registering the same (swimlane, path, hash, request) again is a no-op, in the same transaction or a later one.
/// - The same hash at another path, in another swimlane, or from another request, is another registration.
/// - A transaction of another store is refused with `WrongTx`, and registers nothing.
/// - The user is not part of the key: the same key from another user registers nothing and keeps the first user.
///
/// For an implementation whose probe opts in with [`ToolWritesProbe::registered_deletes`], the same rules hold for
/// [`ToolWrites::register_delete`] (whose key is the swimlane, path, deleted hash and request, so a delete of another version
/// of the file is another registration), and a delete registration is distinct from a hash registration of the same path,
/// hash and request: registering one never replaces, merges with or repeats the other, and neither shows up in the other's
/// list.
/// For one that does not opt in, `register_delete` must say `Ok` and register no hash.
///
/// The suite cannot see an expiry, an invalidation or a late commit (the port has no way to observe); those are tested
/// where the registry is implemented. It cannot see the bound either: [`tool_writes_bounded`] does.
pub async fn tool_writes<T: ToolWritesProbe + ?Sized>(t: &T, txf: &dyn TxFactory) {
    let first = entry("svc/app.yml", b"x: 2\n", "req-tw-1");
    // Another swimlane and another user, with the path and hash of `first`: a registration of its own.
    let second = entry_of("sit2", 2, "svc/app.yml", b"x: 2\n", "req-tw-2");
    let gone = entry("svc/gone.yml", b"z: 0\n", "req-tw-3");

    let before = t.registered().await;
    // A transaction of another store is refused, and registers nothing.
    let foreign = entry("svc/foreign.yml", b"f: 1\n", "req-tw-0");
    assert_eq!(
        register(t, &mut Tx::detached(), &foreign).await,
        Err(ToolWriteError::WrongTx),
        "a transaction of another store is refused with WrongTx"
    );
    assert_eq!(
        t.registered().await,
        before,
        "a refused registration leaves nothing"
    );

    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &first).await.unwrap();
    register(t, &mut tx.tx(), &second).await.unwrap();
    assert_eq!(t.registered().await, before, "not visible before commit");
    tx.commit().await.unwrap();

    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &gone).await.unwrap();
    tx.rollback().await.unwrap();

    let after = t.registered().await;
    assert_eq!(times(&after, &gone), 0, "the rollback left nothing behind");
    assert!(
        ends_with(&after, &[first.clone(), second.clone()]),
        "in order, as made; the same path and hash in another swimlane, from another user, is another registration: {after:?}"
    );

    // A repeat is a no-op, in a later transaction and inside one.
    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &first).await.unwrap();
    register(t, &mut tx.tx(), &first).await.unwrap();
    tx.commit().await.unwrap();
    let again = t.registered().await;
    assert_eq!(times(&again, &first), 1, "a repeat registers nothing");
    assert!(ends_with(&again, &[first.clone(), second.clone()]));

    // The key has no user: the same key from another user is a no-op that keeps the first user.
    let other_user = entry_of("sit1", 7, "svc/app.yml", b"x: 2\n", "req-tw-1");
    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &other_user).await.unwrap();
    tx.commit().await.unwrap();
    let kept = t.registered().await;
    let same_key: Vec<&ToolWriteEntry> = kept
        .iter()
        .filter(|e| {
            e.swimlane == first.swimlane
                && e.path == first.path
                && e.new_hash == first.new_hash
                && e.request_id == first.request_id
        })
        .collect();
    assert_eq!(
        same_key.len(),
        1,
        "a repeat of a hash registration from another user registers nothing: {kept:?}"
    );
    assert_eq!(
        same_key[0].user, first.user,
        "a repeat of a hash registration from another user keeps the first user"
    );
    assert!(ends_with(&kept, &[first.clone(), second.clone()]));

    // A registration made twice inside one open transaction is one registration once it commits.
    let twice = entry_of("sit2", 3, "svc/twice.yml", b"t: 1\n", "req-tw-4");
    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &twice).await.unwrap();
    register(t, &mut tx.tx(), &twice).await.unwrap();
    tx.commit().await.unwrap();
    let all = t.registered().await;
    assert_eq!(times(&all, &twice), 1, "twice in one transaction is once");
    assert!(ends_with(&all, &[twice.clone()]));

    // Another path or another request is a new registration.
    let elsewhere = entry("svc/copy.yml", b"x: 2\n", "req-tw-1");
    let retried_elsewhere = entry("svc/app.yml", b"x: 2\n", "req-tw-9");
    let mut tx = txf.begin().await.unwrap();
    register(t, &mut tx.tx(), &elsewhere).await.unwrap();
    register(t, &mut tx.tx(), &retried_elsewhere).await.unwrap();
    tx.commit().await.unwrap();
    let after = t.registered().await;
    assert!(ends_with(&after, &[elsewhere, retried_elsewhere]));

    delete_registrations(t, txf).await;
}

/// [`tool_writes`] for an implementation that registers deletes: it fails for a probe that does not opt in (its
/// [`ToolWritesProbe::registered_deletes`] returns `None`), so that an implementation that kept the do-nothing default of
/// `register_delete` by mistake, or a wrapper that did not forward it, cannot pass. Use this one for every implementation
/// that has taken the method, and [`tool_writes`] for one that has not.
pub async fn tool_writes_with_deletes<T: ToolWritesProbe + ?Sized>(t: &T, txf: &dyn TxFactory) {
    assert!(
        t.registered_deletes().await.is_some(),
        "this implementation registers deletes, so its probe must show them: `registered_deletes` returned None, which \
         says that it keeps the default `register_delete`, and the delete rules would not be checked"
    );
    tool_writes(t, txf).await;
}

/// The delete half of [`tool_writes`], after its hash registrations are in (it relies on `svc/app.yml` of `sit1` having a
/// hash registration from request `req-tw-1`).
async fn delete_registrations<T: ToolWritesProbe + ?Sized>(t: &T, txf: &dyn TxFactory) {
    let hashes_before = t.registered().await;
    let Some(deletes_before) = t.registered_deletes().await else {
        // No delete registrations: the default is harmless. It says `Ok`, and registers no hash.
        let mut tx = txf.begin().await.unwrap();
        register_delete(
            t,
            &mut tx.tx(),
            &delete_entry("svc/app.yml", b"x: 2\n", "req-tw-1"),
        )
        .await
        .expect("the default register_delete says Ok");
        tx.commit().await.unwrap();
        assert_eq!(
            t.registered().await,
            hashes_before,
            "a delete registration is not a hash registration"
        );
        return;
    };

    // The same swimlane, path and request as the hash registration `first` of `tool_writes`.
    let first = delete_entry("svc/app.yml", b"x: 2\n", "req-tw-1");
    // Another swimlane and another user, with the path of `first`: a registration of its own.
    let second = delete_entry_of("sit2", 2, "svc/app.yml", b"x: 2\n", "req-tw-2");
    let gone = delete_entry("svc/gone.yml", b"z: 0\n", "req-tw-3");

    // A transaction of another store is refused, and registers nothing.
    assert_eq!(
        register_delete(t, &mut Tx::detached(), &first).await,
        Err(ToolWriteError::WrongTx),
        "a transaction of another store is refused with WrongTx by register_delete too"
    );
    assert_eq!(
        deletes_of(t).await,
        deletes_before,
        "a refused delete registration leaves nothing"
    );

    let mut tx = txf.begin().await.unwrap();
    register_delete(t, &mut tx.tx(), &first).await.unwrap();
    register_delete(t, &mut tx.tx(), &second).await.unwrap();
    assert_eq!(
        deletes_of(t).await,
        deletes_before,
        "a delete registration is not visible before commit"
    );
    tx.commit().await.unwrap();

    let mut tx = txf.begin().await.unwrap();
    register_delete(t, &mut tx.tx(), &gone).await.unwrap();
    tx.rollback().await.unwrap();

    let after = deletes_of(t).await;
    assert_eq!(
        times(&after, &gone),
        0,
        "the rollback left no delete registration behind"
    );
    assert!(
        ends_with(&after, &[first.clone(), second.clone()]),
        "delete registrations are visible after the commit, in order, as made; the same path in another swimlane, from another user, is another one: {after:?}"
    );

    // A repeat is a no-op, in a later transaction and inside one.
    let mut tx = txf.begin().await.unwrap();
    register_delete(t, &mut tx.tx(), &first).await.unwrap();
    register_delete(t, &mut tx.tx(), &first).await.unwrap();
    tx.commit().await.unwrap();
    let again = deletes_of(t).await;
    assert_eq!(
        times(&again, &first),
        1,
        "a repeated delete registration registers nothing"
    );
    assert!(ends_with(&again, &[first.clone(), second.clone()]));

    // The key has no user: the same key from another user is a no-op that keeps the first user.
    let other_user = delete_entry_of("sit1", 7, "svc/app.yml", b"x: 2\n", "req-tw-1");
    let mut tx = txf.begin().await.unwrap();
    register_delete(t, &mut tx.tx(), &other_user).await.unwrap();
    tx.commit().await.unwrap();
    let kept = deletes_of(t).await;
    let same_key: Vec<&ToolDeleteEntry> = kept
        .iter()
        .filter(|e| e.swimlane == first.swimlane && e.path == first.path && e.request_id == first.request_id)
        .collect();
    assert_eq!(
        same_key.len(),
        1,
        "a repeat of a delete registration from another user registers nothing: {kept:?}"
    );
    assert_eq!(
        same_key[0].user, first.user,
        "a repeat of a delete registration from another user keeps the first user"
    );
    assert!(ends_with(&kept, &[first.clone(), second.clone()]));

    let twice = delete_entry_of("sit2", 3, "svc/twice.yml", b"t: 1\n", "req-tw-4");
    let mut tx = txf.begin().await.unwrap();
    register_delete(t, &mut tx.tx(), &twice).await.unwrap();
    register_delete(t, &mut tx.tx(), &twice).await.unwrap();
    tx.commit().await.unwrap();
    let all = deletes_of(t).await;
    assert_eq!(
        times(&all, &twice),
        1,
        "a delete registration made twice in one transaction is one"
    );
    assert!(ends_with(&all, &[twice.clone()]));

    // Another path, another request, or another version of the file (another deleted hash) is a new registration.
    let elsewhere = delete_entry("svc/copy.yml", b"x: 2\n", "req-tw-1");
    let retried_elsewhere = delete_entry("svc/app.yml", b"x: 2\n", "req-tw-9");
    let other_version = delete_entry("svc/app.yml", b"y: 3\n", "req-tw-1");
    let mut tx = txf.begin().await.unwrap();
    register_delete(t, &mut tx.tx(), &elsewhere).await.unwrap();
    register_delete(t, &mut tx.tx(), &retried_elsewhere)
        .await
        .unwrap();
    register_delete(t, &mut tx.tx(), &other_version).await.unwrap();
    tx.commit().await.unwrap();
    let all = deletes_of(t).await;
    assert!(
        ends_with(&all, &[elsewhere, retried_elsewhere, other_version.clone()]),
        "a delete of another version of the file is another registration, with the hash it was made with: {all:?}"
    );
    assert_eq!(
        times(&all, &first),
        1,
        "a delete of another version of the file did not replace the delete of this one"
    );

    // Registering deletes added no write registration: a delete registration is not a write registration.
    assert_eq!(
        t.registered().await,
        hashes_before,
        "a delete registration is not a hash registration"
    );
    // And it did not replace the hash registration of the same swimlane, path and request, nor was it replaced by it.
    let hash_first = entry("svc/app.yml", b"x: 2\n", "req-tw-1");
    assert_eq!(
        times(&t.registered().await, &hash_first),
        1,
        "a delete registration is distinct from a hash registration of the same path and request"
    );
    assert_eq!(times(&deletes_of(t).await, &first), 1);

    // Both kinds for one path and request, made in the same transaction, in each order, and then made again: each is
    // registered once, and neither replaced, merged with or repeated the other.
    let (delete_a, hash_a) = (
        delete_entry("svc/pair-a.yml", b"a: 1\n", "req-tw-5"),
        entry("svc/pair-a.yml", b"a: 1\n", "req-tw-5"),
    );
    let (delete_b, hash_b) = (
        delete_entry("svc/pair-b.yml", b"b: 1\n", "req-tw-6"),
        entry("svc/pair-b.yml", b"b: 1\n", "req-tw-6"),
    );
    for round in 0..2 {
        let mut tx = txf.begin().await.unwrap();
        register_delete(t, &mut tx.tx(), &delete_a).await.unwrap();
        register(t, &mut tx.tx(), &hash_a).await.unwrap();
        register(t, &mut tx.tx(), &hash_b).await.unwrap();
        register_delete(t, &mut tx.tx(), &delete_b).await.unwrap();
        tx.commit().await.unwrap();
        let (hashes, deletes) = (t.registered().await, deletes_of(t).await);
        for (e, what) in [(&hash_a, "hash a"), (&hash_b, "hash b")] {
            assert_eq!(
                times(&hashes, e),
                1,
                "round {round}: {what} is registered once, whatever delete registration of its path and request there is"
            );
        }
        for (e, what) in [(&delete_a, "delete a"), (&delete_b, "delete b")] {
            assert_eq!(
                times(&deletes, e),
                1,
                "round {round}: {what} is registered once, whatever hash registration of its path and request there is"
            );
        }
        assert!(
            ends_with(&hashes, &[hash_a.clone(), hash_b.clone()]),
            "round {round}: no delete registration showed up among the hash registrations: {hashes:?}"
        );
        assert!(
            ends_with(&deletes, &[delete_a.clone(), delete_b.clone()]),
            "round {round}: no hash registration showed up among the delete registrations: {deletes:?}"
        );
    }
}

/// One registration of either kind, for [`tool_writes_bounded`].
enum Registration {
    Write(ToolWriteEntry),
    Delete(ToolDeleteEntry),
}

impl Registration {
    /// The `i`th registration of `round`: a hash registration, or a delete registration for every second one when the
    /// implementation registers deletes. Every one is new: its path, hash and request are its own.
    fn nth(round: &str, i: usize, with_deletes: bool) -> Self {
        let path = format!("svc/{round}-{i}.yml");
        let request = format!("bd-{round}-{i}");
        if with_deletes && i % 2 == 1 {
            Self::Delete(delete_entry(&path, format!("{round} {i}").as_bytes(), &request))
        } else {
            Self::Write(entry(&path, format!("{round} {i}").as_bytes(), &request))
        }
    }

    async fn register<T: ToolWritesProbe + ?Sized>(
        &self,
        t: &T,
        tx: &mut Tx<'_>,
    ) -> Result<(), ToolWriteError> {
        match self {
            Self::Write(e) => register(t, tx, e).await,
            Self::Delete(e) => register_delete(t, tx, e).await,
        }
    }
}

/// How many of `regs` the probe shows, each counted as many times as it shows.
async fn visible<T: ToolWritesProbe + ?Sized>(t: &T, regs: &[Registration]) -> usize {
    let hashes = t.registered().await;
    let deletes = t.registered_deletes().await.unwrap_or_default();
    regs.iter()
        .map(|r| match r {
            Registration::Write(e) => times(&hashes, e),
            Registration::Delete(e) => times(&deletes, e),
        })
        .sum()
}

/// Registers `regs` in `tx`, all of which must be accepted, then checks that a full registry refuses a new registration of
/// either kind (`refused`) and is not full for the repeat of one it holds.
async fn fill<T: ToolWritesProbe + ?Sized>(
    t: &T,
    tx: &mut Tx<'_>,
    round: &str,
    regs: &[Registration],
    refused: &mut dyn Iterator<Item = &Registration>,
) {
    for (i, r) in regs.iter().enumerate() {
        r.register(t, tx).await.unwrap_or_else(|e| {
            panic!(
                "round {round}: registration {i} of {} is within the bound, got {e:?}",
                regs.len()
            )
        });
    }
    for r in refused {
        assert_eq!(
            r.register(t, tx).await,
            Err(ToolWriteError::Full),
            "round {round}: a new registration beyond the bound is refused with Full, whichever kind it is"
        );
    }
    for r in [&regs[0], &regs[regs.len() - 1]] {
        r.register(t, tx).await.unwrap_or_else(|e| {
            panic!("round {round}: a repeat of a registration the registry holds is a no-op, not {e:?}")
        });
    }
}

/// The bound of the registry: `bound` registrations that no observation has matched yet, hash registrations and delete
/// registrations together (every queue is bounded). The bound is the registry's rule, soft by at most the number of
/// registering transactions in flight; this suite runs one transaction at a time, so it checks the bound exactly.
///
/// - A new registration beyond the bound is refused by `register` or `register_delete` with `Full`, whichever kind it is.
/// - It is refused there and never at the commit: every registration that was accepted is still there after the commit.
/// - A repeat of a registration the registry holds is a no-op, and is never `Full`, in the open transaction that made it
///   and after it committed.
/// - A rolled-back transaction gives its places back.
///
/// Run it on a fresh probe, one that holds no other registration, configured with a bound of `bound` (at least 2).
/// A delete registration is part of the mix when the probe opts in with [`ToolWritesProbe::registered_deletes`].
pub async fn tool_writes_bounded<T: ToolWritesProbe + ?Sized>(t: &T, txf: &dyn TxFactory, bound: usize) {
    assert!(bound >= 2, "the bound must leave room for both kinds");
    let with_deletes = t.registered_deletes().await.is_some();
    let extra = [
        Registration::Write(entry("svc/over.yml", b"over", "bd-over-hash")),
        Registration::Delete(delete_entry("svc/over.yml", b"over", "bd-over-delete")),
    ];
    let new_ones = || {
        extra
            .iter()
            .filter(|r| with_deletes || matches!(r, Registration::Write(_)))
    };

    // 1. Fill the registry in a transaction that rolls back: it gives every place back.
    let rolled_back: Vec<Registration> = (0..bound)
        .map(|i| Registration::nth("rb", i, with_deletes))
        .collect();
    let mut tx = txf.begin().await.unwrap();
    fill(t, &mut tx.tx(), "rolled back", &rolled_back, &mut new_ones()).await;
    tx.rollback().await.unwrap();
    assert_eq!(
        visible(t, &rolled_back).await,
        0,
        "a rolled-back registration leaves nothing behind"
    );

    // 2. Fill it again in a transaction that commits: the places were given back, and every accepted one is kept.
    let kept: Vec<Registration> = (0..bound)
        .map(|i| Registration::nth("ok", i, with_deletes))
        .collect();
    let mut tx = txf.begin().await.unwrap();
    fill(t, &mut tx.tx(), "committed", &kept, &mut new_ones()).await;
    tx.commit().await.unwrap();
    assert_eq!(
        visible(t, &kept).await,
        bound,
        "every registration the registry accepted is there after the commit: Full comes from register, never from the commit"
    );

    // 3. Full, and the repeat of what it holds is still a no-op, after the commit too.
    let mut tx = txf.begin().await.unwrap();
    for r in new_ones() {
        assert_eq!(
            r.register(t, &mut tx.tx()).await,
            Err(ToolWriteError::Full),
            "a full registry refuses a new registration of either kind after the commit"
        );
    }
    for r in &kept {
        r.register(t, &mut tx.tx()).await.unwrap_or_else(|e| {
            panic!(
                "the repeat of a registration the registry holds is a no-op, not {e:?}, even when it is full"
            )
        });
    }
    tx.commit().await.unwrap();
    assert_eq!(visible(t, &kept).await, bound, "the repeats registered nothing");
    assert_eq!(
        visible(t, &extra).await,
        0,
        "a refused registration is not there after a commit"
    );
}
