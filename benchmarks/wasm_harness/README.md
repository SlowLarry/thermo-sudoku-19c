# Timing harness for Rangsk's `wasm-prototype`

`ThermoBenchInterop.cs` is a timing-only export for revision
`5b3f1a69f915153e3251568e1b4c64119a393e3d` of
`dclamage/SudokuSolver`'s `wasm-prototype` branch.

Copy it into the upstream `SudokuSolverWasm` directory before publishing. It
does not change solver behavior. It accepts the benchmark cases as one JSON
batch, keeps JSON and JS interop outside the timed regions, and records fresh
solver construction and capped counting separately for each call.

The addition is necessary because the branch's normal `BenchCore` constructs
the solver before starting its stopwatch. That search-only scope is useful for
the native-versus-WASM diagnostic but does not represent annealing proposals,
where every layout has different thermo geometry.

`BenchCore-build-plus-count.patch` records the separate native diagnostic that
moves the existing `Build(c)` call inside the upstream stopwatch. It is not
needed for the primary WASM batch.
