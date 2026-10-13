//! The blob store on Postgres and the endpoint that serves it (T8, D47, P7, S11, S12).
//!
//! A blob is the bytes of one version of a file, addressed by its SHA-256. It never changes, so everything about serving it is
//! simple and fast: a cache that never needs invalidating, an `ETag` that is the address, and a browser cache header that says
//! "immutable".
//!
//! - [`store`]: `PgBlobStore`, the `BlobStore` port over the `blobs` table, with batched reads and one read per cold blob however
//!   many users ask for it at once.
//! - [`cache`]: the byte-weighted cache in front of it (512 MiB by default).
//! - [`endpoint`]: `GET /api/v1/blobs/{hash}`, and the policy seam through which the registry (05) withholds denied files.
//! - [`metrics`]: counters with closed label sets.
pub mod cache;
pub mod endpoint;
pub mod metrics;
pub mod store;
