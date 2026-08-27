# Seed-42 exact two-cell exchange shell

## Contents

- [Scope](#scope)
- [Move set](#move-set)
- [Algorithm](#algorithm)
- [Completed result](#completed-result)
- [Artifacts and provenance](#artifacts-and-provenance)
- [Reproduction](#reproduction)
- [Interpretation](#interpretation)

## Scope

`thermo-18c-beam --root-two-cell-neighborhood` exhausts the
solution-preserving two-cell footprint-exchange shell around frozen seed 42:

```text
network SHA-256   000686520eb98f01cfee9ef0be013e1d3758bbb258add2b09866094ae31fd7ae
exact solutions   128
covered cells     18
```

Every one of the root's 128 Sudoku solutions is used as a saturation target.
This is not graph-radius two: it does not add targets introduced by an
intermediate one-cell move. It is not an exhaustive search over all 18-cell
networks.

## Move set

Let `C` be the root's sorted 18-cell footprint and `T` one root solution. For
every unordered removal pair `{r1,r2}` from `C` and addition pair `{a1,a2}`
from the fixed 63-cell complement of `C`, the candidate footprint is

```text
C - {r1,r2} + {a1,a2}.
```

There are `C(18,2) = 153` removal pairs and `C(63,2) = 1,953` addition pairs:
298,809 moves per target and 38,247,552 in total. Removal-pair ordinals are
one-based nested lexicographic pairs of the sorted root cells.

A target-independent geometry pass rejects a final king graph only if it has
an isolated cell; the two added cells may be incident only to each other.
Each survivor is saturated with all target-true king-neighbour comparisons,
required to cover all 18 cells, Hasse-reduced, and canonicalized under D4 plus
optional digit complement (global edge reversal).

## Algorithm

1. Authenticate the 71-seed artifact and replay all 71 exact counts.
2. Require seed ordinal 42, its fixed network hash, and exact count 128.
3. Independently enumerate, validate, sort, and deduplicate exactly 128 root
   solutions; require uncapped exhaustion.
4. Generate all declared removal pairs, all 1,953 addition pairs, and all 128
   targets; apply geometry, saturation, coverage, Hasse reduction, and
   canonicalization.
5. Deduplicate by exact canonical Hasse vector. Reuse a count only for an exact
   seed-cache key; otherwise count the key directly to cap 129.
6. Stop on exact count one. Otherwise classify every key in the shard.

A count below 129 is exact; a cap hit proves only `count >= 129`. Thus a
complete shard without a count from 1 through 127 excludes both uniqueness and
strict improvement on the 128-solution root within that shard.

## Completed result

The 39 disjoint shards `1-4, 5-8, ..., 149-152, 153-153` form an exact cover of
all 153 removal pairs. Every shard completed at cap 129.

| Measure | Result |
| --- | ---: |
| Root targets | 128 |
| Raw moves | 38,247,552 |
| Geometric incidence rejections | 26,873,088 |
| Coverage rejections | 412,654 |
| Accepted observations | 10,961,810 |
| Within-shard duplicate observations | 10,806,620 |
| Shard key instances | 155,190 |
| Cross-shard duplicate key instances | **0** |
| Global distinct canonical keys | **155,190** |
| Keys with exact count below 129 | **0** |
| Keys with count at least 129 | **155,190** |
| Unique keys | **0** |
| Solver calls | 157,998 |
| Solver nodes | 46,418,540 |

Because the global merge found no cross-shard overlap, the shard-key-instance
count is also the exact number of distinct canonical networks in this shell.
None improves the root's 128 solutions.

## Artifacts and provenance

The tracked
[compact summary](18c-root-seed-42-shell2-cap129-summary.json) contains all 39
range rows, artifact hashes, conservation totals, the global key merge, and
digest encodings. The full JSONL artifacts remain outside Git under `runs/`.

```text
artifact schema             thermo-18c-root-two-cell-neighborhood-v1
algorithm revision          strict-seed42-replay-all-targets-removed-pair-shell2-shard-hasse-dedupe-cap129-v1
artifacts                    39
aggregate bytes              190,028,848
aggregate records            160,260
seed artifact SHA-256        8e6a11d7d7e7f51dd497e64ca6d9ed7ae492b42fea60d46bb2aeded51f2bf99e
binary bytes                 937,472
binary SHA-256               93346cce167b9b30a19c1a8a79a0b93df640e0bc29113de3fc849d608a4761d6
ordered solution SHA-256     3fd440f36705a987aa4cdbae369c858afef361e9d2da084af1d824e0d11cc470
artifact-set SHA-256         16acd18a96e6e1e2304e2647940eeb3cfdb340c58e5caa63aae789c818e0c8da
records-2-to-EOF set SHA-256 42dbad7fa339134d9dede79f08d7f3117bee0cbf8e05c597a6230dccecd4866d
global key-set SHA-256       30b140a022340129d1d68212085eb795068aabcc6453464391eada6b9114cbad
```

The ordered-solution digest hashes the 128 lexicographically sorted grids as
81 numeric bytes `0x01..0x09` per grid. The set-digest encodings are specified
in the compact summary. Each JSONL artifact contains a provenance header, all
128 root solutions, canonical network records in exact Hasse-key order, and a
terminal conservation summary. Output is atomic and refuses an existing path.

## Reproduction

Build from the repository root:

```text
cargo build --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-18c-beam
```

For each inclusive removal-pair range in the 39-shard partition, run:

```text
thermo-sudoku-rs/target/release/thermo-18c-beam.exe --root-two-cell-neighborhood --seeds analysis/18c-seeds-v1.jsonl --root-seed-ordinal 42 --root-network-sha256 000686520eb98f01cfee9ef0be013e1d3758bbb258add2b09866094ae31fd7ae --root-removed-pair-start FIRST --root-removed-pair-end LAST --count-cap 129 --output runs/18c-root-seed-42-shell2-removed-pairsFIRST-LAST-cap129-v1.jsonl --progress-every 1000
```

Each range may contain at most four removal pairs. On non-Windows systems,
omit `.exe`. The output path must not exist. Verify the implementation with:

```text
cargo fmt --all --manifest-path thermo-sudoku-rs/Cargo.toml -- --check
cargo test --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-18c-beam
cargo clippy --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-18c-beam -- -D warnings
```

## Interpretation

There is no unique network, and no network with fewer than 128 solutions, in
the complete seed-42-solution-preserving two-cell exchange shell. This result
does not exclude a unique network around another root, a target absent from
seed 42's solution set, or a longer path through intermediate networks.
