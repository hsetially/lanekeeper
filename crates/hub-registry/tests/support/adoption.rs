//! A world with the adoption service on top (T5): an `AdoptionService` over the world's database, Git, blob store, audit log, bus and
//! clock, with the users it needs: real rows of `users` (the baselines, marks and passes name them by foreign key), one for each role.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use domain::{
    CommitId, ContentHash, Guid, NfsPath, Role, ShortText, SwimlaneId, TenantId, User, UserId, UserStatus,
};
use hub_registry::{
    AdoptionDeps, AdoptionReason, AdoptionService, AdoptionView, Bulk, Reason, RegistryConfig, Resolution,
};
use sqlx::PgPool;

use super::rig::path;
use super::world::World;

/// A user with a role (`None` is the "none" role) and a status. The number makes the id.
pub fn user(n: u32, role: Option<Role>, status: UserStatus) -> User {
    let guid = |part: u32| Guid::parse(&format!("00000000-0000-0000-{part:04x}-{n:012x}")).unwrap();
    User {
        id: UserId::new(guid(1), guid(2)),
        email: ShortText::parse(&format!("user{n}@example.com")).unwrap(),
        display_name: ShortText::parse(&format!("User {n}")).unwrap(),
        role,
        status,
        requires_approval: false,
        github_login: None,
        last_seen_at: None,
    }
}

pub fn admin() -> User {
    user(1, Some(Role::Admin), UserStatus::Active)
}

pub fn operator() -> User {
    user(2, Some(Role::Operator), UserStatus::Active)
}

/// Every caller that must be refused by the Admin check, with a name for the failure message.
pub fn not_admins() -> Vec<(&'static str, User)> {
    vec![
        ("pending admin", user(10, Some(Role::Admin), UserStatus::Pending)),
        (
            "disabled admin",
            user(11, Some(Role::Admin), UserStatus::Disabled),
        ),
        ("no role", user(12, None, UserStatus::Active)),
        ("viewer", user(13, Some(Role::Viewer), UserStatus::Active)),
        ("editor", user(14, Some(Role::Editor), UserStatus::Active)),
        ("operator", operator()),
    ]
}

/// Every caller that must be refused by the Operator check.
pub fn not_operators() -> Vec<(&'static str, User)> {
    vec![
        ("pending admin", user(10, Some(Role::Admin), UserStatus::Pending)),
        (
            "disabled operator",
            user(15, Some(Role::Operator), UserStatus::Disabled),
        ),
        ("no role", user(12, None, UserStatus::Active)),
        ("viewer", user(13, Some(Role::Viewer), UserStatus::Active)),
        ("editor", user(14, Some(Role::Editor), UserStatus::Active)),
    ]
}

/// The `users` row of a user, so that the foreign keys that name them hold.
pub async fn insert_user(pool: &PgPool, u: &User) {
    sqlx::query(
        "INSERT INTO users (tid, oid, email, display_name, role, status) VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT DO NOTHING",
    )
    .bind(u.id.tid().as_str())
    .bind(u.id.oid().as_str())
    .bind(u.email.as_str())
    .bind(u.display_name.as_str())
    .bind(u.role.map(Role::as_str))
    .bind(u.status.as_str())
    .execute(pool)
    .await
    .unwrap();
}

pub struct AdoptionRig {
    pub w: World,
    pub svc: AdoptionService,
    pub admin: User,
    pub operator: User,
}

pub fn tenant(name: &str) -> TenantId {
    TenantId::parse(name).unwrap()
}

pub fn lane() -> SwimlaneId {
    SwimlaneId::parse("sit1").unwrap()
}

impl AdoptionRig {
    /// A swimlane `sit1` that deploys `tenants` (none before its first adoption). `None` when there is no database.
    pub async fn new(tenants: &[&str]) -> Option<Self> {
        let w = World::new(tenants).await?;
        Self::over(w).await
    }

