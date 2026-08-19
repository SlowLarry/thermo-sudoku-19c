"""Quick, reproducible Rust-versus-Rangsk thermo classification benchmark."""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from thermo_search.thermo_anneal import (  # noqa: E402
    GeometryError,
    Layout,
    RustSolver,
    canonical_layout,
    idx2str,
    parse_layout_text,
)

BLUE_20: Layout = (
    (18, 27, 28, 19, 20, 11, 12, 13, 4),
    (57, 48, 49),
    (59, 68, 69, 60, 61, 52, 53, 44),
)


@dataclass(frozen=True)
class Case:
    name: str
    group: str
    layout: Layout
    expected: int  # check-mode result: 0, 1, or 2 for 2+


def load_cases(corpus: Path) -> list[Case]:
    distinct: list[tuple[int, int, Layout]] = []
    seen: set[Layout] = set()
    for line_number, line in enumerate(corpus.read_text(encoding="utf-8").splitlines(), 1):
        declared_text, separator, layout_text = line.partition(";")
        if not separator:
            continue
        try:
            declared = int(declared_text)
            layout = canonical_layout(parse_layout_text(layout_text))
        except (GeometryError, ValueError):
            continue
        if layout not in seen:
            seen.add(layout)
            distinct.append((line_number, declared, layout))

    score_three = [record for record in distinct if record[1] == 3]
    if len(score_three) != 14:
        raise RuntimeError(f"expected fourteen score-3 layouts, found {len(score_three)}")

    broad_pool = [record for record in distinct if record[1] != 3]
    broad_indices = sorted(
        {round(index * (len(broad_pool) - 1) / 9) for index in range(10)}
    )

    cases = [Case("blue-20-unique", "blue", BLUE_20, 1)]
    cases.extend(
        Case(f"score3-line-{line}", "score3", layout, 2)
        for line, _, layout in score_three
    )
    cases.extend(
        Case(f"spread-line-{line}-count-{declared}", "spread", layout, 2)
        for line, declared, layout in (broad_pool[index] for index in broad_indices)
    )
    return cases


def run_rust(case: Case, repeats: int, library: Path | None) -> dict[str, object]:
    solver_ms: list[float] = []
    wall_ms: list[float] = []
    for _ in range(repeats):
        solver = RustSolver(library)
        started = time.perf_counter()
        result = solver.count(case.layout, cap=2)
        wall_ms.append((time.perf_counter() - started) * 1_000)
        solver_ms.append(result.duration_seconds * 1_000)
        observed = 1 if result.count == 1 and result.exact else min(result.count, 2)
        if observed != case.expected:
            raise RuntimeError(
                f"Rust result mismatch for {case.name}: {observed} != {case.expected}"
            )
    return {
        "solver_ms": statistics.median(solver_ms),
        "wall_ms": statistics.median(wall_ms),
        "repeats": repeats,
    }


def run_rangsk(case: Case, executable: Path, repeats: int) -> dict[str, object]:
    command = [
        str(executable),
        "--json",
        "--blank=9",
        "--check",
        "--hide-banner",
        *(f"--constraint=thermo:{idx2str(path)}" for path in case.layout),
    ]
    solver_ms: list[float] = []
    wall_ms: list[float] = []
    for _ in range(repeats):
        started = time.perf_counter()
        completed = subprocess.run(
            command,
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=300,
        )
        wall_ms.append((time.perf_counter() - started) * 1_000)
        if completed.returncode != 0:
            raise RuntimeError(
                f"Rangsk failed for {case.name}: {completed.stderr.strip()}"
            )
        response = json.loads(completed.stdout)
        observed = int(response["count"])
        if observed != case.expected:
            raise RuntimeError(
                f"Rangsk result mismatch for {case.name}: {observed} != {case.expected}"
            )
        solver_ms.append(float(response["duration"]) * 1_000)
    return {
        "solver_ms": statistics.median(solver_ms),
        "wall_ms": statistics.median(wall_ms),
        "solver_ms_samples": solver_ms,
        "wall_ms_samples": wall_ms,
        "repeats": repeats,
    }


