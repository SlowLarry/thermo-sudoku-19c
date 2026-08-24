# Exact 17-cell search through the complete classic catalogue

## Why the catalogue reduction is complete

Let a thermo-only puzzle cover the cell set `C`, and let `S` be its unique
solution.  Fixing the cells of `C` to their values in `S` produces an ordinary
Sudoku with `|C|` givens.  Every completion of those exact givens automatically
satisfies all of the thermometers, so that ordinary Sudoku must also be unique.

Consequently, a unique thermo puzzle covering 17 cells must be a morph of one
of the complete 49,158 essentially different 17-clue classics.  The same
argument, combined with the [no-16-clue theorem](https://arxiv.org/abs/1201.0749),
also proves that 17 cells is the absolute lower bound for thermo coverage.

The converse needs an exact check: making the 17 clue values increase along
some paths is not enough, because inequalities are weaker than exact givens.

## Input corpus

The authoritative input is the [49,158-puzzle archive supplied for this
project](https://drive.google.com/file/d/1StS_Sm_Eh9ZJTapOsrRJccM6UP6PmQ3B/view).
This is the post-Royle catalogue; the older Royle collection contained 49,151
representatives, and the later exhaustive scan completed the set at 49,158.
The completion history is recorded in the [Enjoy Sudoku scan
discussion](https://forum.enjoysudoku.com/scan-solution-grids-for-17-clues-as-of-blue-t34012-195.html).

Verified input identity:

```text
17puz49158.zip
size:    441,701 bytes
SHA-256: 89FAAE0653771C6FD64D4DEBA41DC75071C606E721AC5E65FCA1609E93086A1A

17puz49158.txt
size:    4,080,114 bytes
SHA-256: 58EF7D83E8CBAC32495161F9745877FEF82F5E8B3FE58E3CAD4EB3FC004A81B9
```

The text file has exactly 49,158 distinct lines.  Every line has 81 ASCII
cells and exactly 17 clues.  The supplied representatives are essentially
different but should not be assumed to be true minlex strings; the search uses
the stable line number and file hash and does not rely on textual minlex.

The original Royle catalogue was published under CC BY 2.5 with attribution
to Gordon Royle and The University of Western Australia.  The combined archive
does not restate a licence for the seven later additions, so the repository
does not vendor it.  Reproduction uses the public URL plus the hashes above.

## First exact stratum: `9+8`

This is the only two-thermometer partition of 17 cells and therefore has the
maximum possible number of comparisons: `8 + 7 = 15`.

A length-nine thermometer contains every digit exactly once, and a
length-eight thermometer also contains distinct digits.  A catalogue puzzle
can therefore support `9+8` only when its clue multiplicities are exactly

```text
2,2,2,2,2,2,2,2,1
```

Only **10 of the 49,158** records pass this necessary test.

For each eligible record, `thermo-17c-morph` performs an exact symbolic morph
search:

1. The order of the nine source symbols on the length-nine path is the global
   digit relabelling.  No separate `9!` digit-map loop is needed.
2. For each doubled symbol, one occurrence is chosen for the 9-path and the
   other occurrence is forced onto the 8-path.  The singleton appears only on
   the 9-path.  The 8-path follows the same symbol order with that singleton
   omitted.
3. Each possible clue edge carries a 1,296-bit support set for legal Sudoku row
   morphs and another for column morphs.  The DFS intersects these sets after
   every required step and rejects a prefix as soon as either becomes empty.
   This replaces a Cartesian scan of `1,296 × 1,296` spatial morphs.
4. Transpose is redundant for existence: king adjacency is
   transpose-invariant, and the row and column morph groups are identical.
5. A simultaneous reversal of both paths is digit complement, so one of each
   reversed pair is skipped.

For any spatial survivor, uniqueness has a second exact shortcut.  The
9-path is fixed to `1..9`; the 8-path has nine possible templates, one for each
omitted digit.  The template omitting the relabelled singleton is the original
unique 17-clue puzzle.  The thermo layout is unique exactly when all eight
other ordinary 17-given templates are unsatisfiable.  A positive result is
also rechecked by the generic thermo solver.

## Complete `9+8` result

Command, from `thermo-sudoku-rs/`:

```text
target/release/thermo-17c-morph.exe \
  --input <path-to>/17puz49158.txt \
  --end-line 49158 \
  --output ../analysis/17c-9x8-scan-2026-08-21.jsonl \
  --progress-every 0
```

Deterministic result:

```text
records:             49,158
digit-pattern hits:  10
DFS nodes:           7,374
spatial prunes:      75,722
realized 9+8 covers: 0
```

The scan therefore excludes the complete `9+8` stratum before any Sudoku
count is needed: none of the ten arithmetically eligible classics has even one
spatial morph on which both required king paths exist.  The retained JSONL is
[`17c-9x8-scan-2026-08-21.jsonl`](17c-9x8-scan-2026-08-21.jsonl), SHA-256
`FF6E86104CCB22731EBC1A04D376F5FE8F92EFC3A9B1FD79C202A55E79F74CE1`.

An independent direct reference search reached the same result by explicitly
checking all `1,296 × 1,296` row/column morph pairs for each eligible record:
16,796,160 fixed geometries and 381,069,488 reference DFS nodes, again with
zero realizable covers.  Its axis group was independently generated by
filtering all `9!` permutations rather than reusing the production generator.
The slower verifier is retained in the same binary behind
`--reference-direct`; on the pilot machine its full run takes roughly 20 seconds.

## Second exact stratum: three thermometers

The next maximum-information layer has three thermometers and 14 comparisons:

```text
9+6+2   9+5+3   9+4+4   8+7+2   8+6+3
8+5+4   7+7+3   7+6+4   7+5+5   6+6+5
```

These are all unordered three-part partitions of 17 with path lengths from 2
through 9. `thermo-17c-three-path` searches them exactly:

1. Every source symbol must occur. If a symbol is absent from the covered
   cells, it has no precedence incidence and can occupy more than one place in
   the global digit order, producing another thermo solution. A symbol also
   cannot occur more than once on any strict path, so multiplicity greater
   than three is impossible.
2. The DFS chooses one global order of the nine source symbols. Each symbol's
   one, two, or three occurrences is injected into that many distinct path
   roles, while an exact capacity DP enforces the requested path lengths.
3. Consecutive symbols in the global order must share a path. Otherwise they
   are incomparable, and swapping those two consecutive ranks gives a second
   valid digit relabelling and therefore a second solution.
4. Every new path edge intersects the same 1,296-bit row- and column-morph
   support domains used by the `9+8` scanner. Empty support prunes the entire
   prefix. Final layouts are quotiented only by D4 cell symmetry, simultaneous
   reversal/digit complement, and permutation of equal-length paths.
5. Every surviving blank thermo layout is classified by the generic solver at
   cap two. For a multiple layout, both distinct solution grids are emitted.
   A putative unique result is additionally checked against the morphed and
   relabelled source classic.

Exactly 25,152 catalogue records have all nine symbols and maximum
multiplicity three. Their retained multiplicity profiles are:

| `(n3,n2,n1)` | Records |
|---|---:|
| `(0,8,1)` | 10 |
| `(1,6,2)` | 5,300 |
| `(2,4,3)` | 16,123 |
| `(3,2,4)` | 3,695 |
| `(4,0,5)` | 24 |

For a partition with shortest path length `c`, occurrence assignment is
possible exactly when `n3 <= c`. This leaves 21,433 eligible records for
`9+6+2` and `8+7+2`; 25,128 for `9+5+3`, `8+6+3`, and `7+7+3`; and 25,152 for
each of the other five partitions.

## Complete three-path result

Command, from `thermo-sudoku-rs/`:

```text
target/release/thermo-17c-three-path.exe \
  --input <path-to>/17puz49158.txt \
  --partition all \
  --end-line 49158 \
  --output ../analysis/17c-three-path-scan-2026-08-21.jsonl \
  --progress-every 0
```

Every partition completed over all 49,158 source records:

| Partition | Eligible | DFS nodes | Realized | Duplicates | Classified | Multiple | Unique |
|---|---:|---:|---:|---:|---:|---:|---:|
| `9+6+2` | 21,433 | 123,803,932 | 6 | 0 | 6 | 6 | 0 |
| `9+5+3` | 25,128 | 166,516,417 | 61 | 1 | 60 | 60 | 0 |
| `9+4+4` | 25,152 | 171,623,697 | 68 | 34 | 34 | 34 | 0 |
| `8+7+2` | 21,433 | 258,675,258 | 50 | 0 | 50 | 50 | 0 |
| `8+6+3` | 25,128 | 423,366,289 | 35 | 6 | 29 | 29 | 0 |
| `8+5+4` | 25,152 | 478,551,455 | 58 | 2 | 56 | 56 | 0 |
| `7+7+3` | 25,128 | 540,228,445 | 36 | 18 | 18 | 18 | 0 |
| `7+6+4` | 25,152 | 711,317,733 | 76 | 3 | 73 | 73 | 0 |
| `7+5+5` | 25,152 | 764,141,769 | 22 | 11 | 11 | 11 | 0 |
| `6+6+5` | 25,152 | 877,927,521 | 0 | 0 | 0 | 0 | 0 |
| **Total** | | **4,516,152,516** | **412** | **75** | **337** | **337** | **0** |

The deterministic run took 2,942.335 seconds on the pilot machine. Its 348-line
JSONL is
[`17c-three-path-scan-2026-08-21.jsonl`](17c-three-path-scan-2026-08-21.jsonl),
186,175 bytes, SHA-256
`D42F55352585D0699243F749538095621B6987DAF6E92EE1193479ABDD360A9E`.
All ten summaries say `complete:true`. The 337 candidate records each contain
two direct solution witnesses, and no unique layout survived.

An independent audit reconstructed every cited source morph and checked all
674 witness grids for classic Sudoku validity, path satisfaction and pairwise
distinctness. It also reclassified all 337 layouts with ISS at revision
`c43bfb867baa0f9c12c087afe912d626ac13a77a`; every result was again `2+`.
After the final rebuild, a complete `9+6+2` rerun reproduced its seven
candidate/summary lines byte-for-byte.

This is a deterministic exhaustive program result, not a SAT/DRAT-style
independently checkable nonexistence certificate. The emitted witness pairs
independently certify that surviving layouts are multiple; exhaustiveness
rests on the documented reduction and scanner implementation.

## Scope after this scan

The result excludes every 17-cell layout with two or three simple,
cell-disjoint thermometers: the complete 15- and 14-comparison strata. Taken
alone, it left forty of the 51 path-length partitions:

| Thermometers | Partitions | Comparisons |
|---:|---:|---:|
| 4 | 16 | 13 |
| 5 | 13 | 12 |
| 6 | 7 | 11 |
| 7 | 3 | 10 |
| 8 | 1 | 9 |

A negative result for the stronger three-path layer does not automatically
exclude these different disjoint geometries. The later merge-maximal forest
search in `17c-maximal-forest-search.md` has since classified all 151,631
merge-maximal eight-path occurrences as multiple. Any unique non-maximal
eight-path layout would extend to a unique lower-path dominator. Therefore the
global disjoint existence search can continue with only the 39 four- through
seven-path partitions, although the eight-path result is not a standalone
exclusion of every non-maximal layout in that partition.

A broader exact route is now preferable: when shared cells and arbitrary
two-cell thermometers are admitted, the saturation theorem in
`17c-overlap-search.md` tests a superset of all 40 post-three-path partitions,
including non-maximal eight-path layouts, without enumerating them
individually. No complete result from that generalized scan is claimed yet.
