# Non-overlapping topology SAT pilot

This note records the first end-to-end SAT/CEGIS pilot for the actual geometry
of the question: cell-disjoint thermometers, king-adjacent steps, and at most
19 covered cells. It remains a progress result, not a construction or an
exclusion.

## Exact master

`thermo-topology-cnf` builds a deterministic CNF whose satisfying assignments
contain both a solved classic Sudoku and a selected directed-edge graph. The
graph constraints enforce:

- at most one incoming and one outgoing edge at every cell;
- no pair of opposite directed edges;
- an occupied cell if and only if it is incident to a selected edge;
- at most 19 occupied cells; and
- strict digit increase on every selected edge.

The result is a vertex-disjoint union of directed paths. Strict increase makes
directed cycles impossible and limits every path to at most nine cells, so
every component is a valid thermometer. The master also includes the eight
necessary adjacent-symbol-swap conditions and every validated solution-pair
cut from the global checkpoint.

The structural formula has 7,226 variables and 57,384 clauses before pair
cuts. Exact 544-bit cut deduplication is performed during loading: the original
358,435-pair checkpoint contained 330,554 distinct clauses. The current deep
checkpoint contains 578,392 grid pairs and 535,066 distinct pair clauses, for
592,450 clauses in total.
The current exporter writes schema `thermo-topology-cnf-v2`. The optional
`--symmetry-break d4-complement-v1` mode adds 148 clauses and no variables;
`none` remains the default and the selected mode is recorded in the header.

The binary provides six modes:

```text
thermo-topology-cnf stats  --checkpoint PAIRS
thermo-topology-cnf emit   --checkpoint PAIRS --output MASTER.cnf
thermo-topology-cnf emit-active --checkpoint PAIRS \
  --active-cuts ACTIVE --output ACTIVE.cnf
thermo-topology-cnf decode --checkpoint PAIRS --model MODEL
thermo-topology-cnf loop   --checkpoint PAIRS --next-checkpoint NEXT \
  --sat-exe cadical.exe --cnf MASTER.cnf --model MASTER.model \
  --proof MASTER.proof --max-iterations N --conflicts N
thermo-topology-cnf incremental-loop --checkpoint PAIRS \
  --next-checkpoint NEXT --bridge-exe cadical-incremental-bridge.exe \
  --cnf MASTER.cnf --max-iterations N --oracle-batch 32 \
  --pair-mode all --checkpoint-every 10 --prefer-selected \
  --lazy-cuts ACTIVE --lazy-violation-batch 256 \
  --symmetry-break d4-complement-v1
```

`decode` checks the complete SAT model against the Sudoku, topology, coverage,
swap, and retained pair clauses before emitting paths. In `loop`, every SAT
model is decoded, passed to the exact Rust thermo solver, and either reported
unique or blocked by a newly validated pair cut. The patched CNF is tested to
be byte-for-byte identical to fresh regeneration.

The incremental-loop mode instead opens the canonical DIMACS master once,
cross-checks its header, rewinds that same handle, and gives it to CaDiCaL's
strict parser before keeping one library instance alive. Every solve returns
all 7,226 model values before any new clause is added. Exact batched thermo enumeration can
learn either all unordered solution pairs or only pairs anchored at the SAT
target. New clauses are monotone, acknowledged with an exact running clause
count. Full mode also applies them to the mutable CNF snapshot.

The memory-scalable lazy mode keeps the complete validated cut pool in Rust
but starts CaDiCaL with only the structural base and a persisted active subset.
Every SAT model is checked against the active clauses and then scanned against
all pool cuts by 544-bit wordwise intersection. A nonempty deterministic batch
of missed cuts is added and solved again without calling the thermo oracle.
Only a zero-violation full-pool scan permits an oracle call or a uniqueness
result. New oracle pairs and cuts all enter the authoritative checkpoint, while
at most the configured batch enters CaDiCaL immediately; future models rescan
the enlarged pool, so this is exact rather than heuristic.

The active manifest records stable pool IDs and canonical solved-grid witnesses
and is bound to the CNF schema, edge ordering, checkpoint prefix, and symmetry
mode. Normal exits rewrite the CNF as the exact static base-plus-active proof
formula. `emit-active` independently validates the manifest and regenerates
that formula. Thus a provisional lazy UNSAT can be rerun and proved using the
small active subset; inactive pool cuts are not required for soundness.

