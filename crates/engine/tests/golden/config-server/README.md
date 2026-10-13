# Config-server fixtures (synthetic)

Reconstructed from the inputs and outputs that `.claude/skills/lanekeeper-config-semantics/references/worked-examples.md`
documents for the config-server's own test data (plan decision A3). They are made up, not copies: the originals live in the
config-server repository, which is not available here. `tests/golden_config_server.rs` reads the files by name, so byte-exact
originals can replace these without a code change.

- `config/` is the config root: `application.properties`, `application-bis.properties`, the resource `tx-infinity-core.yml`,
  the resource `entryGroupConfig.yml` and its tenant copy in the channel folder `remoteteller/`.
- `expected/` holds what the config-server serves for each request, which the engine's rendering must equal after line endings
  are normalised.
