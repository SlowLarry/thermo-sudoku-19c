# Exact 17-cell search with overlapping and branching thermometers

This note describes the broader search introduced after the disjoint
four-through-seven-path enumeration developed a severe record-level heavy
tail. It gives the mathematical reduction, implementation contract, exact run
identity, and the checks required before a result can be claimed. The full
catalogue run is in progress; no complete negative result is claimed here yet.

## Scope

The base puzzle is standard 9x9 Sudoku with no givens. The generalized thermo
object is a finite set of strict comparisons `cell A < cell B`, where the
cells are orthogonal or diagonal king neighbours. Exactly 17 distinct cells
must be incident to at least one comparison; “17 cells” always means the size
of this union, not the sum of path lengths with repeats. Comparisons may share
cells, branch, merge, form diamonds, and geometrically cross. Equivalently,
each comparison may be drawn as a two-cell thermometer, with no
drawing-imposed count limit. Duplicate edges are redundant, and 17 grid cells
induce at most 46 distinct king edges. Longer overlapping thermometers add no
further semantics because they flatten to their adjacent strict comparisons.

This is deliberately more permissive than a drawing convention that allows
only rooted trees, bounds the number or degree of branches, or forbids
crossings. A negative result in the permissive scope excludes all those
subclasses. A positive saturated network might need to be minimized before it
has a conventional or attractive thermometer drawing.

## Saturation theorem

Let `C` be the 17 covered cells of a hypothetical unique inequality-only
puzzle, and let `S` be its solution. Fixing the digits of `S` on `C` gives a
unique ordinary 17-clue Sudoku: any completion of those givens automatically
satisfies every comparison in the original puzzle. Consequently the complete
49,158-entry catalogue of essentially different 17-clue classics is a
complete source universe, up to Sudoku coordinate morphs and digit relabelling.

For one catalogue record, one Sudoku coordinate morph, and one global digit
relabeling, define `U` to contain **every** king-neighbour comparison between
covered cells that is true in the relabeled target solution, directed from the
lower digit to the higher digit. Equal target digits contribute no comparison.

Every overlapping or branching comparison network `E` on those cells is a
subset of `U`. Adding target-true comparisons cannot remove the target and
cannot add solutions, so

```text
E is unique  =>  U is unique.
```

Coverage is also preserved: every cell incident in `E` remains incident in
the superset `U`. Conversely, in the permissive scope `U` itself is a valid
collection of overlapping two-cell thermometers. Therefore a unique
generalized 17-cell network exists **if and only if** at least one saturated
`U` is unique. There is no need to enumerate its exponentially many edge
subsets or any of the disjoint path partitions. This equivalence is
the reason the generalized scan can be smaller than the disjoint search even
though its object class is strictly larger.

All nine clue digits must occur. If one or more ranks were absent, some
absent/present boundary in the ordered ranks would contain consecutive values;
swapping that pair would preserve every comparison and give a second Sudoku
solution. More generally, the comparison poset must have the chosen
target order as its unique linear extension. Every consecutive pair of ranks
must therefore be comparable. Because no rank lies strictly between them,
that comparability cannot be supplied through a longer directed chain: it
must be one physical comparison. Thus the target order must be a Hamiltonian
path in the nine-symbol physical-adjacency graph. This condition is necessary,
not an approximation; candidates passing it still receive an exact Sudoku
classification.

## Exact scanner

`thermo-17c-overlap` performs the following deterministic search.

1. Reject catalogue records that omit a digit.
2. Generate the 1,296 legal row-axis and 1,296 legal column-axis morphs.
3. Represent the clue-pair king adjacencies as compact masks. Keep only
   inclusion-maximal row masks, column masks, and combined physical networks;
   every discarded mask has a realizable stronger representative.
4. Reject networks that do not incidentally cover all 17 clue cells.
5. Enumerate Hamiltonian symbol orders, retaining one of the two global
   reversal/digit-complement directions.
6. Saturate and orient the comparisons, compute their transitive closures,
   deduplicate equal posets, and retain only inclusion-maximal closures.
7. Ask the exact arbitrary-comparison Sudoku solver for a capped `0 / 1 / 2+`
   classification. The known catalogue target makes the zero case an internal
   consistency failure.

