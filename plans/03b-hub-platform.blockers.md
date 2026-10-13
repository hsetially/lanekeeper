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

## Resolved: A16, option 1 (decided by the human at the gate)

Decision: option 1. Narrow `[graph].targets` in `deny.toml` to the two Linux targets and enable gix's `blocking-http-transport-reqwest-rust-tls`.
`reqwest` arriving transitively through `gix-transport` is explicitly accepted. It is not a direct dependency of any crate and no code of ours uses it.
No licence exception was added, and nothing else in `deny.toml` changed.

What was changed (T7 commit `03b/T7: Git mirror and webhooks`):

- `deny.toml`: `[graph] targets = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]`, with a comment. The Windows and macOS dependency graphs are no longer checked by `cargo deny`.
- Workspace `Cargo.toml`: `gix = "=0.87.1"` (published 2026-08-24, 48 days old at the time of the change, re-checked on crates.io on 2026-10-11; 0.88.0 is 16 days old, 0.89.0 is 3 days old and too new),
  `default-features = false`, features `sha1`, `max-performance-safe`, `revision` and `blocking-http-transport-reqwest-rust-tls`. `crates/hub-platform/Cargo.toml` uses `gix.workspace = true`.

Checks after the change:

- `cargo deny check`: `advisories ok, bans ok, licenses ok, sources ok`. The one warning, `license-exception-not-encountered` for `libfuzzer-sys`, was there before (that crate lives in the separate fuzz workspace).
- `cargo tree -p hub-platform -i ring -e normal` (and with `--target x86_64-unknown-linux-gnu` and `--target aarch64-unknown-linux-gnu`): prints nothing, so `ring` is not in `hub-platform`'s graph.
- aws-lc-rs 1.18.1 remains the only crypto provider in that graph (rustls, tokio-rustls, hyper-rustls and rustls-platform-verifier all use it).
- `openssl`, `openssl-sys` and `native-tls` are not in `Cargo.lock` at all. `openssl-probe` (a certificate-path probe, no cryptography) was already reached through `rustls-native-certs`.
- `webpki-root-certs` is now in `Cargo.lock` (wasm32-only dependency of `rustls-platform-verifier`) but is outside the checked graph, which is why `licenses` passes.

For the PR text: reqwest is a transitive dependency (gix-transport's HTTP client), and `deny.toml`'s graph is narrowed to Linux.

## CI is red on two timing races in `crates/agent` (owned by prompt 02); fixing them here needs Extra-paths

2026-10-13. PR #7's `rust` job fails on `crates/agent/tests/binary.rs:72`
(`binary_serves_health_while_it_waits_for_a_certificate_and_exits_zero_on_sigterm`):
the child exits by signal, not code 0. Main fails the same way (merge of PR #9, run 38303091082), and
`crates/agent` is untouched in this PR — the races are pre-existing, and GitHub's slower runners expose them.

1. **The SIGTERM race** (the reported failure). The health port binds first (`process.rs`), but the
   shutdown future installs its signal handlers only on its first poll, which happens in `assemble`'s
   `select!` — after the kube client, CA load, transport and spool. A SIGTERM in that window is the
   default kill: `status.code() == None`. Reproduced 10/10 locally by mimicking the test; the module
   doc ("a shutdown signal at any of these points ends the process in an orderly way") promises otherwise.
   Fix: poll the shutdown future once at the top of `process::run`, before the port binds.
2. **The `kube_watch` log race** (`kube_watch.rs:310`, once on main: run 38278983056, `[]`). Not a
   timing race. `tracing` fixes per callsite, for the rest of the process, whether its events are
   dispatched at all: the interest is computed once, at the callsite's first hit, against whatever
   subscriber the hitting thread has then, and `set_default` never recomputes it. In a test binary
   there is no global subscriber, so a callsite first hit on a thread without a capture is cached
   "never" and stays silent in every later capture. `a_failing_list_is_retried_and_the_report_follows`
   hits the agent's watch-failure `warn!` (lists fail 500/503) without installing a capture, so whether
   `no_full_report_while_the_lists_are_refused_and_the_log_has_only_the_status` sees its own line
   depends on which test registers the callsite first: deterministic failure at `--test-threads=1`
   (confirmed), ~15% under four threads (6/40 and 3/30 pinned to 2 vCPUs). The failing captures show
   the signature: kube-rs's 403 `warn!` (only hit on `Forbidden`, which the poisoning test's 500s do
   not reach) arrives, but every callsite the poisoning test hit first — the agent's `warn!`,
   `kube_client`'s debug, `stream_backoff`'s debug — is missing. Waiting for the line could not have
   fixed it. Fix: `tests/support/log_capture.rs`'s `install` now installs a once-per-process global
   keep-alive subscriber (answers "always", drops events), so no first hit is ever evaluated against
   `NoSubscriber`, and rebuilds the interest cache per install, so callsites poisoned before it are
   re-enabled. Threads without a capture still drop their events when they dispatch, so capture-less
   tests are unaffected. A binary that uses `install_global` must not also use `install`, because the
   first `install` claims the global default; no binary mixes them today.

The human approved fixing both on this branch (conversation, 2026-10-13): Extra-paths `crates/agent/**`
was added to the plan header for two commits, both tests-only or startup-order only, no behaviour of the
hub platform touched.

### Evidence after the fixes

- SIGTERM: the reproduction script (mimicking the test: SIGTERM once `/healthz` answers) went 10/10
  `code() == None` before, 10/10 orderly 0 after; `--test binary` 10/10 before the session's other
  changes and 5/5 again after.
- `kube_watch`: `--test-threads=1` failed deterministically before the `log_capture` fix, passes after;
  40 batches at `taskset -c 0,1 ... --test-threads=4`: 0 failures (was 6/40 in the same loop).
- `cargo test -p agent`: 55 test binaries, all green — the keep-alive does not expose any test that
  was passing because a poisoned callsite silenced a line.
- `cargo fmt --all`, `cargo clippy -p agent --all-targets -- -D warnings`: clean.
- Known limitation, unchanged: a SIGTERM before `run()`'s first poll — between `exec` and the runtime
  hand-off — is still the default kill; the module doc now states the handlers are installed at the
  first poll, inside `run`.