Checkpoint replacement is temporary-file, flush/sync, and atomic. The default
checkpoint interval is one refinement batch; a larger --checkpoint-every N
loses at most N refinement batches/CEGIS iterations on a crash and always
flushes on a clean or terminal exit. A batch can contain many new pairs and
cuts. Lazy activations acknowledged by the bridge remain in memory until the
same scheduled, terminal, or error-path persistence point. Persistence writes
the pair checkpoint before its active manifest, so either a crash retains the
previous consistent pair/active prefixes or the old manifest remains a valid
prefix of a newly replaced checkpoint; the manifest is never ahead. A
cadence-aligned terminal exit skips an identical second checkpoint rewrite
while still refreshing the terminal manifest and proof CNF.

For multi-gigabyte continuations, `--checkpoint-every 100` is a practical
hobby-machine setting. Relative to ten-batch persistence in the completed
2,970,392-to-5,050,392-pair run shape, it reduces scheduled full-checkpoint
record traffic from about 61.41 GiB to 6.28 GiB (89.77%). The tradeoff is that
a hard process or machine failure can discard at most 100 instead of 10
completed refinement iterations; handled errors and normal exits still flush.
Every restart validates the checkpoint and deterministically rebuilds the CNF
before starting a new bridge, so a stale or ahead-of-checkpoint CNF is never
trusted. --prefer-selected is an optional correctness-neutral positive phase
hint for edge and occupied variables. Each iteration reports SAT, full-model
decode/validation, oracle, refinement, and cumulative timings plus pair/cut
counts. Terminal metadata separately reports `checkpoint_writes` and
`total_checkpoint_write_ms`; scheduled checkpoint time is also contained in
the refinement timing, so those fields must not be summed. Retained positive
pair clauses are validated by wordwise intersection with a precomputed
selected-edge mask rather than materializing each clause.
Run-lifetime operating-system locks on the checkpoint, CNF, and active-manifest
paths prevent concurrent loops from silently overwriting one another.

The D4-times-complement breaker chooses a minimum corner at `r1c1`, requires
that digit to be at most five, and orders `r1c2 <= r2c1`. Complementation maps
digit `d` to `10-d` and reverses every thermometer. The 16 transformed
representatives preserve Sudoku, disjoint king-path geometry, coverage, and
solution multiplicity. A proof made with this option also depends on this
orbit-completeness lemma; rerunning the final static proof without it is the
alternative.

## Deep live run

The current pair checkpoint is:

```text
file:   analysis/thermo-global-cegis-pilot-1000x32-2026-08-20.checkpoint
pairs:  578,392
size:   94,856,432 bytes
FNV:    3012692e445f3d19
SHA256: AD9A46E304E16D0C45930D618E968864AA641A57A49F36C67B67B8B75BC754A1
```

An independent streaming verifier checked every pair and the checkpoint
footer. The first full-scale SAT master used 535,055 deduplicated pair cuts.
Thirteen candidate layouts were inspected and all were multiple; eleven
validated pair cuts were persisted by the automated loop, producing the
currently verified total of 535,066. All candidates were valid
non-overlapping layouts covering 18 or 19 cells, with 10 through 13 comparison
edges in six through eight thermometers. The exact Rust oracle found at least
two solutions for every candidate, generally in 38 through 43 search nodes.

The first decoded full-scale model was:

```text
target=245913678836572491971846523189635247427189356653724189398251764564397812712468935
thermos=5,15|19,28,29|34,24,23|40,41|52,53|56,55|70,71,79|76,68
covered_cells=19
comparison_count=11
classification=2+
```

The regenerated 578,392-pair CNF is deliberately not stored in the repository.
It is 769,906,157 bytes and has SHA-256
`49AA2A09D80892A54BCC32B5BEA15BD67ECFD0748E92C36A8C521086B30E0CA5`.
It is reproduced deterministically from the checkpoint and exporter.

## Lazy active-cut pilot

Materializing the entire growing pool inside CaDiCaL crossed 3 GB of resident
memory and paged badly on the 8 GB development host. A lazy run started from
the same 578,392-pair / 535,066-cut checkpoint with D4-complement symmetry,
positive selected-edge phase hints, no seed cuts, and a 256-cut activation cap.
The bridge therefore loaded 57,532 clauses rather than 592,598. Its first SAT
model already hit every old pool cut; the complete 535,066-cut Rust scan took
4.173 ms. Exact 33-solution enumeration yielded 528 new pairs and 417 distinct
new cuts, all retained in the checkpoint, while only the shortest 256 were
activated.

A restart loaded those 256 witnessed cuts and ran ten further all-pair CEGIS
iterations. Every old-pool scan was clean, each oracle iteration retained 528
new pairs, and the active cap admitted 256 new clauses per iteration. The
combined eleven-iteration disposable pilot ended with:

