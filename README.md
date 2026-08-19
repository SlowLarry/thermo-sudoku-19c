# 19-cell thermo Sudoku research

This workspace now contains two maintained components; the synchronized files
under `sources/` remain read-only.

- `thermo_search/thermo_anneal.py`: corrected, bounded and reproducible
  simulated-annealing search. It can use the installed console solver for
  cross-checks or the in-process Rust backend for normal work.
- `thermo-sudoku-rs/`: dependency-free Rust solver for classic Sudoku plus
  strict, cell-disjoint thermometers. It returns capped solution counts and is
  designed first for `0 / 1 / 2+` classification.

The fixed geometry is:

- zero-based row-major cells;
- bulb-to-tip path order;
- orthogonal or diagonal king-neighbour steps;
- simple paths of length 2 through 9;
- no cell shared by different thermometers;
- diagonal segments may geometrically cross if they do not share a cell.

Quick verification:

```text
cargo test --release --manifest-path thermo-sudoku-rs/Cargo.toml
python -m unittest discover -s thermo_search -p "test_*.py" -v
python thermo_search/thermo_anneal.py validate-corpus \
  --input sources/min_thermos_9_8_2.txt
```

The last command intentionally exits nonzero for the supplied file because it
detects the overlapping record on line 1192; its summary should still show
`matched: 1279` and `mismatched: 0`.

The Rust implementation is deliberately scalar and auditable at this stage.
Each thermometer is propagated as the full table of its possible increasing
digit sequences. It serves as the correctness oracle for later profiling,
batch APIs, pair-cut search, and any Tdoku-inspired SIMD backend.

The supplied corpus currently gives 1,279 exact count matches and one geometry
rejection: line 1192 shares cell 60 between two thermometers. No source record
is modified.

## Performance direction

Tdoku is a useful architecture reference, but its public pencilmark interface
only accepts unary cell restrictions and cannot express a thermometer's binary
ordering constraints. This project therefore does not call Tdoku and filter its
solutions. The next native milestone is a batch operation that fixes a 9+8
base once and classifies every legal two-cell extension together. If profiling
then warrants it, the Sudoku propagation layer can adopt Tdoku-style
box/triad and band-configuration propagation while this scalar implementation
remains the independent oracle. No Tdoku source code has been copied here.

The first reproducible comparison with Rangsk SudokuSolverConsole is in
`benchmarks/README.md`, with the complete per-case JSON alongside it.

## Source notes and acknowledgement

The notebook under `sources/` is preserved as historical exploratory material.
Its saved code contains stale anti-knight and API residue and should not be used
as the maintained classic-Sudoku search implementation. The accompanying
result corpus is classic Sudoku plus thermometers and is independently checked
by the Rust solver.

The 20-cell construction used as the unique regression fixture is by **Blue**
and is included for research and demonstration with the creator's permission.