The 1,296 axis maps are the `6^4` combinations of band permutation and the
three within-band row permutations; columns use the identical group. A full
Sudoku coordinate morph is one row map, one column map, and optionally a
transpose. Transpose need not be enumerated for existence because exchanging
the independently complete row and column domains realizes the transposed
case. Digit relabeling is represented by the Hamiltonian symbol order rather
than by a separate `9!` loop. Simultaneously reversing that order and every
comparison is global digit complement, so retaining one direction removes an
exact fixed-point-free symmetry.

The comparison solver uses 9-bit domains, the ordinary Sudoku singleton and
house queues, one `u64` incident-edge mask per cell, and one `u64`
dirty-comparison frontier. A domain change ORs every incident arc into that
frontier. Any 17 grid cells induce at most 46 undirected king edges, so the
64-edge frontier has exact headroom and no comparison is truncated. For
`A < B`, revision removes from `B` every value not greater than the current
minimum of `A`, and removes from `A` every value not smaller than the current
maximum of `B`. Those bounds give exact arc consistency for this binary
relation even when domains have holes. A restriction also dirties the cell's
three Sudoku houses and queues a newly singleton cell; house revision includes
hidden singles and locked candidates in addition to peer elimination. Changes
requeue all neighbouring arcs, so overlapping branches reach a fixpoint before
the exact MRV search continues, using incident-comparison degree as its tie
break. Arc consistency alone is not claimed to decide an arbitrary network;
exhaustive DFS supplies completeness. The API also accepts overlapping long
paths by flattening and deduplicating their adjacent comparisons. Disjoint path
inputs may delegate to the earlier specialized fast path, while this scanner
calls `ComparisonSolver::blank` on explicit graphs and therefore always uses
the generalized backend.

The exact dominance steps are monotone. Masks and closures are compared on the
fixed 17 clue-vertex labels. If adjacency mask `A` is contained in a realizable
mask `B`, transport the weaker puzzle through the Sudoku coordinate morph
between their representatives; saturating `B` only adds target-true
constraints. Likewise, if directed closure `A` is contained in directed
closure `B`, every solution of `B` satisfies `A`, even when their stored
representatives arose from different target orders. In both cases the stronger
candidate retains its known target and has a nonempty subset of the weaker
candidate's solutions. Therefore a unique discarded case always has a
realizable retained unique dominator. Transitive closure is used only for
logical equality and dominance; the solver receives the realized local king
edges, never nonlocal closure arcs. Candidate equality and dominance use
complete masks and closures, not hash equality. FNV and SHA values are used
only for artifact, checkpoint and run identity checks.

## Persistence and parallel execution

`analysis/run_17c_overlap_chunks.py` divides the 25,370 all-nine-digit records
into small contiguous ranges and assigns them dynamically to independent
single-threaded scanner processes. A chunk is published only after its JSONL
header and terminal accounting validate. The run identity binds the output
directory to the exact corpus and executable hashes. Re-running the command
skips validated chunks; a unique case stops new scheduling and terminates the
other workers.

The launcher holds an OS advisory lock for its complete invocation, so two
launchers cannot mix artifacts in one output directory. Each worker writes a
uniquely named `.partial` file. The parent checks its schema, algorithm
revision, corpus fingerprint, exact line range, cap-two accounting and terminal
completion flags before replacing the final chunk path. Interrupted partials
are never counted. There is intentionally no durable checkpoint inside one
launcher task: stopping preserves every published artifact but recomputes its
unfinished task.

The launcher can permanently replace an unfinished parent chunk with a
deterministic split manifest. Its children each contain one eligible catalogue
record and together form an exact, gap-free and overlap-free partition of the
parent's inclusive source-line interval. Every manifest is bound to the corpus
hash, executable hash and algorithm revision. A logical root is satisfied by
exactly one evidence representation: either its original parent artifact or
all of its validated children, never both. Orphan children, inconsistent
revisions, parent/child ambiguity, and manifests whose declared child ranges
are incomplete or overlapping are hard errors. Missing child artifacts remain
pending work. Published split manifests are discovered automatically on
later resumes, and separate artifact-set and manifest-set hashes make the
selected evidence reproducible. `--split-workers` limits the split lane so a
pathological child cannot consume every worker while ordinary chunks remain.

The bounded scheduler adds two distinct timeout outcomes. A multi-record parent
that exceeds `--parent-timeout-seconds` is terminated individually, reaped, and
atomically replaced by its deterministic split manifest; its singleton
children then enter the split queue. A child exceeding
`--singleton-timeout-seconds` is not classified. Instead, its exact parent,
part, source range, sole eligible line, attempt count, timeout and partial-file
metadata are written atomically to identity-bound `deferred-tasks.json`. Other
workers continue. Normal resumes skip deferred children; only an explicit
`--retry-deferred` pass retries them.

