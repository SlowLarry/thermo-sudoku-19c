# Target-free relaxed comparison CEGIS

This note specifies the exact relaxation that is useful before imposing
thermometer geometry.  It also records the soundness conditions for a
dependency-free implementation.

## Why sixteen comparisons cover the 19-cell question

A non-overlapping thermometer of length `L` contributes `L - 1` strict
comparisons.  If `k` thermometers cover 19 cells, they therefore contribute
`19 - k` comparisons.  A strictly increasing thermometer has length at most
nine, so 19 cells require at least three thermometers and hence at most sixteen
comparisons.

The relaxed universe consists of every directed strict comparison between
king-adjacent cells.  It ignores path, degree, overlap, and component
conditions.  Consequently:

> If no set of at most sixteen directed king comparisons has a unique classic
> Sudoku solution, then no non-overlapping 19-cell thermometer puzzle exists.

There are 272 unordered king adjacencies (72 horizontal, 72 vertical, and 128
diagonal), hence 544 directed comparisons.  Both directions must be separate
master variables.  An unordered variable whose direction is chosen later by
an unknown target is not an equivalent model.

Even after forbidding selection of both directions of one adjacency, there are
`C(272,16) * 2^16`, about `1.79e30`, exact sixteen-comparison sets.  Direct
enumeration is therefore not a realistic baseline; the value of CEGIS is that
each witnessed pair excludes a structured family of sets at once.

Searching exactly sixteen is equivalent to searching at most sixteen in this
relaxation.  If `q` of size at most sixteen uniquely determines target `T`, add
unused comparisons satisfied by `T` until the size is sixteen.  The target
still satisfies the enlarged set, and no alternative can reappear.  There are
always enough such comparisons: the 144 orthogonal adjacencies join cells in
a common row or column, so every Sudoku target satisfies one direction on each
of them.

## Target-free pair cuts

For a solved classic Sudoku `G`, let `A(G)` be the directed comparisons it
satisfies.  For two *distinct* solved grids `G` and `H`, define

```text
C(G,H) = U ∖ (A(G) ∩ A(H)).
```

Equivalently, `C(G,H)` contains every directed comparison violated by at least
one of the two grids.  A selected set `q` permits both grids precisely when
`q` misses this cut.  Therefore every unique `q` must satisfy

```text
q ∩ C(G,H) != empty
```

for every pair of classic Sudoku solutions.  This is a positive hitting-set
clause and does not assume a target.

Equality needs no special case in the proof: if two adjacent cells have equal
digits in a grid, that grid violates both directions.  Duplicate grids must be
removed before forming pairs.  A self-pair would produce an invalid condition
that could wrongly eliminate that grid as the eventual target.

## A combined master with an existential Sudoku witness

The simplest complete dependency-free master is a hitting-set DFS with a
Sudoku feasibility callback:

1. Search for `q`, `|q| <= 16`, that hits every accumulated pair cut.
2. When a DFS node first hits all cuts, run the exact Sudoku oracle for one
   solution satisfying `q`.
3. If the oracle exhausts with no solution, prune the node.  Every superset of
   an unsatisfiable `q` is also unsatisfiable.
4. If it returns witness `T`, return the pair `(q,T)` from the master.
5. Pad `q` to exactly sixteen with unused comparisons satisfied by `T`, and
   enumerate solutions of that padded set.

This avoids a separate family of unverifiable "zero-solution" no-goods.  It is
also complete: the normal hitting-set branches enumerate all ways to hit a
pivot cut, while an unsatisfiable leaf cannot have a satisfiable descendant.
An oracle node limit is not an unsatisfiable result and must abort or suspend
the master rather than prune the node.

A convenient exact branching partition is the one already used by the fixed
target master.  For pivot choices `e1, ..., em`, branch `i` selects `ei` and
forbids `e1, ..., e(i-1)`.  Every possible hitting set occurs in exactly the
branch corresponding to its first selected pivot edge.  Residual disjoint-cut
packing and maximum-coverage bounds remain sound, provided they use only
currently available edges.

