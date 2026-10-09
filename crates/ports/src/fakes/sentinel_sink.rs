use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use domain::{AuditRecordBatch, SentinelConfig, SentinelHello, ShortText};

use super::util::lock;
use crate::conformance::SentinelProbe;
use crate::{AckSeq, SentinelIdentity, SentinelSink, SinkError};

const MAX_BATCH_RECORDS: u32 = 1000;

#[derive(Default)]
struct State {
    last_ack: Option<u64>,
    applied: usize,
}

/// Applies each spooled batch once and acknowledges re-sends without applying them again (D74).
#[derive(Clone, Default)]
pub struct FakeSentinelSink {
    sentinels: Arc<Mutex<HashMap<ShortText, State>>>,
}

impl std::fmt::Debug for FakeSentinelSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeSentinelSink").finish_non_exhaustive()
    }
}

impl FakeSentinelSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many records have been applied for this sentinel.
    pub fn applied_records(&self, id: &SentinelIdentity) -> usize {
        lock(&self.sentinels).get(id.name()).map_or(0, |s| s.applied)
    }
}

#[async_trait]
impl SentinelSink for FakeSentinelSink {
    async fn hello(&self, id: &SentinelIdentity, _h: SentinelHello) -> Result<SentinelConfig, SinkError> {
        lock(&self.sentinels).entry(id.name().clone()).or_default();
        Ok(SentinelConfig {
            max_batch_records: MAX_BATCH_RECORDS,
            flush_interval_secs: 5,
        })
    }

    async fn records(&self, id: &SentinelIdentity, batch: AuditRecordBatch) -> Result<AckSeq, SinkError> {
        let mut sentinels = lock(&self.sentinels);
        let st = sentinels.get_mut(id.name()).ok_or(SinkError::HelloRequired)?;
        if batch.records.len() > MAX_BATCH_RECORDS as usize {
            return Err(SinkError::TooLarge);
        }
        match st.last_ack {
            // Already applied: acknowledge the highest sequence so far and apply nothing.
            Some(last) if batch.seq <= last => Ok(AckSeq::new(last)),
            _ => {
                st.applied += batch.records.len();
                st.last_ack = Some(batch.seq);
                Ok(AckSeq::new(batch.seq))
            }
        }
    }
}

#[async_trait]
impl SentinelProbe for FakeSentinelSink {
    async fn applied_records(&self, id: &SentinelIdentity) -> usize {
        Self::applied_records(self, id)
    }
}
