# thermo-sudoku

Dependency-free Rust reference solver for classic 9x9 Sudoku with strict,
cell-disjoint thermometers. Orthogonal and diagonal king-neighbour steps are
allowed; geometrically crossing diagonal segments are not treated as overlaps.

The solver returns a capped count and is optimized for the `0 / 1 / 2+` query.
It uses 9-bit candidate domains, a deduplicating event queue, bit-parallel
Sudoku-house propagation, and thermo-aware inherited branch ordering. A
thermometer is revised by one forward lower-bound sweep and one backward
upper-bound sweep. Because a thermometer is a simple constraint path, this
arc-consistency pass is also generalized arc consistency for the complete
increasing sequence.

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
Python search script through `ctypes`. Passing a null witness pointer avoids
constructing solutions when the caller only needs a count.

## Screening all short extensions

For a fixed base, the CLI can classify every directed king-neighbour
two-cell thermometer on uncovered cells:

```text
thermo-sudoku-cli.exe \
  --thermos "19,29,28,20,11,12,13,3,4|77,69,78,70,62,53,44,52" \
  --screen-two-cell --collective-prefix 128 --emit-certificate
```

The hybrid algorithm enumerates the requested number of base solutions once,
uses them as shared witnesses, then performs an independent cap-two search only
for edges that still need classification. `--collective-only` is an exact but
usually slower reference mode. `--nine-eight-templates` optionally expands a
blank 9+8 base into its at most nine compatible classic 17-given templates;
this specialization is exact, but was not faster than the generic propagator
on the pilot machine.

`analysis/verify_two_cell_certificate.py` independently checks the emitted
geometry, edge universe, Sudoku grids, and both witnesses for every `2+` edge.
Such witnesses prove exclusion only when every legal extension is `2+`; the
line format deliberately does not pretend to prove the upper bound on records
labelled `0` or `1`.

The `thermo-9x8-pilot` binary supplies deterministic path ranks, safe symmetry
canonicalization, sharding controls, flushed JSONL checkpoints, and resumable
base ranges.
The completed reference shard is documented in `analysis/9x8-pilot.md`.

The `thermo-fixed-target` binary is a separate symbolic pilot for arbitrary
overlapping king-neighbour comparisons true in one solved target grid. It has
a self-contained exact classic-Sudoku oracle, a capped hitting-set master,
batched counterexample generation, and restartable grid checkpoints. This is a
strict relaxation of thermometer geometry, and incomplete runs deliberately
report `provided-alternatives-only`, `target_scope=single-fixed-target`, and
`global_19c_conclusion=false`. See `analysis/fixed-target-pilot.md` for the
million-cut milestone and its limitations.

The `thermo-global-cegis` binary is the target-free next stage. It searches an
unknown Sudoku witness and exactly sixteen of all 544 directed king-neighbour
comparisons, learning globally valid cuts from pairs of complete solutions.
It supports explicit node limits, all-pair batching, checksummed atomic
checkpoints, batched checkpoint writes, and carefully scoped result labels.
The completed bounded pilot and its 578,392-pair evidence corpus are documented in
`analysis/global-cegis-pilot.md`; no global exclusion has been obtained.

The `thermo-topology-cnf` binary is the proof-oriented geometric stage. It
emits a deterministic SAT master for every non-overlapping thermometer union
covering at most 19 cells, validates and decodes complete SAT models, and can
run a bounded CaDiCaL-compatible CEGIS loop using the exact Rust thermo oracle.
Every multiple candidate adds a validated solution-pair cut to both the CNF
and a standard checkpoint. Its `incremental-loop` mode uses the persistent
CaDiCaL bridge in `tools/`, exact batched solution enumeration, `all` or
`anchor` pair learning, atomic checkpoint replacement, optional phase hints,
an optional versioned D4-times-complement symmetry breaker, allocation-free
bitset validation of retained cuts, and per-stage timing. Its exact lazy-cut
mode keeps the complete cut pool in Rust while loading only a small witnessed
active subset into CaDiCaL, scans the full pool before every oracle call, and
regenerates the terminal base-plus-active proof CNF from an atomic manifest.
Large checkpoints are parsed as a stream. Stored solution pairs use an exact
four-bit-per-digit representation, while compact `u32` probe tables reference
the canonical pair and cut vectors; every hash collision is resolved by full
key equality, so hashes are an accelerator rather than evidence. The external
checkpoint, manifest, CNF, FNV, and first-witness formats are unchanged.
Eager continuation reserve is capped, with larger runs growing in bounded
record chunks rather than allocating their full theoretical maximum up front.
Lazy activations are durably batched with the pair checkpoint, ordered
checkpoint-before-manifest so every crash restart sees compatible prefixes;
checkpoint write counts and timings are reported separately. See
`analysis/topology-sat-pilot.md` for the completed bounded full-scale runs,
artifact hashes, and independent formula audit, and `tools/README.md` for
bridge build/run instructions and the separate LRAT certificate path.
The current validated state follows ten completed 1,000-iteration runs plus
556 additional refinement batches: 22,846,872 solution pairs and 20,872,205
unique cuts, with neither a unique candidate nor UNSAT reached. The final
large checkpoint was reread by the independent `stats` path; the most recent
trio has not been rerun through the substantially more memory-intensive
cross-language Python verifier.

Counts use a limit of at least two. Reaching the limit is reported as a lower
bound; a count below it is exact. The Rust API retains the first two witness
solutions, which will support solution-pair cuts in the later CEGIS search.

The default release build is portable. For a binary that will only run on the
machine that builds it, `-C target-cpu=native` can be supplied through
`RUSTFLAGS`; this provided a further roughly 8% improvement on the development
machine, at the cost of portability.
