# Deterministic guided and exact 9+8+2 search

This note records the first partition-specific hybrid for three disjoint
thermometers of lengths 9, 8, and 2.  It has two deliberately separate lanes:

1. a deterministic solution-count gradient for finding a construction; and
2. an exact SAT/CEGIS master for eventual exhaustion of this partition.

The heuristic may change search order and may contribute independently
checkable Sudoku solution pairs.  It never supplies a pruning rule by itself.

## Why all corpus entries are useful

The source corpus contains many more useful basins than its fourteen
three-solution records.  The maintained loader parses all 1,280 lines, rejects
the known shared-cell geometry at line 1192, and independently recounts all
1,279 valid layouts at cap 1,025.  The declarations match exactly.  D4 plus
simultaneous path reversal/digit complement leaves 1,114 distinct full layouts
and 749 distinct length-9/length-8 bases.

Every full layout is verified.  Every canonical base receives a fixed place in
the starting schedule, ordered by the best verified full layout on that base.
This uses all the low-count information without repeatedly evaluating bases
which differ only in the short extension or by a safe symmetry.

## Gradient lane

For a fixed 9+8 base, a length-nine thermometer fixes its cells to 1 through 9
and a length-eight thermometer has only nine possible digit templates.  The
library routine `score_nine_eight_extensions` uses this structure to score all
legal directed two-cell extensions together:

- it enumerates a configurable collective prefix of base solutions once;
- every solution simultaneously updates every compatible two-cell edge;
- unresolved edges are completed independently over the at most nine disjoint
  classic 17-given templates; and
- each result is either exact or the stated common cap lower bound.

The guided binary changes one cell of a long path, rejects illegal or
overlapping geometry, canonicalizes the new base, and globally reoptimizes the
two-cell thermometer.  Candidate bases are compared at the staged common caps
8, 32, and 128.  The beam is deterministic and elitist: a worse generation
cannot discard a better parent.  Unconstrained legal moves are the default;
the narrower solution-preserving-up-to-symmetry mode is available only as an
experimental option.  An explicit anchor cursor and reserved evaluation budget
ensure that the current elite cannot starve unused corpus bases.

An opt-in `--two-cell-reroutes` move simultaneously replaces two consecutive
long-path cells and is interleaved with one-cell moves.  This adds genuine
escape edges: on the four-solution base reported below, 81 legal reroutes had
no legal two-step route through the one-cell neighborhood.  It remains a
heuristic extension and is evaluated separately rather than being described
as a connectivity proof.

The JSONL event stream and atomic search-state checkpoint make a bounded run
reproducible and resumable.  This checkpoint is heuristic state, not a proof
artifact.

Example:

```text
thermo-9x8-guided.exe \
  --input ../sources/min_thermos_9_8_2.txt \
  --output guided.jsonl --checkpoint guided.state \
  --anchor-cap 1025 --gradient-caps 8,32,128 \
  --collective-prefix 128 --beam-width 64 --anchor-batch 32 \
  --rounds 32 --max-base-evaluations 10000 \
  --candidates-per-round 256 \
  --pair-seed-checkpoint guided-pairs.checkpoint \
  --pair-seed-solution-cutoff 65 \
  --pair-seed-pairs-per-anchor 64
```

## Proof-safe bridge

For every exact layout at or below the configured seed cutoff, the guided lane
can enumerate all solutions.  It ranks unordered solution pairs by the length
of their 544-edge distinguishing cut, retains a bounded number of the shortest
cuts per layout, and globally deduplicates the canonical grid pairs.  The
output is the ordinary checksummed `thermo-global-cegis-v1` format.

For two distinct classic Sudoku grids `G` and `H`, any unique comparison puzzle
must select an edge on which they do not both agree.  That statement is global:
it does not depend on the layout which happened to expose the pair.  Therefore
these witnessed pair cuts are safe for both the relaxed and geometric masters.
The gradient score is not recorded as a proof premise.