```text
pairs:                    584,200  (+5,808)
full unique pair cuts:    540,444  (+5,378)
active pair cuts:           2,816
static active CNF clauses:  60,348
static active CNF size:   4,745,478 bytes
ten-run SAT time:           151.855 ms
ten-run pool-scan time:      41.551 ms
ten-run elapsed time:         9.528 s
```

The final manifest was 481,870 bytes. Independent `emit-active` regeneration
was byte-identical to the terminal CNF in both a fresh small run and a resumed
run. The 584,200-pair pilot checkpoint and sidecars remain disposable local
artifacts under `target/`; the repository's audited checkpoint above is still
the 578,392-pair input. No unique construction or UNSAT result was found.

## First completed 1,000-iteration lazy run (256-cut cap)

A durable continuation started from the 890,392-pair / 819,923-cut
`topology-d4-b64-2026-08-20-b.checkpoint`. It used an oracle batch of 64,
all-pair learning, positive selected-edge phase hints, the D4-complement
breaker, atomic checkpointing every ten refinement batches, no active seed,
and a 256-cut activation cap. No per-solve conflict limit was set. The exact
command was:

```powershell
.\thermo-sudoku-rs\target\release\thermo-topology-cnf.exe incremental-loop `
  --checkpoint .\runs\topology-d4-b64-2026-08-20-b.checkpoint `
  --next-checkpoint .\runs\topology-lazy-d4-b64-2026-08-20-c.checkpoint `
  --bridge-exe .\thermo-sudoku-rs\target\release\cadical-incremental-bridge.exe `
  --cnf .\runs\topology-lazy-d4-b64-2026-08-20.cnf `
  --max-iterations 1000 --oracle-batch 64 --pair-mode all `
  --prefer-selected --symmetry-break d4-complement-v1 `
  --checkpoint-every 10 `
  --lazy-cuts .\runs\topology-lazy-d4-b64-2026-08-20.active `
  --lazy-active-seed 0 --lazy-violation-batch 256
```

All 1,000 oracle/refinement iterations found multiple solutions and retained
2,080 new all-pair witnesses each. Forty-nine additional SAT/full-pool passes
activated previously retained cuts before an oracle call, giving 1,049 SAT
solves and 1,049 complete pool scans. The terminal counters were:

```text
status=iteration-limit
proof_certified=false
global_19c_conclusion=false
iterations=1000
initial_pairs=890392
pairs=2970392
pairs_added=2080000
initial_unique_pair_cuts=819923
unique_pair_cuts=2715406
unique_pair_cuts_added=1895483
total_sat_ms=1751335.490
total_pool_scan_ms=15854.094
total_decode_validation_ms=1618.589
total_oracle_ms=258.953
total_refinement_ms=368582.411
total_oracle_nodes=168606
sat_solves=1049
full_pool_scans=1049
lazy_cuts_activated=257960
active_pair_cuts=257960
bridge_total_clauses=315492
bridge_clause_count_identity_verified=true
elapsed_seconds=2253.172444
```

The bridge clause-count identity was checked at termination: 57,532 structural
and symmetry clauses plus 257,960 active pair clauses equals 315,492. The run
used bridge protocol v1 and CaDiCaL commit
`f13d74439a5b5c963ac5b02d05ce93a8098018b8`; the linked CaDiCaL library
SHA-256 was
`6b97694f2c909a9de81eb7c130eccb9f7c41d57b3d66bf2cce5e851dea0518ed`.

The frozen terminal artifacts are local research files under the ignored
`runs/` directory. They are not tracked by Git or pushed to the repository:

| Artifact | Bytes | SHA-256 | FNV-1a 64 |
| --- | ---: | --- | --- |
| `runs/topology-lazy-d4-b64-2026-08-20-c.checkpoint` | 487,144,434 | `6b62e6fd694c26c6ef57d29cbccc9dcf608bc9c3ee101f8518c76abb5d9e7b3d` | `90deeb8c23e64c21` |
| `runs/topology-lazy-d4-b64-2026-08-20.active` | 44,342,764 | `1f48190eb0b051f269a56dfeac135e9efaafbc62f33b8e3494cf94ed229878e6` | `bd8abd5621bf94f1` |
| `runs/topology-lazy-d4-b64-2026-08-20.cnf` | 343,133,002 | `8b999f0fcf3f93ad50cdf498fe2e5cf0dd4d7e765daf8a0be821740d6823df11` | not applicable |

Here the checkpoint FNV covers all 2,970,392 canonical grid pairs. The
manifest FNV covers its ordered 257,960 cut IDs and first-occurrence grid-pair
witnesses. Its pool-prefix metadata binds to the complete terminal checkpoint:
2,970,392 pairs, 2,715,406 unique cuts, and FNV `90deeb8c23e64c21`.
The independently reconstructed directed-edge-order FNV is
`f12501e5f1df08d5`.

```powershell
python -X utf8 analysis/verify_topology_active_cnf.py `
  runs/topology-lazy-d4-b64-2026-08-20-c.checkpoint `
  runs/topology-lazy-d4-b64-2026-08-20.active `
  runs/topology-lazy-d4-b64-2026-08-20.cnf `
  --expected-symmetry-break d4-complement-v1 --json
