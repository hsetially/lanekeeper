//! A file as the config-server serves it (T6, plan A4, D86, A36): `getServedFile` and the port's `served`.
//!
//! The hub asks the agent in the swimlane's cluster to GET the config-server's answer for `(application, tenant, channel, file)`
//! (`HubCommand::FetchServed`) and shows it next to what the engine believes: the engine's role and sources (the layers its effective
//! config is made of) and the placeholders still in the served text, which are by definition the ones the config-server did not
//! replace. It never uses an environment value, and the agent only ever asks the config-server (D88).
//!
//! **Bounds:** one round trip takes at most `served_timeout` (10 s), at most `served_concurrency` are in flight on a replica (the
//! rest are refused, not queued), and a denied file is never fetched: it is answered "withheld" (D79).
//!
//! **Not cached (T10).** The answer is what the config-server says *now*. A cache keyed by the request and the engine's current effective
//! hashes would answer from the files while the config-server, which has not picked a change up (it keeps its list of folders from its
//! start, D83, and a client keeps its answer for the cache lifetime, D85), serves something else: the one thing this view exists to show.
//! The round trip is therefore made for every request, within the limits above, and the entity tag is a hash of the request and of the
//! served bytes, so a request that already has them gets a 304 after the round trip.
//!
//! The nightly comparison of the engine's rendering with this answer is [`crate::served`]; it makes the same round trip
//! ([`Inner::round_trip`]) and tells its cases apart, where the route turns them into one error.

use std::sync::atomic::Ordering;

use domain::{
    AgentReply, ChannelName, EffectiveSource, FileRole, HubCommand, LogicalFile, OpError, OpResult,
    RequestId, ServeRequest, ServedFile, ShortText, SwimlaneId, TenantSet,
};
use engine::{file_role, scan_placeholders};
use ports::{GatewayError, ReadError};
use tracing::warn;

use super::api::Tagged;
use super::etag;
use super::rows;
use super::service::Inner;
use crate::http::dto::served_text;

/// What one round trip to the agent came to, for a caller that tells the cases apart: the route turns them into one error, the nightly
/// verification counts them separately.
#[derive(Debug)]
pub(crate) enum Round {
    /// The config-server served the file (200): its bytes.
    Served(bytes::Bytes),
    /// The config-server answered with another status (404 among them, and the agent's own 429 when it is asked too often). No body is
    /// kept: only a 2xx carries one, and the agent passes on nothing else.
    Status(u16),
    /// The agent refused the path: its deny list covers it (D79). The answer names nothing.
    Denied,
    /// The agent answered something that is not a served file.
    Other,
}

/// Why a round trip came back with no answer at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unreached {
    NotConnected,
    Timeout,
    Busy,
    Closed,
    Unavailable,
}

impl Inner {
    /// The engine's view of the request: its role, and the files its effective config is made of.
    async fn served_context(
        &self,
        swimlane: &SwimlaneId,
        request: &ServeRequest,
    ) -> Result<(FileRole, Vec<EffectiveSource>), ReadError> {
        let tenants = self.tenants_of(swimlane).await?;
        let set = TenantSet::new(tenants).map_err(|_| ReadError::Unavailable)?;
        let apps: Vec<domain::AppName> = {
            let groups = self
                .groups_of(
                    std::slice::from_ref(swimlane),
                    &LogicalFile::from_nfs(request.file.clone()).map_err(|_| ReadError::BadRequest)?,
                )
                .await?;
            groups.get(swimlane).map(|g| g.apps.clone()).unwrap_or_default()
        };
        let role = file_role(&request.file, &set, &apps);
        // The logical file: the request's file without a tenant suffix.
        let mapping = self.mapping_rule().await?;
        let Ok(domain::ReverseMapping::Base { logical, .. } | domain::ReverseMapping::Tenant { logical, .. }) =
            mapping.reverse(&request.file, &set)
        else {
            return Ok((role, Vec::new()));
        };
        let sources = match self
            .effective_config(
                swimlane,
                &logical,
                Some(&request.tenant),
                request.channel.as_ref(),
                None,
            )
            .await
        {
            Ok(Tagged::Fresh { value, .. }) => value.sources,
            Ok(Tagged::NotModified { .. }) | Err(_) => Vec::new(),
        };
        Ok((role, sources))
    }

    /// The answer for a file that is denied: its hash and nothing of its content (D79).
    fn withheld(
        swimlane: &SwimlaneId,
        request: &ServeRequest,
        role: FileRole,
        hash: domain::ContentHash,
    ) -> Tagged<ServedFile> {
        Tagged::Fresh {
            etag: etag::tag(
                "served",
                &[
                    swimlane.as_str(),
                    request.file.as_str(),
                    &hash.to_string(),
                    "withheld",
                ],
            ),
            value: ServedFile {
                swimlane: swimlane.clone(),
                application: request.application.clone(),
                tenant: request.tenant.clone(),
                channel: request.channel.clone(),
                file: request.file.clone(),
                role,
                hash,
                content_withheld: true,
                text: None,
                sources: Vec::new(),
                unresolved_placeholders: Vec::new(),
            },
        }
    }

