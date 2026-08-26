# thermo-sudoku

Dependency-free Rust solver and exact-search tools for standard 9x9 Sudoku
with strict thermometer inequalities. One `Solver` handles ordinary paths,
shared cells, overlapping paths, path unions that branch or merge, and
explicit directed comparisons.

Commands below are run from the repository root unless stated otherwise.

## Contents

- [Constraint model](#constraint-model)
- [Solver algorithm](#solver-algorithm)
- [Rust API](#rust-api)
- [Command-line use](#command-line-use)
- [Specialized searches](#specialized-searches)
- [Build and verification](#build-and-verification)

## Constraint model

- Cells are zero-based row-major indices `0..80`.
- A path is ordered bulb to tip and means a strict increase at every step.
- Paths have length 2 through 9, contain no repeated cell, and use orthogonal
  or diagonal king-neighbour steps.
- Different paths may share cells or edges. Geometric diagonal crossings are
  allowed.
- Explicit `(lower, upper)` pairs mean `digit(lower) < digit(upper)` and are
  represented internally as two-cell paths. Exact duplicate pairs are removed.
- At most 64 path constraints are accepted. The exact saturated 17-cell search
  is safely within this limit: any 17 cells induce at most 46 undirected
  king-neighbour edges.

Givens are an `[u8; 81]`; `0` means blank and `1..9` are fixed digits.

## Solver algorithm

Each cell domain is a nine-bit mask. Propagation uses three event sets:

1. **Singleton queue.** A fixed value is removed from its 20 Sudoku peers.
2. **Dirty houses.** Rows, columns, and boxes detect missing digits and hidden
   singles, then apply pointing and claiming eliminations.
3. **Dirty paths.** A path is revised by one forward lower-bound pass and one
   backward upper-bound pass. For an edge `a < b`, values in `b` not greater
   than `min(a)` and values in `a` not less than `max(b)` are removed. Across a
   chain, the two sweeps are the generalized-arc-consistency fixed point,
   including domains with holes.

Every domain change schedules the cell's three houses and all incident paths;
a newly fixed cell also enters the singleton queue. Shared cells therefore
propagate between overlapping paths until all three event sets reach a common
fixed point. Cyclic or otherwise contradictory comparison networks are
classified as unsatisfiable by the same propagation and search.

If propagation does not decide the puzzle, deterministic DFS:

1. chooses a cell with minimum remaining domain size;
2. among ties, maximizes the number of unresolved Sudoku peers plus unresolved
   comparison neighbours;
3. scores each value by its immediate removals from those neighbours and tries
   the most constraining value first, using the lower digit as the final tie.

These choices affect only traversal order. Search remains exhaustive. The main
classification call counts to two and reports `Zero`, `Unique`, or `Multiple`.
`SolveStats` exposes nodes, branches, propagation rounds, path revisions, and
maximum depth.

## Rust API

The principal constructors and queries are:

```rust
use thermo_sudoku::Solver;

let paths = vec![vec![0, 1, 2], vec![2, 12]];
let solver = Solver::blank(&paths)?;
let result = solver.classify();          // count capped at 2

let comparisons = vec![(0, 1), (0, 10), (10, 20)];
let solver = Solver::blank_comparisons(&comparisons)?;
let result = solver.count_up_to(10);     // exact if count < 10
let grids = solver.enumerate_up_to(10);  // at most 10 solutions
```

Available entry points:

| API | Purpose |
| --- | --- |
| `Solver::new` / `Solver::blank` | Construct from thermometer paths, with or without givens. |
| `Solver::new_comparisons` / `Solver::blank_comparisons` | Construct from directed king-neighbour inequalities. |
| `classify` | Exact `0 / 1 / 2+` classification. |
| `count_up_to` | Count to a caller-selected cap of at least two. |
| `enumerate_up_to` | Return a bounded set of distinct complete grids. |
| `layout` | Inspect validated path geometry and coverage. |
| `screen_two_cell_extensions` | Classify every legal directed two-cell extension that is cell-disjoint from a fixed base. |
| `screen_nine_eight_extensions` | Exact cap-two screen specialized to a disjoint `9+8` base. |
| `score_nine_eight_extensions` | Common-cap scoring for guided `9+8+2` search. |

The crate also exports `thermo_sudoku_count_up_to`, a C ABI used by the Python
search through `ctypes`. A null witness pointer avoids copying a grid when only
the count is needed.

## Command-line use

The compact path format separates cells with commas and paths with `|`:

```text
thermo-sudoku-rs/target/release/thermo-sudoku-cli.exe --limit 2 --thermos "19,29,28,20,11,12,13,3,4|77,69,78,70,62,53,44,52|41,51" --show-solution
```

Add `--givens` followed by 81 digits (`0` or `.` for blanks) to solve a puzzle
with classic clues. `--screen-two-cell` classifies all legal directed two-cell
extensions of the supplied base; `--emit-certificate` includes witness grids
for independent checking.

## Specialized searches

| Binary | Purpose |
| --- | --- |
| `thermo-9x8-guided` | Deterministic local search over disjoint `9+8+2` layouts. |
| `thermo-9x8-pilot` | Sharded exact enumeration of `9+8` bases and two-cell extensions. |
| `thermo-17c-morph` | Exact cell-disjoint two-path (`9+8`) scan of the 17-clue catalogue. |
| `thermo-17c-three-path` | Exact scan of all ten cell-disjoint three-path partitions of 17. |
| `thermo-17c-maximal` | Merge-maximal disjoint-path scan for the remaining path layers. |
| `thermo-17c-overlap` | Exact saturated-network scan for all generalized 17-cell inequalities. |
| `thermo-fixed-target` | Fixed-solution comparison hitting-set experiment. |
| `thermo-global-cegis` | Target-free comparison-set CEGIS experiment. |
| `thermo-topology-cnf` | SAT encoding and persistent CEGIS for cell-disjoint path topology. |

The completed generalized 17-cell scan is reproduced through the validated
chunk runner:

```text
cargo build --release --manifest-path thermo-sudoku-rs/Cargo.toml --bin thermo-17c-overlap
python analysis/run_17c_overlap_chunks.py --corpus <path-to>/17puz49158.txt --binary thermo-sudoku-rs/target/release/thermo-17c-overlap.exe --output-dir <artifact-root>/17c-overlap-v2 --workers 4 --eligible-per-chunk 16
```

On non-Windows systems, omit executable `.exe` suffixes.

This invocation sets no task timeout or solver node limit and uses no alternate
or fallback solver.

The production run validated all 1,586 chunks and classified all 65,561,076
retained candidates as multiple. It found no unique or impossible target and
therefore excludes 17-cell thermo-only puzzles in the documented generalized
scope.

The scanner's saturation theorem, enumeration algorithm, persistence rules,
and completion criteria are specified in
[`../analysis/17c-overlap-search.md`](../analysis/17c-overlap-search.md).
Project-level results are summarized in [`../README.md`](../README.md).

## Build and verification

```text
cargo fmt --all --manifest-path thermo-sudoku-rs/Cargo.toml -- --check
cargo test --release --all-targets --manifest-path thermo-sudoku-rs/Cargo.toml
cargo clippy --all-targets --all-features --manifest-path thermo-sudoku-rs/Cargo.toml -- -D warnings
cargo build --release --manifest-path thermo-sudoku-rs/Cargo.toml
```

The tests include complete domain-pair checks for comparison revision,
overlap/branch/merge/cycle cases, randomized solution-set differentials,
path-capacity boundaries, morph realizations, and exact scanner reductions.