def aggregate(rows: list[dict[str, object]], group: str) -> dict[str, object]:
    selected = [row for row in rows if row["group"] == group]
    rust_solver = [float(row["rust"]["solver_ms"]) for row in selected]
    rust_wall = [float(row["rust"]["wall_ms"]) for row in selected]
    rangsk_solver = [float(row["rangsk"]["solver_ms"]) for row in selected]
    rangsk_wall = [float(row["rangsk"]["wall_ms"]) for row in selected]
    ratios = [right / left for left, right in zip(rust_solver, rangsk_solver)]
    rust_solver_total = sum(rust_solver)
    rangsk_solver_total = sum(rangsk_solver)
    rust_wall_total = sum(rust_wall)
    rangsk_wall_total = sum(rangsk_wall)
    return {
        "cases": len(selected),
        "rust_solver_median_ms": statistics.median(rust_solver),
        "rangsk_solver_median_ms": statistics.median(rangsk_solver),
        "rust_wall_median_ms": statistics.median(rust_wall),
        "rangsk_wall_median_ms": statistics.median(rangsk_wall),
        "median_reported_solver_speedup": statistics.median(ratios),
        "aggregate_reported_solver_speedup": rangsk_solver_total / rust_solver_total,
        "aggregate_wall_speedup": rangsk_wall_total / rust_wall_total,
        "rust_solver_total_ms": rust_solver_total,
        "rangsk_solver_total_ms": rangsk_solver_total,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rangsk", type=Path, required=True)
    parser.add_argument(
        "--corpus", type=Path, default=ROOT / "sources" / "min_thermos_9_8_2.txt"
    )
    parser.add_argument("--rust-library", type=Path)
    parser.add_argument("--rust-repeats", type=int, default=20)
    parser.add_argument("--rangsk-repeats", type=int, default=1)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.rust_repeats <= 0 or args.rangsk_repeats <= 0:
        parser.error("repeat counts must be positive")
    executable = args.rangsk.expanduser().resolve()
    if not executable.is_file():
        parser.error(f"Rangsk executable not found: {executable}")

    cases = load_cases(args.corpus)
    # Warm filesystem/runtime pages; every measured Rangsk sample still starts
    # a fresh process and performs its own JIT compilation.
    run_rust(cases[0], 1, args.rust_library)
    run_rangsk(cases[0], executable, 1)
    rows: list[dict[str, object]] = []
    benchmark_started = time.perf_counter()
    for index, case in enumerate(cases, 1):
        rust = run_rust(case, args.rust_repeats, args.rust_library)
        rangsk = run_rangsk(case, executable, args.rangsk_repeats)
        row = {
            "name": case.name,
            "group": case.group,
            "expected": case.expected,
            "rust": rust,
            "rangsk": rangsk,
            "reported_solver_speedup": float(rangsk["solver_ms"])
            / float(rust["solver_ms"]),
        }
        rows.append(row)
        print(
            f"[{index:02d}/{len(cases)}] {case.name}: "
            f"Rust {rust['solver_ms']:.3f} ms, "
            f"Rangsk {rangsk['solver_ms']:.3f} ms reported",
            file=sys.stderr,
            flush=True,
        )

    groups = {
        group: aggregate(rows, group) for group in ("blue", "score3", "spread")
    }
    groups["all"] = aggregate_all(rows)
    report = {
        "rangsk_executable": str(executable),
        "rangsk_version": "1.3.188",
        "rangsk_mode": "single-threaded --check",
        "rust_mode": "single-threaded in-process count_up_to(2)",
        "rust_repeats_per_case": args.rust_repeats,
        "rangsk_repeats_per_case": args.rangsk_repeats,
        "case_count": len(cases),
        "benchmark_wall_seconds": time.perf_counter() - benchmark_started,
        "groups": groups,
        "cases": rows,
    }
    output = json.dumps(report, indent=2)
    print(output)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output + "\n", encoding="utf-8")
    return 0


def aggregate_all(rows: list[dict[str, object]]) -> dict[str, object]:
    tagged = [{**row, "group": "all"} for row in rows]
    return aggregate(tagged, "all")


if __name__ == "__main__":
    raise SystemExit(main())
