# Plan 18: the human's answers at the approval gate

The plan (`plans/18-hub-assembly.md`) is approved and frozen. This file records what the human decided about the questions it marked "needs a human". Implementers follow this file where it differs from the plan's defaults.

Approved on 2026-10-26 by the repository owner: the plan, its `Extra-paths:` and its contract change (CC1, two additive budget entries).

## Decisions that change or confirm the plan's default

| Question | Answer | What it means for the implementer |
| --- | --- | --- |
| **G2. Ending an SSE stream at logout** | **Per-session, via the small `hub-platform` change** (the plan's default). | The three `hub-platform` Extra-paths are in use: `crates/hub-platform/src/session_auth.rs`, `crates/hub-platform/src/events/sse.rs`, `crates/hub-platform/tests/events_sse.rs`. Keep the change to what the plan lists under "Extra paths and contract changes" (item 3 to 5); `SessionAuth` gains the two provided methods, the SSE handler uses them. T4 owns it. |
| **G8. The body cap** | **Relax S11 for the two edit routes** (`PUT /files`, `POST /drafts`), not the plan's default of keeping 4 MiB everywhere. | The per-route table in `src/limits.rs` gives those two routes the 12 MiB + 64 KiB cap (`MAX_EDIT_BODY` in `hub-writes`). The cap applies **only to a request that carries a live session**: the check moves after `AuthLayer`, the per-IP bucket stays first, and an unauthenticated request is still capped at the global 4 MiB. Every other route stays at 4 MiB; `/hooks/github` is 1 MiB. Add the `docs/decisions.md` entry that relaxes S11 for exactly these two routes (and says why: file edits up to 2 MiB whose JSON escaping can double them, plus headroom), and write `body_limits::the_12_mib_cap_needs_a_live_session`. Prompt 18 does not own `docs/security.md`, so the decisions entry is where the relaxation lives; say in the PR that S11's wording needs a follow-up by whoever owns that file. |
| **G20. Merge order with prompt 09** | **Whichever is ready first.** | No ordering constraint. Make additive hunks in `HARDENING.md`, `hardening.rs`, `Cargo.toml`, `Justfile`, `docs/decisions.md` and `docs/threat-model.md` so either order merges cleanly. Do not wait for prompt 09, and do not edit its branch. |

## Everything else

Every other question marked "needs a human" (G1, G3, G4, G5, G10, G15, G16, G19, G23, G24, G28, G29) takes **the plan's proposed default**, as stated in the plan's Ambiguities table. Approving the plan, with its Extra-paths and CC1, accepted those defaults.

CC1 (`perf/budgets.toml` and `crates/xtask/tests/budgets_registry.rs`) is approved; CC2 (a new `AuditAction`) is not proposed and stays out.
