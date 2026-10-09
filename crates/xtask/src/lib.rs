//! Developer tasks (prompt 01, T8): the budget registry, `bench-check` and the synthetic fixture generator.
//!
//! The binary in `main.rs` is a thin command-line shell around these modules so that the integration tests can call
//! the same code.
#![forbid(unsafe_code)]

pub mod bench_check;
pub mod budgets;
pub mod cli;
pub mod fixtures;
