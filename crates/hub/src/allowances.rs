//! What the composed hub does not mount, and what it mounts that the API document does not describe (plan 18, T8; S4).
//!
//! `tests/route_table_full.rs` checks the table of every route of every module (`Running::routes()`) against the 99 operations of
//! `api/openapi.yaml`: each operation is mounted with the guard the document names, or it is in [`SPEC_ONLY`] with a reason and the
//! prompt that owns it, and each mounted route is an operation or is in [`ROUTE_ONLY`] with a reason. A new route that is in
//! neither, and a new operation that is in neither, fail that test. An allowance for something that *is* mounted (or for a route
//! that is not there) fails it too, so the list shrinks as owners mount their routes and cannot go stale.
//!
//! This is data and nothing else. It lives in the library so that the owner of an operation finds the line to delete in the crate that
//! composes the hub, and it carries no code that a release build runs. The owner of an operation deletes its line in the commit
//! that mounts the route; until then a client generated from the document calls a route that answers `404`, which is why the list
//! is kept honest and not merely long.

/// An operation of `api/openapi.yaml` that the hub does not mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecOnly {
    pub method: &'static str,
    pub path: &'static str,
    /// Why nothing mounts it.
    pub reason: &'static str,
    /// The prompt that is to mount it (its number, such as `06` or `03b`), or [`UNASSIGNED`] (D112: nobody owns it yet).
    pub owner: &'static str,
}

/// A route the hub mounts that is not an operation of `api/openapi.yaml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteOnly {
    pub method: &'static str,
    pub path: &'static str,
    pub reason: &'static str,
}

/// The owner of an operation that nobody has been given (D112, A7).
pub const UNASSIGNED: &str = "unassigned";

const ADMIN_TABLE: &str = "The admin operation for a registry table is in the specification and nobody owns it (D112 A7): \
the registry reads the table and its tests seed the rows with SQL, and `hub seed` covers projects, swimlanes, tenants and sentinels.";
const ADMIN_AGENTS: &str = "The admin operations on agents and their join tokens are in the specification and nobody owns \
them (D112 A7): the platform's `JoinService` and the agents table exist and no route reaches them.";
const ADMIN_AUDIT: &str = "The admin read of the audit log is in the specification and nobody owns it (D112 A7): identity \
writes and verifies the chain and no route lists it.";
const DOCS: &str = "Document search is prompt 14. Until it lands, `UnavailableDocs` answers every docs tool of the MCP server with a \
typed `unavailable` error and the hub mounts no REST docs route (decision D140).";

