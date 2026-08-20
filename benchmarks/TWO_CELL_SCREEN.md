# Two-cell extension batch screen

The fourteen best saved 9+8+2 records all have the same score, three. Removing
their short thermometer leaves a 9+8 base with 370 legal directed short edges.
This benchmark classifies all 370 extensions, capped at two solutions.

Run on 2026-08-20 with ten repetitions per base and mode:

| Strategy | Sum of 14 per-base medians | Residue searches | Relative time |
|---|---:|---:|---:|
| Independent edge searches | 162.265 ms | 5,180 | 5.03x |
| 128 shared solutions, then residue | 32.245 ms | 1,048 | 1.00x |
| Exhaustive shared enumeration | 720.871 ms | 0 | 22.36x |

The 128-solution hybrid is the practical default for this family. It gets two
witnesses for most edges from a single traversal, but avoids enumerating all
16,183 to 18,063 base solutions merely to prove rare or impossible edges.
Every mode produced the same exact classification, and none of the 14 bases
has a unique two-cell extension. The complete versioned result, including a
classification digest for every base and mode, is
`benchmarks/two_cell_screen_2026-08-20.json` (SHA-256
`AA82A487453FD0662A8ABED2F7EE8B7C10D9A905FD7BB62E4C5C432DDCC67A77`).

Reproduce with:

```text
python -X utf8 benchmarks/two_cell_screen.py \
  --prefixes 0,128 --include-collective --repeats 10 \
  --output result.json
```

The timings include one short-lived Rust CLI process per measured screen, so
they are useful for comparing strategies but are not pure in-process solver
throughput.
