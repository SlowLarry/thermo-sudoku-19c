# Generalized 18-cell beam search

## Contents

- [Scope](#scope)
- [Network identity](#network-identity)
- [Search algorithm](#search-algorithm)
- [Results](#results)
- [Artifacts](#artifacts)
- [Reproduction](#reproduction)
- [Interpretation](#interpretation)

## Scope

`thermo-18c-beam` performs bounded constructive search over saturated directed
king-neighbour comparison networks covering exactly 18 cells. Networks may
overlap, branch, merge, or be disconnected; every covered cell must be
incident to a comparison.

The two completed beam rounds exhaust only their declared parent, target, and
one-cell-exchange schedules. They are not an enumeration of all 18-cell
networks and do not support a global nonexistence result. Exhaustive local
searches are documented separately in the
[root-neighbourhood](18c-root-neighborhood.md) and
[seed-42 two-cell-shell](18c-root-two-cell-shell.md) notes.

## Network identity

For an 18-cell footprint `C` and solved Sudoku `T`, saturation adds every
target-true unequal king-neighbour comparison induced by `C`. The result must
cover all 18 cells and is reduced to its unique Hasse graph.

The network key is the exact Hasse edge vector minimized under the eight D4
grid symmetries and digit complement, which reverses every edge. SHA-256 values
identify records but are not equality tests. Solution counts cache by exact
network key. Distinct satisfying target grids remain separate expansion data
because they can produce different neighbouring footprints.

The independent [seed harvest](18c-seed-harvest.md) supplies 71 exact seeds.
Round one authenticates the seed artifact, reconstructs every saturated state,
and replays every stored count. Round two authenticates and reconstructs the
complete round-one archive, then cross-checks its 71 seed records against the
same seed artifact.

## Search algorithm

Both rounds use these moves for every selected parent-target pair:

1. resaturate the unchanged footprint against the target;
2. replace one covered cell by one of the other 63 cells, requiring the added
   cell to be king-adjacent to a retained cell;
3. require complete incidence, Hasse-reduce, canonicalize, and deduplicate by
   exact Hasse key.

A count below its cap is exact; a cap hit is only a lower bound. The target
proves satisfiability, exact zero is an invariant failure, and exact one stops
the search immediately.

### Round one

- archive: all 71 exact seeds;
- frontier: 16 seeds from 12 footprint orbits;
- targets per parent: the representative plus up to three lexicographically
  smallest stored witnesses;
- normal count cap: 129;
- landscape recount cap: 4,096 for every new key;
- deterministic successor schedule: 12 lowest exact eligible states and four
  SHA-bucketed exploration states;
- ceilings: 75,000 new-network counts and 80,000 solver calls.

The fixed schedule contains
`16 * 4 * (1 + 18 * 63) = 72,640` raw observations. Generation,
deduplication, recount, and selection use the tie-breaks recorded in the JSONL
header.

### Round two

Round two reconstructs 9,311 round-one network records and selects 16 parents:

- eight exact generated networks, with parent, footprint-orbit, and topology
  diversity and at most two per resolved parent key;
- four previously unexpanded seeds, with footprint and topology diversity;
- four `>=4096` networks selected for parent, footprint, topology, and Hasse
  density novelty. Lower bounds are never ranked as exact counts.

Every exact parent is fully enumerated; each censored parent supplies the first
4,096 solutions in solver order. Targets are normalized under the parent Hasse
graph's D4/complement stabilizer. Their ternary comparison signatures use all
king-adjacent unordered cell pairs with at least one endpoint in the parent
footprint. Previously expanded signatures are excluded, and eight signatures
are selected farthest-first by maximum minimum Hamming distance.

The 16 parents and eight targets give 145,280 raw observations. Every new key
is first counted to 129, then every cap hit is recounted to 4,096. The
successor schedule contains eight raw-exact-count exploitation slots, one slot
for each local child-to-parent factor band (`<=4x`, `4x..8x`, `8x..16x`, and
`>16x`), and four censored structural slots. Logarithmic scale is used only to
define these multiplicative bands; exact exploitation remains ordered by raw
solution count.

## Results

| Measure | Round one | Round two |
| --- | ---: | ---: |
| Raw observations | 72,640 | 145,280 |
| Geometry/radius rejections | 37,216 | 72,648 |
| Coverage rejections | 1,598 | 2,173 |
| Accepted observations | 33,826 | 70,459 |
| New canonical networks | 9,240 | 12,458 |
| Exact new counts below 4,096 | 72 | 66 |
| New counts at least 4,096 | 9,168 | 12,392 |
| Best new exact count | 518 | 560 |
| Incumbent after round | **128** | **128** |
| Unique networks | 0 | 0 |

Round two probed all 12,458 new networks to cap 4,096. Its 66 exact counts
range from 560 to 4,081; none improves the 128-solution incumbent.

## Artifacts

Compact summaries:

- [round-one landscape summary](18c-beam-round1-landscape4096-v1-summary.json)
- [round-two landscape summary](18c-beam-round2-landscape4096-v1-summary.json)

The full JSONL artifacts are retained outside Git under `runs/`:

| Artifact | Bytes | Whole-file SHA-256 | Records 2 through EOF SHA-256 |
| --- | ---: | --- | --- |
| `18c-beam-round1-landscape4096-v1.jsonl` | 16,269,145 | `d3b7571679a63c4e50085433b05d082000f8683d32baac0fa0564f5e6e1cdc6b` | `a0e80b9224714df4279670e4a6de0ea1612578df41bf6dda68fd5e7f56ff0577` |
| `18c-beam-round2-landscape4096-v1.jsonl` | 64,925,329 | `d1b080f52243775fba5446c0a84c0dfd6a14b705f16b471e57820f433922763a` | `866064a652202a069b00821860908bb18278c5e58f2d2a27e6bb2ee45873a67f` |

The records-2-through-EOF digest encodes each record as UTF-8 followed by one
LF and is independent of the absolute executable path stored in the header.
The round-two header records executable size 763,904 bytes and SHA-256
`5ffbe7d608395ba0793613c9a18727f26048e752e9e7201e8070cfde7dc70e6e`.

## Reproduction

From the repository root on Windows:

```text
cargo build --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-18c-beam
thermo-sudoku-rs/target/release/thermo-18c-beam.exe --seeds analysis/18c-seeds-v1.jsonl --output runs/18c-beam-round1-landscape4096-v1.jsonl --progress-every 1000 --exploration-probes 75000 --exploration-cap 4096
thermo-sudoku-rs/target/release/thermo-18c-beam.exe --seeds analysis/18c-seeds-v1.jsonl --continue-from runs/18c-beam-round1-landscape4096-v1.jsonl --output runs/18c-beam-round2-landscape4096-v1.jsonl --progress-every 1000 --exploration-probes 12458 --exploration-cap 4096 --max-new-counts 75000 --max-total-solver-calls 80000
```

The output path must not already exist. On non-Windows systems, omit `.exe`.
Verify the implementation with:

```text
cargo fmt --all --manifest-path thermo-sudoku-rs/Cargo.toml -- --check
cargo test --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-18c-beam
cargo clippy --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-18c-beam -- -D warnings
```

## Interpretation

Neither completed radius-one beam round found a state below the 128-solution
seed incumbent. The result measures a substantial local barrier but leaves the
generalized 18-cell existence question open.
