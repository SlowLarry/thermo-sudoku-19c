# Quick Rust versus Interactive Sudoku Solver benchmark

Run on 2026-08-19 with:

- Intel Core i5-6500, four logical processors;
- Windows 10.0.19045;
- Rust 1.94.0, release build with the repository's LTO profile;
- Node.js 22.18.0 with V8 12.4.254.21;
- Interactive Sudoku Solver (ISS) revision
  `c43bfb867baa0f9c12c087afe912d626ac13a77a`;
- corpus SHA-256
  `79BEC9AD12BF7C3C6CB28948E1C54CD98809929D5FE5A3003A8C6215367046A7`.

Both solvers were single-threaded and classified solutions as 0, 1, or 2+.
The cases were Blue's unique 20-cell puzzle, all fourteen saved 3-solution
layouts, and ten deterministic distinct layouts spread through the result
file. Every result agreed.

ISS ran in one persistent Node process. After twenty unmeasured full-corpus
passes for V8 warm-up, it performed one hundred measured passes. Each sample
built a fresh solver and called `countSolutions(2)`. Rust likewise performed
one hundred uncached FFI calls per layout; each call constructed a fresh solver
and called `count_up_to(2)`.

| Group | Cases | Rust setup + count median | ISS setup + count median | Aggregate Rust speedup | ISS build median | ISS count median |
|---|---:|---:|---:|---:|---:|---:|
| Blue unique | 1 | 0.872 ms | 1.713 ms | 1.96x | 1.198 ms | 0.453 ms |
| Saved count-3 | 14 | 0.121 ms | 1.566 ms | 13.27x | 1.377 ms | 0.155 ms |
| Spread sample | 10 | 0.187 ms | 1.487 ms | 8.55x | 1.317 ms | 0.154 ms |
| All | 25 | 0.129 ms | 1.559 ms | 9.09x | 1.365 ms | 0.155 ms |

The table's aggregate speedup is the ratio of the sums of the per-layout
median setup-plus-count durations, not a ratio of the displayed group medians.
The displayed multi-case times are themselves medians of per-layout medians.
Across all 2,500 measured calls, the timed regions totalled 580.313 ms for
Rust and 6,739.750 ms for ISS, an 11.61x sustained-work ratio. That second ratio
includes pauses such as V8 garbage collection that per-layout medians suppress.
Constraint-object creation and process/IPC startup are excluded on both sides.
ISS's constraint objects were prepared before timing; its timed setup is
`SudokuBuilder.build`. The Rust timer begins at the FFI boundary and includes
layout/template construction in `Solver::new`.

ISS's search-only number is shown as a diagnostic, not as a direct speed ratio:
the current Rust FFI does not expose a prebuilt reusable solver. For the
intended heuristic search, where the thermo layout changes at every proposal,
fresh setup plus count is the relevant comparison. An incremental ISS
integration or a persistent Rust solver API would require a new benchmark.

Reproduce after cloning ISS with:

```text
python -u benchmarks/quick_compare_iss.py \
  --iss-root C:/path/to/Interactive-Sudoku-Solver \
  --rust-repeats 100 --iss-warmup-rounds 20 --iss-repeats 100 \
  --output benchmarks/result.json
```

The full per-case samples are in `quick_compare_iss_2026-08-19.json`.
