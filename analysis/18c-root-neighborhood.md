# Exact radius-one neighbourhood of one 18-cell root

## Contents

- [Scope](#scope)
- [State and neighbourhood](#state-and-neighbourhood)
- [Algorithm](#algorithm)
- [Artifact](#artifact)
- [Completed corpus](#completed-corpus)
- [Reproduction](#reproduction)
- [Interpretation](#interpretation)

## Scope

The `--root-neighborhood` mode of `thermo-18c-beam` exhausts the saturated
radius-one neighbourhood of one exact 18-cell network across **all** Sudoku
solutions of that network. A root may be a record in the frozen 71-seed corpus
or an exact network record selected from the pinned audited round-two landscape.
The initial production root is frozen seed 42:

```text
network SHA-256   000686520eb98f01cfee9ef0be013e1d3758bbb258add2b09866094ae31fd7ae
exact solutions   128
```

This is a local exhaustive search, not an enumeration of all 18-cell networks.
A negative result excludes a unique network only in the declared root's
neighbourhood. It does not exclude another root, a two-cell exchange, or a
longer path through the search graph.

## State and neighbourhood

A state starts from an 18-cell footprint `C` and a complete Sudoku solution
`T`. It contains every target-true strict comparison between king-adjacent
cells of `C`. All 18 cells must be incident. The state is Hasse-reduced and
canonicalized under the eight D4 grid symmetries and simultaneous digit
complement/global edge reversal.

Network equality is the exact canonical Hasse edge vector. SHA-256 values are
record identifiers only.

For every solution `T` of the root, the radius-one neighbourhood contains:

1. the saturated state on the unchanged root footprint; and
2. every footprint `C - {r} + {a}`, for all 18 choices `r in C` and all 63
   choices `a not in C`.

Thus each target has `1 + 18 * 63 = 1,135` raw moves. Seed 42 has exactly
`128 * 1,135 = 145,280` raw moves. An added cell with no retained
king-neighbour cannot be incident and is rejected without constructing the
state; this is exactly equivalent to the coverage test, not a search
restriction.

## Algorithm

1. Strictly load the tracked 71-seed artifact, checking its fixed byte length,
   SHA-256, schema, record order, exact Hasse identities, canonical states, and
   accounting.
2. Rebuild all 71 solvers and exactly replay their declared counts. These
   same-run results form an exact count cache.
3. Select a frozen root by seed ordinal, network SHA-256, or both. Alternatively,
   select exactly one exact network record by SHA-256 from either the pinned
   64,925,329-byte round-two JSONL artifact or a completed radius-one v1/v2
   artifact. The historical round-two input has a built-in byte and SHA-256
   pin; a completed-neighbourhood input requires its whole-file SHA-256 via
   `--root-record-sha256`. The loader hashes one immutable byte buffer, parses
   that same buffer, and rejects missing or duplicate matches, lower-bound
   scores, incomplete source summaries, and noncanonical record state.
4. For an external root with stored exact count `N`, independently count to
   `N + 1` and require an uncapped exact result of `N`. This result becomes the
   exact cache entry for the declared root. A frozen root is already covered by
   the 71-seed replay.
5. Independently enumerate the root to its declared exact count. Require an
   exhausted, uncapped result; sort and deduplicate the grids; validate every
   complete Sudoku and every root comparison.
6. Generate all 1,135 moves for every sorted target. Saturate, require exact
   18-cell incident coverage, Hasse-reduce, canonicalize, and deduplicate by
   the exact Hasse vector.
7. Reuse only exact counts replayed in steps 2 and 4. Count every other distinct
   network directly to the configured cap, in canonical Hasse-key order. An
   uncapped result is exact; a cap hit is only a lower bound. A generating
   target makes an exact zero an invariant failure.
8. Stop classification immediately on an exact count of one and publish the
   positive witness. Otherwise classify the entire generated set. With a cap
   of at least two, completion without an exact one proves that this declared
   neighbourhood contains no unique network.

There is no node budget, timeout, Monte Carlo estimate, fallback solver, beam
selection, or target sampling in this mode.

## Artifact

Frozen roots use schema `thermo-18c-root-neighborhood-v1`. External records use
the distinct schema `thermo-18c-root-neighborhood-v2`, which additionally binds
the source artifact's canonical path, bytes, whole-file SHA-256, source schema
and algorithm revision, authentication mode, and selected line. Output is
published atomically without replacing an existing file. JSONL record order is:

1. a header binding the seed artifact, executable, root, count cap, identity,
   generation order, classification order, and local proof scope;
2. every root solution in lexicographic order with a one-based ordinal;
3. every distinct network in exact canonical Hasse-key order; and
4. a terminal conservation summary.

Each network record contains its exact key, canonical representative state,
score and cap relation, occurrence count, all generating target ordinals, and
one deterministic replay origin. The origin names a root-solution ordinal and
either the same-footprint move or the removed and added cells in the declared
root's canonical coordinates. A verifier can reconstruct the state from that
solution and move without trusting the SHA identifier.

The summary partitions raw moves into radius rejection, coverage rejection,
and accepted observation; accepted observations into distinct and duplicate
networks; networks into exact, lower-bound, and terminally unclassified
states; and solver calls into seed replay, root enumeration, and direct network
counts. The v2 summary separately accounts for the 71 seed replays, external
root exact replay, root enumeration, external-root and frozen-seed cache hits,
and direct network counts. It also lists every exact state and all improvements
over the root.

## Completed corpus

The all-solutions radius-one neighbourhood is complete for every exact root in
the two initial constructive archives:

| Root set | Roots | Root solutions | Raw moves | Global canonical networks | Result |
| --- | ---: | ---: | ---: | ---: | --- |
| Frozen seed corpus | 71 | 34,290 | 38,919,150 | 47,949 | No unique; none below 128 |
| Beam-generated exact roots | 138 | 373,022 | 423,379,970 | 91,941 | No unique; none below 128 |
| Combined archive union | 209 | 407,312 | 462,299,120 | 136,912 | No unique; none below 128 |
| New 304- and 410-solution descents | 2 | 714 | 810,390 | 1,431 | No unique; no child below 129 |

The two root sets overlap in 2,978 canonical networks. The combined union has
552 exact counts and 136,360 lower bounds at their declared caps. The only
strict descents from a frozen root are 605 to 410 and 514 to 304; neither beats
the global 128-solution incumbent. Closing both descended roots across all 714
of their solutions found no continuation below 129; their two neighbourhoods
contain 1,431 distinct networks with 37 in common.

The tracked compact evidence is the
[frozen-71 summary](18c-frozen71-radius1-cap4096-summary.json) and the
[generated-138 summary](18c-generated138-radius1-cap129-summary.json). The two
follow-up descents are retained in the
[304/410 summary](18c-descents304-410-radius1-cap129-summary.json). The full
JSONL artifacts remain under ignored `runs/`.

## Reproduction

Build from the repository root on Windows:

```text
cargo build --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-18c-beam
```

Run the seed-42 neighbourhood at cap 4,096:

```text
thermo-sudoku-rs/target/release/thermo-18c-beam.exe --root-neighborhood --seeds analysis/18c-seeds-v1.jsonl --root-seed-ordinal 42 --root-network-sha256 000686520eb98f01cfee9ef0be013e1d3758bbb258add2b09866094ae31fd7ae --count-cap 4096 --output runs/18c-root-seed-42-radius1-cap4096-v1.jsonl --progress-every 1000
```

Run the exact 560-solution network from the audited round-two landscape:

```text
thermo-sudoku-rs/target/release/thermo-18c-beam.exe --root-neighborhood --seeds analysis/18c-seeds-v1.jsonl --root-record-input runs/18c-beam-round2-landscape4096-v1.jsonl --root-network-sha256 9c7213a6b17d41ad74b5c3ef33b92af58f9da754f770a4e20277c1d9661996aa --count-cap 4096 --output runs/18c-root-560-radius1-cap4096-v2.jsonl --progress-every 1000
```

Run the exact 304-solution descent from the authenticated seed-2
neighbourhood:

```text
thermo-sudoku-rs/target/release/thermo-18c-beam.exe --root-neighborhood --seeds analysis/18c-seeds-v1.jsonl --root-record-input runs/18c-root-seed-2-radius1-cap4096-v1.jsonl --root-record-sha256 7200eef43617c2fdd17d981fbdd295d279fe56c17e95ca356a30ce66f0041d7e --root-network-sha256 b02c3e9aa72ccb7b21759c397a423384b3c3b28d2bf71db31c40e40d54ae2124 --count-cap 129 --output runs/18c-root-304-radius1-cap129-v2.jsonl --progress-every 250
```

On non-Windows systems, omit `.exe`. The output path must not exist. An
external root requires the network SHA-256 and cannot be combined with a seed
ordinal. Completed-neighbourhood inputs additionally require their whole-file
SHA-256; the legacy pinned round-two input does not.

Verify the implementation with:

```text
cargo fmt --all --manifest-path thermo-sudoku-rs/Cargo.toml -- --check
cargo test --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-18c-beam
cargo clippy --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-18c-beam -- -D warnings
```

## Interpretation

Each run measures the complete one-step landscape around its declared exact
root without the beam search's target sampling. Seed 42 is the 128-solution
incumbent used for the first run; the external-record selector permits the
same experiment around exact descendants such as the 560-solution round-two
state. A lower exact child supplies a deterministic descent direction. A
completed negative run shows that reaching uniqueness from that root requires
at least a different root, more than one footprint exchange, or a move outside
the declared saturated radius-one model.
