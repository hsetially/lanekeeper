# Reviewer agent checklist

Run this against a PR produced by an implementing agent. Post the findings as one review comment, with a section per heading below. Mark each finding as blocking or non-blocking. Approve only when there are no blocking findings.

## 1. Scope

- **Ownership.** Every changed path is owned by the PR's prompt, or is a test or fixture under owned paths.
- **Contracts.** Contract paths are unchanged, unless the PR is labelled `contract-change`.
- **Plan.** The approved plan exists in `plans/`, and the commits follow its task order (`NN/Tk:`).

## 2. Evidence

Re-run the following yourself. Don't trust the PR text alone.

- `just verify-NN`
- `just verify`
- `cargo xtask bench-check`

Then confirm:

- Every acceptance criterion is ticked, with evidence: test names, benchmark numbers or command output.
- Every P# in the prompt has a registered benchmark or load test. The numbers are within budget, and each benchmark measures the real hot path, not a trivial stand-in.

## 3. Security

For each S# the prompt lists, find the code that implements it and the test that proves it. Then check the code for:

- secrets or file contents in logs, errors, metrics or responses;
- `unwrap`, `expect` or panics outside tests;
- `unsafe`;
- blocking calls inside async code;
- unbounded channels, collections, queries or retries;
- missing timeouts;
- handlers without a role guard;
- writes without an expected hash;
- audit events missing, or written outside the transaction;
- user input reaching a regex, a path or SQL without the typed validators;
- raw HTML rendered in the web app;
- new dependencies: each one justified, passing `cargo deny`, and at least 14 days old.

Also confirm that the PR updated `docs/threat-model.md` for the components it touches.

## 4. Correctness

- Tests check behaviour, not implementation details. Look for tautological assertions and over-mocked tests.
- Property tests and fuzz targets exist where the prompt requires them.
- Error paths are tested, not only the happy path.
- Engine outputs are deterministic. The same inputs must give byte-identical outputs.

## 5. Open questions and assumptions

- Every open question the prompt touches is handled with its stated default and is configurable.
- Each assumption the PR lists is reasonable and recorded.

## Verdict

State **APPROVE** or **CHANGES REQUESTED**, then list the blocking findings first.
