# Quick Rust versus Rangsk benchmark

Related comparisons:

- [Interactive Sudoku Solver](ISS.md)
- [Rangsk's .NET WebAssembly prototype](WASM_RANGSK.md)
- [Hybrid all-two-cell extension screen](TWO_CELL_SCREEN.md)

Run on 2026-08-19 with:

- Intel Core i5-6500, four logical processors;
- Windows 10.0.19045;
- Rust 1.94.0, release build with the repository's LTO profile;
- Python 3.13.7;
- Rangsk SudokuSolverConsole 1.3.188;
- corpus SHA-256
  `79BEC9AD12BF7C3C6CB28948E1C54CD98809929D5FE5A3003A8C6215367046A7`.

Both solvers were single-threaded and classified solutions as 0, 1, or 2+.
Rangsk used `--check`; Rust used `count_up_to(2)`. The cases were Blue's unique
20-cell puzzle, all fourteen saved 3-solution layouts, and ten deterministic
distinct layouts spread through the result file. Every result agreed.

Rust was repeated twenty times per layout and Rangsk three times; the table
uses each layout's median. "Solver" is the Rust FFI duration versus Rangsk's
JSON-reported duration. "Wall" additionally includes Python marshalling for
Rust and fresh .NET process launch plus JSON I/O for Rangsk.

| Group | Cases | Rust solver median | Rangsk solver median | Aggregate solver speedup | Rust wall median | Rangsk wall median |
|---|---:|---:|---:|---:|---:|---:|
| Blue unique | 1 | 0.991 ms | 150.787 ms | 152x | 1.133 ms | 1,553.9 ms |
| Saved count-3 | 14 | 0.137 ms | 53.323 ms | 396x | 0.242 ms | 1,120.2 ms |
| Spread sample | 10 | 0.211 ms | 210.427 ms | 1,527x | 0.334 ms | 1,551.0 ms |
| All | 25 | 0.145 ms | 64.351 ms | 810x | 0.251 ms | 1,290.6 ms |

The aggregate speedup is the ratio of the sums of the per-layout median solver
durations, not a ratio of the displayed overall medians. Rangsk's wall time is
dominated by cold process startup, so the 810x solver-duration comparison is
the useful headline for this quick run. Its JSON timer may still include some
per-process JIT work; a persistent `--listen` benchmark would be needed to
remove that completely.

Reproduce with:

```text
python -u benchmarks/quick_compare.py \
  --rangsk C:/path/to/SudokuSolverConsole.exe \
  --rust-repeats 20 --rangsk-repeats 3 \
  --output benchmarks/result.json
```
