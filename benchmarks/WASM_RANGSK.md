# Rust versus Rangsk .NET WebAssembly prototype

> The native Release comparison in [NATIVE_RANGSK.md](NATIVE_RANGSK.md) is the
> primary solver comparison. This page is retained to measure the additional
> WebAssembly host cost.

Run on 2026-08-19 with:

- Intel Core i5-6500, four logical processors;
- Windows 10.0.19045;
- Rust 1.94.0, release build with the repository's LTO profile;
- portable .NET SDK 10.0.400 and .NET runtime 10.0.11;
- Node.js 22.18.0 with V8 12.4.254.21;
- `dclamage/SudokuSolver` branch `wasm-prototype`, revision
  `5b3f1a69f915153e3251568e1b4c64119a393e3d`.

The branch built successfully as single-threaded Release AOT WebAssembly. The
published `wwwroot` was 19.6 MB uncompressed and initialized in Node in about
482 ms. Runtime startup is excluded from the solver timings because the search
would keep one persistent worker alive.

## Fresh-layout result

Both solvers classified the same 25 layouts as 0, 1, or 2+ with a solution cap
of two. The cases were Blue's unique 20-cell puzzle, all fourteen saved
3-solution layouts, and ten deterministic distinct layouts spread through the
result file. Every result agreed.

The WASM runtime received twenty unmeasured full-corpus warm-up passes, followed
by one hundred measured passes. Every measurement constructed a fresh classic
Sudoku with the three thermo constraints and then counted to two. Rust likewise
performed one hundred uncached FFI calls per layout, each including fresh
`Solver::new` plus `count_up_to(2)`.

| Group | Cases | Rust build + count median | WASM build + count median | Aggregate Rust speedup | WASM build median | WASM count median |
|---|---:|---:|---:|---:|---:|---:|
| Blue unique | 1 | 0.924 ms | 5.284 ms | 5.72x | 3.316 ms | 1.464 ms |
| Saved count-3 | 14 | 0.117 ms | 5.027 ms | 42.04x | 3.313 ms | 1.312 ms |
| Spread sample | 10 | 0.175 ms | 5.203 ms | 31.96x | 3.316 ms | 1.496 ms |
| All | 25 | 0.121 ms | 5.130 ms | **30.18x** | 3.316 ms | 1.376 ms |

The aggregate speedup is the ratio of the sums of the per-layout median total
durations, not the ratio of the displayed group medians. Across all 2,500
measured calls, the timed regions totalled 571.367 ms for Rust and 15,519.465 ms
for WASM, a **27.16x** measured-work ratio that includes runtime and garbage-
collection outliers.

Of the raw WASM timed work, 67.36% was generic solver construction and 32.64%
was capped counting. Those shares come from raw totals; the displayed phase
medians are not additive.

Constraint-string construction, JSON/JS interop, and runtime startup are outside
the WASM timed region. Python validation, canonicalization, and ctypes-buffer
construction are outside the Rust timed region. On both sides the primary
timing begins with the engine's fresh layout/constraint construction and ends
after capped counting.

## How much is WebAssembly itself?

The upstream benchmark deliberately times only `CountSolutions`, building the
solver before its stopwatch. Running that unmodified timing core on the same
cases in native .NET and in the AOT WASM bundle gives:

| Group | Native .NET count median | WASM count median | Aggregate WASM tax |
|---|---:|---:|---:|
| Blue unique | 1.392 ms | 2.371 ms | 1.70x |
| Saved count-3 | 0.497 ms | 1.328 ms | 2.54x |
| Spread sample | 0.507 ms | 1.543 ms | 3.10x |
| All | 0.502 ms | 1.364 ms | **2.67x** |

Thus the roughly 30x gap to the specialized Rust solver is not 30x of WASM
overhead. On these thermometers, about 2.7x is the count-only WASM tax in the
upstream harness; the remainder is primarily general-purpose solver setup and
algorithmic specialization. Fresh constraint construction is especially
important for annealing because it cannot be amortized across different
layouts.

The 30.18x result characterizes the branch's current general
`SolverFactory.CreateBlank` path, including thermo-string parsing and generic
constraint finalization. It is not a prediction for a future direct or
incremental annealing API designed specifically to avoid that setup.

As an additional diagnostic, moving the native harness's existing `Build(c)`
call inside its stopwatch gave a 7.03x aggregate native-.NET gap to Rust and a
4.29x aggregate WASM gap to native .NET for fresh build plus count. This is not
the primary table because the patched native harness forces a collection before
every sample while the round-robin WASM batch lets garbage collection occur
naturally; it nevertheless gives a useful decomposition close to
`7.03 x 4.29 = 30.2`.

## Reproduction

The only upstream source addition was `ThermoBenchInterop.cs`, a timing export;
the solver itself was unchanged. Copy the provided file into
`SudokuSolverWasm`, then publish the requested branch:

```text
dotnet workload install wasm-tools
dotnet publish SudokuSolverWasm/SudokuSolverWasm.csproj -c Release -o <output>
```

Run the comparison with:

```text
python -u benchmarks/quick_compare_wasm_rangsk.py \
  --bundle <output>/wwwroot \
  --upstream-root <SudokuSolver-checkout> \
  --rust-repeats 100 --wasm-warmup-rounds 20 --wasm-repeats 100 \
  --output benchmarks/result.json
```

The full raw samples are in `quick_compare_wasm_rangsk_2026-08-19.json`.
The native and WASM count-only files come from the branch's unmodified
`BenchCore` with 100 repetitions. The native build-plus-count diagnostic and
its minimal `BenchCore` timing patch are also retained alongside the harness.

For artifact identification, the AOT solver modules used by this run have
SHA-256 values `A4691081DF4EC61D137900778D86296E9A4CEB0DFA278FAFE7FFF0BF02A665D4`
(`SudokuSolver`) and
`B41CE2D4E456CBCE39E0CE9450D0F765AA049F01D093E8CD1137B97FD81E4B7D`
(`SudokuSolverWasm`).