    /// One round trip to the agent: what the config-server answered, within `served_timeout`. The caller holds a slot of `served_slots`.
    /// The only command it sends is `FetchServed`, which the agent answers read-only (D86).
    pub(crate) async fn round_trip(
        &self,
        swimlane: &SwimlaneId,
        request: &ServeRequest,
    ) -> Result<Round, Unreached> {
        let id = self.request_seq.fetch_add(1, Ordering::Relaxed);
        let request_id = RequestId::parse(&format!("served-{}-{id}", self.clock.now().unix_millis()))
            .map_err(|_| Unreached::Unavailable)?;
        let reply = self
            .gateway
            .request(
                swimlane,
                HubCommand::FetchServed {
                    request_id,
                    request: request.clone(),
                },
                self.config.served_timeout,
            )
            .await
            .map_err(|e| {
                warn!(op = "served", ?e, "the agent did not answer");
                match e {
                    GatewayError::NotConnected => Unreached::NotConnected,
                    GatewayError::Timeout => Unreached::Timeout,
                    GatewayError::Busy => Unreached::Busy,
                    GatewayError::Closed => Unreached::Closed,
                    GatewayError::Unavailable => Unreached::Unavailable,
                }
            })?;
        Ok(match reply {
            AgentReply::Served {
                status: 200, bytes, ..
            } => Round::Served(bytes),
            AgentReply::Served { status, .. } => Round::Status(status),
            AgentReply::Op(OpResult {
                error: Some(OpError::Denied),
                ..
            }) => {
                warn!(op = "served", why = "denied", "the agent refused the path");
                Round::Denied
            }
            _ => {
                warn!(
                    op = "served",
                    "the agent answered something other than a served file"
                );
                Round::Other
            }
        })
    }

    /// One round trip to the agent: the bytes the config-server answered with. The caller holds a slot of `served_slots`.
    async fn fetch_served(
        &self,
        swimlane: &SwimlaneId,
        request: &ServeRequest,
    ) -> Result<bytes::Bytes, ReadError> {
        match self.round_trip(swimlane, request).await {
            Ok(Round::Served(bytes)) => Ok(bytes),
            Ok(Round::Status(404)) => Err(ReadError::NotFound),
            Ok(Round::Status(status)) => {
                warn!(op = "served", status, "the config-server did not serve the file");
                Err(ReadError::Unavailable)
            }
            Ok(Round::Denied | Round::Other) | Err(_) => Err(ReadError::Unavailable),
        }
    }

    /// `getServedFile`.
    pub async fn served_file(
        &self,
        swimlane: &SwimlaneId,
        request: &ServeRequest,
        if_none_match: Option<&str>,
    ) -> Result<Tagged<ServedFile>, ReadError> {
        self.require_swimlane(swimlane).await?;
        // A tenant the swimlane does not deploy is not a request the config-server could answer for it.
        if !self.tenants_of(swimlane).await?.contains(&request.tenant) {
            return Err(ReadError::NotFound);
        }
        let row = self.file_row(swimlane, &request.file).await?;
        let denied = crate::denied::is_denied(
            &self.deny,
            &request.file,
            row.as_ref().is_some_and(|r| r.denied),
            row.as_ref().map_or(domain::FileClass::Text, |r| r.class),
        );
        if denied {
            let (role, _) = self.served_context(swimlane, request).await?;
            let hash = row.and_then(|r| r.hash).ok_or(ReadError::NotFound)?;
            return Ok(Self::withheld(swimlane, request, role, hash));
        }
        // A replica makes at most `served_concurrency` round trips at once; the rest are refused now, before any work is done for them.
        let Ok(_slot) = self.served_slots.try_acquire() else {
            warn!(
                op = "served",
                why = "busy",
                "too many served-file calls in flight"
            );
            return Err(ReadError::Unavailable);
        };
        let (role, sources) = self.served_context(swimlane, request).await?;
        let bytes = self.fetch_served(swimlane, request).await?;
        let hash = ports::content_hash(&bytes);
        let tag = etag::tag(
            "served",
            &[
                swimlane.as_str(),
                request.application.as_str(),
                request.tenant.as_str(),
                request.channel.as_ref().map_or("", ChannelName::as_str),
                request.file.as_str(),
                &hash.to_string(),
            ],
        );
        if etag::matches(if_none_match, &tag) {
            return Ok(Tagged::NotModified { etag: tag });
        }
        let text = served_text(&bytes, &self.limits);
        let unresolved: Vec<ShortText> = text
            .as_deref()
            .map(|t| {
                scan_placeholders(t)
                    .iter()
                    .filter_map(|p| rows::short(&p.key).ok())
                    .collect()
            })
            .unwrap_or_default();
        Ok(Tagged::Fresh {
            etag: tag,
            value: ServedFile {
                swimlane: swimlane.clone(),
                application: request.application.clone(),
                tenant: request.tenant.clone(),
                channel: request.channel.clone(),
                file: request.file.clone(),
                role,
                hash,
                content_withheld: false,
                text,
                sources,
                unresolved_placeholders: unresolved,
            },
        })
    }
}
