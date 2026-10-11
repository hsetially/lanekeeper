# 03b blockers

## T7 is blocked on plan A16: no `gix` HTTPS transport passes `cargo deny` as the repository is configured

Nothing in the worktree was changed except this file. All measurements below were taken in a throwaway copy of `HEAD`
(`git archive`, outside the repository) with `gix` added to `crates/hub-platform`. No `deny.toml` edit and no direct `reqwest` dependency was made
in the worktree.

### What A16 asked for

A `gix` transport feature whose TLS is rustls on aws-lc-rs and that passes `cargo deny`, with `cargo tree -i ring` reported. If none fits, escalate.

### What exists (gix 0.87.1 with gix-transport 0.59.2; gix 0.88.0 and 0.89.0 have the same feature set)

`gix-transport` offers exactly these HTTPS backends:

| Feature | Result |
|---|---|
| `blocking-http-transport-reqwest-rust-tls` | rustls with aws-lc-rs through `reqwest` 0.13.5 and `rustls-platform-verifier` 0.7.1. **`cargo deny check` fails: `licenses FAILED`**, one error. See below. |
| `blocking-http-transport-reqwest-rust-tls-trust-dns` | The same, plus hickory-dns. Same failure. |
| `blocking-http-transport-reqwest-native-tls` | OpenSSL or the platform TLS. Violates S6 and the stack (rustls only); `openssl` is a banned crate in `deny.toml`. Not tried. |
| `blocking-http-transport-curl-openssl` | C libcurl and OpenSSL. Same objection. Not tried. |
| `blocking-http-transport-curl-rustls` (`curl/rustls`) | libcurl (C, built by `curl-sys`) linking `rustls-ffi`: a second rustls instance behind a C ABI, not the workspace's aws-lc-rs build. Read from the feature table only; not resolved or built. |
| `blocking-http-transport-curl` (no TLS feature) | No HTTPS. |

### The failure (reqwest + rustls, the only candidate that matches "rustls with aws-lc-rs")

```
error[rejected]: failed to satisfy license requirements
  webpki-root-certs-1.0.9/Cargo.toml: license = "CDLA-Permissive-2.0"  (rejected: license is not explicitly allowed)
  webpki-root-certs v1.0.9
  └── rustls-platform-verifier v0.7.1
      └── reqwest v0.13.5
          └── gix-transport v0.59.2
              └── gix v0.87.1
advisories ok, bans ok, licenses FAILED, sources ok
```

`deny.toml` deliberately leaves CDLA-Permissive-2.0 out (the comment on `sqlx` in `Cargo.toml` says so for `webpki-roots`).

Cause, from `rustls-platform-verifier-0.7.1/Cargo.toml`: `webpki-root-certs` is a dependency only for `target_arch = "wasm32"` (and a dev-dependency). On Linux the verifier uses
`rustls-native-certs` and `rustls-webpki`. The error appears only because `deny.toml`'s `[graph]` has no `targets`, so `cargo deny` walks the dependency graph of every platform, wasm32 included.

### Facts for the decision

- With `gix` on the reqwest rustls feature, for the host target: `cargo tree -p hub-platform -i ring -e normal` prints nothing, so `ring` is **not** in `hub-platform`'s graph; the only crypto provider is aws-lc-rs 1.18.1 (aws-lc-sys 0.45.0).
  `ring` is already in the workspace lockfile through `bollard` (testcontainers, dev only) and `kube` and is untouched by this.
  `openssl-sys` and `native-tls` are absent; `webpki-root-certs` is absent on Linux; `quinn` is absent on Linux. Roots come from `rustls-native-certs` (already pinned) through the platform verifier.
- Dependency growth for `hub-platform`'s normal graph: 260 to 409 lines of `cargo tree --prefix none | sort -u` (gix's crates, `reqwest`, `hyper-rustls`, `rustls-platform-verifier` and their small trees). `reqwest` would be in the lockfile, but only as gix-transport's HTTP client, on the same single rustls/aws-lc-rs stack that the hub uses elsewhere. It is not a second TLS stack.
- In the throwaway copy, adding `targets = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]` to `[graph]` in `deny.toml` made `cargo deny check` report `advisories ok, bans ok, licenses ok, sources ok`. I did not test any other `deny.toml` change.
- Pins: gix 0.87.1 (2026-08-24, 48 days old) or 0.88.0 (2026-09-25, 16 days old); 0.89.0 (2026-10-08) is too new. The toolchain is 1.88, which all of these accept.

### Options (a human decides; none is applied)

1. **Narrow `[graph].targets` in `deny.toml` to the Linux targets the hub and agent ship on, and enable `blocking-http-transport-reqwest-rust-tls`.** Smallest change; removes a class of false positives (wasm32 and Android-only crates) for every crate. Cost: `deny.toml` stops checking Windows and macOS dependency graphs (`xtask` and the agent are built on Windows under WSL2 only, per the docs, so this does not look like a loss, but a human should confirm). It also needs the plan's `Extra-paths: deny.toml` scope widened beyond the one `aws-lc-sys` exception that the approved plan describes. This also contradicts the instruction not to add `reqwest`, because reqwest comes in transitively; I need an explicit yes.
2. **Add one licence exception** `[[licenses.exceptions]] crate = "webpki-root-certs"  allow = ["CDLA-Permissive-2.0"]`. Same effect for this build and the same reqwest note. It is narrower in `deny.toml`'s own terms but admits a licence the project has avoided so far, for a crate that is not compiled on our targets.
3. **A small own `gix_transport::client::blocking_io::http::Http` implementation** over a blocking rustls/aws-lc-rs client that we already use (`GoogleHttp`'s TLS config and `rustls-native-certs`), handed to `gix` through `Remote::to_connection_with_transport(Transport::new_http(..))`. No `reqwest`, no `deny.toml` change, one TLS stack. Cost: code we maintain (HTTP request and response streaming for the smart-HTTP protocol, redirects, auth header injection, timeouts). The plan said it did not propose a hand-written fetch because that is protocol code; this one is only the HTTP carrier (gix keeps the git protocol), but it is still a deviation from A16 and from the plan's list of files for T7, so it needs approval. I have not tried it and have no size measurement.
4. **Do not use a `gix` HTTPS transport for T7.** Mirrors fetch over the `file` transport from a path that another component populates (for example `git clone --mirror` run by an init container or the GitHub App token flow in prompt 06). It moves a requirement from 03b to the deployment prompt and changes P2's webhook-to-fetch path; not recommended.

My recommendation is option 1 (or 2 if the human would rather keep `[graph]` untouched), because the reqwest route keeps one TLS stack on aws-lc-rs and the only obstacle is a licence on a wasm32-only crate. Both options need `deny.toml` and the "no reqwest" instruction to be revisited by a human, which is why I stopped.

### State of T7

No T7 code was written. T7 can start as soon as the A16 decision is made; the tests for the webhook, payload, queue, poll, reader and tree index do not depend on the HTTPS transport (they use the file transport, as the plan says). The pending pieces that do depend on it are only the `Mirror`'s `https` remote handling and the credential hook wiring.
