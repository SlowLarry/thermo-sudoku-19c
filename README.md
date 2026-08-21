# 19-cell thermo Sudoku research

This workspace now contains two maintained components; the synchronized files
under `sources/` remain read-only.

**Status:** a unique 19-cell construction has been found in the 9+8+2
partition and independently verified by several exact solvers. Its paths,
solution, and discovery provenance are in
[`analysis/unique-19c-9x8x2-2026-08-21.md`](analysis/unique-19c-9x8x2-2026-08-21.md).

- `thermo_search/thermo_anneal.py`: corrected, bounded and reproducible
  simulated-annealing search. It can use the installed console solver for
  cross-checks or the in-process Rust backend for normal work.
- `thermo-sudoku-rs/`: dependency-free Rust solver for classic Sudoku plus
  strict, cell-disjoint thermometers. It returns capped solution counts and is
  designed first for `0 / 1 / 2+` classification.

The fixed geometry is:

- zero-based row-major cells;
- bulb-to-tip path order;
- orthogonal or diagonal king-neighbour steps;
- simple paths of length 2 through 9;
- no cell shared by different thermometers;
- diagonal segments may geometrically cross if they do not share a cell.

Quick verification:

```text
cargo test --release --manifest-path thermo-sudoku-rs/Cargo.toml
python -m unittest discover -s thermo_search -p "test_*.py" -v
python thermo_search/thermo_anneal.py validate-corpus \
  --input sources/min_thermos_9_8_2.txt
```

The last command intentionally exits nonzero for the supplied file because it
detects the overlapping record on line 1192; its summary should still show
`matched: 1279` and `mismatched: 0`.

The Rust implementation is specialized and auditable: 9-bit candidate domains,
an event-driven propagation queue, bit-parallel house checks, thermo-aware
branch ordering, and exact forward/backward propagation along each increasing
path. It also has an exact hybrid batch screen that fixes a base layout once,
shares solution witnesses among all legal two-cell extensions, and finishes
only the unresolved extensions independently.

The supplied corpus currently gives 1,279 exact count matches and one geometry
rejection: line 1192 shares cell 60 between two thermometers. No source record
is modified.

## Exact 9+8+2 pilot

The first symmetry-reduced native shard classified 99,389,208 directed
two-cell extensions across 257,776 canonical 9+8 bases. It found no unique
19-cell puzzle in that shard. The exact result, reproduction command, and scope
are recorded in `analysis/9x8-pilot.md`. Batch witness output can be checked
without trusting the solver by `analysis/verify_two_cell_certificate.py`.

The measured scale also confirms that direct base-by-base enumeration is not a
credible hobby-resource route to a global exclusion. This motivated the
trade-cut CEGIS and symbolic master for the relaxed sixteen-comparison problem
described below. A negative result for that relaxation would exclude every
disjoint thermometer layout covering at most 19 cells, regardless of its
length partition.

## Guided exact 9+8+2 portfolio

The `thermo-9x8-guided` binary now uses every valid layout in the low-solution
corpus as a verified starting point for a deterministic, elitist gradient
search.  A move changes one cell of the length-nine or length-eight path; the
specialized solver then scores every legal two-cell thermometer for that base
together.  Counts are refined through a common cap ladder, so differently
censored results are never compared as exact values.  Symmetry-equivalent full
layouts and bases are deduplicated, while a fixed anchor schedule ensures that
high-count corpus entries cannot be starved by the current elite beam.

This heuristic is paired with, but deliberately separated from, the proof
search.  It can export only fully enumerated Sudoku solution pairs as standard
global CEGIS cuts; the topology tool can merge those cuts into any validated
checkpoint.  Gradient scores themselves never prune anything.  The topology
master also has an opt-in `exact-9+8+2` scope, so continued SAT/CEGIS search is
deterministic and exhaustive for that partition even if the local search gets
stuck.  The design, commands, and bounded pilot results are in
`analysis/9x8-guided-hybrid.md`.

