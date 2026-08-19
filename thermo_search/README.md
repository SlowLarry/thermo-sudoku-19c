# Thermometer search

`thermo_anneal.py` is the maintained replacement for the exploratory notebook
under `sources/` (which remains read-only). It defaults to classic Sudoku,
diagonal-or-orthogonal king-neighbour thermometer steps, and no overlaps.

Build the in-process Rust backend first:

```text
cargo build --release --manifest-path thermo-sudoku-rs/Cargo.toml
```

Check the first record in the legacy result file:

```text
python thermo_search/thermo_anneal.py check \
  --input sources/min_thermos_9_8_2.txt --line 1 --cap 4
```

Run a bounded, seeded anneal and write a new JSONL log:

```text
python thermo_search/thermo_anneal.py anneal \
  --input sources/min_thermos_9_8_2.txt --line 1 \
  --seed 20260819 --output runs/example.jsonl
```

For cross-checking, select the existing console solver explicitly:

```text
python thermo_search/thermo_anneal.py check --backend console \
  --solver C:/path/to/SudokuSolverConsole.exe \
  --input sources/min_thermos_9_8_2.txt --line 1 --cap 4
```

The script never silently adds anti-knight or any other variant constraint.
Console-only extras require an explicit `--extra-constraint` option.

Recount the complete saved corpus and reject malformed geometry:

```text
python thermo_search/thermo_anneal.py validate-corpus \
  --input sources/min_thermos_9_8_2.txt
```
