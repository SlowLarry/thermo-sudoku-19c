# Solver benchmarks

The benchmark reports separate solver time from process startup and data
marshalling. Each report records its machine, software revisions, corpus
identity, repetition policy, and raw samples.

| Report | Purpose |
| --- | --- |
| [Native Rangsk comparison](NATIVE_RANGSK.md) | Primary comparison with Rangsk's solver running persistently on the desktop CLR. |
| [Interactive Sudoku Solver](ISS.md) | Independent solution-count cross-check. |
| [Rangsk WebAssembly comparison](WASM_RANGSK.md) | Native-versus-WASM host-cost measurement. |
| [Two-cell extension screen](TWO_CELL_SCREEN.md) | Throughput and correctness of the shared-prefix extension algorithm. |

Supporting harnesses are in [`native_harness/`](native_harness/) and
[`wasm_harness/`](wasm_harness/). The Python drivers write machine-readable raw
results; committed reports should be regenerated only with the exact upstream
revision and corpus stated in the corresponding document.
