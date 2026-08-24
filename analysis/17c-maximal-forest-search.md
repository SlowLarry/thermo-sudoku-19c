# Exact maximal-forest search for 17-cell thermo coverage

This note records the exact search developed after the complete 9+8 and
three-thermometer catalogue scans. It covers the four- through eight-
thermometer layers without extending the earlier labelled-role DFS directly.
The merge-maximal eight-path representatives are complete; global existence
work in this disjoint fallback can continue with four through seven paths.

## Reduction

Let a unique thermo layout cover exactly 17 cells, and let `S` be its unique
solution. Revealing the values of `S` on those 17 cells gives a unique ordinary
17-clue Sudoku: every completion of those givens necessarily satisfies all of
the thermometers. Therefore its clue pattern occurs, up to a Sudoku morph and
digit relabelling, in the complete 49,158-entry catalogue.

All nine digits must occur on the covered cells. If one were absent, swapping
it with an adjacent present digit in `S` would preserve every inequality and
produce another solution.

The thermometers induce a partial order on the nine digit symbols. A finite
partial order has a unique linear extension only if every consecutive pair in
that extension is comparable. With no symbol between consecutive ranks, that
comparison must be a physical thermometer edge. Thus every candidate contains
eight distinguished edges, one for each consecutive pair in one global symbol
order.

Removing every other edge from the thermometers leaves a directed linear
forest with 17 vertices, eight edges, and exactly nine components. A layout
with `k` thermometers has `17-k` edges, so it is obtained by adding only
`9-k` component joins:

| Thermometers | Extra joins |
| ---: | ---: |
| 8 | 1 |
| 7 | 2 |
| 6 | 3 |
| 5 | 4 |
| 4 | 5 |

This replaces the factorial assignment of clue occurrences to labelled path
roles with one common, component-unlabelled forest search.

## Why only merge-maximal layouts are needed

Suppose a target-true king-neighbour edge joins the tip of one path to the bulb
of another. Because each path is increasing, the bridge is legal precisely
when the maximum digit rank of the first path is below the minimum rank of the
second. Their digit sets are then disjoint, so the merged path has length at
most nine. Adding that comparison preserves uniqueness.

Repeatedly merge any such pair. A hypothetical unique layout either reaches a
merge-maximal layout with four through eight paths, or reaches the already
excluded two-/three-path strata. Consequently, searching only merge-maximal
layouts is complete for the existence question once those lower strata are
accepted as prerequisites. A negative result is a cumulative statement about
the union of the strata, not an isolated claim that every non-maximal member of
one partition was explicitly visited.

## Enumerator

For each eligible catalogue record the scanner:

1. chooses the global order of its nine source symbols;
2. chooses one concrete occurrence edge for each consecutive symbol pair;
3. incrementally intersects independent 1,296-bit row- and column-morph
   domains after every physical edge;
4. maintains the directed degree-one forest and rejects repeated symbols,
   cycles, overlong components, and impossible isolated-cell counts;
5. extends the nine-component skeleton by one through five ordered legal
   component joins;
6. rejects a final forest unless it is merge-maximal and uses the canonical
   distinguished edge for every adjacent-rank category; and
7. asks the exact Rust Sudoku/thermo solver for at most two solutions.

The simultaneous reversal of the global symbol order and all arcs is a
fixed-point-free symmetry. Enforcing the endpoint order before testing the
eighth physical edge reduced the 100-record eight-path pilot from 15.7 seconds
to 6.6 seconds with identical survivors. Spatial D4 canonicalization was made
optional: it removed only about 0.14% of candidate occurrences while costing a
full 16 transforms per candidate. Catalogue records are sharded by fixed line
ranges; this affects scheduling only.

Two plausible shortcuts were measured and rejected:

- A cache of globally valid two-grid pair cuts avoided some Sudoku solves but
  cost more in containment lookups at every tested cache size.
- Starting from the known catalogue target and searching only for a different
  solution was exact, but the biased traversal took 135.4 seconds versus 75.6
  seconds for ordinary cap-two search on the same 100-record four-to-seven-path
  block.

