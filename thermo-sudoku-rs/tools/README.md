# Persistent CaDiCaL bridge

`cadical-incremental-bridge.cpp` is a small standalone process linked against
CaDiCaL's C++ API. It loads one canonical DIMACS master once, retains the same
solver and learned state across every CEGIS refinement, and accepts only four
line-oriented commands: `SOLVE`, `ADD`, `PING`, and `QUIT`.

Startup opens the plain, seekable CNF once. It reads the header from that handle
to cross-check the explicit `--variables` and `--clauses` arguments, rewinds the
same handle, and passes it to CaDiCaL's
`read_dimacs(FILE*, ..., strict=2)` for the only full-body pass. Using one handle
removes the pathname-replacement window between header validation and parsing.
The strict parse rejects noncanonical header whitespace, out-of-range literals,
unterminated clauses, and too few or too many clauses. Avoiding a redundant
token-by-token body pass matters for the hundreds-of-megabytes master. The CNF
must not be modified in place during startup; the run-lifetime sidecar lock
coordinates project writers but is not a security boundary against unrelated
processes that ignore it.

No CaDiCaL source or binary is vendored here. CaDiCaL is independently
obtained under its MIT license. The tested version is 2.1.3, tag
`rel-2.1.3`, commit
`f13d74439a5b5c963ac5b02d05ce93a8098018b8`.

## Build and protocol test on Windows

Build CaDiCaL first, then from `thermo-sudoku-rs` run:

```powershell
.\tools\build-cadical-bridge.ps1 -CadicalRoot C:\path\to\cadical
.\tools\test-cadical-bridge.ps1
cargo build --release --bin thermo-topology-cnf
```

The build helper embeds and reports the exact CaDiCaL Git revision and static
library SHA-256, and reports the resulting bridge executable SHA-256. The
protocol test covers canonical-header acceptance, noncanonical-header and
header/count rejection, a complete SAT model, an
empty incremental clause and UNSAT, conflict-limited UNKNOWN, malformed input,
and clean shutdown.

An equivalent direct build on Unix-like systems is:

```sh
g++ -std=c++17 -Wall -Wextra -Werror -O3 -DNDEBUG \
  -DTHERMO_CADICAL_REVISION=YOUR_40_HEX_COMMIT \
  -DTHERMO_CADICAL_LIBRARY_SHA256=YOUR_64_HEX_HASH \
  -I/path/to/cadical/src -I/path/to/cadical/build \
  tools/cadical-incremental-bridge.cpp \
  /path/to/cadical/build/libcadical.a \
  -o target/release/cadical-incremental-bridge
```

## Run

```powershell
.\target\release\thermo-topology-cnf.exe incremental-loop `
  --checkpoint ..\analysis\thermo-global-cegis-pilot-1000x32-2026-08-20.checkpoint `
  --next-checkpoint .\target\topology-next.checkpoint `
  --bridge-exe .\target\release\cadical-incremental-bridge.exe `
  --cnf .\target\topology-current.cnf `
  --max-iterations 1000 `
  --oracle-batch 32 `
  --pair-mode all `
  --prefer-selected `
  --symmetry-break d4-complement-v1 `
  --checkpoint-every 10 `
  --lazy-cuts .\target\topology-active.cuts `
  --lazy-active-seed 0 `
  --lazy-violation-batch 256
```

`--pair-mode all` learns every unordered pair among the SAT target and the
enumerated alternatives; `anchor` learns only target/alternative pairs.
`--prefer-selected` optionally gives positive initial phase hints to edge and
occupied-cell variables. It changes search order only and is off by default.
`--conflicts N` is a per-solve limit and must not exceed `INT_MAX`.
`--symmetry-break d4-complement-v1` optionally adds the versioned 148-clause
D4-times-digit-complement representative constraints. It is off by default;
the selected mode is recorded in the v2 CNF header and every terminal record.

`--lazy-cuts ACTIVE-MANIFEST` is the memory-scalable mode. The validated pair
checkpoint remains the complete authoritative pool in Rust, but the bridge
starts with the 57,384-clause base (plus an optional small
`--lazy-active-seed`) rather than materializing every pool cut in CaDiCaL.
After every SAT result, Rust validates the model against the base and active
cuts, then scans every 544-bit pool cut by wordwise intersection. If any are
missed, the shortest stable batch (256 by default, or `all`) is acknowledged
into the live bridge and the thermo oracle is not called. The oracle is reached
only after a complete scan reports zero missed pool cuts.

All new oracle pairs and all their deduplicated cuts are still appended to the
full checkpoint. At most the configured lazy batch is activated immediately;
later SAT models rescan the whole enlarged pool, so leaving the remaining new
cuts inactive is exact. The active manifest is atomically checksummed and
binds stable pool IDs to canonical solved-grid witnesses, the CNF schema, edge
order, and symmetry mode. A manifest from an append-only checkpoint prefix is
accepted on restart; an ahead, corrupt, mismatched, or wrong-witness manifest
is rejected.

The checkpoint is atomically replaced after every `--checkpoint-every N`
refinement batches and on every clean or terminal exit. Conservatively, a
crash can lose up to `N` CEGIS iterations (each may contain many pairs/cuts),
never corrupt the last checkpoint. In lazy mode, acknowledged active cuts are
kept in memory between those persistence points instead of forcing a large
checkpoint rewrite. Persistence always replaces the checkpoint before the
active manifest. Thus a crash leaves either the previous consistent pair and
active prefixes or an older manifest that validates as an append-only prefix
of the newer checkpoint; the manifest is never ahead. A terminal exit directly
after a scheduled write does not rewrite the identical checkpoint again.
Terminal output includes the actual `checkpoint_writes` and
`total_checkpoint_write_ms`; that time overlaps the reported refinement time.

For very large checkpoints, `--checkpoint-every 100` reduces write volume
substantially while bounding hard-crash loss to 100 completed refinement
batches. Clean exits and handled errors still flush immediately. In full mode,
the generated CNF is a
mutable, current snapshot rebuilt from the validated checkpoint at startup.
In lazy mode, it is a disposable startup snapshot while the bridge is live;
every normal exit rewrites it as the exact static base-plus-active formula.
The manifest can regenerate that same proof formula independently:

```powershell
.\target\release\thermo-topology-cnf.exe emit-active `
  --checkpoint .\target\topology-next.checkpoint `
  --active-cuts .\target\topology-active.cuts `
  --output .\target\topology-proof.cnf `
  --symmetry-break d4-complement-v1
```

The persistent mode takes run-lifetime operating-system locks on sidecar files
beside `--next-checkpoint`, `--cnf`, and the lazy active manifest. A second
writer using any path fails immediately. The sidecar files remain for
provenance after exit, but the lock itself is released automatically even if
the process crashes.

Incremental UNSAT is deliberately reported as provisional and without a
global conclusion. In lazy mode, base plus the active witnessed cuts is already
a sufficient exact negative formula; inactive pool cuts are unnecessary for
the proof. Freeze and hash the final checkpoint, manifest, and regenerated
CNF, rerun a fresh proof-producing solver on that exact static CNF, and
independently verify the LRAT proof before claiming an exclusion.
If the optional symmetry breaker was enabled, the independently checked proof
also relies on the documented D4-times-complement orbit lemma; alternatively,
repeat the final static proof without the breaker.
