"""Compare the Rust thermo solver with Interactive Sudoku Solver (ISS)."""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from benchmarks.quick_compare import Case, load_cases  # noqa: E402
from thermo_search.thermo_anneal import RustSolver  # noqa: E402


def run_rust(case: Case, repeats: int, library: Path | None) -> dict[str, object]:
    samples: list[float] = []
    for _ in range(repeats):
        # A fresh adapter prevents its result cache from hiding solver work.
        solver = RustSolver(library)
        result = solver.count(case.layout, cap=2)
        observed = 1 if result.count == 1 and result.exact else min(result.count, 2)
        if observed != case.expected:
            raise RuntimeError(
                f"Rust result mismatch for {case.name}: {observed} != {case.expected}"
            )
        samples.append(result.duration_seconds * 1_000)
    sorted_samples = sorted(samples)
    return {
        "median_ms": statistics.median(samples),
        "p10_ms": _quantile(sorted_samples, 0.1),
        "p90_ms": _quantile(sorted_samples, 0.9),
        "min_ms": sorted_samples[0],
        "max_ms": sorted_samples[-1],
        "samples_ms": samples,
        "repeats": repeats,
    }


def _quantile(sorted_values: list[float], fraction: float) -> float:
    if len(sorted_values) == 1:
        return sorted_values[0]
    position = (len(sorted_values) - 1) * fraction
    lower = int(position)
    upper = min(lower + 1, len(sorted_values) - 1)
    remainder = position - lower
    return (
        sorted_values[lower] * (1 - remainder)
        + sorted_values[upper] * remainder
    )


def run_iss(
    cases: list[Case],
    *,
    node: str,
    iss_root: Path,
    warmup_rounds: int,
    repeats: int,
) -> tuple[dict[str, object], float]:
    request = {
        "warmup_rounds": warmup_rounds,
        "repeats": repeats,
        "cases": [
            {
                "name": case.name,
                "layout": [list(path) for path in case.layout],
                "expected": case.expected,
            }
            for case in cases
        ],
    }
    command = [
        node,
        str(ROOT / "benchmarks" / "iss_adapter.mjs"),
        str(iss_root),
    ]
    started = time.perf_counter()
    completed = subprocess.run(
        command,
        input=json.dumps(request),
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=600,
    )
    wall_seconds = time.perf_counter() - started
    if completed.returncode != 0:
        raise RuntimeError(
            "ISS benchmark adapter failed:\n"
            f"stdout: {completed.stdout}\n"
            f"stderr: {completed.stderr}"
        )
    return json.loads(completed.stdout), wall_seconds


def aggregate(rows: list[dict[str, object]], group: str) -> dict[str, object]:
    selected = [row for row in rows if group == "all" or row["group"] == group]
    rust = [float(row["rust"]["median_ms"]) for row in selected]
    iss_total = [float(row["iss"]["total_ms"]["median"]) for row in selected]
    iss_build = [float(row["iss"]["build_ms"]["median"]) for row in selected]
    iss_count = [float(row["iss"]["count_ms"]["median"]) for row in selected]
    rust_measured_total = sum(
        sum(float(value) for value in row["rust"]["samples_ms"])
        for row in selected
    )
    iss_measured_total = sum(
        sum(float(value) for value in row["iss"]["total_ms"]["samples"])
        for row in selected
    )
    return {
        "cases": len(selected),
        "rust_ffi_median_ms": statistics.median(rust),
        "iss_total_median_ms": statistics.median(iss_total),
        "iss_build_median_ms": statistics.median(iss_build),
        "iss_count_median_ms": statistics.median(iss_count),
        "aggregate_median_latency_speedup": sum(iss_total) / sum(rust),
        "measured_work_speedup": iss_measured_total / rust_measured_total,
        "rust_ffi_total_ms": sum(rust),
        "iss_total_ms": sum(iss_total),
        "rust_measured_work_ms": rust_measured_total,
        "iss_measured_work_ms": iss_measured_total,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--iss-root", type=Path, required=True)
    parser.add_argument("--node", default="node")
    parser.add_argument(
        "--corpus", type=Path, default=ROOT / "sources" / "min_thermos_9_8_2.txt"
    )
    parser.add_argument("--rust-library", type=Path)
    parser.add_argument("--rust-repeats", type=int, default=100)
    parser.add_argument("--iss-warmup-rounds", type=int, default=20)
    parser.add_argument("--iss-repeats", type=int, default=100)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.rust_repeats <= 0 or args.iss_repeats <= 0:
        parser.error("repeat counts must be positive")
    if args.iss_warmup_rounds < 0:
        parser.error("ISS warm-up rounds must be non-negative")
    iss_root = args.iss_root.expanduser().resolve()
    required = iss_root / "js" / "solver" / "sudoku_builder.js"
    if not required.is_file():
        parser.error(f"not an Interactive Sudoku Solver checkout: {iss_root}")

    cases = load_cases(args.corpus)
    # Load the Rust library and warm code/data pages before measurements.
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

    iss_report, iss_wall_seconds = run_iss(
        cases,
        node=args.node,
        iss_root=iss_root,
        warmup_rounds=args.iss_warmup_rounds,
        repeats=args.iss_repeats,
    )
    iss_by_name = {row["name"]: row for row in iss_report["cases"]}
    rows = []
    for case in cases:
        iss = iss_by_name[case.name]
        rows.append(
            {
                "name": case.name,
                "group": case.group,
                "expected": case.expected,
                "rust": rust_by_name[case.name],
                "iss": iss,
                "total_speedup": float(iss["total_ms"]["median"])
                / float(rust_by_name[case.name]["median_ms"]),
            }
        )
        print(
            f"[ISS] {case.name}: build+count "
            f"{iss['total_ms']['median']:.3f} ms "
            f"(build {iss['build_ms']['median']:.3f}, "
            f"count {iss['count_ms']['median']:.3f})",
            file=sys.stderr,
            flush=True,
        )

    groups = {
        group: aggregate(rows, group)
        for group in ("blue", "score3", "spread", "all")
    }
    git_revision = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=iss_root,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()
    report = {
        "iss_repository": "https://github.com/sigh/Interactive-Sudoku-Solver",
        "iss_revision": git_revision,
        "iss_mode": "persistent Node process; fresh build + countSolutions(2)",
        "node_version": iss_report["node_version"],
        "v8_version": iss_report["v8_version"],
        "rust_mode": "single-threaded in-process FFI; fresh Solver::new + count_up_to(2)",
        "rust_repeats_per_case": args.rust_repeats,
        "iss_warmup_rounds": args.iss_warmup_rounds,
        "iss_repeats_per_case": args.iss_repeats,
        "case_count": len(cases),
        "iss_process_wall_seconds": iss_wall_seconds,
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