A timeout always means unresolved, never multiple, unique, or exhausted. Only a
fully validated terminal JSONL contributes counts. If a valid final artifact
wins the deadline race it supersedes the timeout; a later successful retry
similarly removes its stale deferred entry. Partials are never evidence. The
aggregate remains `complete:false` while any deferred singleton exists and
records the deferred-manifest hash and exact line list.

```text
python analysis/run_17c_overlap_chunks.py \
  --corpus <path-to>/17puz49158.txt \
  --binary thermo-sudoku-rs/target/release/thermo-17c-overlap.exe \
  --output-dir <artifact-root>/overlap-exact \
  --workers 4 --eligible-per-chunk 16
```

The audited intervention in the production run used:

```text
python analysis/run_17c_overlap_chunks.py \
  --corpus <path-to>/17puz49158.txt \
  --binary thermo-sudoku-rs/target/release/thermo-17c-overlap.exe \
  --output-dir <artifact-root>/overlap-exact \
  --workers 4 --eligible-per-chunk 16 \
  --split-chunk 34 --split-chunk 515 \
  --split-chunk 598 --split-chunk 675 --split-workers 1
```

The repeated `--split-chunk` options are needed only to commit new manifests;
normal resumes auto-adopt existing ones. This implementation subdivides at
record boundaries. A pathological single record, and an individual
`count_up_to(2)` call within it, remain atomic and may require a later
candidate-level or solver-frontier split.

After a second all-worker stall, the next bounded production restart command
is:

```text
python analysis/run_17c_overlap_chunks.py \
  --corpus <path-to>/17puz49158.txt \
  --binary thermo-sudoku-rs/target/release/thermo-17c-overlap.exe \
  --output-dir <artifact-root>/overlap-exact \
  --workers 4 --eligible-per-chunk 16 --split-workers 1 \
  --split-chunk 676 --split-chunk 766 --split-chunk 767 \
  --parent-timeout-seconds 1800 --singleton-timeout-seconds 300
```

The four older split manifests are adopted automatically. The three new flags
avoid repeating already-observed multi-hour parent work. One split lane and
three ordinary lanes preserve catalogue throughput. The main pass deliberately
omits `--retry-deferred`; hard records are a separately auditable backlog.

The scanner also has an in-process checkpoint for bounded diagnostics. It
writes a synced same-directory temporary and installs it with a validated
backup fallback, binding the state to the flushed JSONL byte prefix. A resume
therefore rejects or truncates an unrelated or partial tail.
The chunk runner deliberately avoids frequent checkpoints: on a small exact
record, syncing every 32 candidates was measured at about 6.8 times the
checkpoint-free runtime.

## Reproduction and exact run identity

The catalogue is not vendored because the combined 49,158 archive does not
restate a licence for its seven post-Royle additions. Download
`17puz49158.txt` through the provenance link in
`17c-classic-morph-search.md`, then verify its identity before running:

| Input or implementation | Bytes | SHA-256 |
|---|---:|---|
| `17puz49158.txt` | 4,080,114 | `58EF7D83E8CBAC32495161F9745877FEF82F5E8B3FE58E3CAD4EB3FC004A81B9` |
| `thermo-sudoku-rs/Cargo.toml` | 354 | `F6C1E3721F4DB01E7FD1FB9E6FB4A8F58DEC86B2AAC62180DBFBC328CD4204A0` |
| `src/comparison.rs` | 48,618 | `75D7F7D1604FC718C8647136E013E9F375A24E7A6F5757EAD59D85B287B27981` |
| `src/lib.rs` | 92,036 | `599A8C4E14B4F856F9891AF894368E764A20CDF8E39849751C2B6F8ECDDA75C8` |
| `src/bin/thermo-17c-overlap.rs` | 121,659 | `17769B0068DBB01F0D0EC59AD40C4B5605E250113ED4914C7626369DE7C3F066` |
| `analysis/run_17c_overlap_chunks.py` | 66,457 | `CC65595F253BEF0185757303A8009E4ACD98EE319866C3150C70AF6173C24626` |
| run's `thermo-17c-overlap.exe` | 408,576 | `117DC22FCBD0914AED6D9A8D88C9964D5A403A9FB1CCE0F64C93346D2F17B658` |

