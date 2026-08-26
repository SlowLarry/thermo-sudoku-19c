# Persistent CaDiCaL bridge

`cadical-incremental-bridge.cpp` keeps one CaDiCaL instance alive while
`thermo-topology-cnf` adds CEGIS cuts. Learned clauses survive between solves;
the bridge contains no Sudoku-specific logic.

## Contents

- [Requirements](#requirements)
- [Build and test](#build-and-test)
- [Protocol](#protocol)
- [Use with the topology search](#use-with-the-topology-search)
- [Persistence and proof boundary](#persistence-and-proof-boundary)

## Requirements

CaDiCaL is not vendored. The tested version is 2.1.3 (`rel-2.1.3`, commit
`f13d74439a5b5c963ac5b02d05ce93a8098018b8`). Build it as a static library
before building the bridge. CaDiCaL is distributed under the MIT licence.

At startup the bridge opens one plain seekable DIMACS file, checks its declared
variable and clause counts, and asks CaDiCaL to parse the same open handle in
strict mode. The file must not be modified during startup.

## Build and test

Windows, from `thermo-sudoku-rs`:

```powershell
./tools/build-cadical-bridge.ps1 -CadicalRoot C:/path/to/cadical
./tools/test-cadical-bridge.ps1
cargo build --release --bin thermo-topology-cnf
```

The build helper embeds the CaDiCaL Git revision and static-library SHA-256 in
the executable and prints those values together with the bridge hash.

Unix-like systems:

```sh
g++ -std=c++17 -Wall -Wextra -Werror -O3 -DNDEBUG \
  -DTHERMO_CADICAL_REVISION=YOUR_40_HEX_COMMIT \
  -DTHERMO_CADICAL_LIBRARY_SHA256=YOUR_64_HEX_HASH \
  -I/path/to/cadical/src -I/path/to/cadical/build \
  tools/cadical-incremental-bridge.cpp \
  /path/to/cadical/build/libcadical.a \
  -o target/release/cadical-incremental-bridge
```

## Protocol

The bridge writes a `READY` line containing the protocol version, CNF counts,
CaDiCaL signature, revision, library hash, and phase-hint mode. It then accepts
one whitespace-separated command per line:

| Command | Meaning | Response |
| --- | --- | --- |
| `SOLVE N` | Solve with conflict limit `N`; `-1` means unlimited. | `RESULT SAT`, `RESULT UNSAT`, or `RESULT UNKNOWN`; SAT is followed by a complete `MODEL` line. |
| `ADD N L... 0` | Add one non-tautological clause containing exactly `N` literals. | `ADDED` with incremental and total clause counts. |
| `PING` | Check liveness and counts. | `PONG`. |
| `QUIT` | Exit cleanly. | `BYE`. |

Malformed commands, invalid literals, duplicate or opposite literals, and CNF
identity errors terminate the process. End-of-file is a normal cleanup path.

## Use with the topology search

```powershell
./target/release/thermo-topology-cnf.exe incremental-loop `
  --checkpoint <input-pairs.checkpoint> `
  --next-checkpoint <output-pairs.checkpoint> `
  --bridge-exe ./target/release/cadical-incremental-bridge.exe `
  --cnf <working.cnf> `
  --max-iterations 1000 `
  --oracle-batch 32 `
  --pair-mode all `
  --checkpoint-every 10
```

`--pair-mode all` learns every unordered pair among the SAT target and the
enumerated alternatives; `anchor` learns only target/alternative pairs.
`--prefer-selected` supplies initial positive phase hints for topology edge and
occupied-cell variables and changes search order only. The optional
`--symmetry-break d4-complement-v1` mode is recorded in the CNF and terminal
metadata.

For large cut pools, add:

```text
--lazy-cuts <active.cuts> --lazy-active-seed 0 --lazy-violation-batch 256
```

The pair checkpoint remains the complete authoritative cut pool. Before each
oracle call, Rust validates the SAT model against the base formula, active
cuts, and then every inactive pool cut. Missed cuts are activated and the
oracle is skipped. The oracle is called only after a full pool scan finds no
violation, so lazy activation changes memory use, not the search result.

## Persistence and proof boundary

Checkpoint writes use checksummed same-directory replacement.
In lazy mode the pair checkpoint is replaced before the active manifest; an
older valid manifest may therefore describe an append-only checkpoint prefix,
but a manifest ahead of or incompatible with its checkpoint is rejected.
Sidecar operating-system locks prevent two project writers from sharing the
same checkpoint, CNF, or active-manifest path.

Regenerate the exact static base-plus-active formula with:

```powershell
./target/release/thermo-topology-cnf.exe emit-active `
  --checkpoint <output-pairs.checkpoint> `
  --active-cuts <active.cuts> `
  --output <proof.cnf> `
  --symmetry-break d4-complement-v1
```

Incremental `UNSAT` is provisional. A formal exclusion requires freezing and
hashing the checkpoint, manifest, and regenerated CNF; solving that exact
static CNF with proof output; and independently verifying the LRAT proof. If a
symmetry breaker is used, the proof also relies on its documented orbit lemma,
or the final proof must be repeated without the breaker.
