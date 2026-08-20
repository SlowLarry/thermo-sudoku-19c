"""Compare the Rust thermo solver with Rangsk's .NET AOT WASM prototype."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from benchmarks.quick_compare import Case, load_cases  # noqa: E402
from benchmarks.quick_compare_iss import run_rust  # noqa: E402
from thermo_search.thermo_anneal import idx2str  # noqa: E402


def summarize(values: list[float]) -> dict[str, object]:
    ordered = sorted(values)
    return {
        "median": statistics.median(values),
        "p10": quantile(ordered, 0.1),
        "p90": quantile(ordered, 0.9),
        "min": ordered[0],
        "max": ordered[-1],
        "samples": values,
    }


def quantile(ordered: list[float], fraction: float) -> float:
    if len(ordered) == 1:
        return ordered[0]
    position = (len(ordered) - 1) * fraction
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    remainder = position - lower
    return ordered[lower] * (1 - remainder) + ordered[upper] * remainder


def run_wasm(
    cases: list[Case],
    *,
    node: str,
    bundle: Path,
    warmup_rounds: int,
    repeats: int,
) -> tuple[dict[str, object], float]:
    request = {
        "warmupRounds": warmup_rounds,
        "repeats": repeats,
        "cases": [
            {
                "name": case.name,
                "category": case.group,
                "blank": 9,
                "constraints": [
                    f"thermo:{idx2str(path)}" for path in case.layout
                ],
                "op": "count",
                "expected": case.expected,
                "maxCount": 2,
                "multiThread": False,
            }
            for case in cases
        ],
    }
    command = [
        node,
        str(ROOT / "benchmarks" / "wasm_rangsk_adapter.mjs"),
        str(bundle),
    ]
    started = time.perf_counter()
    completed = subprocess.run(
        command,
        input=json.dumps(request),
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=900,
    )
    wall_seconds = time.perf_counter() - started
    if completed.returncode != 0:
        raise RuntimeError(
            "WASM benchmark adapter failed:\n"
            f"stdout: {completed.stdout}\n"
            f"stderr: {completed.stderr}"
        )
    return json.loads(completed.stdout), wall_seconds


def aggregate(rows: list[dict[str, object]], group: str) -> dict[str, object]:
    selected = [row for row in rows if group == "all" or row["group"] == group]
    rust = [float(row["rust"]["median_ms"]) for row in selected]
    wasm_total = [float(row["wasm"]["total_ms"]["median"]) for row in selected]
    wasm_build = [float(row["wasm"]["build_ms"]["median"]) for row in selected]
    wasm_count = [float(row["wasm"]["count_ms"]["median"]) for row in selected]
    rust_measured_total = sum(
        sum(float(value) for value in row["rust"]["samples_ms"])
        for row in selected
    )
    wasm_measured_total = sum(
        sum(float(value) for value in row["wasm"]["total_ms"]["samples"])
        for row in selected
    )
    return {
        "cases": len(selected),
        "rust_ffi_median_ms": statistics.median(rust),
        "wasm_total_median_ms": statistics.median(wasm_total),
        "wasm_build_median_ms": statistics.median(wasm_build),
        "wasm_count_median_ms": statistics.median(wasm_count),
        "aggregate_median_latency_speedup": sum(wasm_total) / sum(rust),
        "measured_work_speedup": wasm_measured_total / rust_measured_total,
        "rust_median_total_ms": sum(rust),
        "wasm_median_total_ms": sum(wasm_total),
        "rust_measured_work_ms": rust_measured_total,
        "wasm_measured_work_ms": wasm_measured_total,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--upstream-root", type=Path, required=True)
    parser.add_argument("--node", default="node")
    parser.add_argument(
        "--corpus", type=Path, default=ROOT / "sources" / "min_thermos_9_8_2.txt"
    )
    parser.add_argument("--rust-library", type=Path)
    parser.add_argument("--rust-repeats", type=int, default=100)
    parser.add_argument("--wasm-warmup-rounds", type=int, default=20)
    parser.add_argument("--wasm-repeats", type=int, default=100)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.rust_repeats <= 0 or args.wasm_repeats <= 0:
        parser.error("repeat counts must be positive")
    if args.wasm_warmup_rounds < 0:
        parser.error("WASM warm-up rounds must be non-negative")

    bundle = args.bundle.expanduser().resolve()
    if not (bundle / "_framework" / "dotnet.js").is_file():
        parser.error(f"not a published .NET WASM wwwroot: {bundle}")
    upstream_root = args.upstream_root.expanduser().resolve()
    if not (upstream_root / "SudokuSolverWasm" / "SudokuSolverWasm.csproj").is_file():
        parser.error(f"not the SudokuSolver checkout: {upstream_root}")

    cases = load_cases(args.corpus)
    run_rust(cases[0], 1, args.rust_library)
    rust_by_name: dict[str, dict[str, object]] = {}
    for index, case in enumerate(cases, 1):
        result = run_rust(case, args.rust_repeats, args.rust_library)
        rust_by_name[case.name] = result
        print(
            f"[Rust {index:02d}/{len(cases)}] {case.name}: "
            f"{result['median_ms']:.3f} ms",
            file=sys.stderr,
            flush=True,
        )

    wasm_report, process_wall_seconds = run_wasm(
        cases,
        node=args.node,
        bundle=bundle,
        warmup_rounds=args.wasm_warmup_rounds,
        repeats=args.wasm_repeats,
    )
    wasm_by_name = {row["name"]: row for row in wasm_report["cases"]}
    rows = []
    for case in cases:
        raw = wasm_by_name[case.name]
        if int(raw["observed"]) != case.expected:
            raise RuntimeError(
                f"WASM result mismatch for {case.name}: "
                f"{raw['observed']} != {case.expected}"
            )
        wasm = {
            "observed": int(raw["observed"]),
            "build_ms": summarize([float(v) for v in raw["buildMs"]]),
            "count_ms": summarize([float(v) for v in raw["countMs"]]),
            "total_ms": summarize([float(v) for v in raw["totalMs"]]),
        }
        rust = rust_by_name[case.name]
        rows.append(
            {
                "name": case.name,
                "group": case.group,
                "expected": case.expected,
                "rust": rust,
                "wasm": wasm,
                "median_latency_speedup": float(wasm["total_ms"]["median"])
                / float(rust["median_ms"]),
            }
        )
        print(
            f"[WASM] {case.name}: build+count "
            f"{wasm['total_ms']['median']:.3f} ms "
            f"(build {wasm['build_ms']['median']:.3f}, "
            f"count {wasm['count_ms']['median']:.3f})",
            file=sys.stderr,
            flush=True,
        )

    groups = {
        group: aggregate(rows, group)
        for group in ("blue", "score3", "spread", "all")
    }
    revision = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=upstream_root,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()
    report = {
        "upstream_repository": "https://github.com/dclamage/SudokuSolver",
        "upstream_branch": "wasm-prototype",
        "upstream_revision": revision,
        "timing_harness": "ThermoBenchInterop.cs added to temporary checkout; solver unchanged",
        "wasm_mode": "single-threaded .NET 10 Release AOT under persistent Node/V8",
        "runtime": wasm_report["runtime"],
        "node_version": wasm_report["nodeVersion"],
        "v8_version": wasm_report["v8Version"],
        "host": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "processor": platform.processor(),
            "logical_processors": os.cpu_count(),
        },
        "bundle_artifacts_sha256": wasm_report["bundleArtifactsSha256"],
        "timing_harness_sha256": hashlib.sha256(
            (ROOT / "benchmarks" / "wasm_harness" / "ThermoBenchInterop.cs")
            .read_bytes()
        ).hexdigest(),
        "runtime_startup_ms": wasm_report["runtimeStartupMs"],
        "batch_wall_ms": wasm_report["batchWallMs"],
        "process_wall_seconds": process_wall_seconds,
        "rust_mode": "single-threaded in-process FFI; fresh Solver::new + count_up_to(2)",
        "rust_repeats_per_case": args.rust_repeats,
        "wasm_warmup_rounds": args.wasm_warmup_rounds,
        "wasm_repeats_per_case": args.wasm_repeats,
        "case_count": len(cases),
        "groups": groups,
        "cases": rows,
    }
    output = json.dumps(report, indent=2)
    print(output)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