The executable was built in release mode on
`rustc 1.94.0 (4a4ef493e 2026-03-02)`, target
`x86_64-pc-windows-msvc`, with Cargo 1.94.0. A rebuild on another toolchain
need not reproduce the executable hash; the source hashes and tests define the
auditable implementation, while `run-identity.json` binds one artifact set to
the exact executable that produced it.

Before the long run, the following checks passed:

```text
cargo build --release --manifest-path thermo-sudoku-rs/Cargo.toml \
  --bin thermo-17c-overlap
cargo test --release --all-targets \
  --manifest-path thermo-sudoku-rs/Cargo.toml
cargo clippy --release --all-targets --all-features \
  --manifest-path thermo-sudoku-rs/Cargo.toml -- -D warnings
cargo fmt --all --manifest-path thermo-sudoku-rs/Cargo.toml -- --check
python -m unittest analysis.test_run_17c_overlap_chunks -v
```

The comparison tests exhaust all `512 × 512` pairs of nine-bit endpoint
domains and cover branches, diamonds, cycles, dense 17-cell graphs, disjoint
fast-path parity, and randomized exact solution-set comparisons. Scanner tests
brute-check mask dominance, Hamiltonian orders, closure antichains, morph
realization, resume equivalence, path-alias rejection, and checkpoint/output
safety. Runner tests cover identity binding, artifact accounting, the exclusive
output-directory lock, deterministic child covers, manifest
binding, parent-versus-children exclusivity, restart behavior, safe bootstrap
failure, stable child-failure reporting, and aggregate accounting over logical
roots. Timeout tests cover process-local cancellation, parent auto-split,
durable singleton deferral and retry, exact ledger validation, deadline-final
and unique-result races, stale-entry reconciliation, and total-start budgets.
The full Python analysis test discovery passed 39 tests for this revision.

The production identity is schema `thermo-17c-overlap-chunk-run-v1`, algorithm
revision `saturated-axis-poset-antichain-hamiltonian-v1`, corpus FNV-1a64
`96baf249978384bb`, 49,158 records, 25,370 eligible records, 16 eligible
records per chunk, and exactly 1,586 contiguous chunks. Its command is the one
above with those explicit options. A read-only planning check is available as:

```text
python analysis/run_17c_overlap_chunks.py \
  --corpus <path-to>/17puz49158.txt \
  --binary thermo-sudoku-rs/target/release/thermo-17c-overlap.exe \
  --output-dir <artifact-root>/overlap-exact \
  --workers 4 --eligible-per-chunk 16 --dry-run
```

The `--dry-run` output must report the same corpus and binary hashes, 25,370
eligible records and 1,586 chunks. The actual run directory is local under
ignored `runs/`; neither partial progress nor a future result is silently
included in the Git commit.

## Bounded measurements

The fixed identity-coordinate and natural-order slice traversed all 49,158
records in under a quarter second and produced no candidate. This is an exact
result for that one slice only, not for the generalized search.

Before the target-aware alternative shortcut, a stratified exact sample of
100 eligible records classified 312,895 closure-maximal candidates in 59.45
seconds. The median record took 0.335 seconds and the 90th percentile 0.618
seconds. One exceptional record took about 19 seconds and 16.6 million solver
nodes. That sample suggested roughly 4.2 serial CPU hours, or two to four wall
hours with four workers. The full run falsified this timing extrapolation: the
rare record-level tail is far heavier than the stratified pilot captured.

The exact full run began at 2026-08-23 18:37 CEST. At the fixed 2026-08-24
06:14 snapshot it had published 324 of 1,586 chunks (20.43%), covering 5,184
eligible records and classifying 17,884,839 closure-maximal networks, all
multiple. No unique case or error had occurred. Among completed chunks the
median scanner time was 9.33 seconds, the 95th percentile 757 seconds, and the
maximum 26,258 seconds (7.29 hours); two then-active chunks had already run for
more than ten hours each. A censor-aware estimate at that snapshot was roughly
55 to 85 additional wall hours. These figures are an operational progress
record only. They neither sample the remaining catalogue uniformly nor support
a mathematical completion percentage.

