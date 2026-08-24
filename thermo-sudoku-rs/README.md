# thermo-sudoku

Dependency-free Rust reference solvers for classic 9x9 Sudoku with strict
thermometer inequalities. The original `Solver` is specialized for
cell-disjoint paths; `ComparisonSolver` handles shared cells, overlaps, and
branching networks. Orthogonal and diagonal king-neighbour steps are allowed,
and geometrically crossing diagonal segments are not cell overlaps.

The solver returns a capped count and is optimized for the `0 / 1 / 2+` query.
It uses 9-bit candidate domains, a deduplicating event queue, bit-parallel
Sudoku-house propagation, and thermo-aware inherited branch ordering. A
thermometer is revised by one forward lower-bound sweep and one backward
upper-bound sweep. Because a thermometer is a simple constraint path, this
arc-consistency pass is also generalized arc consistency for the complete
increasing sequence.

`ComparisonSolver` is not a mode switch on the disjoint-path representation.
It stores deduplicated directed comparisons explicitly, together with one
`u64` incident-edge mask per cell and one `u64` dirty frontier for the whole
network. Restricting a domain schedules every edge incident to that cell; each
dirty `A < B` edge removes unsupported values from both endpoints and may in
turn schedule all neighbouring edges. The ordinary singleton and Sudoku-house
queues run in the same fixpoint loop, followed by exact depth-first search.
This is what makes shared cells and branches exact without introducing a
general-purpose constraint framework. Inputs which are genuinely disjoint
paths may still delegate to the original faster `Solver`. The public backend
rejects graphs above 64 distinct comparisons; this is complete for the exact
17-cell search because any 17 grid cells induce at most 46 undirected
king-neighbour edges.

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

## Deterministic guided 9+8+2 search

`score_nine_eight_extensions` is the configurable-cap scoring counterpart to
the cap-two screen.  It shares a collective base-solution prefix across every
legal directed two-cell extension and then completes only unresolved edges
over the nine disjoint length-eight digit templates.  Every returned score is
either exact or the common stated lower bound; cap hits are never mislabeled
as exact.

`thermo-9x8-guided` uses that routine to search the long-path geometry.  It
rechecks and schedules all valid corpus anchors, deduplicates under D4 plus
global path reversal/digit complement, uses an elitist deterministic beam, and
globally reoptimizes the two-cell path after each one-cell long-path move.
Unconstrained legal moves are the default; the narrower
`--solution-preserving-moves` mode is experimental.  The opt-in
`--two-cell-reroutes` neighborhood also replaces consecutive pairs of long-path
cells, reaching legal bases which have no legal one-cell intermediate.  A
typical bounded run is:

```text
thermo-9x8-guided.exe \
  --input ../sources/min_thermos_9_8_2.txt \
  --output guided.jsonl --checkpoint guided.state \
  --gradient-caps 8,32,128 --max-base-evaluations 10000 \
  --pair-seed-checkpoint guided-pairs.checkpoint \
  --pair-seed-solution-cutoff 65 --pair-seed-pairs-per-anchor 64
```

The optional pair file contains only pairs from layouts whose solutions were
fully enumerated.  Shorter pair cuts are retained first and the global pair
set is deterministic.  `thermo-topology-cnf merge-checkpoints` validates and
deduplicates such a file against an existing CEGIS checkpoint.  The local
gradient affects discovery only; it never becomes a proof premise.

On 2026-08-21 the reroute-enabled search found a unique disjoint 9+8+2 layout
covering exactly 19 cells at base evaluation 15,251. Multiple independent
solvers confirmed the sole solution. The paths, solution, run provenance, and
verification results are in `../analysis/unique-19c-9x8x2-2026-08-21.md`.

## Exact 17-cell classic-morph search

`thermo-17c-morph` searches the complete set of essentially different
17-clue classics for a thermo-only representation. A unique thermo puzzle on
17 covered cells necessarily induces a unique classic when those cells are
fixed, so this catalogue reduction is complete up to standard Sudoku morphs
and digit relabelling.

The initial implemented scope is exact `9+8`. Its digit multiplicity filter
reduces the 49,158-record corpus to only ten records. The scanner folds the
global digit relabelling into the nine-path order and carries all 1,296 row and
1,296 column morphs as bitset domains, intersecting them after each prospective
king step. A full scan is:

```text
thermo-17c-morph.exe \
  --input <path-to>/17puz49158.txt \
  --end-line 49158 \
  --output 17c-9x8.jsonl --progress-every 0
```

