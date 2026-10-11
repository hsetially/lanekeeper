//! Event bus and SSE (T5, D66, D80, P13).
//!
//! - [`wire`]: the outbox envelope and the event id.
//! - [`relay`]: the cursor and overlap window that read new rows exactly once.
//! - [`source`], [`visibility`], [`sse`], [`metrics`]: the SSE endpoint, and where its events come from.
//! - [`fake`]: an in-memory source for tests of the endpoint (`fakes` feature).

pub mod bus;
#[cfg(any(test, feature = "fakes"))]
pub mod fake;
pub mod listen;
pub mod metrics;
pub mod outbox;
pub mod relay;
pub mod source;
pub mod sse;
pub mod visibility;
pub mod wire;
