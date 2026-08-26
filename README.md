# Minimal Thermo Sudoku

This repository contains exact and heuristic tools for finding thermo-only
Sudoku puzzles with the smallest possible number of covered cells. A completed
generalized search excludes 17-cell constructions, while the current best
construction covers 19 cells. The minimum is therefore 18 or 19 cells.

## Contents

- [Current results](#current-results)
- [Puzzle model](#puzzle-model)
- [Exact 17-cell method](#exact-17-cell-method)
- [Reproduction](#reproduction)
- [Repository layout](#repository-layout)
- [Evidence and data](#evidence-and-data)

## Current results

| Question | Current result | Details |
| --- | --- | --- |
| Generalized 17-cell networks | **No** | The complete `v2` saturated-network scan classified 65,561,076 candidates as multiple, with zero unique or impossible targets. It covers disjoint, overlapping, branching, and merging comparison networks. |
| General lower bound | **At least 18 covered cells** | The no-16-clue theorem excludes coverage of 16 or fewer cells; the completed generalized scan excludes exactly 17. |
| 19-cell existence | **Yes** | A cell-disjoint `9+8+2` puzzle is uniquely solvable and independently verified. [Puzzle and verification](analysis/unique-19c-9x8x2-2026-08-21.md). |
| Exact minimum | **18 or 19 cells** | The 18-cell case remains open. |

## Puzzle model

The minimum-coverage question uses standard 9x9 Sudoku and no givens other
than thermometer inequalities. A thermometer is a strict bulb-to-tip chain.
Consecutive cells are orthogonal or diagonal king neighbours, paths are simple,
and a path has length 2 through 9. Diagonal segments may cross geometrically.

Coverage means the number of distinct cells in the union of all constraints.
The project distinguishes two scopes:

- **Cell-disjoint paths:** different thermometers share no cells.
- **Generalized networks:** thermometer paths may share cells or segments, and
  their union may branch or merge. Equivalently, the layout is any finite set of directed two-cell
  king-neighbour inequalities. Every one of the 17 covered cells must be
  incident to at least one inequality.

The generalized scope strictly contains the cell-disjoint scope.

## Exact 17-cell method

Let `C` be the 17 covered cells of a hypothetical unique thermo puzzle and
let `S` be its solution. Fixing `S` on `C` produces a unique ordinary
17-given Sudoku: every completion of those givens automatically satisfies the
original inequalities. It is therefore sufficient to search the complete
catalogue of 49,158 essentially different 17-clue classics, up to Sudoku
coordinate morphs and digit relabelling.

For the generalized search, `thermo-17c-overlap` processes each eligible
catalogue record as follows:

1. Reject records in which the 17 clues do not contain all nine symbols.
2. Enumerate the 1,296 legal row-axis maps and 1,296 legal column-axis maps.
   Transposition is covered by exchanging the two complete axis domains.
3. For each realizable geometry and symbol order, build the **saturated
   network** containing every target-true king-neighbour comparison between
   the 17 cells, directed from lower to higher rank.
4. Require every cell to be incident. Require each consecutive rank pair to
   have a physical edge; otherwise swapping that pair gives a second Sudoku
   solution. The valid symbol orders are therefore Hamiltonian paths in the
   nine-symbol adjacency graph. Global reversal/digit complement removes one
   of each paired orientation.
5. Keep inclusion-maximal realizable adjacency masks. For directed candidates,
   compute transitive closure, deduplicate equal posets, and keep
   inclusion-maximal closures. These exact dominance steps cannot discard the
   last unique candidate because every removed network has a realizable,
   target-preserving stronger representative.
6. Classify each retained network with the unified Rust solver, counting only
   to `0`, `1`, or `2+`. The known catalogue solution makes `0` an internal
   consistency failure.

The saturation reduction is the key completeness argument. If `E` is any
admissible network and `U` is its saturated target-true superset, then
`E ⊆ U`. Adding constraints preserves the target and cannot add solutions,
so `E` unique implies `U` unique. Conversely, `U` is itself admissible in the
generalized two-cell-thermometer scope. Searching one saturated network per
realization therefore replaces enumeration of every edge subset and every
path partition.

The completed run covered all 49,158 catalogue records and all 25,370 eligible
records in 1,586 validated chunks. Every one of the 65,561,076 retained
candidates was multiple. The compact aggregate is
[`analysis/17c-overlap-v2-summary-2026-08-26.json`](analysis/17c-overlap-v2-summary-2026-08-26.json).

The implementation details, dominance proofs, corpus identity, and completion
criteria are in [the generalized 17-cell method note](analysis/17c-overlap-search.md).
The direct two- and three-path reductions are in
[the classic-morph search note](analysis/17c-classic-morph-search.md).

## Reproduction

Build and verify the maintained code:

```text
cargo fmt --all --manifest-path thermo-sudoku-rs/Cargo.toml -- --check
cargo test --release --all-targets --manifest-path thermo-sudoku-rs/Cargo.toml
cargo clippy --all-targets --all-features --manifest-path thermo-sudoku-rs/Cargo.toml -- -D warnings
python -m unittest discover -s analysis -p "test_*.py" -v
python -m unittest discover -s thermo_search -p "test_*.py" -v
```

Build and run the current generalized scanner:

```text
cargo build --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-17c-overlap
python analysis/run_17c_overlap_chunks.py --corpus <path-to>/17puz49158.txt --binary thermo-sudoku-rs/target/release/thermo-17c-overlap.exe --output-dir <artifact-root>/17c-overlap-v3 --workers 4 --eligible-per-chunk 16
```

On non-Windows systems, omit the executable's `.exe` suffix.

The retained completed `v2` run used commit
`551db12c0924a3d7c594f489bbc971d30f8763f9`. Check out that commit before
rebuilding if the historical `v2` implementation and identities are required.

This invocation sets no task timeout or solver node limit and uses no alternate
or fallback solver.

Use the same command to resume. The runner binds an output directory to the
corpus, executable, chunk size, and algorithm revision; validates every chunk
before publication; and refuses mixed or overlapping evidence. Worker count
may be changed between invocations. Run artifacts are intentionally ignored by
Git.

## Repository layout

| Path | Purpose |
| --- | --- |
| [`thermo-sudoku-rs/`](thermo-sudoku-rs/) | Unified exact Rust solver and research binaries. |
| [`analysis/`](analysis/) | Mathematical reductions, run specifications, verifiers, and retained evidence. |
| [`thermo_search/`](thermo_search/) | Seeded Python search and corpus checking. |
| [`benchmarks/`](benchmarks/) | Reproducible solver comparisons. |
| [`sources/`](sources/) | Read-only synchronized reference material. |

## Evidence and data

The required combined catalogue is `17puz49158.txt`:

```text
records   49,158
bytes     4,080,114
SHA-256   58ef7d83e8cbac32495161f9745877fef82f5e8b3fe58e3cad4eb3fc004a81b9
```

Download and licence provenance are recorded in
[the catalogue note](analysis/17c-classic-morph-search.md#input-corpus).
The file is not vendored. The original 49,151-record Royle collection was
published under CC BY 2.5; the combined archive does not explicitly restate a
licence for its seven later additions.

The completed aggregate reports all 49,158 source records, all 25,370 eligible
records, and all 1,586 logical chunks with no deferred task, gap, overlap,
identity mismatch, zero classification, or unique classification. Its detailed
artifact-set SHA-256 is
`312b76f3f41112042f6918447abbd08bd1c2f52a6426cc814e02d6d8c04e554f`.
This is a deterministic exhaustive-program result, not a SAT/LRAT
nonexistence certificate. The production scan does not retain a witness pair
for every ordinary multiple classification.
