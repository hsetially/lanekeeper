# Plan 09 blockers

Status on 2026-10-25, second T1 run (after huggingface.co and grype.anchore.io were allowed): **B2 is resolved. B1 is
narrowed but still open. B3 is new.** T1 is not done until B1 and B3 are closed.

## B1 (T1, open): the ONNX graph cannot be downloaded, so the model lock is still UNRESOLVED

**What changed.** huggingface.co now answers (the API and `resolve/` both return 200 or a redirect). But Hugging Face
serves LFS files through its CDN: `resolve/<rev>/onnx/model.onnx` answers `302` to
`https://us.aws.cdn.hf.co/xet-bridge-us/...`, and the egress proxy refuses that host (`connect_rejected`, 403 to CONNECT,
recorded in the proxy's `recentRelayFailures` at 2026-10-25T11:21:32Z and 11:21:53Z). The redirect target is Hugging
Face's choice, so neither `update-model-lock.sh` nor the Dockerfile's `model` stage can avoid it. No other host was
tried.

`deploy/images/update-model-lock.sh main` therefore stops at the first file (`curl: (22) ... 403`) and leaves the lock
unchanged. The lock was not filled in by hand, and the hash Hugging Face reports for the ONNX file was not copied into it:
the script's rule is that every file is downloaded and hashed here and only then compared with Hugging Face's own hash.

**What was resolved, for the human who finishes it** (none of this is in the lock yet):

| Item | Value | How it was checked |
|---|---|---|
| repo | `BAAI/bge-small-en-v1.5` | plan A3 |
| revision (`main` today) | `5c38ec7c405ec4b44b94cc5a9bb96e735b38267a`, "onnx support (#9)" | `/api/models/BAAI/bge-small-en-v1.5/revision/main` and `/commits/main` |
| date | `2024-02-22T03:36:23Z`, about 975 days old (rule 15 needs 14) | the same responses |
| `tokenizer.json` | sha256 `d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66`, 711396 bytes | downloaded and hashed here; `git hash-object` equals Hugging Face's blob id `688882a7…` |
| `config.json` | sha256 `094f8e891b932f2000c92cfc663bac4c62069f5d8af5b5278c4306aef3084750`, 743 bytes | the same; blob `3992bf89…` |
| `special_tokens_map.json` | sha256 `b6d346be366a7d1d48332dbc9fdf3bf8960b5d879522b7799ddba59e76237ee3`, 125 bytes | the same; blob `a8b3208c…` |
| `tokenizer_config.json` | sha256 `9261e7d79b44c8195c1cada2b453e55b00aeb81e907a6664974b4d7776172ab3`, 366 bytes | the same; blob `37fca747…` |
| `onnx/model.onnx` | **not downloaded.** Hugging Face reports LFS sha256 `828e1496d7fabb79cfa4dcd84fa38625c0d3d21da474a00f08db0f559940cf35`, 133093490 bytes | reported only (`/api/models/.../tree/<rev>/onnx`, `x-linked-etag`, `x-linked-size`); not verified |

**What a human needs to do.** Either allow `us.aws.cdn.hf.co` (and whatever other `*.cdn.hf.co` host Hugging Face
redirects to) for the session and re-run T1, or on a machine that can reach it:

```
deploy/images/update-model-lock.sh 5c38ec7c405ec4b44b94cc5a9bb96e735b38267a
git diff deploy/images/hub-model.lock    # five files; compare with the table above
```

CI on GitHub-hosted runners reaches the CDN, so the image build there works once the lock is committed. After the lock is
in: `just images-repro` (hub), `just images-sbom hub`, `just images-scan hub`, and `just images-policy-test`, whose last
step (`model-lock.mjs check` on the committed lock) is the one that fails today.

## B2 (T1): resolved

The scan now runs. Two defects in `scan.sh` were found and fixed while doing it:

1. **The grype container could not reach the database.** It ran on Docker's bridge network with no proxy, so the lookup of
   `grype.anchore.io` went to 8.8.8.8 and failed (`no such host`), even with the host allowed. `scan.sh` now takes the
   same settings as `build.sh`: `LK_BUILD_NETWORK` goes to `--network`, `LK_BUILD_CA` is mounted read-only as the
   container's CA bundle (`SSL_CERT_FILE`), and `HTTPS_PROXY` and `NO_PROXY` are passed by name when set. CI sets none
   of them.
2. **The scan reported the planted log4j finding by its GHSA id** (`GHSA-jfh8-c2jp-5v3q`), so `scan-test.sh` correctly
   failed: it requires `CVE-2021-44228` in the output. `scan.sh` now passes `--by-cve`, so findings are named by CVE id
   where they have one. That also matches `grype.yaml`, whose exceptions are written with CVE ids. The test was not changed.

Evidence (grype v0.120.1, digest-pinned in `tools.env`; database v6.1.10 built 2026-10-25T06:27:34Z, checksum
`sha256:4bdb4292…a713ea`, cached in `target/grype-db`):

- `just images-scan-test`: `scan-test: the planted critical finding (CVE-2021-44228) fails the scan` (exit 0). The scan
  itself exits 2: `log4j-core 2.14.1 ... CVE-2021-44228 Critical`, `discovered vulnerabilities at or above the severity
  threshold`.
- Mutation: with `--fail-on critical` removed from `scan.sh`, the test fails (`the planted critical SBOM passed the
  scan`, exit 1). Restored, it passes again.
- `shellcheck -x` (koalaman/shellcheck v0.11.0) is clean on `scan.sh`, `update-model-lock.sh` and `scan-test.sh`.

## B3 (T1, new): the agent image fails its scan on two critical findings in the distroless base

`just images-scan agent` (image at commit `f02523a`, `sha256:b1755533…`) exits 1. Both critical findings are in
`gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79…`, the base this plan pinned. The crates have none: the agent's
merged SBOM (1580 components) adds no finding above the base's, and the hub's crate SBOM alone (431 components, from
`cargo cyclonedx`) has no vulnerabilities at all. The hub image uses the same base, so its scan will fail the same way.

| Package | Installed | CVE | CVSS | Fix | What it is |
|---|---|---|---|---|---|
| libc6 | 2.36-9+deb12u14 | CVE-2026-5450 | 9.8 | won't fix in Debian 12 | `scanf` with `%mc` and a width over 1024 can overflow the heap by one byte |
| libssl3 | 3.0.20-1~deb12u2 | CVE-2026-75803 | 9.1 | 3.0.22-1~deb12u1 | ChaCha20-Poly1305 and AES-OCB decryption of an empty ciphertext through `EVP_Cipher()` can skip the tag check |

There is no newer cc-debian12 to move to. The pinned digest is the newest `nonroot` index in
`gcr.io/v2/distroless/cc-debian12/tags/list`. It was uploaded 2026-08-21, and it now also carries the tag
`deprecated-public-image-9dac0a79…`: distroless no longer updates Debian 12.

Facts that bear on the options (read from the agent image's own binary):

- `/usr/local/bin/agent` needs only `libgcc_s.so.1`, `libm.so.6`, `libc.so.6` and `ld-linux-x86-64.so.2` (`readelf -d`).
  It never loads `libssl.so.3` or `libcrypto.so.3`, which are in the base but unused. TLS is rustls with aws-lc-rs.
- It imports `__isoc99_sscanf`, but `strings` finds no `%mc`, `%ms` or `%m[` format in it. That suggests the libc CVE is
  not reachable, but it does not prove it.

**Options. A human chooses; I made no change to the base, the tests or `grype.yaml`.**

1. **Move the runtime to `gcr.io/distroless/cc-debian13:nonroot`.** Candidate:
   `sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2`, uploaded 2026-10-01 (24 days old, so it
   meets rule 15). The newer digests from 2026-10-20 onwards are too young. Scanned with the same database: **no
   critical findings** (11 high, all `won't fix` or unfixed in gcc-14, libc6 and zlib1g). The catch is that the build
   stage is `rust:1.88.0-bookworm`, which the plan fixes. A bookworm-built binary (glibc 2.36 symbols) runs on trixie's
   glibc 2.41, but the agent test asserts that build and runtime share a Debian release. So this needs:
   - either a plan change to `rust:1.88.0-trixie@sha256:…` for the build stages, or acceptance of the mismatch;
   - Extra-paths for `crates/agent/tests/hardening.rs` and `crates/agent/HARDENING.md`. Both assert or name
     `cc-debian12` and are not in this plan's Extra-paths. `crates/hub/tests/hardening.rs`, `crates/hub/HARDENING.md`,
     both Dockerfiles and `deploy/policy/dockerfile_test.rego` are already T1's.
2. **Except both CVEs in `deploy/images/grype.yaml` as not applicable**, with the reasons above and a review date. This
   keeps a base that will get no more fixes. `grype.yaml` itself says a human approves every exception.
3. Both: move to cc-debian13 now, and keep option 2 as the rule for future won't-fix findings.

Recommendation: option 1, with the trixie builder, because the cc-debian12 line is deprecated and every future fix
lands only in cc-debian13.

## What this run proved, and what is left

Proved (Docker 29.9.2, its built-in BuildKit, 4 vCPUs, `LK_BUILD_CA=/root/.ccr/ca-bundle.crt LK_BUILD_NETWORK=host`):

- `reproducible-check.sh agent` at commit `f02523a`, with the build cache emptied first: `reproducible: agent
  sha256:b175553354141423395634222f4ca3a9b4cbeaf83d3bd2287bbcdbebc1aa8e67` (also recorded in `REPRODUCIBILITY.md`);
- `just images-sbom agent`: `agent.cdx.json` with 1580 components;
- `just images-scan-test`: pass (B2 above);
- `just images-scan agent`: runs, and fails on B3;
- `just images-policy-test`: lock tests 25/25, Rego unit tests 22/22, the planted Dockerfile triggers every expected
  rule, both Dockerfiles pass the policy (30/30), `cargo test -p hub --test hardening` 5/5,
  `cargo test -p agent --test hardening` 5/5, and the model-stage test accepts the matching lock and refuses all seven
  planted mismatches. Its last step, the check of the committed lock, fails as designed while the lock is UNRESOLVED.

Left: B1 (lock, then hub reproducibility, hub SBOM and hub scan) and B3 (a human decision, then a re-scan of both
images).
