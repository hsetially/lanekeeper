//! Closed enums. The `snake_case` wire names are part of the contract (REST, MCP, SQL, SSE).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A string that is not a variant of the requested enum. Carries no input text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unknown value for {0}")]
pub struct UnknownVariant(pub &'static str);

/// Defines a fieldless enum with one wire name per variant, shared by serde, `Display` and `FromStr`.
macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $( $(#[$vmeta:meta])* $variant:ident => $wire:literal ),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum $name {
            $( $(#[$vmeta])* #[serde(rename = $wire)] $variant ),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: [Self; wire_enum!(@count $($variant)+)] = [ $( Self::$variant ),+ ];

            /// The wire name.
            pub const fn as_str(self) -> &'static str {
                match self { $( Self::$variant => $wire ),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = UnknownVariant;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $( $wire => Ok(Self::$variant), )+
                    _ => Err(UnknownVariant(stringify!($name))),
                }
            }
        }
    };
    (@count $($x:ident)+) => { <[()]>::len(&[ $( wire_enum!(@unit $x) ),+ ]) };
    (@unit $x:ident) => { () };
}

wire_enum! {
    /// Access level. The order is the privilege order: `Viewer < Editor < Operator < Admin`.
    /// A user with no role is represented as `Option<Role>::None`.
    Role {
        Viewer => "viewer",
        Editor => "editor",
        Operator => "operator",
        Admin => "admin",
    }
}

impl Role {
    /// True when this role is at least `min`.
    pub fn at_least(self, min: Role) -> bool {
        self >= min
    }
}

wire_enum! {
    UserStatus { Pending => "pending", Active => "active", Disabled => "disabled" }
}

wire_enum! {
    /// Where a file comes from (domain-model, "Files on NFS").
    FileKind { Base => "base", Tenant => "tenant", Untracked => "untracked" }
}

wire_enum! {
    /// How a file is compared. `Denied` files are recorded by name, size and hash only (D79).
    FileClass {
        Structured => "structured",
        Text => "text",
        Binary => "binary",
        Denied => "denied",
    }
}

wire_enum! {
    EolStyle { Lf => "lf", Crlf => "crlf", Mixed => "mixed" }
}

wire_enum! {
    TextEncoding { Utf8 => "utf8", Utf16Le => "utf16le", Utf16Be => "utf16be", Latin1 => "latin1" }
}

wire_enum! {
    /// How tenant settings combine with base ones (C1 differs by mode).
    MergeMode { WholeFile => "whole_file", SpringMerge => "spring_merge" }
}

wire_enum! {
    /// Drift states from the table in `docs/domain-model.md` (D19-D20).
    DriftState {
        InSync => "in_sync",
        GitAhead => "git_ahead",
        NfsAhead => "nfs_ahead",
        Conflict => "conflict",
        Unknown => "unknown",
        Untracked => "untracked",
        IntentionalDivergence => "intentional_divergence",
    }
}

impl DriftState {
    pub fn is_in_sync(self) -> bool {
        self == Self::InSync
    }

    /// True for a divergence somebody marked as intended.
    pub fn is_acknowledged(self) -> bool {
        self == Self::IntentionalDivergence
    }
}

wire_enum! {
    /// How a request reached a write path.
    Via { Ui => "ui", Mcp => "mcp" }
}

wire_enum! {
    ProposalStatus {
        Draft => "draft",
        Pending => "pending",
        Approved => "approved",
        Rejected => "rejected",
        Stale => "stale",
        Expired => "expired",
        Applied => "applied",
    }
}

wire_enum! {
    /// Consistency checks C1-C11, in numeric order. See [`FindingKind::code`].
    FindingKind {
        MissedBaseChanges => "missed_base_changes",
        RedundantTenantFile => "redundant_tenant_file",
        UnevenTenantFiles => "uneven_tenant_files",
        OrphanFile => "orphan_file",
        Drift => "drift",
        EnvironmentHostMismatch => "environment_host_mismatch",
        OverwrittenNfsChange => "overwritten_nfs_change",
        DuplicateKeys => "duplicate_keys",
        AmbiguousFileName => "ambiguous_file_name",
        UnresolvedPlaceholder => "unresolved_placeholder",
        ConfigServerRestartRequired => "config_server_restart_required",
    }
}

impl FindingKind {
    /// The check id used in the docs: `C1` ... `C11`.
    pub const fn code(self) -> &'static str {
        match self {
            Self::MissedBaseChanges => "C1",
            Self::RedundantTenantFile => "C2",
            Self::UnevenTenantFiles => "C3",
            Self::OrphanFile => "C4",
            Self::Drift => "C5",
            Self::EnvironmentHostMismatch => "C6",
            Self::OverwrittenNfsChange => "C7",
            Self::DuplicateKeys => "C8",
            Self::AmbiguousFileName => "C9",
            Self::UnresolvedPlaceholder => "C10",
            Self::ConfigServerRestartRequired => "C11",
        }
    }
}

wire_enum! {
    /// How the config-server treats a file (D82).
    FileRole { PropertySource => "property_source", Resource => "resource" }
}

wire_enum! {
    /// When a change takes effect for a consuming service (D85). Never show a bare "pending restart".
    PickupState {
        Live => "live",
        LiveWithinTtl => "live_within_ttl",
        NeedsNotifyOrRestart => "needs_notify_or_restart",
        NeedsConfigServerRestart => "needs_config_server_restart",
    }
}

wire_enum! {
    RepoKind { Base => "base", Tenant => "tenant" }
}

wire_enum! {
    /// How a change was attributed (D73), strongest evidence first.
    AttributionSource {
        ToolWrite => "tool_write",
        SentinelLogin => "sentinel_login",
        SyncJob => "sync_job",
        NfsClient => "nfs_client",
        FsOwnerHint => "fs_owner_hint",
        Unknown => "unknown",
    }
}

wire_enum! {
    Confidence { Low => "low", Medium => "medium", High => "high", Certain => "certain" }
}

wire_enum! {
    Severity { Low => "low", Medium => "medium", High => "high", Critical => "critical" }
}

wire_enum! {
    /// What produced a file observation.
    ObservationSource { Scan => "scan", ToolWrite => "tool_write", Sync => "sync" }
}

wire_enum! {
    AgentStatus { Connected => "connected", Disconnected => "disconnected", NeverSeen => "never_seen" }
}

wire_enum! {
    PrState { Open => "open", Merged => "merged", Closed => "closed" }
}

wire_enum! {
    /// PR job progress (D77).
    PrJobState {
        Pending => "pending",
        Preparing => "preparing",
        Committing => "committing",
        Opening => "opening",
        Completed => "completed",
        Failed => "failed",
    }
}
