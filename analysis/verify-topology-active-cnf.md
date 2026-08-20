# Independent lazy proof-formula verification

`verify_topology_active_cnf.py` audits the three artifacts that define a lazy
topology SAT formula:

1. the complete `thermo-global-cegis-v1` solution-pair checkpoint;
2. the `thermo-topology-active-cuts-v1` manifest; and
3. the emitted lazy base-plus-active DIMACS file.

It is a standard-library Python implementation. It does not run, import, or
parse source from the Rust exporter. The duplicated implementation is
intentional: a final proof should not rely on the same code path both to write
and to validate its formula.

## Run it

From the repository root:

```powershell
python -X utf8 analysis/verify_topology_active_cnf.py `
  path\to\final.checkpoint `
  path\to\final.active `
  path\to\final.cnf `
  --expected-symmetry-break d4-complement-v1 `
  --json
```

Omit `--expected-symmetry-break` to accept either supported mode while still
requiring the manifest and CNF to agree. The checkpoint budget defaults to 16
and can be changed explicitly with `--budget`.

The JSON result records SHA-256 hashes of all three files, both FNV checksums,
the checkpoint and cut counts, the symmetry mode, the independently derived
edge-order checksum, the exact DIMACS clause count, and whether the full CNF
matched the independent deterministic re-emission.

## Checks performed

The checkpoint pass validates:

- the exact schema and metadata fields, footer, pair count, and FNV-1a hash;
- every 81-digit grid as a complete classic Sudoku;
- strict canonical ordering and exact non-duplication of grid pairs;
- the 544 directed king edges in lexicographic unordered-pair, forward/reverse
  order; and
- every deduplicated pair cut, with its stable ID assigned at first occurrence.

The manifest pass validates:

- schema, supported symmetry mode, directed-edge count and edge-order hash;
- record syntax, solved and canonically ordered witness grids, unique indices,
  active count, footer, and the order-sensitive active FNV-1a hash;
- the declared pair checkpoint as an exact append-only prefix, including its
  pair checksum and number of first-occurrence cuts; and
- every active ID against the first checkpoint pair that generated that cut.

Finally, the verifier independently regenerates the classic-Sudoku clauses,
strict-comparison clauses, disjoint directed-path geometry, 19-cell sequential
counter, adjacent-symbol-swap witnesses, optional 148-clause
`d4-complement-v1` breaker, and the active positive pair clauses. It compares
every comment and clause line byte-for-byte with the supplied DIMACS file and
computes its SHA-256 during that comparison. This is stronger than merely
checking that the active clauses occur somewhere in the file. All three files
are hashed again at the end so concurrent replacement cannot silently mix
different artifact generations into one successful report.

The pair and cut duplicate checks are exact rather than probabilistic, so
memory grows with the corpus size. Grid validation and edge masks use a bounded
cache; the checkpoint itself is streamed and is not loaded as a list.

## Tests

```powershell
python -X utf8 -m unittest analysis/test_verify_topology_active_cnf.py -v
```

The adversarial suite covers a valid append-only descendant, checkpoint hash
corruption, a duplicate pair with internally consistent metadata, an invalid
Sudoku, a wrong but freshly checksummed cut witness, a duplicate active ID, a
false prefix cut count, CNF clause tampering, and an explicit symmetry-mode
mismatch. A cross-language smoke check should additionally be run on every
frozen Rust-produced artifact set.

## Proof boundary

Success establishes the exact provenance and contents of the static SAT
formula. It does not prove the formula UNSAT and does not validate an LRAT
proof. The final exclusion workflow still needs a fresh proof-producing SAT
run on the verified CNF and an independent proof checker. With
`d4-complement-v1`, the conclusion also depends on the separately documented
orbit-completeness lemma; proving the final unsymmetrized CNF avoids that extra
lemma.
