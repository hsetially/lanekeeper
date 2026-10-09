# Proof patterns per requirement

Each requirement gets a test that would fail if the protection were removed.

| S# | Test shape |
|---|---|
| S1 | Mock OIDC provider: a wrong `tid`, a replayed nonce, an expired token, an unknown key id, and a pending user each get the expected 401 or 403. |
| S2 | Session fixation: the id rotates at sign-in. Idle and absolute expiry. A role change ends sessions on two app instances that share one database. |
| S3 | A cross-origin POST with a valid cookie but a missing or wrong CSRF token or `Origin` gets 403. |
| S4 | The route-table test walks every registered route and asserts a guard whose role matches the OpenAPI `x-required-role`. The MCP tool table gets the same check. |
| S5 | Join with a forged ID token, a wrong service-account email, a reused join token, and an expired certificate all fail. A test asserts no code path loads a CA private key. |
| S6 | A TLS 1.2 client is refused. A certificate for swimlane A presented with `Hello` for swimlane B is rejected. |
| S7 | `SecretSource` is the only reader of runtime secrets, verified with a grep-style test that no other `std::env::var` reads a secret name. |
| S8 | Decrypting with swapped associated data fails. Plaintext is zeroized, tested with a drop-observer type. Classic `ghp_` tokens are rejected. |
| S9 | Altering a row (trigger disabled in the test) is detected by verification. A checkpoint signature verifies. Concurrent writers produce one linear chain. |
| S10 | Log capture across the full end-to-end suite: no file contents, token patterns, or environment values. |
| S11 | Property tests on `NfsPath`. Regex size-limit errors. A 4 MiB body limit. Uploads that aren't UTF-8 Markdown are rejected before reaching storage. |
| S12 | An XSS corpus renders harmless. A Playwright run under the production CSP reports no violations. |
| S13 | A planted private key or `github_pat_` value blocks a save, and the message shows line numbers only. |
| S14, S14b | A client with a wrong audience or scope, or a pending user, is refused. A Viewer can't call write tools. A planted fake key never appears in tool output. |
| S15 | MCP output wraps file text in labelled data blocks (snapshot test). |
| S16, S17 | Policy tests on rendered charts (non-root, read-only filesystem, capabilities dropped). The agent's RBAC has no Secret list. Denied-glob files never put bytes on the stream (stream capture). |
| S18 | From inside pods, egress to a non-allowlisted host fails. |
| S19, S20 | A CI test that every action reference is a commit SHA. `cargo deny` and `osv-scanner` gates. cosign verification of the published image. |
| S21 | Workspace lints. A forbid-unsafe check per crate. |
| S22 | `just fuzz-smoke` runs clean. Every crash becomes a regression test. |
| S25 | `systemd-analyze security` gives an exposure score of 2.0 or lower. The sentinel never opens files under the export root (strace-style test in a container). |
