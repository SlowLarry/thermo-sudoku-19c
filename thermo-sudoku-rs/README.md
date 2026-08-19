# thermo-sudoku

Dependency-free Rust reference solver for classic 9x9 Sudoku with strict,
cell-disjoint thermometers. Orthogonal and diagonal king-neighbour steps are
allowed; geometrically crossing diagonal segments are not treated as overlaps.

The solver returns a capped count and is optimized for the `0 / 1 / 2+` query.
Each thermometer is propagated as the complete table of its possible increasing
digit sequences. A length-`L` thermometer has only `C(9,L)` such sequences, at
most 126.

Build and test:

```text
cargo test --release --manifest-path thermo-sudoku-rs/Cargo.toml
cargo build --release --manifest-path thermo-sudoku-rs/Cargo.toml
```

Example (the first three-solution 9+8+2 record):

```text
thermo-sudoku-rs/target/release/thermo-sudoku-cli.exe --limit 4 \
  --thermos "19,29,28,20,11,12,13,3,4|77,69,78,70,62,53,44,52|41,51"
```

The library also exports `thermo_sudoku_count_up_to`, a small C ABI used by the
Python search script through `ctypes`. This scalar implementation is the
correctness oracle for subsequent profiling and Tdoku-inspired SIMD work.

Counts use a limit of at least two. Reaching the limit is reported as a lower
bound; a count below it is exact. The Rust API retains the first two witness
solutions, which will support solution-pair cuts in the later CEGIS search.
