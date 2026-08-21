# Target-free relaxed-16 CEGIS pilot

This note records the first target-free symbolic pilot.  It is an implemented
research method and a reproducible progress checkpoint, not an exclusion of a
19-cell thermometer puzzle.

## Model and implementation

`thermo-global-cegis` searches exactly sixteen of the 544 directed strict
comparisons between king-adjacent cells.  The selected comparisons may overlap
and need not form paths, so this is a strict relaxation of non-overlapping
thermometer geometry.

The master carries a solved classic Sudoku satisfying its selected comparisons
and hits every accumulated solution-pair cut.  The checker then searches
exactly for further Sudokus satisfying the same comparisons.  Two distinct
solutions `G,H` add the globally valid cut

```text
U minus (comparisons true in both G and H).
```

The joint hitting-set/Sudoku search is exhaustive when its limits are removed.
With limits, every exit is labelled inconclusive.  Searching exactly sixteen
is sufficient: any unique set of fewer comparisons can be padded with unused
comparisons true in its unique witness without reintroducing a solution.

The binary also uses the exact-three 9+8+2 example as a branch-order bias,
learns adjacent-digit-swap pairs, batches alternatives, and stores every pair
in a checksummed, atomically replaced checkpoint.  The reasoning and
soundness conditions are detailed in `analysis/target-free-cegis-design.md`.

## Reproducible bounded run

One uninterrupted batch-16 run used:

```text
thermo-global-cegis.exe --max-iterations 1000 --oracle-batch 16 \
  --master-node-limit 2000000 --master-sudoku-node-limit 5000000 \
  --oracle-node-limit 5000000 --summary-only \
  --checkpoint analysis/thermo-global-cegis-pilot-1000-2026-08-20.checkpoint \
  --output analysis/thermo-global-cegis-pilot-1000-2026-08-20.txt
```

It stopped conservatively at the master-Sudoku node limit after 142 completed
iterations:

```text
pair cuts:                  6,483
master nodes:             296,379
master Sudoku nodes:   97,618,162
checker nodes:               2,241
elapsed:                   126.054 s
result:                    inconclusive
```

The checkpoint SHA-256 is
`32520CC618870B3C2C9CF040974C07E57B147DE6FAD251EE4176F1D4F853E584`;
the report SHA-256 is
`DD8CE9F2FA3ACE0AD181229AE557C1EA4CB53EB4A7110F3935C80B58E29AB088`.

## Deeper pair corpus

Batch-32 runs were resumed with both the original score-3 bias and an unbiased
root.  In-place cut partitioning and batched atomic checkpoints made the later
continuations substantially cheaper.  Thirteen further pairs came from the
topology-specific SAT loop described in `analysis/topology-sat-pilot.md`.  The
durable checkpoint now contains 578,392 distinct canonical pairs of complete
classic Sudoku grids:

```text
file:   analysis/thermo-global-cegis-pilot-1000x32-2026-08-20.checkpoint
size:   94,856,432 bytes
FNV:    3012692e445f3d19
SHA256: AD9A46E304E16D0C45930D618E968864AA641A57A49F36C67B67B8B75BC754A1
```

Because outer-run counters are not part of checkpoint schema v1, the compact
companion report is deliberately a zero-iteration replay.  It verifies the
pair count and checksum but does not invent cumulative iteration or node
totals.  Its SHA-256 is
`B5D7ABD40F64E302B3EF38E269D394154DEB58656D91C4E28773CFEC45CB1C51`.

The independent verifier checked all 578,392 pairs: both grids in every pair
are valid solved classic Sudokus, grids are distinct and canonically ordered,
no pair is duplicated, and the declared count, budget, edge count, FNV hash,
and footer agree:

```text
python -X utf8 analysis/verify_global_cegis_checkpoint.py \
  analysis/thermo-global-cegis-pilot-1000x32-2026-08-20.checkpoint --json
```

Run this verifier only after the CEGIS writer stops; Windows cannot replace an
open checkpoint.

The final source and release-executable SHA-256 values used for replay are,
respectively,
`C70F5BB8B2C54228A18ED23E359B26FA2BDA2AF1D8E87358EB87273ADADE064B`
and
`3510B66D8FF15A77C1302E1FA1B2F8B0CBA14B72C85EAB40276652AF52C6CC01`.

## Independent local controls

- Blue's full 17-comparison construction is unique; all 17 leave-one-out
  16-comparison sets are multiple.
- The complete 8,448-set one-edge replacement neighbourhood of the known
  exact-three 16-comparison example contains 722 unsatisfiable sets, 7,726
  multiple sets, and no unique set.  Rust and Interactive Sudoku Solver agree
  on every classification.

These are meaningful local exclusions, not a global relaxed-16 result.

## Interpretation

This bounded pilot found no unique sixteen-comparison set, and its master did
not exhaust the search. A later guided search nevertheless found a unique
9+8+2 construction whose `8+7+1` adjacent inequalities are exactly sixteen
comparisons. It is therefore also a positive witness for this relaxation and
resolves the original 19-cell existence question. The construction and its
independent verification are recorded in
`analysis/unique-19c-9x8x2-2026-08-21.md`.

The pilot does establish that target-free pair-cut CEGIS is correct and
restartable.  The custom master is now allocation-light and can batch
checkpoint writes, but hard joint-master branches remain order-sensitive.  A
topology-specific SAT master and exact oracle loop are now implemented and
documented in `analysis/topology-sat-pilot.md`.  Its present bottleneck is
restarting and reparsing a large static SAT instance; the next useful step is a
persistent incremental SAT bridge, with a proof-producing static rerun if the
master eventually becomes UNSAT.
