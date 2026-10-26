# The sentinel's package (RPM, systemd unit, audit rules)

Prompt 09 (T8) builds, signs and tests the package; prompt 17 writes the daemon in it. The binary is a stub today (`fn main() {}`, plan 09
A18), so "starts" proves that the unit loads and that the process runs under every restriction, and nothing about auditd records.

The sentinel runs on each NFS VM (Rocky Linux 8.9 and 8.10), reads auditd's records of writes under the export through the audisp af_unix
plugin, and tells the hub who made a change: paths, operations, times, OS Login names and executable names, never file content (D72, S25).
It listens on no port. It is installed **only through configuration management**, which also enables and starts it: the package does neither.

## What is here

| File | What it is |
|---|---|
| `Dockerfile.build` | Builds the static `x86_64-unknown-linux-musl` binary (`--locked`, stripped, paths rewritten, and a check that it has no interpreter and no shared library) and the RPM, and exports both. Not an image to run. |
| `lanekeeper-sentinel.spec` | The RPM: the user, the files, the scriptlets. Requires `audit >= 3.0`. |
| `lanekeeper-sentinel.service` | The unit (S25 and plan 09 A15). Only directives that the systemd of Rocky 8.9 and 8.10 reads. |
| `audisp/af_unix.conf` | Installed as `/etc/audit/plugins.d/lanekeeper-af_unix.conf`: the plugin writes to `/run/lanekeeper-audisp/audit.sock`. |
| `lanekeeper-sentinel.tmpfiles.conf` | Makes `/run/lanekeeper-audisp` at every boot: `root:lanekeeper-sentinel`, `0750` (plan 09 A17). |
| `audit/lanekeeper.rules.in` | The audit rule **template**, `-w @EXPORT_ROOT@ -p wa -k lanekeeper`. It goes to `/usr/share/lanekeeper-sentinel/audit/`; whoever knows the export roots renders one rule for each into `/etc/audit/rules.d/`. |
| `RPM-GPG-KEY-lanekeeper.pub` | The release key's public half. **Not a key yet**: a marker (below, B13). |
| `build.sh`, `sign.sh`, `sbom.sh`, `publish.sh` | Build, sign (GPG), CycloneDX SBOM, cosign. |
| `tests/` | `install.sh`, `exposure.sh`, `sign-test.sh`, `publish-test.sh`, `reproducible.sh`, `unit2json.mjs` (the unit parser for the policy) and `audit-stand-in.spec`. |

The policies are in `deploy/policy/systemd.rego` (S25, on the unit) and `dockerfile_artifact.rego` (on `Dockerfile.build`).

## Commands

```
just sentinel-check     # no RPM, no key: the policies on the unit and on Dockerfile.build, with their planted cases (part of verify-09)
just sentinel-build     # target/sentinel/unsigned: the binary and the unsigned RPM; and the image with rpm and gpg
just sentinel-repro     # two --no-cache builds give one binary and one RPM
just sentinel-sign      # a throwaway key signs it; the SBOM is written and scanned
just sentinel-test      # all of the above, then: install on Rocky 8.9 and 8.10, the exposure score, the cosign script
```

`sentinel-test` needs Docker, a privileged container that boots systemd, and the Rocky repositories (for the real `audit` package).
`LK_SENTINEL_AUDIT=stand-in just sentinel-test` uses `tests/audit-stand-in.spec` instead, for a place that cannot reach them; the test says so
in capitals, and CI never does. Behind a TLS-intercepting proxy the same settings as `deploy/images/build.sh` apply (`LK_BUILD_CA`,
`LK_BUILD_NETWORK`, `HTTPS_PROXY`), and `LK_APT_MIRROR` names an https Ubuntu archive.

## What is pinned, and where it was read

| Pin | Value | Read from |
|---|---|---|
| Rust | `rust:1.88.0-trixie@sha256:f2a17efb...` (the agent's and hub's builder) | the registry, T1 |
| musl standard library | `rust-std-1.88.0-x86_64-unknown-linux-musl.tar.xz`, sha256 `0c42b63c...` (released 2025-06-26) | a download made for T8, hashed here; equals the `.sha256` file Rust publishes |
| rpm tools | `ubuntu:noble-20260930.1@sha256:68ee2200...`, `rpm=4.18.2+dfsg-2.1build2`, `gpg` and `gpg-agent` at `2.4.4-2ubuntu17*` (installed: 17.6) | the registry; Ubuntu's package index (rpm's version is that of the release pocket, which does not change; gpg's is the upstream version, and the glob leaves the archive's security rebuilds in) |
| Rocky Linux | `rockylinux:8.9@sha256:b1282187...` (systemd 239-78.el8) and `8.10@sha256:e8a49c54...` (239-82.el8) | the registry, on docker.io and mirror.gcr.io (same digests) |
| Newer systemd-analyze | `almalinux/9-init:9.8-20261002@sha256:85b089b5...` (systemd 252-67) | the registry |