```

`analysis/verify_topology_active_cnf.py` then performed a separate,
standard-library Python audit of all three files. In 244.504290 seconds it
validated every solved Sudoku pair, exact pair and cut uniqueness, checkpoint
and manifest checksums, append-only prefix binding, stable cut indices, and
every first-occurrence witness. It independently regenerated all 57,532 base
clauses and all 257,960 active clauses; the complete 7,226-variable,
315,492-clause DIMACS stream matched the terminal CNF byte for byte. The
adversarial verifier tests are in `analysis/test_verify_topology_active_cnf.py`
and its scope is documented in `analysis/verify-topology-active-cnf.md`.

This audit establishes formula provenance: the frozen CNF is exactly the
documented base-plus-active formula, and every active cut has its claimed
solution-pair witness. It is not an UNSAT proof or proof certification. The
CEGIS run stopped only because it reached its configured iteration limit; it
found neither a unique thermometer construction nor an UNSAT result, and no
LRAT proof was produced. A future UNSAT claim still requires a fresh
proof-producing solve of this exact static CNF and independent LRAT checking.
Because this formula uses `d4-complement-v1`, such a claim would additionally
rely on the documented orbit-completeness lemma, unless reproduced without the
symmetry breaker.

## Second completed 1,000-iteration lazy run (64-cut cap)

The second durable segment continued from the first run's 2,970,392-pair /
2,715,406-cut checkpoint, but started a new empty active manifest and reduced
the activation cap from 256 to 64. All other important search settings stayed
the same: batch 64, all-pair learning, positive phase hints, D4-complement
symmetry, unlimited conflicts, and checkpointing every ten refinements. The
exact command was:

```powershell
.\thermo-sudoku-rs\target\release\thermo-topology-cnf.exe incremental-loop `
  --checkpoint .\runs\topology-lazy-d4-b64-2026-08-20-c.checkpoint `
  --next-checkpoint .\runs\topology-lazy64-d4-b64-2026-08-20-d.checkpoint `
  --bridge-exe .\thermo-sudoku-rs\target\release\cadical-incremental-bridge.exe `
  --cnf .\runs\topology-lazy64-d4-b64-2026-08-20.cnf `
  --max-iterations 1000 --oracle-batch 64 --pair-mode all `
  --prefer-selected --symmetry-break d4-complement-v1 `
  --checkpoint-every 10 `
  --lazy-cuts .\runs\topology-lazy64-d4-b64-2026-08-20.active `
  --lazy-active-seed 0 --lazy-violation-batch 64
```

Again, every one of the 1,000 oracle/refinement iterations found multiple
solutions and retained 2,080 new all-pair witnesses. The smaller activation
cap required 118 additional SAT/full-pool passes before oracle calls, for
1,118 solves and scans in total. The exact terminal summary was:

```text
status=iteration-limit
proof_certified=false
global_19c_conclusion=false
iterations=1000
initial_pairs=2970392
pairs=5050392
pairs_added=2080000
initial_unique_pair_cuts=2715406
unique_pair_cuts=4610947
unique_pair_cuts_added=1895541
checkpoint_fnv1a64=c31d95dcf4747041
total_sat_ms=394212.121
total_pool_scan_ms=37285.560
total_decode_validation_ms=862.805
total_oracle_ms=288.774
total_refinement_ms=1250338.733
total_oracle_nodes=168226
sat_solves=1118
full_pool_scans=1118
lazy_cuts_activated=66168
active_pair_cuts=66168
active_fnv1a64=657cc9d6c505f615
bridge_total_clauses=123700
bridge_clause_count_identity_verified=true
elapsed_seconds=2323.975617
```

The second frozen trio is also local-only under ignored `runs/` and was not
committed or pushed:

| Artifact | Bytes | SHA-256 | FNV-1a 64 |
| --- | ---: | --- | --- |
| `runs/topology-lazy64-d4-b64-2026-08-20-d.checkpoint` | 828,264,434 | `c733d7856a7cc2e9fad82664e3d0fe8b3be311829f326f7555696af683c798aa` | `c31d95dcf4747041` |
| `runs/topology-lazy64-d4-b64-2026-08-20.active` | 11,380,166 | `80856b2cb76bfb17f16a551d17f4f6231128cc080f4b0fa437f8e112ad3298f3` | `657cc9d6c505f615` |
| `runs/topology-lazy64-d4-b64-2026-08-20.cnf` | 86,503,133 | `8a09011770661bfa977e725ddd22cce42c85f2ca7fc4ecf92709037fc7b0d34c` | not applicable |

The independent Python verifier completed in 341.246714 seconds. It validated
all 5,050,392 checkpoint pairs, 4,610,947 unique cuts, the exact full-prefix
manifest binding, and all 66,168 first-occurrence witnesses. It independently
regenerated the 57,532 base clauses plus 66,168 active clauses; the resulting
7,226-variable, 123,700-clause stream matched the terminal CNF byte for byte.
The edge-order FNV remained `f12501e5f1df08d5`. This establishes the same
formula provenance as for the first run, but it is not an UNSAT certificate.

Relative to the 256-cut-cap segment, the 64-cut cap ended with 66,168 rather
than 257,960 active cuts, 123,700 rather than 315,492 total clauses, and an
86,503,133-byte rather than 343,133,002-byte CNF. Despite 1,118 rather than
1,049 SAT solves, cumulative SAT time fell from 1,751,335.490 ms to
394,212.121 ms. Total elapsed time remained similar—2,323.975617 seconds
versus 2,253.172444 seconds—because the reported refinement stage, which
includes cadence-10 persistence of the increasingly large checkpoint, rose
from 368,582.411 ms to 1,250,338.733 ms and dominated the second run. The full
pool was also larger, raising scan time from 15,854.094 ms to 37,285.560 ms.
Thus cap 64 materially reduced the live/static SAT formula and SAT time, but
not end-to-end time under this checkpoint cadence.

This run also ended only at its configured iteration limit. It found no unique
thermometer construction, did not reach UNSAT, and produced no LRAT proof. The
19-cell question therefore remains open.

## Three further completed continuations (`f`, `g`, and `h`)

An interrupted `d`-to-`e` continuation first left a crash-consistent prefix
after 106 completed refinement batches. Restart validation accepted its
5,270,872 pairs, 4,812,216 unique cuts, and checkpoint FNV
`f22692d4702ffe81`. This was recovered input, not a completed 1,000-iteration
run, and is not counted as one below. Its 73,219-cut active manifest was copied
byte-for-byte to the `f` manifest before the next launch.

The next three runs shared `--max-iterations 1000`, `--oracle-batch 64`,
`--pair-mode all`, `--prefer-selected`,
`--symmetry-break d4-complement-v1`, `--checkpoint-every 100`,
`--lazy-active-seed 0`, `--lazy-violation-batch 64`, and unlimited conflicts.
The exact command shape was:

```powershell
.\thermo-sudoku-rs\target\release\thermo-topology-cnf.exe incremental-loop `
  --checkpoint INPUT.checkpoint --next-checkpoint OUTPUT.checkpoint `
  --bridge-exe .\thermo-sudoku-rs\target\release\cadical-incremental-bridge.exe `
  --cnf OUTPUT.cnf --max-iterations 1000 --oracle-batch 64 `
  --pair-mode all --prefer-selected --symmetry-break d4-complement-v1 `
  --checkpoint-every 100 --lazy-cuts OUTPUT.active `
  --lazy-active-seed 0 --lazy-violation-batch 64
```

The path and active-set substitutions were:

| Run | `INPUT.checkpoint` | `OUTPUT` stem | Initial active set |
| --- | --- | --- | ---: |
| `f` | `runs/topology-lazy64-d4-b64-2026-08-20-e` | `runs/topology-lazy64-d4-b64-2026-08-20-f` | copied `e.active`, 73,219 cuts |
| `g` | `runs/topology-lazy64-d4-b64-2026-08-20-f` | `runs/topology-lazyreset-d4-b64-2026-08-20-g` | fresh, 0 cuts |
| `h` | `runs/topology-lazyreset-d4-b64-2026-08-20-g` | `runs/topology-lazyreset2-d4-b64-2026-08-20-h` | fresh, 0 cuts |