At 2026-08-24 21:35 CEST the first launcher invocation was deliberately stopped
after 671 parent chunks had been published: 10,736 eligible records and
30,930,497 closure-maximal candidates had been classified, with no unique case
or error. Four in-flight parent chunks had then occupied cores for roughly 26,
8.2, 6.8 and 4.0 hours. Across the 671 completed chunks, elapsed time correlated
almost perfectly with solver nodes (Pearson `r = 0.9986`) but not with candidate
count (`r = 0.0357`), locating the heavy tail inside the Sudoku comparison
search rather than morph enumeration. The four parents were committed to exact
one-record child partitions and the run was resumed with one split worker and
three ordinary workers. All 671 published artifacts were retained; four
zero-length interrupted partials were ignored. This intervention changes only
scheduling and evidence packaging, not the candidate set or classification
algorithm, and it is not a search result.

The record-only intervention then exposed the flaw in retaining a permanently
active hard lane. At the controlled 2026-08-25 07:09 CEST stop, 760 parent
artifacts and six singleton children covered 12,166 eligible records and
34,901,476 classified candidates, all multiple, with zero unique or zero-solution
errors. No artifact had been published since 02:02. The four active tasks had
run for about 9.6, 9.6, 7.4 and 5.1 hours; the split task was already the single
eligible record on catalogue line 835. This confirmed that record subdivision
identifies the hard case but does not by itself prevent renewed saturation.

The bounded policy follows the measured tail. Only 19 of the 760 completed
parents exceeded 30 minutes, but they consumed 52.09 of 69.47 aggregate scanner
hours. Parents therefore receive 1,800 seconds before automatic subdivision,
while singleton records receive 300 seconds before deferral. The latter is
generous relative to the six completed siblings of line 835, which each took
under four seconds. These thresholds affect scheduling only; they cannot turn
an unresolved case into evidence.

A target-aware shortcut was implemented and measured, then rejected for the
scanner. It stopped after the first solution differing from the 17 mapped
catalogue clues, and every candidate on lines 1 and 803 did take that shortcut.
Nevertheless the expensive work was reaching the first solution, not finding
the second: line 803 remained at about 16.6 million nodes and slowed from about
19.1 to 24.2 seconds, while line 1 also became slightly slower. The generic
solver keeps the audited target-projection API, but the production scanner uses
the faster ordinary cap-two traversal.

The retained historical pilot artifacts are:

- `17c-overlap-identity-2026-08-23.jsonl`: complete only for the identity
  coordinate and natural-order slice;
- `17c-overlap-guided-100-2026-08-23.jsonl`: 100 dense-first guided cases, all
  multiple, explicitly incomplete.

Neither artifact supports a global 17-cell conclusion.

## Evidence boundary

A positive candidate can be verified independently by blocking its target and
solving the comparison Sudoku again. A completed negative scan is a
deterministic exhaustive-program result conditional on the audited 49,158
catalogue and the reductions above; it is not a SAT/LRAT nonexistence
certificate.

Before reporting a negative result, an auditor must at minimum check:

1. the corpus SHA-256, record count and syntax;
2. `run-identity.json` against the executable hash, algorithm revision, chunk
   size, eligible count and total chunk count above;
3. that each of the 1,586 logical parent chunks is represented by either its
   parent artifact or one committed, complete child partition, never both, and
   that the selected leaves cover source lines 1 through 49,158 exactly once;
4. every selected leaf through the launcher's `validate_artifact` checks and
   every split manifest against its bound corpus, executable, revision, parent
   range and exact child cover, including
   header/summary fingerprint agreement, exact range exhaustion, target-true
   zero count of zero, and `classified = unique + multiple` cap-two
   accounting;
5. that `deferred-tasks.json`, if present, passes its schema and run-identity
   validation, has an empty task list, matches the aggregate's recomputed
   `deferred_manifest_sha256`, and that the aggregate reports zero deferred
   singletons; and
6. terminal `summary.json` fields `complete:true`,
   `completed_chunks:1586`, `unique_found:false`, `totals.unique:0`, and an
   artifact-set SHA-256 plus split-manifest-set SHA-256 recomputed from the
   validated selected leaves and manifests.

Running the identical launcher command after completion performs checks 2–6
again and has no pending work. This independently validates the orchestration
and accounting, but it does not turn the negative into a proof certificate:
multiplicity of the non-emitted ordinary cases and exhaustiveness of the
reductions still rest on the exact Rust implementation. A positive case, if
one appears, is emitted immediately and requires a second solver or a fresh
target-blocking exhaustive solve before publication.

Until every exact chunk has completed and this aggregate audit has succeeded,
the existence of a 17-cell construction remains open.