### Restart-safe high-path scheduling

The first combined `k=4..7` run exposed a severe record-level heavy tail: one
catalogue record could occupy a worker for hours, while a fixed 3,072-line
shard wrote no durable completion state until it exited.  That exploratory run
was stopped after ten of sixteen shards completed.  Its ten outputs and the
exact scanner source are frozen under
`analysis/17c-maximal-stopped-2026-08-23/`; they are explicitly partial and are
not a negative result for the remaining strata.

The replacement launcher, `analysis/run_17c_maximal_chunks.py`, runs exactly
one path count at a time, in the order `k=7`, `k=6`, `k=5`, then `k=4`.  It
partitions the complete catalogue into small ranges containing a fixed number
of eligible records, assigns those ranges dynamically to independent workers,
validates each completed JSONL, and atomically publishes it.  Existing valid
chunks are skipped on restart.  An immutable run identity binds a directory to
the corpus hash, executable hash, layer, and chunk size, preventing results
from different binaries or scopes from being mixed.

For example, the first layer is launched from the repository root with:

```text
python analysis/run_17c_maximal_chunks.py \
  --corpus <path-to>/17puz49158.txt \
  --binary thermo-sudoku-rs/target/release/thermo-17c-maximal.exe \
  --output-dir <artifact-root>/k7 \
  --paths 7 \
  --workers 4
```

The default durable chunk contains 16 eligible records.  `--max-new-chunks N`
is available for a bounded smoke test; using the same command again resumes
from the next unpublished chunk.

The scanner also now applies three exact early bounds:

- reject a digit-order prefix as soon as no unused symbol can be its
  reversal-canonical final symbol;
- raise the per-record minimum path count to the maximum multiplicity of any
  source digit; and
- reject an extra adjacent-rank edge as soon as its code is smaller than the
  distinguished mandatory edge for that rank pair.  The rejected edge is
  still counted for the parent layout's maximality test.

On the same 100-eligible-record `k=4..7` pilot these bounds reduced elapsed
time from 96.198 to 73.900 seconds (23.2%), with all 39 partition counts and
all 246,253 classifications identical.  The raw benchmark artifacts and exact
hashes are under `analysis/17c-maximal-benchmarks/`.

## Completed merge-maximal eight-path representatives

The sole eight-path partition is `3+2+2+2+2+2+2+2`. Four non-overlapping
catalogue shards covered all 49,158 records and all 25,370 records containing
all nine digits. They produced 151,631 maximal layout occurrences. Every one
was multiple; none was unique.

This is not a standalone exclusion of every eight-path layout. A hypothetical
unique non-maximal eight-path layout can be merged to a unique layout with
fewer paths, as proved above. The completed maximal representatives therefore
remove the eight-path layer from the remaining **global existence workload**,
provided the lower-path layers are still searched; they do not certify that
each non-maximal eight-path member is itself multiple.

Unlike the much larger lower-path run, this layer retained both Sudoku
solutions for every occurrence. `analysis/verify_17c_maximal.py` independently
checked all 303,262 grids, all thermometers, catalogue identities, morphs,
digit orders, non-overlapping shard ranges, and summary counts. The four large
JSONL artifacts are local temporary research artifacts and are not intended
for the Git repository.

## Certificate boundary

Each emitted pair of distinct Sudoku grids is a direct, independently
checkable multiplicity certificate for its layout. Exhaustiveness is a
deterministic program result resting on the reduction and implementation; it
is not a SAT/LRAT-style independently checkable nonexistence proof. Any final
negative 17-cell conclusion must also cite the catalogue completeness and the
completed 9+8 and three-path scans.

This disjoint-path enumerator is now the conservative fallback rather than the
preferred next full run. If overlapping two-cell thermometers are admitted,
the saturated-network theorem in `17c-overlap-search.md` tests a strict
superset of all 40 post-three-path disjoint partitions, including non-maximal
eight-path layouts, without enumerating them separately. A complete negative
saturated scan would therefore make resuming the slower disjoint shards
unnecessary.
