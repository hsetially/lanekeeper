# Plan 09 blockers

## B1 (T1): the embedding model lock cannot be resolved from the implementer's environment

**What blocks.** T1 pins the hub's embedding model (A3: `BAAI/bge-small-en-v1.5`) by Hugging Face commit, size and sha256 in
`deploy/images/hub-model.lock`. The implementer's sandbox cannot reach `huggingface.co`: the egress proxy refuses the
CONNECT (`connect_rejected`, 403) for every Hugging Face host. So neither the revision nor any file hash could be read, and
none was guessed.

**State left.** Everything around the lock is in place and tested:

- the `model` stage of `crates/hub/Dockerfile`, which fetches only `https://huggingface.co/<repo>/resolve/<revision>/<path>`
  and checks every file's size and sha256 (`sha256sum -c`);
- `deploy/images/model-lock.mjs`, the checker, with 25 tests (`node --test deploy/images/tests/*.test.mjs`);
- `deploy/images/tests/model-stage-test.sh`, which runs the Dockerfile's own `model` stage script against synthetic locks
  with a curl shim: a matching lock passes, and a wrong sha256, a wrong size, an unresolved or branch revision, a path that
  leaves `/models`, a repo with a host and an empty lock are each refused (and a mutation that removes the checks is caught);
- `deploy/images/update-model-lock.sh`, which resolves a revision, downloads and hashes each file, cross-checks the LFS
  hashes Hugging Face reports, writes the lock and runs the checker (including the 14-day age rule).

The committed lock says `revision UNRESOLVED`. The checker refuses it, and the hub image build fails in the `model` stage on
it. Nothing skips.

**What a human needs to do.** On a machine that can reach huggingface.co:

```
deploy/images/update-model-lock.sh main      # or a specific commit at least 14 days old
git diff deploy/images/hub-model.lock        # review: five files, the revision, its date
```

Then commit it on this branch (or let the next T1 run commit it). After that, `just images-repro` can be run for the hub.

## B2 (T1): the vulnerability database is not reachable from the implementer's environment

`deploy/images/scan.sh` runs grype (digest-pinned image). grype downloads its database from `grype.anchore.io`, which does
not resolve in the sandbox. `deploy/images/tests/scan-test.sh` therefore fails here, correctly: it requires the planted
CVE-2021-44228 to appear in the output, so a scan that fails for lack of a database does not count as a pass. It needs to
be run where `grype.anchore.io` is reachable (CI on GitHub-hosted runners is).

## What T1 proved here, and what is left

Proved in the sandbox (Docker 29.9.2, commit `8563b6c`): the agent image is reproducible (two `--no-cache` builds,
`sha256:167548e1…`); the hub image without its `model` stage is reproducible (`sha256:2810c71c…`); both binaries start
on distroless (the hub refuses by design, A1); the agent SBOM is produced (301 crates from cargo-cyclonedx plus the image's
packages and files); the Rego tests (22), the planted Dockerfile through conftest's parser (12 rules), both Dockerfiles
under the policy, the crates' hardening tests and the lock tests (25) pass.

Left for a run with network access to huggingface.co and grype.anchore.io: resolve the lock (B1), then
`just images-repro` for the hub, `just images-sbom hub`, `just images-scan hub|agent` and `just images-scan-test` (B2).