The first clean one-cell guided run evaluated 8,136 bases and found a
four-solution mutation. Its frontier then exhausted at 8,188 evaluations.
A fresh run with the opt-in two-cell reroutes escaped that neighborhood and
found a unique 19-cell 9+8+2 construction at evaluation 15,251 after about
32 minutes. The construction and its independent Rust, ISS, native Rangsk,
and standalone-DFS verification are recorded in
`analysis/unique-19c-9x8x2-2026-08-21.md`.

Separately, the first 100 exact-master candidates all still had at least 65
solutions. That negative-lane run remains only a bounded diagnostic, not an
exclusion.

## Fixed-target symbolic pilot

The `thermo-fixed-target` binary implements the first relaxed trade-cut CEGIS
stage for one chosen solved grid. It alternates a hitting-set master with an
exact classic-Sudoku oracle over arbitrary, possibly overlapping
king-neighbour comparisons. Its restartable pilot reached 2,983,306 explicit
alternative grids. A structural adjacent-digit-swap argument now certifies a
fixed-target lower bound of eight; the saved alternatives plus those seeds
still admit an 11-comparison hitting set. It therefore makes no fixed-target or
global exclusion claim. The precise scope, checkpoint hash, and next
target-free step are recorded in `analysis/fixed-target-pilot.md`.

## Target-free relaxed-16 pilot

The `thermo-global-cegis` binary implements the target-free relaxation: an
unknown Sudoku witness, 544 possible directed king-neighbour comparisons, a
joint exact hitting-set/Sudoku master, and an exact batched second-solution
checker. Its persisted pilot corpus contains 578,392 independently validated
solution pairs. That bounded pilot found no unique 16-comparison set and did
not exhaust its master. The later guided 9+8+2 construction is itself a unique
16-comparison witness, so existence is now settled positively even though the
master's exhaustive negative lane remains unfinished. The formulation,
commands, hashes, checkpoints, and present scaling boundary are recorded in
`analysis/global-cegis-pilot.md` and `analysis/target-free-cegis-design.md`.

## Non-overlapping topology SAT pilot

The `thermo-topology-cnf` binary turns the pair checkpoint into a deterministic
SAT master for the actual geometry: a cell-disjoint union of directed paths
covering at most 19 cells. It validates and decodes SAT models, calls the exact
Rust thermo oracle, and appends one checkable solution-pair cut for every
multiple candidate. Its persistent CaDiCaL mode instead learns batched
all-pair or anchor-pair cuts without restarting the SAT solver. The first
thirteen full-scale candidates were all multiple. Ten completed
1,000-iteration lazy runs, plus 556 additional validated refinement batches,
have now grown the corpus to 22,846,872 solution pairs and 20,872,205 unique
cuts. Every completed segment stopped at its configured iteration limit
without finding a unique candidate or reaching UNSAT. That exhaustive lane
remains unexhausted; the existence question was subsequently resolved
positively by the guided 9+8+2 construction above. The
encoding, full-scale run, hashes, and proof-verification path are recorded in
`analysis/topology-sat-pilot.md`.

Tdoku remains a useful architecture reference, but its public pencilmark
interface only accepts unary cell restrictions and cannot express a
thermometer's binary ordering constraints. This project therefore does not call
Tdoku and filter its solutions. No Tdoku source code has been copied here.

The primary reproducible comparison with Rangsk's native Release solver is in
[`benchmarks/NATIVE_RANGSK.md`](benchmarks/NATIVE_RANGSK.md), with complete raw
per-case samples alongside it. The older console and WebAssembly measurements
remain linked from [`benchmarks/README.md`](benchmarks/README.md).

## Source notes and acknowledgement

The notebook under `sources/` is preserved as historical exploratory material.
Its saved code contains stale anti-knight and API residue and should not be used
as the maintained classic-Sudoku search implementation. The accompanying
result corpus is classic Sudoku plus thermometers and is independently checked
by the Rust solver.

The earlier 20-cell positive-control and regression fixture is by **Blue** and
is included for research and demonstration with the creator's permission. The
new independently verified 19-cell construction is also retained as a solver
regression fixture.