The first implementation can call the Sudoku solver only at all-cuts-hit
nodes.  A more practical exact variant carries a witness `T` satisfying the
current selected set at every DFS node.  Try pivot edges satisfied by `T`
first; those children inherit the witness without an oracle call.  When a
choice violates `T`, ask the exact oracle for a replacement witness satisfying
the enlarged selected set, pruning only after an exhausted zero-solution
result.  The prefix-forbidden branches are unchanged, so this ordering does
not fix the target or lose completeness.

Later, safe propagation can also be interleaved after selecting an edge.  The
selected comparison list is at most sixteen, so copying the 81-cell domain
state is inexpensive.  Structural rejections such as opposite directions, a
directed cycle, or a ten-cell strict chain are useful but are optimizations,
not replacements for the exact Sudoku witness check.

## Padding and the adjacent-digit-swap condition

For any target `T`, globally swap digits `d` and `d+1`.  The result is another
classic Sudoku.  All strict cell comparisons keep their truth value except
comparisons whose endpoint values are exactly `{d,d+1}`.  Thus a unique clue
set consistent with `T` must contain at least one such cell comparison for
every `d = 1, ..., 8`.

These eight categories are disjoint.  In a fixed-target candidate universe
they give eight disjoint trade cuts and a directly checkable lower bound of
eight.  If one category has no king-adjacent occurrence in `T`, no set of local
comparisons can make `T` unique.

For the target-free master, this is a witness-dependent condition, not eight
globally disjoint cuts: comparisons violated by `T` appear in many unrestricted
pair cuts.  Once witness `T` is known, padding should first add one
`T`-consistent edge from every missing consecutive-digit category.  The
remaining slots can be chosen by a deterministic discrimination heuristic.
The eight digit-swapped grids are also cheap, valid pair-cut seeds for every
new witness.

## Batched counterexamples

Enumerate a batch of distinct solutions satisfying the padded candidate.  If
the search exhausts with exactly one, the candidate is unique.  If at least two
are found, every pair in the batch yields a valid cut, and every such cut is
missed by both the padded candidate and its unpadded master core.  Adding all
`B(B-1)/2` pair cuts is therefore sound and guarantees progress.

Important status rules:

- Zero solutions after padding is an internal error because the padding
  witness satisfies every selected comparison.
- One solution followed by a node limit is inconclusive, not unique.
- Two or more solutions are sufficient to add cuts even if a later node limit
  stops the batch.
- Solutions and cuts should be deduplicated.  If one cut is a subset of
  another, only the smaller cut is needed.
- At least one newly generated cut must be missed by the old master `q`; this
  assertion catches indexing, direction, and duplicate-solution errors.

`B = 32` gives 496 pairs and is a sensible first pilot.  `B = 64` gives 2,016
pairs and may amortize oracle work better, but retaining every weak, large cut
can make the master memory-bound.  Periodic exact deduplication and
inclusion-minimal reduction should be measured before increasing the batch.

## Symmetry

Only symmetries preserving both classic Sudoku and the king-adjacency graph
are safe: the square's `D4` isometries, optionally combined with global digit
complement.  Complement reverses every directed comparison.  Arbitrary digit
relabeling does not preserve `<`, and ordinary Sudoku band, stack, row, or
column permutations do not generally preserve cell adjacency.

Applying a symmetry simultaneously to both grids of a witness pair gives
another valid cut, so adding a whole orbit is sound.  Canonicalizing a cut in
isolation need not refute the current candidate; retain the original cut if
orbit expansion is used.  Strong master symmetry breaking should wait until a
complete orbit/stabilizer argument exists.

## Termination and claims

Each non-unique iteration adds a pair cut missed by the current master set, so
that set cannot recur.  The combined master never returns an unsatisfiable
candidate.  Since the comparison universe and budget are finite, an unlimited
exact run eventually either finds a unique candidate or proves that the
accumulated cuts have no hitting set of size at most sixteen.  Resource limits
change either outcome to `inconclusive`.

