# Testing on a Windows machine

The Rust workspace is Linux-shaped: the agent's scanner reads Unix-only file metadata through cap-std (`ino`, `dev`, `ctime`) to decide what changed, the agent targets NFS mounts, kubelet signal handling and distroless containers, and CI runs on Linux. A native Windows build of `crates/agent` fails with about 48 compile errors (Unix-only cap-std metadata APIs), and no Windows-native path is planned. **Test through WSL2.**

Everything below was set up and proven on 2026-10-11: `just verify-02` passed in about 40 minutes, with every prompt-02 budget inside its limit (P1 9597 ms of 15000, P3 672 of 1024 bytes/min, P4 11.2 ms of 20000, P4.stat_walk 4.6 of 1000 ms, P4.cpu_mcores 4 of 50, P4.memory_mib 23.6 of 64, P15 287,831 of 1000 versions/s).

## One-time setup

**1. WSL2 with Ubuntu 24.04.** `wsl --install -d Ubuntu-24.04`, then open it. Do all Rust work on the Linux filesystem (ext4), never on a Windows drive through `/mnt/...`: the 9p filesystem makes cargo builds and the 80,000-file fixture scans several times slower.

**2. Clone the repo into the Linux filesystem.** Cloning from the Windows checkout is fastest:

```bash
mkdir -p ~/working && git clone /mnt/d/working/lanekeeper ~/working/lanekeeper
cd ~/working/lanekeeper
git remote set-url origin https://github.com/hsetially/lanekeeper.git
```

**3. System packages.** `sudo` is needed once. Go is in the list because CI runners ship it and `aws-lc-rs` (rustls' crypto provider) can need it when it builds from source:

```bash
sudo apt-get update && sudo apt-get install -y build-essential clang cmake pkg-config protobuf-compiler golang-go
```

**4. Rust and the pinned gate tools.** These match the versions in the `Justfile`; `tools-check` and `fuzz-tools-check` fail with the exact install command when a version is wrong.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
source ~/.cargo/env
rustup component add rustfmt clippy            # the minimal profile leaves them out; agent-lint needs both
rustup component add rust-src --toolchain nightly-2026-09-14
cargo install --locked cargo-deny --version 0.19.9
cargo install --locked cargo-audit --version 0.22.2
cargo install --locked just --version 1.53.0
rustup toolchain install nightly-2026-09-14 --profile minimal
cargo +nightly-2026-09-14 install --locked cargo-fuzz --version 0.13.2
```

The workspace toolchain (Rust 1.88.0) comes from `rust-toolchain.toml` and rustup installs it on the first build.

## Running the gates

From Windows, or from any shell:

```bash
wsl -d Ubuntu-24.04 -e bash -lc 'cd ~/working/lanekeeper && source ~/.cargo/env && just verify-02'
```

Inside WSL, `cd ~/working/lanekeeper && source ~/.cargo/env` first, then the normal commands (`just verify-02`, `cargo test -p agent --lib --tests`, and so on).

Rough wall times from the proven run: full first build and fixture generation dominate the first run (about 40 minutes for `verify-02`; later runs are faster because `target/` is warm). `verify-02` runs `agent-tools-check`, `agent-fixtures`, `agent-lint`, `agent-test` (742 tests), `agent-slow-test`, `agent-fuzz` (6 targets, 30 s each) and `agent-bench`, in that order, stopping at the first failure.

`just verify` needs one more tool not listed above: pnpm 10 and Node 22 for `web-verify` (see `web/.nvmrc`), plus `pnpm install --frozen-lockfile` in the repo root for `buf` and `redocly`. This part has not been proven from WSL2 yet; on Windows, Docker Desktop with the WSL2 backend provides Docker to the WSL distro for `db-verify`.

## Testing the agent locally

There is no standalone demo binary; the fake hub, fake Kubernetes API, fake metadata server and fake config-server live in `crates/agent/tests/support/` and compile only into the integration tests. The tests are the local test bed.

**Watch the full loop in real time.** The one test that runs on a real directory in real time and is worth pointing a human at:

```bash
cargo test -p agent --test scan_real a_file_written_on_disk -- --nocapture
```

Files (CRLF + BOM, binary, a symlink, an NFS silly-rename leftover) are written to a temp directory, found by the real cap-std walk, and arrive at the fake hub over real TLS 1.3 gRPC byte-for-byte, within about 10 seconds.

**P1 latency numbers.** The real-timing test (10 s walk, 3 s quiet period, 30 s maximum deferral, about 2 minutes):

```bash
just agent-slow-test
```

**Running the binary itself.** It needs the eleven required `LK_*` variables (`crates/agent/src/config.rs`, top table): `LK_HUB_ENDPOINT`, `LK_HUB_AUDIENCE`, `LK_HUB_CA_FILE`, `LK_SWIMLANE`, `LK_CLUSTER`, `LK_PROJECT`, `LK_NFS_SERVER`, `LK_NFS_EXPORT`, `LK_NFS_ROOT`, `LK_CERT_SECRET`, `LK_NAMESPACES`. It also needs a Kubernetes config: `kube::Client::try_default()` failing is a hard exit 1, so point `KUBECONFIG` at a dummy kubeconfig. A temp directory stands in for the NFS root; `LK_SPOOL_DIR` and `LK_TMP_DIR` must exist and must not be inside `LK_NFS_ROOT`. With the hub unreachable the agent stays up in its join backoff loop (5 s base, 60 s cap) and serves:

```
curl localhost:9090/healthz     # ok, or unhealthy: <loop names>
curl localhost:9090/readyz      # not ready: starting|connecting|disconnected
curl localhost:9090/metrics     # lanekeeper_agent_* Prometheus series
```

Logs are JSON lines on stdout; `LK_LOG=debug` raises the level (kube targets stay silenced; S10). Exit codes: 0 orderly shutdown (SIGINT/SIGTERM), 1 stopped on failure, 2 could not start.

## What still works on Windows itself

The Windows side of the machine only needs the pinned `cargo-deny`, `cargo-audit` and the nightly toolchain for crates that do compile there. `crates/agent` (and anything that depends on Unix file metadata) does not; run it through WSL2 as above.
