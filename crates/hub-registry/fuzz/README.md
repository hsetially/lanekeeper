# The registry's fuzz targets (S22)

The registry's own cargo-fuzz crate, as the agent's, the identity crate's, the platform's and the engine's are. It is outside the root workspace, with its own `Cargo.lock`, so the main toolchain stays on stable.

| Target | Input | What must hold |
|---|---|---|
| `registry_delta` | bytes read as a program: one to four messages of a scan delta, each with files, removals and skips | the checks a scan delta passes before the hub stores anything never panic and agree with an oracle written from the rules (the first bad entry decides the error); an accepted message equals the entries given; content is kept only for an entry that is not denied, and hashes to its entry; a denied entry keeps no content; no time is out of range; the totals of a logical delta never pass a bound; the plan for the versions of a path chains and never moves the state back |

| `registry_cursor` | bytes read twice: as the text of a cursor, and as a program that builds one (a list kind, the parts of a query, up to eight fields that are numbers or byte strings of 0 to 300 bytes) | parsing any text never panics; a text that parses is a valid cursor of at most 512 characters within the bounds of a key and has exactly one spelling; a key is made exactly when an oracle written from the bounds says it fits, and then it parses back, decodes under its own list and query, is refused under any other, and no prefix of it parses |
| `registry_query` | bytes read as a program: one of the fourteen query parsers, then a raw query string (up to 9,000 bytes) or up to eight `name=value` pairs from the operation's own names, names it does not have, and a pool of awkward values | no parser panics and each answers the same twice; a query over 8 KiB, a name the operation does not have and a name twice are refused; what is accepted is within its bounds (limits, cursors, clean relative paths, lists without repeats, patterns, a content source with exactly its own parameters) and survives being written and read again |

T7 adds `registry_sentinel_path`.

The invariants are in `invariants.rs`. Both the fuzz targets and the stable test `crates/hub-registry/tests/corpus_replay.rs` include that file, so `cargo test -p hub-registry` (and so `just verify`) replays every seed in `corpus/` with the same checks, and proptest feeds it random bytes.

## Run

Needs the pinned nightly and cargo-fuzz from the `Justfile` (`fuzz_nightly`, `cargo_fuzz_version`):

```
cargo +nightly-2026-09-14 fuzz run registry_delta --fuzz-dir crates/hub-registry/fuzz \
    target/fuzz-corpus/registry_delta crates/hub-registry/fuzz/corpus/registry_delta -- -max_total_time=30
```

New inputs go to the first directory, not into `corpus/`; copy a useful one into `corpus/<target>/` by hand with a descriptive name. A crash leaves its input in `artifacts/<target>/`. `just registry-fuzz` runs every target and checks the lockfile with `cargo deny` and `cargo audit`.

## The input of `registry_delta`

A byte stream, zero after its end. The first byte is the number of messages minus one (modulo 4). Each message: the counts of files (modulo 7), removals and skips (modulo 3), then the files, then the removed paths and the skipped paths. A path is a length byte (modulo 24, plus one) and that many bytes, each an index into the alphabet `ab/./-_kepmrivt01` and NUL, so that traversal, empty components and the deny globs (`*.pem`, `*private*`) all occur. A file is its path, a flags byte (denied, has bytes, hash true, size true, and two bits for the time: sane, negative, beyond year 9999, a year ahead), a content length (modulo 40) and the content, and a jitter byte. After the messages come the bytes that describe the stored state of the path the plan is run against.

The seeds hold no secret: the "keys" are the words `key` and `-----BEGIN-----`.

## The inputs of `registry_cursor` and `registry_query`

`registry_cursor` reads the bytes twice. As text (lossily, at most 700 bytes) they are offered to the cursor parser; a text that parses must assemble back to itself. As a program they are: a list kind (modulo 9), a count of query parts (modulo 4) with each part a length and characters from the alphabet, a field count (modulo 9), and the fields, each a tag byte (even: a number, from eight bytes; odd: a byte string whose length class is chosen by the next byte: tiny, small, 253 to 257, or up to 355). The seeds put keys at the bounds (255 and 256 bytes, six and seven fields, two long fields over the raw limit) and cursor texts that are good, changed, padded, cut short and of another version.

`registry_query` takes its first byte as the operation (low four bits, modulo 14) and, with the top bit set, the rest as a raw query string (so any bytes become some text, including fragments, escapes and non-UTF-8 repaired by the lossy reading). Without it, the next byte is the number of pairs (modulo 9); each pair picks a name (the operation's own, or one of four it does not have) and a value from a pool of 40 awkward ones (empty, limits at and beyond their bounds, traversal paths with and without escapes, a NUL and bytes that are not UTF-8 as escapes, lists with empty and repeated items, every source of a content read) or from generated text.
