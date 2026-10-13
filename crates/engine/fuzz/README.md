# The engine's fuzz targets (S22)

The engine's own cargo-fuzz crate (plan decision A2). The root `fuzz/` crate belongs to prompt 01 and holds the domain's targets, and `crates/domain/tests/corpus_replay.rs` (a contract path) asserts that it holds only those. This crate is outside the root workspace too, with its own `Cargo.lock`, so the main toolchain stays on stable.

| Target | Input | What must hold |
|---|---|---|
| `engine_bytes` | any bytes | `detect_format`, `classify`, `normalise_eol`, `apply_eol` and `decode_text` never panic; normalising twice is normalising once; LF to CRLF and back is the identity on normalised data; bytes that decode are written back unchanged by `encode_text` |
| `engine_yaml` | text | `parse` and `flatten` never panic, stay within the limits (small ones here: 4 KiB, depth 16, 2,000 nodes) and give the same answer twice; what is accepted is a graph without cycles with its positions inside the text; every setting path is unique and survives decode and encode |
| `engine_json` | text | the same for JSON |
| `engine_properties` | text | the same for `.properties` |
| `engine_index_settings` | a selector byte (YAML, JSON or `.properties`), then any bytes | `index_settings` and `locate` never panic and give the same answer twice; the index is there exactly when `validate` accepts the bytes, with the same settings; every range is inside the text; a YAML file is located by tree-sitter or says that it could not be, and every disagreement names a setting of the file. Agreement itself is not an invariant here (the grammars differ on odd input, and the row then keeps the parser's range with a warning); it is checked on the fixtures and on random documents in `tests/locations.rs` |
| `engine_flatten` | NUL-separated pieces | a path built from any keys and indexes is read back to the same keys and indexes; any text offered as a setting path is refused, or read to something that writes and reads back the same |

The invariants are in `invariants.rs`. Both the fuzz targets and the stable test `crates/engine/tests/corpus_replay.rs` include that file, so `cargo test -p engine` (and so `just verify`) replays every seed in `corpus/` with the same checks. Later tasks add `engine_path_mapping` (T4), `engine_regex` and `engine_secret_scan` (T7), and `engine_render` (T10).

## Seeds

Every corpus has at least 8 seeds (the `engine_index_settings` ones are the parser seeds under a selector byte, plus block scalars, flow pairs, empty items, anchors redefined, document markers, UTF-16, and the odd inputs on which tree-sitter and the parser disagree): valid files, truncated ones, one over the (small) size limit, an alias bomb and a merge-key bomb, deep nesting in flow and block style, wrong encodings, hostile keys (dots, brackets, quotes, control characters, a key over the path limit), and duplicate keys. The seeds hold no real configuration and no secret: they are made up for the purpose (AGENTS.md rule 12).

## Run

Needs the pinned nightly and cargo-fuzz from the `Justfile` (`fuzz_nightly`, `cargo_fuzz_version`):

```
cargo +nightly-2026-09-14 fuzz run engine_yaml --fuzz-dir crates/engine/fuzz \
    target/fuzz-corpus/engine_yaml crates/engine/fuzz/corpus/engine_yaml -- -max_total_time=30
```

New inputs go to the first directory, not into `corpus/`; copy a useful one into `corpus/<target>/` by hand with a descriptive name. A crash leaves its input in `artifacts/<target>/`: add it to the corpus with a name that says what it was, and the replay test keeps it from coming back. `just engine-fuzz` runs every target and checks the lockfile with `cargo deny` and `cargo audit` first.
