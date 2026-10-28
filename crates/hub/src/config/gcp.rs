//! Google Cloud (plan 18, T1, G9): the project that Secret Manager (and, from T3, KMS) calls are made against.
//!
//! Only the project is a setting. The services' addresses are `GoogleEndpoints::production()`, a constant, so no variable can
//! send a call, and the workload's token with it, to another host (S18).

use hub_platform::gcp::ProjectId;

use super::ConfigError;
use super::reader::Reader;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcpSettings {
    /// The project that holds the secrets (`LK_GCP_PROJECT`).
    pub project: ProjectId,
}

impl GcpSettings {
    pub(super) fn read(reader: &Reader<'_>) -> Result<Option<Self>, ConfigError> {
        let Some(text) = reader.parse("LK_GCP_PROJECT")? else {
            return Ok(None);
        };
        let project = ProjectId::parse(&text).ok_or(ConfigError::Invalid("LK_GCP_PROJECT"))?;
        Ok(Some(Self { project }))
    }
}