**The tools image (stage `rpm-tools`): what is pinned and what is not.** Pinned: the base by digest; `rpm` to one exact version (it builds
the package, so it is part of what the package is); `gpg` and `gpg-agent` to the upstream version, `2.4.4`, with a glob over the Ubuntu
revision (`2.4.4-2ubuntu17*`, quoted), because the archive removes a superseded revision from its index, so an exact pin would fail the
build at every security update, and a tool that signs should take them; a new upstream version fails the build until a person changes the
pin. NOT pinned: the libraries they load (`libgcrypt20`, `libgnutls30t64`, `libassuan0`, `libnpth0t64`, `libarchive13t64`, `libsqlite3-0`
and the rest of the 124 packages in the image), which float inside `noble`, `noble-updates` and `noble-security`; apt checks all of them
against the Ubuntu archive keyring and no other repository is configured. Recorded: the build prints every installed version, and the image
keeps the list in `/usr/share/lanekeeper-tools-packages.txt` (`docker run --rm lanekeeper-rpm-tools cat /usr/share/lanekeeper-tools-packages.txt`),
so the log of a release build says what signed it. As built on 2026-10-26: `rpm 4.18.2+dfsg-2.1build2`, `gpg`, `gpg-agent` and `gpgconf`
`2.4.4-2ubuntu17.6`, `libgcrypt20 1.10.3-2ubuntu0.2`, `libgnutls30t64 3.8.3-1.1ubuntu3.6`, `libassuan0 2.5.6-1build1`,
`libarchive13t64 3.7.2-2ubuntu0.9`. The RPM does not depend on the floating packages for its bytes except through `rpm`'s own
libraries; the reproducibility test is what would notice them changing the RPM. Renovate moves the digests (after 14 days) and never the
Rocky releases, which are the targets; it cannot move the apt versions, so those are a person's.

## Signing (plan 09 A16): no key is stored

- **A pull request** is signed with a **throwaway RSA key** made inside a container with no network, for that run (`sign.sh ephemeral`). The
  secret half exists only in a tmpfs until the command ends. The signature shows that the package is intact; it says nothing about who
  built it. Its public half goes with the artifact so that `rpm -K` and dnf's check can be tried with a key.
- **A release** (`sentinel-v<version>` tag) is signed with the **release key**. Its private half is in Secret Manager. The `sign-release`
  job, in the protected `release` environment, reads it through Workload Identity Federation and pipes it into the signing container's
  memory (`sign.sh release`), which refuses unless its fingerprint is that of `RPM-GPG-KEY-lanekeeper.pub`, and which checks the result with
  that file. The key is never on the runner's disk. That job compiles no crate and installs nothing from a language registry: the SBOM of
  the signed RPM is written and scanned by the later `sbom` job, which holds no token (`check-workflows.mjs` W6 refuses a build command in
  any job with `id-token: write`). What the job does run is `build.sh --tools`, before it authenticates: a docker build of the
  `rpm-tools` stage of `Dockerfile.build`, whose only step is `apt-get install` of rpm and gpg from Ubuntu's signed archive (as root in a
  build container; the maintainer scripts of Ubuntu's packages run there). That stage depends on no other, so the sentinel is not compiled
  in that job: `dockerfile_artifact.rego` refuses a change that gives the stage a dependency or a build tool, and `tests/tools-graph.sh`
  (`just sentinel-tools-graph`, run by the `policy` job) reads BuildKit's own plan of the target. (An earlier version built `--target rpm`,
  which copies the binary from the `binary` stage, so it did run `cargo build` and every crate's build script there.) Then
  `publish` signs the RPM and that SBOM keyless with cosign (`publish.sh`), verifies both from the same job, attaches a provenance
  attestation and creates the GitHub release.
- The signature is **RSA** with SHA-256, a kind the rpm 4.14 of Rocky 8 verifies (the install test shows it; no other key type was tried). The key has no subkey, so there is one fingerprint to compare.
- dnf **does not check the signature of a local file** unless `localpkg_gpgcheck=1` is set; `install.sh` sets it and shows that an unsigned
  package and a package with a changed byte are refused. Config management should set it, or check with `rpm -K`.

### B13: the release key does not exist, and nothing here makes it up

`RPM-GPG-KEY-lanekeeper.pub` is a marker until a person makes the key and commits the public half:

```
deploy/sentinel/build.sh --tools
deploy/sentinel/sign.sh new-release-key --out deploy/sentinel/RPM-GPG-KEY-lanekeeper.pub |
  gcloud secrets versions add <secret> --data-file=-
```

