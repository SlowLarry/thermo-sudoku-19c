# Rust versus Rangsk's native .NET solver

This is the primary Rangsk comparison for the 19-cell search. Rangsk's advice
to benchmark the native build was correct: WebAssembly adds a substantial host
penalty and should not be used to characterize the underlying C# solver.

The primary run uses `dclamage/SudokuSolver`'s `wasm-prototype` revision
`5b3f1a69f915153e3251568e1b4c64119a393e3d`, exactly the solver revision used
in the earlier WASM test. "Native" means the upstream project's documented
Release CLR/JIT mode (`dotnet run -c Release`) with ServerGC, not .NET
NativeAOT.

## Method

Both engines were single-threaded and classified the same 25 layouts as 0, 1,
or 2+:

- Blue's unique 20-cell puzzle;
- all fourteen saved 3-solution layouts;
- ten deterministic distinct layouts spread through the result file.

Every classification agreed. The corpus SHA-256 is
`79BEC9AD12BF7C3C6CB28948E1C54CD98809929D5FE5A3003A8C6215367046A7`.

The report merges three independent runs. Each run gave each engine twenty
unmeasured full-corpus warm-up rounds and one hundred measured round-robin
rounds, for 300 samples per case after merging. No run forced garbage
collection. Process startup, JSON, Python validation, constraint-string
creation, and ctypes-buffer creation were outside the timers.

The primary timing scope is fresh construction plus capped counting:

- native .NET: `SolverFactory.CreateBlank(9, thermoStrings)` followed by
  `CountSolutions(maxSolutions: 2, multiThread: false)`;
- Rust: the uncached FFI call, including `Layout` construction, `Solver`
  construction, and `count_up_to(2)`.

This scope represents the search workload: a new thermo layout cannot reuse a
solver constructed for the previous proposal. Native construction and counting
are also timestamped separately. The count-only column is a decomposition, not
an alternative headline or a like-for-like comparison with a Rust count-only
API.

The host was an Intel Core i5-6500 with four logical processors on Windows
10.0.19045. The run used Rust 1.94.0, Rust revision `743783f`, .NET SDK
10.0.400, and the native .NET 10.0.11 runtime. The runtime reported ServerGC.

## Result

| Group | Cases | Rust fresh total median | Native fresh total median | Aggregate Rust speedup | Native build median | Native count median |
|---|---:|---:|---:|---:|---:|---:|
| Blue unique | 1 | 0.097 ms | 0.986 ms | 10.18x | 0.541 ms | 0.379 ms |
| Saved count-3 | 14 | 0.030 ms | 0.904 ms | 30.21x | 0.541 ms | 0.340 ms |
| Spread sample | 10 | 0.040 ms | 0.985 ms | 25.11x | 0.546 ms | 0.395 ms |
| All | 25 | 0.031 ms | 0.916 ms | **25.89x** | 0.543 ms | 0.347 ms |

The aggregate speedup is the ratio of the sums of the 25 per-case medians:
23.90475 ms native versus 0.923300 ms Rust. It is therefore not the ratio of
the two displayed all-case medians. Across all 7,500 measured calls per engine,
the raw timed-work ratio was 24.40x. Native solver construction accounted for
57.86% of its raw timed work and capped counting for 42.14%.

The specialized Rust solver is thus still about 26 times faster for the actual
fresh-layout screening operation, even after removing the WebAssembly host
penalty. This comparison describes Rangsk's general public construction path,
including thermo-string parsing and generic constraint finalization; it does
not predict a future direct or incremental native API specialized for changing
thermo geometry.

## Current-upstream check

Revision `b588ea4c6b58aa51ab78f6b1b3fdfa55faaf80c5` was tested separately with the
same protocol. Its changes since the pinned revision are confined to Renban and
Whispers handling, not thermometer or core search code. It gives the same
conclusion:

| Group | Cases | Rust fresh total median | Current native total median | Aggregate Rust speedup | Native build median | Native count median |
|---|---:|---:|---:|---:|---:|---:|
| Blue unique | 1 | 0.095 ms | 0.920 ms | 9.69x | 0.529 ms | 0.360 ms |
| Saved count-3 | 14 | 0.029 ms | 0.874 ms | 30.23x | 0.525 ms | 0.335 ms |
| Spread sample | 10 | 0.038 ms | 0.959 ms | 24.88x | 0.524 ms | 0.387 ms |
| All | 25 | 0.029 ms | 0.882 ms | **25.70x** | 0.525 ms | 0.339 ms |

Its sums of per-case medians were 22.82965 ms native and 0.888250 ms Rust.
The modest difference from the pinned run does not change the performance
assessment.

## Relation to the earlier WASM result

Using the old pinned WASM samples and the new pinned native medians, the WASM
fresh build-plus-count total is 5.36x the native total by sums of per-case
medians. The corresponding count-phase diagnostic is 3.85x. These are
cross-session ratios, not paired measurements, but they confirm that WASM was a
material confounder.

Do not multiply these ratios into the old 30.18x WASM-versus-Rust headline: the
Rust solver was substantially optimized between the two runs. The defensible
current headline is the fresh, paired native-versus-Rust result above.

## Reproduction and artifacts

The retained harness does not modify the upstream solver. The driver rejects
tracked upstream changes, performs a Release rebuild in an isolated output
directory, uses a persistent native process, and records the upstream commit,
solver tree, runtime GC mode, case manifest, and binary hashes.

Run three independent reports:

```text
python -u benchmarks/quick_compare_native_rangsk.py \
  --upstream-root C:/path/to/SudokuSolver \
  --dotnet C:/path/to/dotnet \
  --output C:/temp/native-rangsk-1.json
```

Then merge them without discarding raw samples:

```text
python -u benchmarks/merge_native_rangsk_runs.py \
  C:/temp/native-rangsk-1.json \
  C:/temp/native-rangsk-2.json \
  C:/temp/native-rangsk-3.json \
  --output benchmarks/quick_compare_native_rangsk_merged.json
```

The full 300-sample primary report is
`quick_compare_native_rangsk_2026-08-20.json`. The current-upstream check is
`quick_compare_native_rangsk_current_2026-08-20.json`.
