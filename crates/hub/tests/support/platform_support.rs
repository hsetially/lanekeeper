//! The platform's own test clients, included as they are (plan 18, T3): a CA over a software KMS key with a clock the test moves, a
//! gRPC client over real mutual TLS, and a fake agent that speaks the real protocol. They are the nearest thing to an agent that
//! the repository has, and the hub's wiring is proved against the same clients as the platform's code is.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../../hub-platform/tests/support/ca.rs"]
pub mod ca;
#[path = "../../../hub-platform/tests/support/clock.rs"]
pub mod clock;
#[path = "../../../hub-platform/tests/support/fake_agent.rs"]
pub mod fake_agent;
#[path = "../../../hub-platform/tests/support/grpc.rs"]
pub mod grpc;
#[path = "../../../hub-platform/tests/support/soft_kms.rs"]
pub mod soft_kms;