    pub async fn over(w: World) -> Option<Self> {
        let admin = admin();
        let operator = operator();
        insert_user(&w.rig.pool, &admin).await;
        insert_user(&w.rig.pool, &operator).await;
        let svc = Self::service_of(&w, RegistryConfig::default());
        Some(Self {
            w,
            svc,
            admin,
            operator,
        })
    }

    /// A service over the world's stores: a second one is what a restart or another replica is.
    pub fn service_of(w: &World, config: RegistryConfig) -> AdoptionService {
        AdoptionService::new(AdoptionDeps {
            pool: w.rig.pool.clone(),
            git: Arc::new(w.git.clone()),
            blobs: Arc::clone(&w.rig.blobs) as _,
            bus: Arc::clone(&w.rig.bus) as _,
            audit: Arc::clone(&w.rig.audit) as _,
            clock: Arc::clone(&w.rig.clock) as _,
            config,
            metrics: w.rig.metrics.clone(),
            pipeline: Some(w.handle.clone()),
        })
        .unwrap()
    }

    pub async fn start(&self) -> AdoptionView {
        self.svc
            .start(&self.admin, &lane(), AdoptionReason::Initial)
            .await
            .unwrap()
    }

    /// Choose branches and accept the proposed base commit: the pass is at the files (or, with none that differ, at the secrets).
    pub async fn choose(&self, id: i64, tenants: &[&str]) -> AdoptionView {
        let tenants: Vec<TenantId> = tenants.iter().map(|t| tenant(t)).collect();
        self.svc.choose_branches(&self.admin, id, &tenants).await.unwrap();
        self.svc.choose_base_commit(&self.admin, id, None).await.unwrap()
    }

    pub async fn resolve(&self, id: i64, items: &[(&str, Resolution)]) -> AdoptionView {
        self.svc
            .resolve(
                &self.admin,
                id,
                items.iter().map(|(p, r)| (path(p), r.clone())).collect(),
            )
            .await
            .unwrap()
    }

    pub async fn resolve_all(&self, id: i64, scope: Bulk, resolution: Resolution) -> AdoptionView {
        self.svc
            .resolve_all(&self.admin, id, scope, resolution)
            .await
            .unwrap()
    }
}

pub fn intentional(reason: &str) -> Resolution {
    Resolution::Intentional(Reason::parse(reason).unwrap())
}

pub fn commit_of(view: &AdoptionView, t: &str) -> CommitId {
    view.tenant_commits[&tenant(t)].clone()
}

// ---------------------------------------------------------------------------------------------------------------- the tables

/// A baseline row as the tests read it: (hash, repo, branch, commit, raw version, adopted by).
pub type BaselineRow = (
    ContentHash,
    String,
    Option<String>,
    String,
    Option<ContentHash>,
    Option<String>,
);

type BaselineTuple = (
    Vec<u8>,
    String,
    Option<String>,
    String,
    Option<Vec<u8>>,
    Option<String>,
);

pub async fn baseline(pool: &PgPool, lane: &str, p: &str) -> Option<BaselineRow> {
    let row: Option<BaselineTuple> = sqlx::query_as(
        "SELECT hash::bytea, repo, branch, commit_id, blob_hash::bytea, adopted_by_oid FROM baselines \
         WHERE swimlane_id = $1 AND path = $2",
    )
    .bind(lane)
    .bind(p)
    .fetch_optional(pool)
    .await
    .unwrap();
    let h = |b: Vec<u8>| ContentHash::from_bytes(b.try_into().unwrap());
    row.map(|(hash, repo, branch, commit, blob, by)| (h(hash), repo, branch, commit, blob.map(h), by))
}

pub async fn baseline_paths(pool: &PgPool, lane: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT path FROM baselines WHERE swimlane_id = $1 ORDER BY path COLLATE \"C\" LIMIT 100000",
    )
    .bind(lane)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The marks of a swimlane: (path, N, G, reason, marked by).
pub type MarkRow = (
    String,
    Option<ContentHash>,
    Option<ContentHash>,
    Option<String>,
    String,
);