/// Operations the hub does not mount: 50 of the 99 (33 admin, 10 backed by document search, 7 waiting for their owners).
pub const SPEC_ONLY: &[SpecOnly] = &[
    // ---- the admin operations of the registry's tables: nobody owns them (D112 A7)
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/folder-mappings",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "DELETE",
        path: "/api/v1/admin/folder-mappings/{id}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "PUT",
        path: "/api/v1/admin/folder-mappings/{id}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/host-tiers",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "DELETE",
        path: "/api/v1/admin/host-tiers/{id}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "PUT",
        path: "/api/v1/admin/host-tiers/{id}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/merge-modes",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "DELETE",
        path: "/api/v1/admin/merge-modes/{id}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "PUT",
        path: "/api/v1/admin/merge-modes/{id}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/notifications",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "DELETE",
        path: "/api/v1/admin/notifications/{id}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "PUT",
        path: "/api/v1/admin/notifications/{id}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/os-login-users",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "DELETE",
        path: "/api/v1/admin/os-login-users/{login}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "PUT",
        path: "/api/v1/admin/os-login-users/{login}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/path-mappings",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "DELETE",
        path: "/api/v1/admin/path-mappings/{id}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "PUT",
        path: "/api/v1/admin/path-mappings/{id}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/retention",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "PUT",
        path: "/api/v1/admin/retention",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/sentinels",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/severity-rules",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "DELETE",
        path: "/api/v1/admin/severity-rules/{id}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "PUT",
        path: "/api/v1/admin/severity-rules/{id}",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/swimlanes/{id}/auto-notify",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "PUT",
        path: "/api/v1/admin/swimlanes/{id}/auto-notify",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/swimlanes/{id}/tenants",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "PUT",
        path: "/api/v1/admin/swimlanes/{id}/tenants",
        reason: ADMIN_TABLE,
        owner: UNASSIGNED,
    },
    // ---- the agents: join tokens and the agents table
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/agents",
        reason: ADMIN_AGENTS,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "DELETE",
        path: "/api/v1/admin/agents/{swimlane}",
        reason: ADMIN_AGENTS,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "PUT",
        path: "/api/v1/admin/agents/{swimlane}",
        reason: ADMIN_AGENTS,
        owner: UNASSIGNED,
    },
    SpecOnly {
        method: "POST",
        path: "/api/v1/admin/agents/{swimlane}/join-token",
        reason: ADMIN_AGENTS,
        owner: UNASSIGNED,
    },
    // ---- the audit log, read by an admin
    SpecOnly {
        method: "GET",
        path: "/api/v1/admin/audit",
        reason: ADMIN_AUDIT,
        owner: UNASSIGNED,
    },
    // ---- document search (prompt 14)
    SpecOnly {
        method: "GET",
        path: "/api/v1/docs",
        reason: DOCS,
        owner: "14",
    },
    SpecOnly {
        method: "POST",
        path: "/api/v1/docs",
        reason: DOCS,
        owner: "14",
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/docs/for-file",
        reason: DOCS,
        owner: "14",
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/docs/fs/grep",
        reason: DOCS,
        owner: "14",
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/docs/fs/read",
        reason: DOCS,
        owner: "14",
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/docs/fs/render",
        reason: DOCS,
        owner: "14",
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/docs/search",
        reason: DOCS,
        owner: "14",
    },
    SpecOnly {
        method: "DELETE",
        path: "/api/v1/docs/{id}",
        reason: DOCS,
        owner: "14",
    },
    SpecOnly {
        method: "PUT",
        path: "/api/v1/docs/{id}",
        reason: DOCS,
        owner: "14",
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/features/status",
        reason: DOCS,
        owner: "14",
    },
    // ---- operations that wait for a server (docs/interfaces.md, "Operations that wait for a server")
    SpecOnly {
        method: "GET",
        path: "/api/v1/drafts/{id}",
        reason: "`getDraft` is backed by `WriteService::draft`, which exists; the route is a `LATER` row of prompt 06's route test and fails that test when mounted.",
        owner: "06",
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/git/refs",
        reason: "`listGitRefs` is backed by `GitReader::branches` and `tags`, which exist; no test pins the missing route, so this line is its only reminder.",
        owner: "03b",
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/me/github-token",
        reason: "`getGithubCredential` is backed by `TokenVault::status`, which exists; the route is a `LATER` row of prompt 06's route test.",
        owner: "06",
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/proposals/{id}",
        reason: "`getProposal` is backed by `ProposalDesk::get_proposal`, which exists; the route is a `LATER` row of prompt 06's route test.",
        owner: "06",
    },
    SpecOnly {
        method: "POST",
        path: "/api/v1/prs/preview",
        reason: "`previewPr` is backed by `PrService::plan` without side effects; the route is a `LATER` row of prompt 06's route test.",
        owner: "06",
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/swimlanes/{id}/config-server-restart",
        reason: "`getConfigServerRestartOffer` is backed by `WriteHub::config_server_restart_offer`; the route is a `LATER` row of prompt 06's route test.",
        owner: "06",
    },
    SpecOnly {
        method: "GET",
        path: "/api/v1/swimlanes/{id}/restart-offer",
        reason: "`getRestartOffer` is backed by `WriteHub::restart_offer`; the route is a `LATER` row of prompt 06's route test.",
        owner: "06",
    },
];

/// Routes the hub mounts that are not operations of the API document: the MCP server's own protocol and the discovery documents
/// its clients read before they have a credential.
pub const ROUTE_ONLY: &[RouteOnly] = &[
    RouteOnly {
        method: "POST",
        path: "/mcp",
        reason: "The MCP endpoint speaks the protocol of docs/mcp-tools.md, not an API operation; it is public to the guard and authenticates itself with a bearer token (D109 D-1).",
    },
    RouteOnly {
        method: "GET",
        path: "/mcp",
        reason: "The MCP endpoint's stream, not an API operation; it authenticates itself with a bearer token (D109 D-1).",
    },
    RouteOnly {
        method: "DELETE",
        path: "/mcp",
        reason: "The end of an MCP session, not an API operation; it authenticates itself with a bearer token (D109 D-1).",
    },
    RouteOnly {
        method: "GET",
        path: "/.well-known/oauth-protected-resource",
        reason: "OAuth protected-resource metadata (RFC 9728) for the MCP server: a client reads it before it has a credential, so it is public by design.",
    },
    RouteOnly {
        method: "GET",
        path: "/.well-known/oauth-protected-resource/mcp",
        reason: "The same document at the path RFC 9728 derives from the resource `/mcp`: public by design.",
    },
];