The 2026-08-21 run found zero spatially realizable `9+8` covers. This closes
the sole two-path / 15-comparison stratum. See
`../analysis/17c-classic-morph-search.md` for the input hashes, proof reduction,
deterministic result, and independent reference scan. Passing
`--reference-direct` reruns a slower independent algorithm over all
16,796,160 fixed row/column geometries; it also finds zero covers.

`thermo-17c-three-path` covers the complete next layer: all ten partitions of
17 cells into three paths, each carrying 14 comparisons. A full run is:

```text
thermo-17c-three-path.exe \
  --input <path-to>/17puz49158.txt \
  --partition all \
  --output 17c-three-path.jsonl --progress-every 0
```

The complete scan found 337 distinct spatially realizable layouts. Every one
had at least two solutions; the JSONL retains both grids as direct witnesses.
All ten partition summaries report `complete:true` and zero unique layouts.
`--max-eligible` is for bounded pilots only and cannot support that exhaustive
claim. The result closes the two- and three-thermometer strata, but 40 disjoint
partitions initially remained. The later scan classified all 151,631
merge-maximal eight-path occurrences as multiple. A unique non-maximal
eight-path layout would extend to a unique lower-path dominator, so the global
disjoint existence search now needs only the 39 four- through seven-path
partitions. This is not a standalone exclusion of every eight-path layout. The
optimized `thermo-17c-maximal` fallback searches the remaining merge-maximal
representatives and
`analysis/run_17c_maximal_chunks.py` schedules restart-safe small ranges.

`ComparisonSolver` is the exact generalized oracle for shared cells,
overlapping paths, and branching comparison networks. `thermo-17c-overlap`
uses it with a saturation theorem: for each catalogue morph and relabelling it
tests the network containing every target-true king-neighbour comparison on
the 17 cells. If any smaller overlapping network were unique, this stronger
network would be unique as well; when arbitrary two-cell thermometers are
allowed, the stronger network is itself admissible. Exact row/column-mask and
transitive-poset dominance remove weaker realizations before solving. The
dynamic launcher is:

```text
python analysis/run_17c_overlap_chunks.py \
  --corpus <path-to>/17puz49158.txt \
  --binary thermo-sudoku-rs/target/release/thermo-17c-overlap.exe \
  --output-dir <artifact-root>/overlap-exact \
  --workers 4 --eligible-per-chunk 16
```

The first full exact run began on 2026-08-23 with 1,586 chunks over 25,370
eligible catalogue records. It exposed a much heavier runtime tail than the
100-record timing pilot: most chunks are short, while a few can occupy a core
for many hours. Completed chunks are independently validated before atomic
publication and are skipped on restart. A hard stop preserves them but must
redo any in-flight chunks, so suspension or reduced process priority is safer
during temporary workstation use.

The run directory is intentionally ignored and local. No complete generalized
17-cell conclusion is claimed until its `summary.json` says `complete:true`,
`completed_chunks:1586`, and the aggregate has been independently audited.
Re-running the identical command validates every published chunk before doing
any remaining work. See
`../analysis/17c-overlap-search.md` for the proof reduction, measurements, and
scope boundary.

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
Passing `--topology-scope exact-9+8+2` instead restricts that same audited
pipeline to exactly three paths of lengths 9, 8, and 2.  The scoped formula has
9,656 variables and 69,959 base clauses before the optional 148-clause
D4-times-complement symmetry breaker; scope is bound into lazy manifests so
artifacts cannot silently cross between formulas.
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
The exact topology runs' current validated state follows ten completed
1,000-iteration runs plus 556 additional refinement batches: 22,846,872
solution pairs and 20,872,205 unique cuts, with neither a unique candidate nor
UNSAT reached in that lane. The guided lane later supplied the independently
verified unique candidate above; the topology run itself remains unexhausted.
The final large checkpoint was reread by the independent `stats` path; the
most recent trio has not been rerun through the substantially more
memory-intensive cross-language Python verifier.

Counts use a limit of at least two. Reaching the limit is reported as a lower
bound; a count below it is exact. The Rust API retains the first two witness
solutions, which will support solution-pair cuts in the later CEGIS search.

The default release build is portable. For a binary that will only run on the
machine that builds it, `-C target-cpu=native` can be supplied through
`RUSTFLAGS`; this provided a further roughly 8% improvement on the development
machine, at the cost of portability.
