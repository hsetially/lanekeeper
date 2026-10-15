//! Confirmation of writes (D52).
//!
//! T2 has the gate in its one safe form: no write tool acts, each answers `outcome: draft_link` with a link to the web UI, where the
//! person confirms. T3 adds the real thing in front of it, the elicitation (`confirm::elicit`), and keeps this form for draft mode and
//! for a client that did not declare elicitation support.

pub mod gate;

pub use gate::{Plan, gate};