`thermo-topology-cnf merge-checkpoints` validates every input, preserves the
base checkpoint's ordered pair sequence as an exact logical prefix, appends
only new canonical pairs, and keeps the first pair witness for each deduplicated
cut.  Merging the same seed again is idempotent.

## Exact partition master

The topology binary accepts `--topology-scope exact-9+8+2`.  In addition to the
generic Sudoku and directed-path variables it assigns occupied cells to three
labels, fixes the label sizes to 9, 8, and 2, forbids selected edges between
labels, and requires exactly three path sources.  Since the generic graph has
indegree and outdegree at most one and strict digit increase forbids cycles,
this forces exactly one path of each requested length.

The scoped structural formula has 9,656 variables and 69,959 clauses before
pair cuts, or 70,107 clauses with `d4-complement-v1`.  Its scope is bound into
lazy active-cut manifests.  The default at-most-19 formula and legacy manifest
bytes remain unchanged.

Example persistent exact run:

```text
thermo-topology-cnf.exe incremental-loop \
  --checkpoint merged.checkpoint --next-checkpoint next.checkpoint \
  --bridge-exe cadical-incremental-bridge.exe --cnf exact-982.cnf \
  --max-iterations 100 --oracle-batch 64 --pair-mode all \
  --prefer-selected --checkpoint-every 100 \
  --symmetry-break d4-complement-v1 \
  --topology-scope exact-9+8+2 \
  --lazy-cuts exact-982.active --lazy-active-seed 0 \
  --lazy-violation-batch 64
```

An incremental UNSAT result remains provisional.  A negative theorem still
requires a fresh frozen CNF, a proof-producing solve, and independent LRAT
verification, including the documented symmetry lemma when that breaker is
enabled.  A reported unique layout is independently rechecked by the Rust
oracle.

## Bounded pilot evidence

A conservative corpus-only seed run used the exact layouts with at most 65
solutions and retained at most 64 pairs per layout.  It finished in 3.996 s
and produced 33,833 distinct pairs, 29,679 distinct cuts, and FNV-1a
`f02a9b733dd047c6`.  Merging it into the 578,392-pair global pilot checkpoint
gave 612,225 pairs and 564,745 distinct cuts.

Two 100-candidate exact-9+8+2 runs were then made from the same global prefix,
one without this seed and one with it in the complete lazy pool.  In both
runs, every candidate hit the 65-solution enumeration cap.  Both learned the
same 208,000 new pairs and 191,796 distinct cuts, with identical aggregate
oracle node count (15,546).  The extra corpus cuts did not alter the first 100
models in this lazy-seed-zero experiment.  This is useful negative evidence:
the seed is sound and cheap, but its proof-search benefit is not established.

The unseeded run ended at its configured limit in 41.121 s; the seeded run in
34.536 s.  Those wall times are not a speed comparison because other search
work was sharing the four-logical-processor machine.  Neither run found a
unique puzzle or reached UNSAT.

### Gradient A/B

A 3,000-base breadth control scored all 749 canonical corpus bases and then
2,251 balanced one-hop mutations without child feedback.  The recursive
gradient treatment used the same total base budget but spent 2,704 evaluations
on mutations selected over successive generations.  At that cutoff both had
minimum mutation count five, but the guided lane reached the low-count tail
much more often:

| mutation score | breadth hits | guided hits | first breadth evaluation | first guided evaluation |
| --- | ---: | ---: | ---: | ---: |
| at most 8 | 3 | 12 | 2,008 | 392 |
| at most 16 | 7 | 26 | 1,449 | 208 |
| at most 32 | 12 | 70 | 1,449 | 106 |

The breadth control took 358.340 s.  Its time is not directly comparable with
the full guided run because the latter also enumerated proof-pair seeds.  The
count distribution is the useful result: deterministic gradient feedback
materially enriched low-count candidates, although it did not yet reach two
or one solution.

