//! Events published on the `EventBus` and delivered to SSE subscribers (`docs/interfaces.md`).
//!
//! The first ten variants are the base set; the last five are the D72-D80 additions.
//! The JSON shape (`{"type": "<snake_case variant>", ...fields}`) is pinned by a snapshot test.

use serde::{Deserialize, Serialize};

use crate::{
    AgentStatus, CommitId, ContentHash, DocId, GitRef, JobRef, NfsPath, ObservationSource, PrJobId,
    PrJobState, PrLinkId, PrState, ProposalId, ProposalStatus, RepoKind, ServiceRef, SwimlaneId, UserId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainEvent {
    /// A file version was seen (scan, tool write or sync).
    FileObserved {
        swimlane: SwimlaneId,
        path: NfsPath,
        hash: ContentHash,
        source: ObservationSource,
    },
    /// Drift state changed for these paths.
    DriftChanged {
        swimlane: SwimlaneId,
        paths: Vec<NfsPath>,
    },
    FindingsChanged {
        swimlane: SwimlaneId,
    },
    PendingRestartChanged {
        swimlane: SwimlaneId,
        services: Vec<ServiceRef>,
    },
    ProposalChanged {
        id: ProposalId,
        status: ProposalStatus,
    },
    AgentStatusChanged {
        swimlane: SwimlaneId,
        status: AgentStatus,
    },
    GitHeadMoved {
        repo: RepoKind,
        branch: GitRef,
        from: Option<CommitId>,
        to: CommitId,
    },
    AccessRequestCreated {
        user: UserId,
    },
    DocIndexed {
        doc: DocId,
    },
    /// The subscriber lagged and dropped events: refetch state.
    Resync,
    // D72-D80 additions
    AttributionUpdated {
        swimlane: SwimlaneId,
        path: NfsPath,
        hash: ContentHash,
    },
    PrStateChanged {
        link_id: PrLinkId,
        state: PrState,
    },
    PrJobProgress {
        job_id: PrJobId,
        state: PrJobState,
    },
    SyncWindowOpened {
        swimlane: SwimlaneId,
        job: JobRef,
    },
    SyncWindowClosed {
        swimlane: SwimlaneId,
        job: JobRef,
    },
}

impl DomainEvent {
    /// The swimlane the event concerns, for filtering by what a user may see. Global events return `None`.
    pub fn swimlane(&self) -> Option<&SwimlaneId> {
        match self {
            Self::FileObserved { swimlane, .. }
            | Self::DriftChanged { swimlane, .. }
            | Self::FindingsChanged { swimlane }
            | Self::PendingRestartChanged { swimlane, .. }
            | Self::AgentStatusChanged { swimlane, .. }
            | Self::AttributionUpdated { swimlane, .. }
            | Self::SyncWindowOpened { swimlane, .. }
            | Self::SyncWindowClosed { swimlane, .. } => Some(swimlane),
            Self::ProposalChanged { .. }
            | Self::GitHeadMoved { .. }
            | Self::AccessRequestCreated { .. }
            | Self::DocIndexed { .. }
            | Self::Resync
            | Self::PrStateChanged { .. }
            | Self::PrJobProgress { .. } => None,
        }
    }
}
