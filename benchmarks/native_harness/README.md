# Native timing harness for Rangsk's `wasm-prototype`

This small .NET program benchmarks the native CLR build of
`dclamage/SudokuSolver`. The Python driver builds it against a specified
upstream checkout by supplying the `SudokuSolverProject` MSBuild property; the
solver source itself is not modified.

Here, "native" has the same meaning as the upstream benchmark documentation's
`dotnet run -c Release`: Release IL executed by the desktop CLR/JIT with
ServerGC. It does not mean .NET NativeAOT. The driver records the runtime's
actual GC mode rather than inferring it from the project setting.

The primary measurement is a fresh
`SolverFactory.CreateBlank(9, constraints)` followed by
`CountSolutions(maxSolutions: 2, multiThread: false)`. Every measured proposal
therefore constructs and finalizes its own thermo geometry, as the 19-cell
search must. Construction and counting also have separate timestamps. The
count-only number is a diagnostic decomposition, not the primary comparison:
an annealing proposal cannot reuse a solver built for a different layout.

The harness deliberately does not force garbage collection. It warms the full
corpus, then measures it round-robin so that natural JIT, GC, and temperature
effects are spread over the 25 cases. JSON parsing and process startup are
outside the timed region.

Run through `benchmarks/quick_compare_native_rangsk.py`; do not invoke this
project directly unless supplying an absolute upstream project reference:

```text
dotnet build -c Release \
  -p:SudokuSolverProject=C:/path/to/SudokuSolver/SudokuSolver/SudokuSolver.csproj \
  benchmarks/native_harness/NativeThermoBench.csproj
```

The driver refuses tracked changes in the upstream checkout and untracked files
under its solver project by default. It performs a Release `Rebuild` into a
unique revision-prefixed output directory. Its report records the upstream
commit and solver tree and hashes the driver, harness, and solver assemblies.
The unique build directory prevents a binary from another checkout or
concurrent benchmark from being reused accidentally.
