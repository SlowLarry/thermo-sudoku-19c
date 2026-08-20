# Exact 9+8+2 pilot

This note records the first end-to-end exact shard of the disjoint 19-cell
search. It is a pilot result, not a global exclusion.

## Search model

- A length-9 and a length-8 simple king path are chosen without shared cells.
- Both orthogonal and diagonal steps are allowed.
- The safe symmetry group is the eight square symmetries, together with
  simultaneous reversal of both thermometers and Sudoku digit complement.
- Every directed king-neighbour edge on two uncovered cells is then classified
  exactly as admitting 0, 1, or at least 2 Sudoku solutions.
- A 128-solution collective prefix supplies shared witnesses; unresolved edges
  are finished with independent capped searches.

Paths use deterministic DFS rank order: starting cell first, then ascending
neighbour cell at every step. The reproducible shard is:

```text
length-9 ranks [0, 64)
length-8 ranks [16,414,504, 16,418,600)
```

Run it with:

```text
cargo run --release --manifest-path thermo-sudoku-rs/Cargo.toml \
  --bin thermo-9x8-pilot -- \
  --output analysis/thermo-9x8-pilot.jsonl
```

When progress output is enabled, the JSONL stream includes flushed checkpoint
records. Resume the same rank rectangle in a new output file with
`--base-offset` set to the checkpoint's `resume_base_offset`; at most one
progress interval is repeated.

## Result

The 2026-08-20 run processed:

- 262,144 raw path pairs;
- 257,776 canonical, disjoint 9+8 bases;
- 99,389,208 legal directed two-cell extensions;
- 1,013,821 house-compatible length-8 digit templates;
- 9,100,397 independent residue searches after collective screening.

No unique extension was found. Of the canonical bases, 176,173 had no Sudoku
solution at all. The final provenance-recorded run took 496.799 seconds on the
development machine. Its
machine-readable output is
`analysis/thermo-9x8-pilot-2026-08-20.jsonl` (SHA-256
`9E954C71413387EEDB24EE8FD31DA8A78BFCBDA9842A65B806D42FD9136CDA4D`).
The executable SHA-256 was
`E849FE74735B447D98B6F7756248EA6455A7AE3C84A98173BE005B4297B2F3BD`;
the corresponding `src/lib.rs` and pilot-source hashes were respectively
`21C7DADE4FF63B18F6161714C165F3A0D4E2138BFED565E96F956A3ACB333272`
and `9F5B75D467B6389945AEE6BA153A7FF5F6FC1BA1508AB09491E86DA424524E7E`.

## Interpretation

The exact catalog contains 85,743,256 directed length-9 paths and 16,418,600
directed length-8 paths. After disjointness and the safe factor-16 symmetry
reduction, the estimated full base count is about 3.65e13. This corner-biased
pilot processed about 519 canonical bases per second, which would extrapolate
to roughly 2,230 single-core years. The estimate is only a scale indicator,
because work per base is nonuniform; it is decisive enough to rule out a plain
base-by-base exhaustive run as a hobby-resource strategy.

The next proof-oriented step is therefore symbolic. Fixed-target and
target-free trade-cut CEGIS pilots are now implemented and documented in
`analysis/fixed-target-pilot.md` and `analysis/global-cegis-pilot.md`. The
remaining boundary is a proof-producing SAT/PB master for the relaxed class of
any sixteen directed king-neighbour comparisons. If that relaxed class is
unsatisfiable, every disjoint thermometer layout covering at most 19 cells is
excluded at once.