The secret key goes to standard output once, and nowhere else. Then set the repository variables `LK_RPM_WIF_PROVIDER`,
`LK_RPM_SERVICE_ACCOUNT` (which may read that one secret and nothing else, and is not the account that signs images) and
`LK_RPM_SIGNING_SECRET`, create the `release` environment (required reviewers, tags `sentinel-v*` only), and make the Workload Identity
pool admit only this repository's `sentinel-v*` tags in that environment. None of that is in the repository. Until then a release tag stops
at `sign-release`, and pull requests are signed and verified with throwaway keys.

## The exposure score (S25, plan 09 A15)

`systemd-analyze security` does not exist in systemd 239 (it arrived in 240, and `--offline` in 252), so `tests/exposure.sh` scores the
packaged unit with the systemd-analyze of AlmaLinux 9.8 (systemd 252). A score reached with directives that Rocky 8 ignores would mean
nothing, so the same script runs `systemd-analyze verify` in the Rocky 8.9 and 8.10 images, which prints `Unknown lvalue` for such a
directive **and exits 0**, so its output is what is checked.

**Measured: 1.7** (limit 2.0), at the commit of T8. What is counted against the unit, and why:

| Not set | Cost | Why |
|---|---|---|
| `PrivateNetwork` | 0.5 | the sentinel talks to the hub |
| refusing internet sockets, refusing local sockets | 0.3, 0.1 | it needs both (`RestrictAddressFamilies` is set to exactly those two kinds) |
| `ProtectClock`, `ProtectKernelLogs`, `ProtectProc` | 0.2 each | unknown to 239 (checked: `Unknown lvalue` on 8.9 and 8.10) |
| `ProtectHostname`, `ProcSubset` | 0.1 each | unknown to 239 |
| `PrivateUsers` | 0.2 | not set: a user namespace is for prompt 17 to prove against the OS Login name lookups before it is turned on |
| `IPAddressDeny` | 0.2 | the hub's address is configuration, not known at install |
| `RootDirectory` | 0.1 | |

**`RestrictSUIDSGID` is set although upstream 239 does not have it**: Rocky's 239-78.el8 and 239-82.el8 backport it (`verify` is silent for it
on both). That is a measurement of those two builds, not a promise about any other EL8 systemd.

## What the install test shows, and what it cannot

On Rocky 8.9 and 8.10, with the pinned images and the systemd of each: `rpm -K` refuses the package before the key is imported and after a
byte changes; dnf refuses the unsigned and the changed package; dnf refuses the package until `audit` is there; the user, the files, their
modes and `rpm -V`; the unit under `systemd-analyze verify` (silent); `systemctl start` and the journal; and **a probe** run through the same
unit in place of the binary, which shows what the restrictions do to a real process: it runs as the sentinel's user with every capability
set empty, `NoNewPrivs`, a system call filter that refuses `nice(2)`, a read-only `/etc`, `/usr` and any other directory the user owns, a
writable spool, empty `/home` and `/root`, a private `/tmp` and `/dev`. Removal and reinstall.

Each check was shown to fail on a worse unit (a directive removed, one that 239 does not know added): see the T8 notes in
`plans/09-deployment-ci.blockers.md`. **Not proven here**, and for CI or prompt 17:

- **The real `audit` package.** The Rocky repositories (`dl.rockylinux.org`, `mirrors.rockylinux.org`) are refused in the sandbox that wrote this,
  so the install test ran with a labelled stand-in that only provides `audit` and its directories. CI uses the real one (`--audit repo`); that path
  has never run.
- **cgroup v2 hosts.** The sandbox runs cgroup v1; GitHub's runners run v2. systemd 239 supports the unified hierarchy, but the boot of
  the Rocky 8 containers on v2 has not been seen.
- **The audisp plugin and its socket (plan 09 A17).** The plugin runs as root and cannot set a group, so the configuration relies on the
  `0750 root:lanekeeper-sentinel` directory and an open socket mode (`0666`). Whether audit 3.0.7 (8.9) and 3.1.2 (8.10) accept that argument and
  create the socket there is for prompt 17, against the daemon. SELinux (Rocky runs it enforcing) has no policy module here: the plugin writing
  into `/run/lanekeeper-audisp` and the sentinel connecting may be denied.
- **A C toolchain for musl.** The build uses `cc` (glibc's gcc) as the linker, which is enough for a crate with no C code. A dependency with C code
  (`aws-lc-rs` for the sentinel's mutual TLS) needs a musl C compiler, which `Dockerfile.build` does not have, because `deb.debian.org` is refused where
  this was written and nothing could be installed and tested. Prompt 17 adds it with its first such dependency.
- **The daemon.** A18: the start test runs a stub that exits at once, and `Restart=on-failure` does not restart a clean exit. The unit's `ExecStart` takes
  no arguments and the unit reads no configuration file yet; prompt 17 adds both (and the policy allows arguments on `ExecStart`).
