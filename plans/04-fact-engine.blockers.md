# 04 blockers

## T3: `P6.index_settings` (40 ms p95) is not met with tree-sitter on the hot path: 45.8 ms

Everything else in T3 is done and green (tests, fuzz target, real-snapshot harness, benchmark). The one thing that is not is a
budget the plan proposed itself, so it is not registered (`registered = false` in `perf/budgets.toml`) and `bench-check` lists
it as UNMET with the number. Nothing was loosened: the threshold in the file is still the plan's 40 ms. A human decides which
way to go (options at the end); the entry is then registered by flipping one flag and adding the id to `engine_budgets` in the
Justfile.

### The budget

Plan A1 / the table in "Design notes": `engine/index_settings_6000_lines`, `P6.index_settings`, p95, 40 ms, "threshold is my
proposal" (the prompt gives no number for it). The benchmark is the 6,000-line YAML file of swimlane 1 of the target fixtures
(`catalog-big-01.yml`: 6,173 lines, 145,981 bytes, CRLF, a merge key and an alias in every item), end to end as the hub runs it
for a unique blob: decode, parse, flatten, then tree-sitter reading the same text and being checked against the parser.

### Measurement

Command: `just engine-bench` (release build, `cargo bench -p engine`, then `cargo xtask bench-check`). Machine: 4 vCPU, shared,
load average about 1.

```
PASS  P6.parse_flatten                          7.995 p95 <= 20 ms
FAIL  P6.index_settings                        45.811 p95 <= 40 ms  over budget
```

Criterion's own estimate for the same run: `[36.7 ms 37.7 ms 38.7 ms]`. Other runs of the benchmark gave a mean of 35 to 40 ms; the
machine is noisy (tree-sitter's parse alone varies between 14 and 37 ms across runs of a standalone loop).

Where the time goes (medians over 328 iterations, after the optimisation below; a timer in `confirm` that was removed again):

| Stage | median | min | p95 |
|---|---|---|---|
| decode, parse and flatten with the Rust parser (`P6.parse_flatten`: criterion mean 6.9 to 7.7 over three runs) | about 7 | n/a | 8.0 (bench-check) |
| tree-sitter parse of the file (the C library, `tree-sitter-yaml` 0.7.2) | 17.7 | 13.4 | 22.0 |
| tree-sitter tree turned into the parser's tree shape (`TreeBuilder`) | 5.1 | 3.3 | 6.7 |
| `flatten` of that tree, and the comparison | 1.1 | 0.6 | 1.3 |

tree-sitter alone, standalone loop over the same bytes (60 runs, release): min 14.4 to 17.3 ms, median 21 to 26 ms, p95 30 to 37 ms.
The CRLF and an LF copy of the file parse equally fast (22.8 against 24.0 ms median), so line endings are not the cost. The
progress callback is called 1,139 times for the file; it is not a cost either.

### What was tried

- One shared `TreeCursor` for the whole walk and numeric node and field ids instead of names: the translation went from 7.4 ms to
  5.1 ms median. This is in the commit.
- Nothing can make the C parse faster from our side: it is the grammar's external scanner and lexer.
- Best case, adding the lowest value seen for each stage: about 7 + 13.4 + 3.3 + 0.6 = 24 ms. The median case is about 31 ms and
  the p95 is 45.8 ms, because the machine's noise lands mostly on the C parse.

So 40 ms at p95 cannot be met reliably while tree-sitter reads every file in full on this machine. It is not a fault of the
translation any more.

### Why it does not threaten P6 itself

At target scale the structured files are 67,107 files and 76 MB, but only 20,026 unique blobs and 23.7 MB (77 blobs of 60 KB or
more, 11 MB of it). The cost is per unique blob and per byte: about 0.05 ms per KB for the Rust parser and about 0.12 ms per KB
for tree-sitter, so indexing every unique blob once costs about 4 to 5 CPU-seconds in all, against a nightly budget of 180 s on
2 vCPU. The 6,000-line file is the worst single case, not the common one.

### Options (a human picks; none is taken here)

1. **Raise the threshold of `P6.index_settings` to 60 ms (p95).** Keeps the design (tree-sitter always on, every file checked).
   `perf/budgets.toml` and the plan's frozen table row change, so the parity test row in `crates/xtask/tests/budgets_registry.rs`
   changes with them. Measured p95 is 45.8 ms, so 60 ms leaves room for the machine's noise.
2. **Take tree-sitter out of `index_settings` and make it a separate `confirm_locations` check.** Today a row always keeps the
   parser's range, whatever tree-sitter says (a disagreement only adds a warning), so `index_settings`' rows would be identical.
   The hub would run the check in tests and CI, and on a sample or nightly (prompt 05 decides), not on every blob. The
   `P6.index_settings` budget of 40 ms is then met by a wide margin (about 8 ms). This changes the prompt's wording, "locations
   come from tree-sitter", to "are checked by tree-sitter".
3. **Run tree-sitter only on files up to a size** (for example 64 KB), and use the parser's ranges alone above that, with the
   benchmark on a file under the cap. Most real config files are far smaller than the 146 KB fixture. It keeps the check where
   it is cheap, but leaves the biggest files unchecked, which are the ones where a wrong line matters most for a person.

Recommendation: option 1. The 40 ms was a guess made before anything was measured, nothing downstream depends on it, and the
nightly cost is small; option 2 is the choice if the check should stay out of the request path altogether.

### Other things found while doing T3 (for the record, all fixed in the T3 commit)

- The parser (T2) gave a block scalar one line too many when the next token was indented (`  c: |` followed by `  d: 2`), started
  a block scalar in a sequence or at the root one line late, counted the next token's line for an empty block scalar, and got an
  empty block scalar at the very end of the text wrong. The cross-check caught all four (the hand-written table, the random
  documents of `tests/locations.rs` and the fuzzer); the regression test is `block_scalars_are_on_the_lines_from_their_indicator_to_their_last_line_of_text`.
- Known and tested disagreements that are left as they are, each falling back to the parser's range with a
  `LocationMismatch` warning: a block scalar used as an explicit key (`? |`), a tag with no content (`[!]`), an anchor name that
  contains a quote, a `...` before the first document, a byte order mark inside a plain scalar, and a last bare key of a flow
  mapping whose closing bracket is on a later line (the parser puts its implicit null at the bracket, one line late).