Each completed all 1,000 oracle/refinement iterations, added exactly 2,080,000
new solution pairs, and ended at `status=iteration-limit` with
`proof_certified=false` and `global_19c_conclusion=false`. The exact terminal
counters were:

| Counter | `f` | `g` | `h` |
| --- | ---: | ---: | ---: |
| initial pairs | 5,270,872 | 7,350,872 | 9,430,872 |
| final pairs | 7,350,872 | 9,430,872 | 11,510,872 |
| initial unique cuts | 4,812,216 | 6,705,225 | 8,600,131 |
| final unique cuts | 6,705,225 | 8,600,131 | 10,497,322 |
| unique cuts added | 1,893,009 | 1,894,906 | 1,897,191 |
| SAT solves / full scans | 1,103 / 1,103 | 1,190 / 1,190 | 1,195 / 1,195 |
| active cuts activated | 65,726 | 68,436 | 68,768 |
| final active cuts | 138,945 | 68,436 | 68,768 |
| terminal clauses | 196,477 | 125,968 | 126,300 |
| oracle nodes | 592,722 | 169,711 | 168,023 |
| total SAT ms | 1,434,821.481 | 361,493.872 | 400,813.815 |
| total pool-scan ms | 53,990.814 | 76,357.786 | 95,955.461 |
| total decode/validation ms | 1,462.542 | 849.686 | 853.799 |
| total oracle ms | 508.805 | 287.078 | 275.636 |
| total refinement ms | 93,311.087 | 121,314.614 | 163,636.157 |
| checkpoint writes | 11 | 11 | 11 |
| total checkpoint-write ms | 64,234.624 | 95,537.475 | 138,613.145 |
| elapsed seconds | 1,634.141970 | 607.649214 | 722.858954 |

Resetting the active set for `g` and `h` was exact because every SAT model was
still scanned against the complete retained cut pool before an oracle call.
Although those runs used more SAT solves and much larger full pools than `f`,
their roughly 68,000 active clauses kept SAT time at 361–401 seconds instead
of `f`'s 1,435 seconds with 138,945 active cuts. End-to-end time fell from
1,634 seconds to 608 and 723 seconds. Checkpoint persistence then became a
growing share of refinement time as the text corpus expanded, despite the
cadence reduction from ten to 100.

All nine terminal files below are local-only under ignored `runs/`; none is
tracked or pushed. Independent Python verification reconstructed every pair,
unique cut, prefix binding, first witness, base clause, and active clause, and
matched each CNF byte for byte:

| Run | Artifact | Bytes | SHA-256 | FNV-1a 64 |
| --- | --- | ---: | --- | --- |
| `f` | checkpoint | 1,205,543,154 | `ac7a86ba6004dc9aec3e74fb5476335916df173c113ea549a4ab654534505c03` | `b8c4984bf9332d81` |
| `f` | active manifest | 23,896,961 | `85020436cc55f0f16b9e625036bbbbd467e6a36c5dbd717ed83110d05f6f6ef6` | `f35923eac9718a16` |
| `f` | CNF | 180,670,807 | `ccee23d55ebc9e579399d0374eff3e0fa9ab2684a892a96d58f5202b74973916` | not applicable |
| `g` | checkpoint | 1,546,663,154 | `c510e488d3bcf4eefe2e0d4724bf699d3d2e7e4f655467e2bfcf44d98970d0be` | `9ab296239d4bcf79` |
| `g` | active manifest | 11,770,621 | `5c3029ef4dc2f2c8d989a925fd54bd5d2a16e09d861a193b5340c2b06c4a8a17` | `aa21ef2d2508e460` |
| `g` | CNF | 89,468,749 | `83ce9c8b056911e13f3c9d889a1294183c1a592c9eea6bae8f693409fe3e6ee2` | not applicable |
| `h` | checkpoint | 1,887,783,156 | `261c09331a69e082149f26bf8d0957c6ec3e3e32c4877ed86e09d7f7620cb687` | `7f40539ac4cebba9` |
| `h` | active manifest | 11,844,539 | `fc70f2fb718ee40dcb88662ac0dbdfada0f2e7eb7a4f8202dce8135d0299b6df` | `a8f762b3302099dc` |
| `h` | CNF | 89,917,085 | `8a048a195234120505c1f4a7d01a2c07e0d094b0d720fb81f3b01d044dca84a7` | not applicable |

The independent verifier runtimes, including final stability rehashes, were
436.326614 seconds for `f`, 561.561107 seconds for `g`, and 966.001226 seconds
for `h`; the last audit paged under the Python verifier's exact duplicate sets
at this corpus size. The common edge-order FNV was `f12501e5f1df08d5`.

