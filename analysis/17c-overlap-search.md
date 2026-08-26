# Exact generalized 17-cell thermo search

This note specifies the exhaustive search implemented by
`thermo-sudoku-rs/src/bin/thermo-17c-overlap.rs`, its completeness reduction,
the completed production result, and the evidence needed to reproduce it.

## Contents

- [Result](#result)
- [Scope](#scope)
- [Completeness reduction](#completeness-reduction)
- [Catalogue and eligibility](#catalogue-and-eligibility)
- [Exact enumeration](#exact-enumeration)
- [Sudoku classification](#sudoku-classification)
- [Production identity and command](#production-identity-and-command)
- [Audit and evidence boundary](#audit-and-evidence-boundary)

## Result

The complete generalized 17-cell scan found **no unique puzzle**. All
65,561,076 retained candidates had at least two Sudoku solutions.

This excludes every thermo-only standard 9x9 Sudoku in the [scope below](#scope)
whose constraints cover exactly 17 distinct cells. The scope includes ordinary
cell-disjoint thermometers as well as overlapping paths, shared cells, branches,
merges, and arbitrary collections of local two-cell thermometers.

Together with the no-16-clue theorem, the result proves that such a puzzle must
cover at least 18 cells. A verified 19-cell construction exists, so the minimum
is **18 or 19 cells**. The 18-cell case remains open.

The retained aggregate is
[`17c-overlap-v2-summary-2026-08-26.json`](17c-overlap-v2-summary-2026-08-26.json).
Its frozen runner identity is
[`17c-overlap-v2-run-identity-2026-08-25.json`](17c-overlap-v2-run-identity-2026-08-25.json).

## Scope

The searched puzzle model is:

- standard 9x9 Sudoku;
- no givens other than thermometer inequalities;
- exactly 17 distinct covered cells;
- every covered cell incident to at least one comparison;
- each comparison joins orthogonally or diagonally adjacent cell centres;
- strict bulb-to-tip increase;
- arbitrary sharing, overlap, branching, merging, and geometric crossing;
- no drawing-imposed limit on the number of distinct comparisons.

Equivalently, a layout is any finite directed set of target-consistent
king-neighbour inequalities on 17 cells. Duplicate comparisons have no effect.
Any conventional thermometer path is the conjunction of its consecutive
two-cell inequalities, so the model contains every collection of ordinary
thermometers, including the cell-disjoint case. There are at most 46 distinct
undirected king edges induced by any 17 grid cells.

The result does not cover nonstandard Sudoku regions, inequalities between
non-neighbouring cells, thermometers using cells outside the 17-cell union, or
puzzles with additional givens or other constraint types.

## Completeness reduction

### From a thermo puzzle to a 17-given classic

Let `C` be the covered cells of a hypothetical unique thermo puzzle and `S`
its solution. Fixing the values of `S` on `C` gives an ordinary 17-given
Sudoku. Any completion of those givens has the same values on every comparison
endpoint and therefore satisfies every thermo inequality. The 17-given Sudoku
must consequently be unique.

It is therefore sufficient to search a complete catalogue of essentially
different unique 17-given classic Sudokus, together with every Sudoku
coordinate morph and every digit relabelling.

### Saturating the local comparisons

Fix one catalogue target, one 17-cell realization, and one digit order. Let
`U` contain every target-true king-neighbour comparison among the 17 cells,
oriented from the lower target digit to the higher target digit.

Every admissible comparison network `E` on that realization satisfies
`E ⊆ U`. Adding the comparisons in `U \ E` preserves the target and can only
remove solutions. Thus:

```text
E unique  =>  U unique
```

Conversely, `U` is itself admissible as a collection of two-cell
thermometers. Existence of any unique generalized network is therefore
equivalent to existence of a unique saturated network. The scanner need not
enumerate edge subsets, thermometer decompositions, or path-length partitions.

### Necessary digit order

All nine digits must occur on the covered cells. If a digit were absent,
swapping it with a neighbouring rank across an absent/present boundary would
preserve every comparison and produce a second Sudoku solution.

The comparison poset must also have a unique linear extension. In particular,
each consecutive pair in the target digit order must share a physical
comparison edge. No longer directed chain can order consecutive ranks because
there is no intermediate rank. Hence every admissible target order is a
Hamiltonian path in the nine-symbol physical-adjacency graph.

These are necessary reductions only. Every survivor is still classified by
an exact Sudoku search.

## Catalogue and eligibility

The input is the combined catalogue `17puz49158.txt`:

```text
records   49,158
bytes     4,080,114
SHA-256   58ef7d83e8cbac32495161f9745877fef82f5e8b3fe58e3cad4eb3fc004a81b9
FNV-1a64  96baf249978384bb
```

Download and licence provenance are recorded in
[`17c-classic-morph-search.md`](17c-classic-morph-search.md#input-corpus).
The catalogue is not vendored.

Exactly 25,370 records contain all nine clue symbols and enter geometric
enumeration. The other 23,788 records are excluded by the absent-digit swap
argument above.

## Exact enumeration

For each eligible catalogue record the scanner performs these steps.

1. Generate all 1,296 legal row-axis morphs and all 1,296 legal column-axis
   morphs. Each axis group has `6^4` elements: one band or stack permutation
   and three within-band or within-stack permutations.
2. Precompute which morphs make each of the 136 clue-cell pairs king-adjacent.
   Row and column supports are intersected as bitsets.
3. Group equal axis masks and retain inclusion-maximal masks. Intersect the
   retained row and column classes, group equal physical networks, and again
   retain inclusion-maximal masks.
4. Reject networks that do not make every one of the 17 cells incident.
5. Build the nine-symbol adjacency graph and enumerate its Hamiltonian orders.
   Simultaneous order reversal and comparison reversal is global digit
   complement, so one of each pair is retained.
6. Orient every target-true local edge for each order and compute the directed
   transitive closure on the 17 labelled clue vertices.
7. Deduplicate equal closures and retain inclusion-maximal closures.
8. Realize the representative's actual king-neighbour edges and classify the
   resulting comparison Sudoku to a cap of two solutions.

Transpose is complete without an additional factor of two: exchanging the
independently complete row and column domains realizes every transposed case.

The mask and closure reductions are exact monotonicity reductions. If a
weaker representative `A` is contained in a realizable stronger
representative `B`, transport `A` through the corresponding Sudoku coordinate
morph to `B`'s realization. Every solution of `B` then satisfies `A`, while
the catalogue target satisfies both. A unique `A` would force a unique `B`,
so deleting `A` cannot delete the last unique case. Comparisons are made on
the full labelled masks and closures, not on hash equality.

Transitive closure is used only as a logical-equivalence and dominance key.
Nonlocal closure arcs are never passed to the solver; it receives only the
representative's realized king-neighbour comparisons.

## Sudoku classification

`Solver::blank_comparisons` uses the same solver as ordinary thermometer
paths. Explicit comparisons become two-cell paths; shared endpoints and longer
overlapping paths are handled by one incident-path work queue.

Each cell has a nine-bit domain. Propagation alternates:

- singleton peer elimination;
- row, column, and box revision, including hidden singles and locked
  candidates;
- forward lower-bound and backward upper-bound revision of every dirty
  increasing path.

Changes at a shared cell requeue every incident path until a common fixed point
is reached. If propagation is incomplete, exhaustive DFS selects a
minimum-domain cell, breaks ties by unresolved Sudoku and comparison pressure,
and tries values by descending immediate reduction score. These choices affect
only traversal order.

The scanner requests two solutions. `0` is an internal consistency failure
because the catalogue target is known; `1` is a unique candidate; `2+` is
multiple. The production run used no node limit, task timeout, alternate
solver, or fallback classification.

## Production identity and command

The completed run used repository commit
`551db12c0924a3d7c594f489bbc971d30f8763f9`, Rust/Cargo 1.94.0 on
`x86_64-pc-windows-msvc`, and the following identities:

| Input or implementation | Bytes | SHA-256 |
|---|---:|---|
| `17puz49158.txt` | 4,080,114 | `58EF7D83E8CBAC32495161F9745877FEF82F5E8B3FE58E3CAD4EB3FC004A81B9` |
| `thermo-sudoku-rs/Cargo.toml` | 357 | `2EF150F573911E9890DB35DC9D6858CCB5B084F337C99CF146D2163F2A6BB25F` |
| `thermo-sudoku-rs/src/lib.rs` | 106,593 | `6C3FFCE7C751F5F354143A025B28B7081D8381B7F3B38D947775FDB9F2250D91` |
| `thermo-sudoku-rs/src/bin/thermo-17c-overlap.rs` | 121,675 | `76D89F253CC97CBEF0696AE89B0CCFAFB7F3F6EB97EBB7AA6ECE403F9414D621` |
| `analysis/run_17c_overlap_chunks.py` | 66,457 | `CC65595F253BEF0185757303A8009E4ACD98EE319866C3150C70AF6173C24626` |
| release executable | 401,920 | `23E3491164A658A1F59E705483D37C97CC2F383CBA4C4267A9F3CBC43EBDB215` |

Algorithm revision:

```text
saturated-axis-poset-antichain-hamiltonian-unified-dynamic-mcv-v2
```

To reproduce the `v2` implementation, check out that commit, then build and
run from the repository root:

```text
cargo build --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-17c-overlap
python analysis/run_17c_overlap_chunks.py --corpus <path-to>/17puz49158.txt --binary thermo-sudoku-rs/target/release/thermo-17c-overlap.exe --output-dir <artifact-root>/17c-overlap-v2 --workers 4 --eligible-per-chunk 16
```

On non-Windows systems, omit `.exe`. Repeating the same runner command validates
published artifacts and resumes only missing chunks. The output directory is
bound to the corpus hash, executable hash, chunk size, and algorithm revision.

The completed run produced:

| Field | Value |
|---|---:|
| Logical chunks | 1,586 / 1,586 |
| Source records | 49,158 / 49,158 |
| Eligible records | 25,370 |
| Retained candidates | 65,561,076 |
| Multiple | 65,561,076 |
| Unique | 0 |
| Impossible target | 0 |
| Solver nodes | 10,044,810,358 |
| Wall time | 9,483.964 s |
| Artifact bytes | 6,020,687 |
| Artifact-set SHA-256 | `312b76f3f41112042f6918447abbd08bd1c2f52a6426cc814e02d6d8c04e554f` |

The 1,586 detailed JSONL files are local run artifacts and are ignored by Git.
The tracked compact summary preserves the aggregate counts and identities.

## Audit and evidence boundary

The final audit re-ran the launcher's artifact validator and aggregate logic
over all 1,586 JSONL files. It confirmed:

- exactly the expected filenames and no missing or unexpected artifact;
- a contiguous, nonoverlapping cover of source lines 1 through 49,158;
- matching header and terminal fingerprints for every artifact;
- exact requested-range completion for every chunk;
- uniform corpus, executable, and algorithm identities;
- `classified = multiple + unique + zero` in every artifact and aggregate;
- no partial file, split manifest, deferred task, timeout, or fallback result;
- an empty stderr log and normal launcher termination;
- exact agreement between the recomputed aggregate, `summary.json`, and the
  terminal stdout record.

Verification commands for the maintained source are:

```text
cargo fmt --all --manifest-path thermo-sudoku-rs/Cargo.toml -- --check
cargo test --release --all-targets --manifest-path thermo-sudoku-rs/Cargo.toml
cargo clippy --all-targets --all-features --manifest-path thermo-sudoku-rs/Cargo.toml -- -D warnings
python -m unittest discover -s analysis -p "test_*.py" -v
```

This is a deterministic exhaustive-program result conditional on the
completeness of the 49,158-record catalogue and the correctness of the
documented reductions and implementation. It is not a SAT/LRAT nonexistence
certificate. Ordinary multiple cases were counted to two but their witness
pairs were not emitted, so the compact aggregate alone is not an independently
checkable certificate for every classification. A fresh run reproduces the
search and accounting.