A reproducible solver transcript is not automatically an independently
checkable proof.  A strong certificate should contain:

- the exact ordered 544-edge universe and its hash;
- every pair as two full grids, with cuts reconstructed by the verifier;
- the selected comparisons and unique target for a positive result;
- an independently verifiable master UNSAT proof for a negative result; and
- for uniqueness, a proof that Sudoku plus the comparisons plus `grid != T`
  is unsatisfiable.

Without an external proof-producing SAT/PB solver, the master can emit a DFS
proof tree.  A verifier can check pivot branch partitions and leaves justified
by an unhit cut, exhausted budget, a disjoint residual-cut packing, or a
recomputed coverage bound.  The Sudoku uniqueness side similarly needs either
an auditable exhaustive trace or should be described as a solver-verified
result rather than a proof certificate.

## Bounded first pilot

Use release mode, deterministic universe ordering, a fixed branch bias (which
must be labelled as an ordering heuristic, not a fixed target), atomic
checkpoints, and explicit master/oracle node limits.  Start with batch 32,
10,000 outer iterations, and periodic checkpoints.  Record pair-cut counts and
size distribution, selected core size, padded size, master nodes, oracle nodes,
and the number of witnesses rejected by resource limits.

Blue's known 20-cell construction is a useful positive-control family.  Its
three directed paths are

```text
(18,27,28,19,20,11,12,13,4)
(57,48,49)
(59,68,69,60,61,52,53,44)
```

The full seventeen comparisons have one solution.  Each of the seventeen
leave-one-out sixteen-comparison sets has multiple solutions, independently
confirmed by both the Rust solver and Rangsk at a cap of two.  At a Rust cap of
10,000, fifteen reach the cap; omitting `57<48` gives exactly 8,710 and omitting
`48<49` gives exactly 3,676.  These cases are therefore valuable regression
tests for direction handling, exact-one exhaustion, batched enumeration, and
all-pair cut generation, though they are not close candidates by raw solution
count.

Another exact local control starts from the first known three-solution 9+8+2
set.  Its entire one-edge replacement neighbourhood has
`16 * (544 - 16) = 8,448` distinct sets.  An ISS cap-two screen found 722
unsatisfiable, 7,726 multiple, and no unique replacement.  The independent
Rust arbitrary-comparison oracle reproduced all 8,448 classifications exactly
in its ignored exhaustive regression test.  This is useful target-free evidence
and a compact cross-implementation corpus, but it excludes only
Hamming-distance-one replacements of that one comparison set.  The
reproducible script and result are `analysis/search_score3_neighborhood.mjs`
and `analysis/score3-one-edge-neighborhood-2026-08-20.json`.

`analysis/verify_global_cegis_checkpoint.py` independently validates a saved
pair corpus: schema and budget, the 544-edge declaration, every complete
classic Sudoku, canonical pair ordering and uniqueness, pair count, and both
checksum declarations.  It reconstructs the evidence inputs but is not an
UNSAT-proof verifier; run it only after the writer has stopped, because Windows
cannot atomically replace a checkpoint while another process holds it open.

The pilot is successful if it validates steady non-repeating progress and
shows whether master search or Sudoku enumeration dominates.  Failure to find
a unique set within the pilot limits is not an exclusion of 19-cell thermos.

## Implemented topology stage

The subsequent `thermo-topology-cnf` binary now instantiates the geometric
master directly: selected edges have in- and outdegree at most one, incident
cells are counted with a limit of 19, strict inequalities eliminate cycles,
and all eight adjacent-symbol-swap necessities are tied to the existential
Sudoku target. It supports deterministic CNF emission, complete-model
validation, path decoding, and a bounded CaDiCaL-compatible oracle loop. The
first full-scale run and its still-inconclusive result are recorded in
`analysis/topology-sat-pilot.md`.