None of `f`, `g`, or `h` found a unique thermometer construction or reached
UNSAT, and no LRAT proof was produced. Formula provenance is independently
checked; proof certification is not. The 19-cell question remains open.

## Two-hour continuation from `h` (`i` through `o`)

On 2026-08-20 the exact lazy topology search continued for approximately two
hours from the independently verified `h` checkpoint. Each segment used a
fresh active set but retained and scanned the complete checkpoint cut pool,
so resetting the in-solver subset changed search order and cost, not the
formula being enforced. The configuration remained batch 64, all-pair
learning, selected-edge phase hints, `d4-complement-v1`, checkpoint cadence
100, lazy violation batches of 64, and unlimited CaDiCaL conflicts.

Five segments (`i` through `m`) completed 1,000 refinements each. Segment `n`
completed 400 and the final segment `o` completed 50, for 5,450 additional
oracle/refinement iterations. Because every iteration retained all pairs from
65 enumerated grids, the continuation added exactly 11,336,000 solution pairs.
It added 10,374,883 distinct cuts, a 91.521551% new-cut rate. Every segment
ended at its configured iteration limit; none found a unique topology or
reached UNSAT.

The final state was reread from disk through the separate `stats` path, which
recomputed the record counts and checkpoint FNV:

```text
checkpoint_pairs=22846872
unique_pair_cuts=20872205
duplicate_pair_clauses=1974667
checkpoint_fnv1a64=87b0f87beded3631
```

The terminal `o` segment had 4,754 active cuts, active FNV
`9cd72b02d7798389`, and 62,286 bridge clauses (57,532 base plus 4,754 active).
It reported `bridge_clause_count_identity_verified=true`. The full continuation
used 7,098.536449 seconds of measured process time; artifact rotation brought
the elapsed wall-clock interval to about two hours and three minutes so the
last atomic checkpoint could complete cleanly.

The final retained trio is local-only under ignored `runs/` and is not pushed:

| Artifact | Bytes | SHA-256 | FNV-1a 64 |
| --- | ---: | --- | --- |
| `runs/topology-lazyreset9-d4-b64-2026-08-20-o.checkpoint` | 3,746,887,156 | `f0a5c4d68825e0443d13329293a6f8482da24de6fdbea2d077171463fc55a9ee` | `87b0f87beded3631` |
| `runs/topology-lazyreset9-d4-b64-2026-08-20-o.active` | 821,460 | `68bfd6ce6ae62e6fd60f5567c0e516e1f22cf77030a7f6f3d9fb7fcb79df321b` | `9cd72b02d7798389` |
| `runs/topology-lazyreset9-d4-b64-2026-08-20-o.cnf` | 7,027,849 | `01a7291d150b44f5c8a3ab1f5cb26016d15c8bc74417196f888243da11203c6f` | not applicable |

Superseded scratch trios `i` through `n` were removed only after their atomic
successors committed; the independently audited `h` trio and final `o` trio
remain. At this larger scale the cross-language Python verifier would require
substantial memory and paging, so it was not rerun for `o`. The checkpoint
reread, terminal bridge identity check, and SHA-256 hashes above validate the
retained local artifacts but do not constitute an independent formula audit or
an UNSAT proof.

## Exact checkpoint-memory hardening

The original loader materialized the complete text file and then retained two
standard hash sets whose keys duplicated every 162-byte grid pair and every
72-byte cut. At 5,050,392 pairs and 4,610,947 unique cuts, the three growing
vectors and two hash tables alone accounted for about 3,824 MiB. A monitored
read-only `stats` load of that 828,264,434-byte checkpoint peaked at
5,118,996,480 private bytes (4,881.86 MiB) and 3,143,061,504 working-set bytes.

The loader now validates one line at a time, packs each solved grid into 41
bytes, stores a pair in 82 bytes, and uses flat `u32` probe tables containing
indices into the append-ordered pair and cut vectors. A hash determines only
the initial probe bucket: lookup always compares the complete stored key, so
collisions cannot merge records or change first-cut witnesses. The standard
batch-64 / 1,000-iteration continuation reserves its exact 2,080,000-pair
maximum. Larger theoretical runs cap eager reserve at that amount and grow
the vectors in bounded 262,144-record chunks, avoiding an oversized allocation
before the first solve. The durable
text checkpoint, FNV stream, cut order, first-witness convention, manifest,
and CNF formats did not change.

