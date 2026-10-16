//! Confirmation of writes (D52).
//!
//! T2 had the gate in its one safe form: no write tool acts, each answers `outcome: draft_link` with a link to the web UI, where the
//! person confirms. T3 puts the real thing in front of the write: [`gate`] asks the user through MCP elicitation and proceeds only
//! on an exact accept ([`decision`]); [`draft_mode`] is the old form, kept for a deployment that asks for it and for a client that
//! did not declare elicitation support. The question's text is built in [`message`] from server data only, the keys that make a
//! retry safe are in [`idempotency`], and [`replay`] remembers finished writes for ten minutes so that a retry is not asked twice.

pub mod decision;
pub mod draft_mode;
pub mod elicit;
pub mod gate;
pub mod idempotency;
pub mod message;
pub mod replay;

pub use elicit::{Answer, Asker};
pub use gate::{Action, Plan, gate, map_outcome};