pub async fn marks(pool: &PgPool, lane: &str) -> Vec<MarkRow> {
    type Tuple = (String, Option<Vec<u8>>, Option<Vec<u8>>, Option<String>, String);
    let rows: Vec<Tuple> = sqlx::query_as(
        "SELECT path, nfs_hash::bytea, git_hash::bytea, reason, marked_by_oid FROM intentional_divergence \
         WHERE swimlane_id = $1 ORDER BY path COLLATE \"C\" LIMIT 100000",
    )
    .bind(lane)
    .fetch_all(pool)
    .await
    .unwrap();
    let h = |b: Vec<u8>| ContentHash::from_bytes(b.try_into().unwrap());
    rows.into_iter()
        .map(|(p, n, g, r, by)| (p, n.map(h), g.map(h), r, by))
        .collect()
}

pub async fn tenants_of(pool: &PgPool, lane: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT tenant_id FROM swimlane_tenants WHERE swimlane_id = $1 ORDER BY tenant_id")
        .bind(lane)
        .fetch_all(pool)
        .await
        .unwrap()
}

/// (editing unlocked, baseline confirmed, confirmed by)
pub async fn lane_flags(pool: &PgPool, lane: &str) -> (bool, bool, Option<String>) {
    sqlx::query_as(
        "SELECT editing_unlocked, baseline_confirmed_at IS NOT NULL, baseline_confirmed_by_oid FROM swimlanes WHERE id = $1",
    )
    .bind(lane)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// How many audit events there are for an action.
pub async fn audit_count(pool: &PgPool, action: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The text of every audit event, the outbox and the adoption rows: where a value that must never be stored would show.
pub async fn everything_stored(pool: &PgPool) -> String {
    let mut text = String::new();
    for sql in [
        "SELECT event_json FROM audit_events ORDER BY seq LIMIT 100000",
        "SELECT event::text FROM outbox ORDER BY id LIMIT 100000",
        "SELECT row_to_json(a)::text FROM adoptions a ORDER BY id LIMIT 100000",
        "SELECT row_to_json(r)::text FROM adoption_resolutions r LIMIT 100000",
        "SELECT row_to_json(m)::text FROM intentional_divergence m LIMIT 100000",
        "SELECT row_to_json(b)::text FROM baselines b LIMIT 100000",
    ] {
        let rows: Vec<String> = sqlx::query_scalar(sql).fetch_all(pool).await.unwrap();
        for row in rows {
            text.push_str(&row);
            text.push('\n');
        }
    }
    text
}

/// A path of the world's swimlane.
pub fn nfs(p: &str) -> NfsPath {
    path(p)
}

// ---------------------------------------------------------------------------------------------------------------- secrets

/// A token the engine's scan flags (`ghp_` and 36 characters). The seed makes each one different.
pub fn token(seed: usize) -> String {
    const BODY: &str = "aB3dE5gH7jK9mN1pQ3sT5vW7yZ9bD1fH3jL5";
    let n = seed % BODY.len();
    format!("ghp_{}{}", &BODY[n..], &BODY[..n])
}

/// The planted secrets of [`secret_world`], and where each one is.
pub struct Planted {
    /// In a base file that NFS does not have in that form.
    pub base: String,
    /// In a file that is the same on NFS and in the base repository.
    pub shared: String,
    /// In a file of the chosen tenant branch.
    pub tenant: String,
    /// In a file only NFS has.
    pub nfs: String,
    /// In a file the glob denies, in Git and on NFS: never read.
    pub denied: String,
    /// In a file of a tenant branch that is not chosen.
    pub other_tenant: String,
}

impl Planted {
    pub fn all(&self) -> Vec<&str> {
        vec![
            &self.base,
            &self.shared,
            &self.tenant,
            &self.nfs,
            &self.denied,
            &self.other_tenant,
        ]
    }
}

/// A swimlane that has its inventory, with secrets planted in the base repository, the tenant repository (a chosen branch and another),
/// on NFS, in a denied file, and one file that cannot be checked. Returns the rig and the planted values.
pub async fn secret_world() -> Option<(AdoptionRig, Planted)> {
    use bytes::Bytes;
    use domain::Timestamp;

    use super::rig::{T, delta, entry, root};

    let p = Planted {
        base: token(1),
        shared: token(5),
        tenant: token(9),
        nfs: token(13),
        denied: token(17),
        other_tenant: token(21),
    };
    let r = AdoptionRig::new(&[]).await?;
    let shared = format!("shared: {}\n", p.shared);
    r.w.base_commit(&[
        (
            "config/app/base.yml",
            Some(&format!("name: base\ntoken: {}\n", p.base)),
        ),
        ("config/app/shared.yml", Some(&shared)),
        ("config/tls/server.key", Some(&format!("KEY {}", p.denied))),
        ("config/app/clean.yml", Some("clean: true\n")),
    ]);
    r.w.tenant_commit(
        "sit1",
        &[("data/config/app/t.yml", Some(&format!("tenant: {}\n", p.tenant)))],
    );
    r.w.tenant_commit(
        "sit2",
        &[(
            "data/config/app/other.yml",
            Some(&format!("other: {}\n", p.other_tenant)),
        )],
    );
    // The inventory is made by hand, because one file has bytes that are no text and one is denied (its bytes are not sent).
    r.w.rig.hello().await;
    let mut entries = vec![
        entry("app/shared.yml", shared.as_bytes(), T),
        entry("app/base.yml", b"name: base\n", T),
        entry("app/clean.yml", b"clean: true\n", T),
        entry("app/local.yml", format!("token: {}\n", p.nfs).as_bytes(), T),
        entry("app/t-sit1.yml", format!("tenant: {}\n", p.tenant).as_bytes(), T),
        entry("app/weird.yml", b"\xEF\xBB\xBF\xFF\xFE", T),
    ];
    let mut key = entry("tls/server.key", format!("KEY {}", p.denied).as_bytes(), T);
    key.denied = true;
    key.bytes = None::<Bytes>;
    key.observed_at = Timestamp::from_unix_millis(T);
    entries.push(key);
    r.w.rig.delta(delta(1, None, root(1), entries)).await.unwrap();
    Some((r, p))
}

/// Walk the pass to the secret scan: start, the branch, the base commit, everything that differs resolved ("Git is right"), the scan.
pub async fn scanned(r: &AdoptionRig, tenants: &[&str]) -> AdoptionView {
    let id = r.start().await.id;
    r.choose(id, tenants).await;
    r.resolve_all(id, Bulk::default(), Resolution::GitIsRight).await;
    r.svc.scan_secrets(&r.admin, id).await.unwrap()
}

// ---------------------------------------------------------------------------------------------------------------- ranking

/// What the ranking of a swimlane must look like: its tenants take the first places (a swimlane with two tenants has files of both, and
/// each shares with its own branch), each with something in common, and every other branch has nothing in common with it.
pub fn assert_ranked_right(view: &AdoptionView, tenants: &[String]) {
    let lane = view.swimlane.as_str();
    let names: Vec<&str> = view.ranking.iter().map(|b| b.branch.as_str()).collect();
    assert!(names.len() >= tenants.len(), "{lane}: {names:?}");
    let first: std::collections::BTreeSet<&str> = names[..tenants.len()].iter().copied().collect();
    let expected: std::collections::BTreeSet<&str> = tenants.iter().map(String::as_str).collect();
    assert_eq!(
        first, expected,
        "{lane}: the branches it was deployed from come first: {names:?}"
    );
    for b in &view.ranking[..tenants.len()] {
        assert!(b.shared > 0, "{lane}: {} has nothing in common with it", b.branch);
    }
    for b in &view.ranking[tenants.len()..] {
        assert_eq!(
            b.shared, 0,
            "{lane}: {} is not one of its branches and shares files with it",
            b.branch
        );
    }
    if tenants.len() == 1 {
        assert_eq!(
            names[0], tenants[0],
            "{lane}: a swimlane with one tenant has that branch first"
        );
    }
    assert!(
        view.ranking
            .iter()
            .all(|b| !b.branch.as_str().starts_with("templates/")),
        "{lane}: a template is never ranked"
    );
}
