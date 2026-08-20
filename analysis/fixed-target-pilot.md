# Fixed-target relaxed-comparison pilot

This is a progress record for the symbolic direction suggested by the exact
9+8+2 pilot. It is neither a 19-cell construction nor an exclusion result.

## Model

The fixed target is the solved classic Sudoku

```text
326891745985674123714523869832769514697415238451238697243157986178946352569382471
```

For that target, `thermo-fixed-target` constructs every strict comparison on a
horizontal, vertical, or diagonal neighbouring pair that is true in the
target. There are 263 such oriented comparisons; the nine equal-digit
neighbour pairs cannot be strict clues and are omitted.

Each alternative classic Sudoku produces a *trade cut*: the set of candidate
comparisons it violates. A hitting set for the accumulated cuts is then tested
by an exact Sudoku oracle, which returns more alternatives satisfying all
selected comparisons. Comparisons may overlap arbitrarily, so this is a
relaxation of disjoint thermometer geometry. A proof that more than 16 such
comparisons are required for every possible target would exclude every
disjoint thermometer layout covering at most 19 cells.

## 2026-08-20 checkpoint

The current implementation injects the eight Sudokus obtained by globally
swapping digits 1/2, 2/3, through 8/9. Their trade cuts are pairwise disjoint:
a strict comparison can distinguish at most one adjacent-digit swap. For this
target none of the eight cuts is empty, so they give a rigorous fixed-target
lower bound of eight. (If one were empty, no set of these comparisons could
isolate the target at all.)

Starting from an earlier million-grid checkpoint, 30 further batched oracle
iterations added 1,966,080 alternatives in 3,965,181 search nodes. The latest
checkpoint therefore contains 2,983,306 distinct, validated classic Sudoku
alternatives. The eight structural seeds are regenerated in memory and are not
stored in that file. The final master pass reported:

```text
candidate_edges=263
trade_cuts=2983314
certificate_lower_bound=8
search_nodes=0
cegis_status=iteration-limit
result=feasible-over-provided-cuts
selected_size=11
selected_edge_ids=13;25;28;41;86;91;120;122;150;151;180
```

The 11 comparisons hit every saved cut and all eight structural seeds, but
they have not been shown to isolate the target. In particular,
`iteration-limit` means that no fixed-target conclusion was reached.

The checkpoint was deliberately not copied into the repository: it is
244,631,214 bytes. On the development machine it has 2,983,308 lines (two
metadata lines followed by the alternatives) and SHA-256
`98337C85D73671E6659B82D24B8FC4187112B28CBEB601F8DBB0182622797177`.

The master can load it, add the eight seeds in memory, and recompute its
summary without modifying the file:

```text
thermo-sudoku-rs/target/release/thermo-fixed-target.exe \
  --target 326891745985674123714523869832769514697415238451238697243157986178946352569382471 \
  --alternatives C:/Users/User/AppData/Local/Temp/fixed-target-structural.grids \
  --budget 16 --cegis --max-iterations 0 --summary-only
```

The resulting greedy hitting set is:

| Edge ID | Comparison | Target digits |
|---:|---|---:|
| 13 | r2c5 < r1c4 | 7 < 8 |
| 25 | r1c8 < r1c9 | 4 < 5 |
| 28 | r2c9 < r1c8 | 3 < 4 |
| 41 | r2c4 < r2c5 | 6 < 7 |
| 86 | r3c8 < r3c7 | 6 < 8 |
| 91 | r4c7 < r3c8 | 5 < 6 |
| 120 | r5c8 < r4c7 | 3 < 5 |
| 122 | r4c8 < r5c7 | 1 < 2 |
| 150 | r5c7 < r5c8 | 2 < 3 |
| 151 | r5c7 < r6c6 | 2 < 8 |
| 180 | r6c6 < r7c7 | 8 < 9 |

The validation command intentionally reports zero newly added alternatives
and zero oracle nodes; those are properties of the zero-iteration replay, not
of the run that built the checkpoint.

## Interpretation and next step

The multi-million-cut milestone validates the restartable cut/oracle machinery and
shows that explicit alternatives can be generated quickly. The structural
digit-swap argument certifies a lower bound of eight, but the checkpoint plus
those cuts still admits an 11-edge hitting set. Continuing to store millions
of grids therefore has sharply diminishing value.

The next target-free master has now been implemented as
`thermo-global-cegis`; its first bounded results are in
`analysis/global-cegis-pilot.md`. The remaining useful boundary is an
incremental SAT or pseudo-Boolean master with proof-producing UNSAT support.
The fixed-target program remains useful for oracle validation, candidate cuts,
and small reproducible experiments; any terminal claim must distinguish the
single fixed target from the global 19-cell question and supplied cuts from
all classic Sudoku alternatives.