The clean elitist construction run completed all 32 configured rounds and
8,136 distinct base evaluations in 1,025.726 s.  It evaluated 7,657 one-cell
mutations: 621 had exact positive scores, 5,878 reached the cap of 128, and
1,158 bases admitted no positive two-cell extension.  Eighteen mutations had
at most eight solutions, 38 at most sixteen, and 100 at most thirty-two.  The
best mutation had exactly four solutions and first appeared at evaluation
3,234 (cells below are zero-based row-major):

```text
9: 2,3,12,11,19,28,37,27,36
8: 52,61,62,70,78,77,76,68
2: 16,24
```

An independent generic solver call reconfirmed its exact count of four.  The
run did not improve on the corpus minimum of three and found no unique layout.
It enlarged the proof-safe seed to 44,554 distinct solution pairs and 39,293
distinct cuts from 1,114 corpus layouts and 186 exact guided finalists; that
seed has FNV-1a `5d592fd86e476e05`.  The run stopped at its declared round
limit, so absence of a hit is not an exclusion.

The final seed cross-loaded through the topology checkpoint validator.  Its
merge added all 44,554 pairs and 39,293 cuts to the 578,392-pair global prefix,
producing 622,946 pairs, 574,359 cuts, and FNV-1a `9433cfe87208a049`.

### Reroute check

The complete neighborhood of the four-solution base was then scored with the
new opt-in reroute move.  Its 24 ordinary one-cell neighbors included two
layouts at count eight or below and had minimum five.  The 127 genuinely new
two-cell reroutes also had minimum five, with two at count eight or below.
Thus the extra neighborhood demonstrated real geometric reach but no fitness
advantage in this focused test.  It remains available behind
`--two-cell-reroutes` and stays off by default.

### Reroute-enabled discovery

A longer fresh run with `--two-cell-reroutes` enabled found a unique 19-cell
layout at round 57 and base evaluation 15,251, after 1,928.490030 seconds. The
winning candidate was a one-cell child, but it occurred only in the expanded
trajectory: the one-cell-only run had already exhausted its deterministic
frontier at 8,188 evaluations.

```text
9: 2,3,12,11,19,27,36,28,37
8: 52,61,62,70,78,77,76,68
2: 16,24
```

The generic Rust solver, ISS, retained native Rangsk builds, Rangsk's 1.3.188
console release, and an independent exhaustive DFS all returned exactly the
same sole solution. Full paths, solution, run hashes, and the precise proof
boundary are recorded in `analysis/unique-19c-9x8x2-2026-08-21.md`.

## Reproducibility

The final local SHA-256 values for this implementation are:

| Artifact | SHA-256 |
| --- | --- |
| `src/lib.rs` | `944BB51FBD6D953D47A7902983E11F67B762DF7E04F9221650B2D4119C84B911` |
| `src/bin/thermo-9x8-guided.rs` | `A22E2A5338CF3856A20949A1A6EB40EE380F16030FE95592399E53A5451A5FE6` |
| guided release executable | `E06842272A052E6183043DDB7C705AAA45F41D32E79F6D61AA2207C40BDB9634` |
| `src/bin/thermo-topology-cnf.rs` | `EDE9265562E4667BB44B07FFA9CCEF5ACBF384F77B7AFED9042989220B6F6EB7` |
| topology release executable | `8E9B73D98EE0416D255D093862B7F8ADA83226A90D744010F8751B97FFF0937B` |

The pilot JSONL, heuristic state, pair checkpoint, and merged checkpoint are
local temporary artifacts and are not committed.  Their sufficient counts and
checksums are recorded above; the pair files independently cross-loaded
through the exact topology checkpoint validator.

## Decision rule

The positive existence objective for this partition is complete: the guided
lane found a unique construction. Further positive search needs a new stated
objective, such as additional examples or fewer thermometers, rather than more
unbounded continuation. The negative/exclusion lane earns more work only if
proof-relevant frontiers close or active clauses begin materially restricting
models. A steady stream of capped candidates or novel pair cuts is not a
completion percentage and is not a reason for an open-ended run.