On the later local 5,270,872-pair / 4,812,216-cut prefix, a monitored read-only
load completed in 26.685 seconds and peaked at 903,327,744 private bytes
(861.48 MiB), with an 869,355,520-byte working set. A second controlled load
reserved the complete worst-case capacity for another 1,000 all-pair,
batch-64 iterations; it was deliberately stopped by a held manifest lock
before any output write and peaked at 1,299,890,176 private bytes
(1,239.67 MiB) and 935,936,000 working-set bytes. Thus the operational
continuation footprint is about 70% below the previous roughly 4.03 GiB live
footprint and no longer approaches the limit of an 8 GiB host at this depth.

Focused tests cover packed-grid round trips and lexicographic order, exact
collision resolution, duplicate rejection, checksum and byte-for-byte
checkpoint re-emission, and stable first witnesses. As a full-scale format
check, regenerating the active CNF from the frozen 5,050,392-pair checkpoint
produced the same 86,503,133 bytes and SHA-256
`8a09011770661bfa977e725ddd22cce42c85f2ca7fc4ecf92709037fc7b0d34c`.

## SAT and proof toolchain

The live run used CaDiCaL 2.1.3, tag `rel-2.1.3`, commit
`f13d74439a5b5c963ac5b02d05ce93a8098018b8`. The locally built executable had
SHA-256
`9E9FC7F8ACBD4700DB6F674D13F29FD1C78A1D4F1215FE9500B817F618182E47`.
On the full 770 MB master, a fresh process parsed and found a SAT model in
roughly 35 seconds; avoiding CNF regeneration leaves process restart and parse
time as the dominant cost.

The future negative-certificate path was smoke-tested separately: CaDiCaL
emitted a textual LRAT proof for a small UNSAT formula, and CakeLPR commit
`a36874a8b750b43fe4b385b8ddbf5b033e46a3fa` independently returned
`s VERIFIED UNSAT`. The local CakeLPR executable SHA-256 was
`31996B543DE64E384751792A07D93E394F4712245BBBD3C7D7E87A1D04761DCB`.
No proof exists for the thermo master: the completed bounded runs did not
return UNSAT, and no terminal static formula has been certified by an LRAT
proof.

## Interpretation

The SAT model and exact oracle now form a correct end-to-end CEGIS loop for all
non-overlapping thermometer partitions covering at most 19 cells. The initial
pilot candidates, ten completed 1,000-iteration durable runs, and 556 further
validated refinement batches advanced only through multiple candidates. Every
completed segment reached its iteration limit without testing its terminal
refinement to exhaustion. The current validated state contains 22,846,872
solution pairs and 20,872,205 unique cuts. The final large checkpoint was
reread by `stats`, while the last cross-language formula audit remains the
smaller `h` trio. Therefore the original question remains open.

An UNSAT result accompanied by a checked LRAT proof would exclude 19-cell
puzzles directly. A unique candidate would instead be certified by its paths,
target grid, and an UNSAT proof for a second distinct Sudoku. The persistent
incremental CaDiCaL bridge has now completed ten durable 1,000-iteration runs;
the other 556 batches are validated input history rather than additional full
runs. Further refinement can resume from the retained `o` checkpoint with a
fresh active manifest. If it
reaches UNSAT, that result remains provisional until a fresh proof-producing
run on the exact final static CNF is independently verified.

The pre-bridge exporter source and release executable SHA-256 values recorded
for the original live run were, respectively,
`191D70BDD112B8B1A42859050BC6C3E3159C3E385270B8B8A68B2522A61FC141`
and
`18014B2DDB4532DCB2E097A7C19F185EB5F0F4C5BBDCFFEFFCF1C8B4A40A349B`.

The `f`/`g`/`h` runs used bridge-enabled topology source and local release
executable SHA-256 values
`6AF8E6792105BD2FFEBE510561241990648DCEC1619F00ADEBC1008A2BEE85DC`
and
`226F12710A3036233AE8DC80F9A52C95AF43B2EF2BA9DD8B4E1822D9DC650E37`.
A subsequent test-only strengthening left production behavior unchanged; the
current source and rebuilt executable SHA-256 values are
`A1595CCDC9A43D203C027870FEF35BF206B6818AA462B78DF4E764C4D69D2D61`
and
`150916D26AF7B105A709381816915B31E36949C9A439A1BBD1E11DC96AF78C8C`.
The bridge source and local executable SHA-256 values are
`7A76C6FA01D1A6439A0884AD6907E459218C4511B65EB11870C07ACFFF981FA2`
and
`D9D10392B201267AE6B826C8B9D4041706AF72F4D92D8585BC680BB6E88BA41E`.
