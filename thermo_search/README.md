# Python thermometer search

`thermo_anneal.py` provides seeded local search, exact recounting, and corpus
validation for cell-disjoint thermometer layouts. It uses standard Sudoku,
orthogonal or diagonal king-neighbour steps, and no extra variant constraint
unless one is explicitly requested.

Build the in-process Rust backend:

```text
cargo build --release --manifest-path thermo-sudoku-rs/Cargo.toml
```

Check one saved layout:

```text
python thermo_search/thermo_anneal.py check --input sources/min_thermos_9_8_2.txt --line 1 --cap 4
```

Run a bounded reproducible search:

```text
python thermo_search/thermo_anneal.py anneal --input sources/min_thermos_9_8_2.txt --line 1 --seed 20260819 --output runs/example.jsonl
```

Validate every saved record and reject malformed geometry:

```text
python thermo_search/thermo_anneal.py validate-corpus --input sources/min_thermos_9_8_2.txt
```

The checked-in input currently contains 1,279 valid cell-disjoint records and
one record with a shared cell. Validation reports that geometry error and exits
nonzero; it does not modify the input.

The Rust backend is the default. An external console solver can be selected for
cross-checking:

```text
python thermo_search/thermo_anneal.py check --backend console --solver C:/path/to/SudokuSolverConsole.exe --input sources/min_thermos_9_8_2.txt --line 1 --cap 4
```

Console-only constraints require an explicit `--extra-constraint` argument;
the script never adds anti-knight or another variant rule implicitly. Files
under `sources/` are read-only inputs, and generated logs belong under `runs/`.
