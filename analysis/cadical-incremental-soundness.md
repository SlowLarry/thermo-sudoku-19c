# Persistent CaDiCaL bridge: soundness notes

This note separates the fast incremental search loop from the artifacts needed
for a proof-grade conclusion.  It applies to CaDiCaL 2.1.3 (`rel-2.1.3`, commit
`f13d74439a5b5c963ac5b02d05ce93a8098018b8`).  CaDiCaL is MIT-licensed; a
distributed bridge binary or bundled source must retain its license notice.

## Safe incremental lifecycle

1. Validate the pair checkpoint and deterministically reconstruct the complete
   master before starting the bridge.
2. Open the plain, seekable master once, cross-check its header, rewind the same
   handle, and let CaDiCaL parse every initial clause exactly once in
   `strict=2` mode. This binds the precheck and parser to one opened file rather
   than resolving the pathname twice, and checks the canonical header, declared
   variable bound, clause count, literal bounds, and clause termination. Do not
   modify the opened CNF in place while startup parsing is in progress.
3. Before each `solve`, set that call's conflict limit.  CaDiCaL resets search
   limits when `solve` returns.  Results are exactly `10` (SAT), `20` (UNSAT),
   or `0` (UNKNOWN).
4. On SAT, read `val(i)` for every master variable.  CaDiCaL reconstructs
   eliminated variables and returns exactly `i` or `-i`.  The Rust side must
   nevertheless reject a missing, duplicate, contradictory, out-of-range, or
   clause-violating assignment before invoking the thermo oracle.
5. When the oracle finds alternatives, derive every requested pair cut
   independently, require that each cut blocks the current selected edge set,
   and deduplicate both pairs and 544-bit cuts. In eager mode, update the
   mutable CNF snapshot before adding each clause to the live solver. In lazy
   mode, retain every cut in the authoritative checkpoint pool, add only the
   selected active cuts to the live solver, and mark a cut active only after
   the bridge acknowledges it. Adding a clause after SAT is supported by
   CaDiCaL and restores internally eliminated clauses; previous learned
   clauses remain valid because the formula only becomes stronger.
6. Treat bridge EOF, malformed output, a signal/crash, or UNKNOWN as
   inconclusive.  Do not continue using a bridge whose acknowledged clause
   count differs from the parent.  Recovery reconstructs a new bridge solely
   from a checkpoint that passes the independent verifier.

The checkpoint is the durable source of truth. A generated CNF and the live
solver are disposable caches. Checkpoint writes use a same-directory temporary
file, flush/sync it, and atomically replace the destination; a torn in-place
rewrite never becomes the only resumable copy. With `--checkpoint-every N`, a
crash can conservatively discard up to `N` refinement batches/CEGIS
iterations; each batch may contain many pairs and cuts. Clean exits, terminal
results, and handled bridge errors flush all in-memory progress. On restart
the CNF is always regenerated from the validated checkpoint, so a CNF that
was ahead of the last checkpoint after a crash is harmless. The disposable
CNF append is flushed but need not be durably synced; the atomic checkpoint is
the only cache whose data is synced before replacement.

Lazy-cut acknowledgements may advance the live bridge and in-memory active
pool between scheduled checkpoint writes. They do not independently advance
the durable active manifest while the pair checkpoint is dirty. Every
scheduled, clean, terminal, or handled-error persistence writes the checkpoint
first and the manifest second. A crash before the checkpoint therefore leaves
the previous mutually consistent pair and active prefixes; a crash between the
two replacements leaves an older manifest that is still a validated
append-only prefix of the newer checkpoint. An active manifest is never
written ahead of its durable pair pool. This deferral changes only restart
work, not the formula searched in the live bridge.

Run-lifetime operating-system locks on sidecars beside the checkpoint and CNF
paths, and beside the active-manifest path in lazy mode, make cooperating
concurrent writers fail fast instead of racing an atomic replace or
interleaving mutable-CNF updates. These locks are path-based coordination, not
a security boundary against an unrelated process that ignores them and writes
directly to the CNF.

## Why incremental UNSAT is not the final certificate

An incremental solver result is useful as a trigger, but it is not by itself a
negative certificate.  In particular, pair cuts added after an earlier solve
are new axioms, not consequences of the earlier formula, so an incremental
DRAT/LRAT stream cannot simply be checked against the original CNF as if those
cuts had been derived.

After incremental UNSAT:

1. persist and independently verify the final checkpoint;
2. regenerate one static final CNF and record its hash;
3. run a fresh proof-producing CaDiCaL process on that exact CNF; and
4. verify the complete LRAT proof with an independent checker such as CakeLPR.

Until all four steps succeed, report the result as provisional rather than as
an exclusion.

## D4 times complement symmetry breaker

The following optional breaker is orbit-complete for the square's eight
dihedral symmetries and digit complementation.  Complementation maps digit
`d` to `10-d` and reverses every selected arc.

- `r1c1 <= r1c9`, `r1c1 <= r9c1`, and `r1c1 <= r9c9`;
- `r1c1 <= 5`; and
- `r1c2 <= r2c1`.

For any orbit, first choose the original or complemented grid so that the
smallest corner is at most five, map a smallest corner to `r1c1`, and use the
main-diagonal reflection if necessary to order `r1c2` and `r2c1`.  The latter
cells share a box, so they cannot tie.  Opposite corner ties can leave multiple
representatives, which only makes the breaker incomplete, not unsound.

With one-hot digit variables, encode each `A <= B` using the 36 binary clauses
`(-A_a OR -B_b)` for all `a > b`, and encode `r1c1 <= 5` with four negative
unit clauses for digits 6 through 9.  The breaker therefore adds 148 clauses
and no variables. Enabling it changes the master. Schema
`thermo-topology-cnf-v2` records the explicit `symmetry_break` discriminator,
and all hashes must identify the chosen mode. An LRAT proof for this CNF
excludes only the symmetry-broken domain; either retain the orbit argument as
part of the result or rerun the final static proof without the breaker.
