# Independent 18-cell seed harvest

## Contents

- [Purpose and scope](#purpose-and-scope)
- [Input corpus](#input-corpus)
- [Algorithm](#algorithm)
- [Completed v1 harvest](#completed-v1-harvest)
- [Artifact format and verification](#artifact-format-and-verification)
- [Reproduction](#reproduction)
- [Beam stage](#beam-stage)

## Purpose and scope

`thermo-18c-seed-harvest` constructs a deterministic corpus of low-solution
generalized 18-cell comparison networks. It derives targets and footprints
only from this project's frozen 19-cell search corpus, saturates each footprint
with every available target-true king-neighbour inequality, and counts each
canonical network exactly below a declared ceiling.

This is a constructive seed search, not an enumeration of all 18-cell thermo
Sudokus. A completed run is exhaustive only over its stated parent corpus,
target solutions, and deletion mode. Failure to find a unique seed supplies no
18-cell nonexistence result.

## Input corpus

The v1 discovery input is
[`sources/min_thermos_9_8_2.txt`](../sources/min_thermos_9_8_2.txt), produced by
this project's simulated-annealing search. Philip Newman's corpora, Denis
Berthier's 18-cell collection, and the pictured 11-solution `9+9` puzzle were
not used as seeds.

```text
bytes       108,967
SHA-256     79bec9ad12bf7c3c6cb28948e1c54cd98809929d5fe5a3003a8c6215367046a7
rows        1,280
valid       1,279
invalid     line 1,192 (shared cell 60)
```

Safe D4 plus simultaneous path reversal reduces the valid rows to 1,114
distinct `9+8+2` parents. The harvester independently exhausts each parent and
verifies its declared count. Their 84,531 distinct target solutions have
declared counts at most 848; no parent target is sampled.

## Algorithm

For each canonical parent:

1. Enumerate solutions up to the declared count, require exact exhaustion and
   an exact count match, then sort the target grids lexicographically.
2. Produce an 18-cell footprint. `long-terminals` deletes either endpoint of
   the length-9 or length-8 path. `all-cells` deletes each of the 19 covered
   cells in turn. The original path partition is provenance only after this
   step.
3. For every king-neighbour pair in the footprint with unequal target digits,
   add the strict comparison from the lower digit to the higher digit. Reject
   the candidate unless all 18 cells are incident.
4. Compute the unique transitive reduction. The saturated edge set and its
   Hasse DAG have the same transitive closure and therefore the same Sudoku
   solutions.
5. Transform the network and target together under the eight D4 grid
   symmetries and digit complement. Complement maps `d` to `10-d` and reverses
   every comparison. Select the lexicographically least Hasse relation, then
   the least target on symmetry ties.
6. Deduplicate by the exact canonical Hasse edge vector. SHA-256 identifiers
   label records but are never used as equality proofs.
7. Count each distinct Hasse network to cap `K`. An uncapped count below `K`
   is exact and is emitted. A cap hit means only `solutions >= K` and is
   retained in aggregate accounting, not emitted as an exact seed.

Saturation is lossless for positive discovery on a fixed target and footprint:
if a target-compatible subset were unique, its saturated target-true superset
would retain the target and could not gain solutions.

## Completed v1 harvest

The durable v1 run used `all-cells` and `K = 1024`:

| Measure | Result |
| --- | ---: |
| Verified parents | 1,114 |
| Parent target solutions | 84,531 |
| Deletion attempts | 1,606,089 |
| Rejected for incomplete incident coverage | 132,963 |
| Valid target/footprint states | 1,473,126 |
| Canonical networks | 17,597 |
| Exact counts below 1,024 | 71 |
| Counts at least 1,024 | 17,526 |
| Best exact count | **128** |
| Unique networks | 0 |

The 71 exact seeds cover 24 footprints and two Hasse topology families. The
lowest 34 have counts below 512; the remaining records supply additional
footprint and topology diversity for exploration. This is an independent
starting corpus, not evidence against 18-cell existence.

The retained JSONL corpus is
[`18c-seeds-v1.jsonl`](18c-seeds-v1.jsonl):

```text
bytes       124,417
SHA-256     8e6a11d7d7e7f51dd497e64ca6d9ed7ae492b42fea60d46bb2aeded51f2bf99e
```

The run used executable SHA-256
`81ab5b63b5da021b1aab50efc4d59fe9c81cd3d18c08be15814589c3a21fbd06`.

## Artifact format and verification

The JSONL order is:

1. a header binding the schema, algorithm, input and executable hashes,
   deletion mode, count ceiling, canonicalization, and non-exhaustive scope;
2. explicit invalid-input records;
3. exact seed records in canonical Hasse-key order; and
4. a terminal summary with all conservation counters and completion flags.

Each seed contains its exact count, canonical footprint, saturated and Hasse
edges, canonical target, two solver witnesses where available, deterministic
minimum provenance, occurrence multiplicity, stable identifiers, and solver
statistics.

Verification must check that every target and witness is a valid Sudoku, every
edge is king-local and target-true, exactly 18 cells are incident, saturated
and Hasse closures agree, all 16 symmetry images have the same key, exact
counts replay below the stated cap, and the summary equations hold. Logical
record order is deterministic. Elapsed time and machine paths are descriptive
and can differ between reruns.

## Reproduction

```text
cargo build --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-18c-seed-harvest
thermo-sudoku-rs/target/release/thermo-18c-seed-harvest.exe --input sources/min_thermos_9_8_2.txt --output runs/18c-seeds-v1.jsonl --deletions all-cells --solution-cap 1024 --progress-every 5000
```

On non-Windows systems, omit `.exe`. Output publication is same-directory,
synced, atomic, and no-clobber; choose a nonexistent destination.

Verify the implementation with:

```text
cargo fmt --all --manifest-path thermo-sudoku-rs/Cargo.toml -- --check
cargo test --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-18c-seed-harvest
cargo clippy --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-18c-seed-harvest -- -D warnings
```

## Beam stage

The generalized beam state is a saturated `(18-cell footprint, target Sudoku)`
pair. For each retained target, the radius-1 move replaces one covered cell by
one uncovered cell, resaturates, requires 18 incident cells, and canonicalizes
before consulting the visited set. Different target witnesses for the same
network remain search information even though solution counts cache by network.

If the current best exact count is `B`, exploitation counts new networks only
to `B + 1`; a cap hit cannot improve the incumbent. A deterministic exploration
quota receives a higher exact cap so the beam can cross worse intermediate
states. Every visited network, including cap hits, remains cached.

The two completed rounds classified 9,240 and 12,458 new canonical networks.
Complete recounts to cap 4,096 found best new counts of 518 and 560,
respectively; neither improved the 128-solution incumbent. The method,
artifact identities, and results are recorded in
[Generalized 18-cell beam search](18c-beam-search.md).
