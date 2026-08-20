"""Compare the Rust thermo solver with Rangsk's native .NET solver library."""

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
from thermo_search.thermo_anneal import RustSolver, idx2str  # noqa: E402

HARNESS_PROJECT = ROOT / "benchmarks" / "native_harness" / "NativeThermoBench.csproj"


def quantile(ordered: list[float], fraction: float) -> float:
    if len(ordered) == 1:
        return ordered[0]
    position = (len(ordered) - 1) * fraction
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    remainder = position - lower
    return ordered[lower] * (1 - remainder) + ordered[upper] * remainder


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


def observed_count(count: int, exact: bool) -> int:
    return 1 if count == 1 and exact else min(count, 2)


def run_rust_batch(
    cases: list[Case],
    *,
    warmup_rounds: int,
    repeats: int,
    library: Path | None,
) -> dict[str, dict[str, object]]:
    for round_index in range(warmup_rounds):
        for offset in range(len(cases)):
            case = cases[(offset + round_index) % len(cases)]
            result = RustSolver(library).count(case.layout, cap=2)
            if observed_count(result.count, result.exact) != case.expected:
                raise RuntimeError(f"Rust result mismatch during warm-up for {case.name}")

    samples = {case.name: [] for case in cases}
    for round_index in range(repeats):
        for offset in range(len(cases)):
            case = cases[(offset + round_index) % len(cases)]
            # A fresh adapter bypasses the Python result cache. Its construction and
            # ctypes-array preparation remain outside the solver's own timed FFI call.
            result = RustSolver(library).count(case.layout, cap=2)
            observed = observed_count(result.count, result.exact)
            if observed != case.expected:
                raise RuntimeError(
                    f"Rust result mismatch for {case.name}: {observed} != {case.expected}"
                )
            samples[case.name].append(result.duration_seconds * 1_000)

    return {
        case.name: {
            "observed": case.expected,
            "total_ms": summarize(samples[case.name]),
        }
        for case in cases
    }


def build_native_harness(
    dotnet: str, upstream_project: Path, output_directory: Path
) -> Path:
    command = [
        dotnet,
        "build",
        str(HARNESS_PROJECT),
        "--configuration",
        "Release",
        "--nologo",
        "--target",
        "Rebuild",
        "--output",
        str(output_directory),
        f"-p:SudokuSolverProject={upstream_project}",
    ]
    completed = subprocess.run(
        command,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=900,
        env={
            **os.environ,
            "DOTNET_CLI_TELEMETRY_OPTOUT": "1",
            "DOTNET_NOLOGO": "1",
        },
    )
    if completed.returncode != 0:
        raise RuntimeError(
            "Native harness build failed:\n"
            f"stdout: {completed.stdout}\n"
            f"stderr: {completed.stderr}"
        )
    native_dll = output_directory / "NativeThermoBench.dll"
    if not native_dll.is_file():
        raise RuntimeError(f"native harness build did not produce {native_dll}")
    return native_dll


def make_request(
    cases: list[Case], *, warmup_rounds: int, repeats: int
) -> dict[str, object]:
    return {
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


def run_native(
    request: dict[str, object], *, dotnet: str, native_dll: Path
) -> tuple[dict[str, object], float]:
    started = time.perf_counter()
    completed = subprocess.run(
        [dotnet, str(native_dll)],
        input=json.dumps(request, separators=(",", ":")),
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=900,
    )
    wall_seconds = time.perf_counter() - started
    if completed.returncode != 0:
        raise RuntimeError(
            "Native benchmark failed:\n"
            f"stdout: {completed.stdout}\n"
            f"stderr: {completed.stderr}"
        )
    return json.loads(completed.stdout), wall_seconds


def aggregate(rows: list[dict[str, object]], group: str) -> dict[str, object]:
    selected = [row for row in rows if group == "all" or row["group"] == group]
    rust = [float(row["rust"]["total_ms"]["median"]) for row in selected]
    native_total = [
        float(row["native"]["total_ms"]["median"]) for row in selected
    ]
    native_build = [
        float(row["native"]["build_ms"]["median"]) for row in selected
    ]
    native_count = [
        float(row["native"]["count_ms"]["median"]) for row in selected
    ]
    rust_measured = sum(
        sum(float(value) for value in row["rust"]["total_ms"]["samples"])
        for row in selected
    )
    native_measured = sum(
        sum(float(value) for value in row["native"]["total_ms"]["samples"])
        for row in selected
    )
    native_build_measured = sum(
        sum(float(value) for value in row["native"]["build_ms"]["samples"])
        for row in selected
    )
    native_count_measured = sum(
        sum(float(value) for value in row["native"]["count_ms"]["samples"])
        for row in selected
    )
    return {
        "cases": len(selected),
        "rust_build_plus_count_median_ms": statistics.median(rust),
        "native_build_plus_count_median_ms": statistics.median(native_total),
        "native_build_median_ms": statistics.median(native_build),
        "native_count_only_median_ms": statistics.median(native_count),
        "aggregate_median_latency_speedup": sum(native_total) / sum(rust),
        "measured_work_speedup": native_measured / rust_measured,
        "rust_median_total_ms": sum(rust),
        "native_median_total_ms": sum(native_total),
        "rust_measured_work_ms": rust_measured,
        "native_measured_work_ms": native_measured,
        "native_build_measured_work_ms": native_build_measured,
        "native_count_measured_work_ms": native_count_measured,
        "native_build_share": native_build_measured / native_measured,
        "native_count_share": native_count_measured / native_measured,
    }


def git_text(root: Path, *arguments: str) -> str:
    return subprocess.run(
        ["git", *arguments],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def resolve_rust_library(argument: Path | None) -> Path:
    if argument is not None:
        return argument.expanduser().resolve()
    release = ROOT / "thermo-sudoku-rs" / "target" / "release"
    candidates = [
        release / "thermo_sudoku.dll",
        release / "libthermo_sudoku.so",
        release / "libthermo_sudoku.dylib",
    ]
    return next((path for path in candidates if path.is_file()), candidates[0])


def command_text(*arguments: str) -> str:
    return subprocess.run(
        list(arguments),
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream-root", type=Path, required=True)
    parser.add_argument("--dotnet", default="dotnet")
    parser.add_argument("--allow-dirty-upstream", action="store_true")
    parser.add_argument(
        "--corpus", type=Path, default=ROOT / "sources" / "min_thermos_9_8_2.txt"
    )
    parser.add_argument("--rust-library", type=Path)
    parser.add_argument("--rust-warmup-rounds", type=int, default=20)
    parser.add_argument("--rust-repeats", type=int, default=100)
    parser.add_argument("--native-warmup-rounds", type=int, default=20)
    parser.add_argument("--native-repeats", type=int, default=100)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    if args.rust_warmup_rounds < 0 or args.native_warmup_rounds < 0:
        parser.error("warm-up rounds must be non-negative")
    if args.rust_repeats <= 0 or args.native_repeats <= 0:
        parser.error("repeat counts must be positive")
    upstream_root = args.upstream_root.expanduser().resolve()
    upstream_project = upstream_root / "SudokuSolver" / "SudokuSolver.csproj"
    if not upstream_project.is_file():
        parser.error(f"not a SudokuSolver checkout: {upstream_root}")
    upstream_revision = git_text(upstream_root, "rev-parse", "HEAD")
    upstream_solver_tree = git_text(
        upstream_root, "rev-parse", "HEAD:SudokuSolver"
    )
    upstream_status = git_text(
        upstream_root, "status", "--porcelain", "--untracked-files=no"
    )
    upstream_solver_status = git_text(
        upstream_root,
        "status",
        "--porcelain",
        "--untracked-files=all",
        "--",
        "SudokuSolver",
    )
    if (upstream_status or upstream_solver_status) and not args.allow_dirty_upstream:
        parser.error(
            "upstream checkout has tracked changes or untracked solver inputs; "
            "use a clean checkout or --allow-dirty-upstream"
        )

    output_directory = (
        ROOT
        / "benchmarks"
        / "native_harness"
        / "bin"
        / f"{upstream_revision[:12]}-{os.getpid()}-{time.time_ns()}"
    )
    native_dll = build_native_harness(
        args.dotnet, upstream_project, output_directory
    )

    corpus = args.corpus.expanduser().resolve()
    cases = load_cases(corpus)
    request = make_request(
        cases,
        warmup_rounds=args.native_warmup_rounds,
        repeats=args.native_repeats,
    )
    manifest = json.dumps(request["cases"], sort_keys=True, separators=(",", ":"))

    rust_started = time.perf_counter()
    rust_by_name = run_rust_batch(
        cases,
        warmup_rounds=args.rust_warmup_rounds,
        repeats=args.rust_repeats,
        library=args.rust_library,
    )
    rust_wall_seconds = time.perf_counter() - rust_started
    native_report, native_wall_seconds = run_native(
        request,
        dotnet=args.dotnet,
        native_dll=native_dll,
    )

    native_by_name = {row["name"]: row for row in native_report["cases"]}
    rows: list[dict[str, object]] = []
    for case in cases:
        raw = native_by_name.get(case.name)
        if raw is None:
            raise RuntimeError(f"native report omitted {case.name}")
        if int(raw["observed"]) != case.expected:
            raise RuntimeError(
                f"native result mismatch for {case.name}: "
                f"{raw['observed']} != {case.expected}"
            )
        native = {
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
                "thermos": [list(path) for path in case.layout],
                "rust": rust,
                "native": native,
                "build_plus_count_speedup": (
                    float(native["total_ms"]["median"])
                    / float(rust["total_ms"]["median"])
                ),
            }
        )

    if set(native_by_name) != {case.name for case in cases}:
        raise RuntimeError("native report contained unexpected cases")

    groups = {
        group: aggregate(rows, group)
        for group in ("blue", "score3", "spread", "all")
    }
    rust_library = resolve_rust_library(args.rust_library)
    native_solver_dll = native_dll.with_name("SudokuSolver.dll")
    if not native_solver_dll.is_file():
        raise RuntimeError(
            f"native harness directory does not contain {native_solver_dll.name}"
        )
    report = {
        "upstream_repository": "https://github.com/dclamage/SudokuSolver",
        "upstream_branch": git_text(upstream_root, "branch", "--show-current"),
        "upstream_revision": upstream_revision,
        "upstream_solver_tree": upstream_solver_tree,
        "upstream_tracked_changes": upstream_status,
        "upstream_solver_changes": upstream_solver_status,
        "timing_scope": {
            "primary": "fresh solver construction plus capped counting",
            "native_build": "SolverFactory.CreateBlank(9, thermo strings)",
            "native_count": "CountSolutions(maxSolutions: 2, multiThread: false)",
            "rust": "FFI call including Layout construction, Solver construction, and count_up_to(2)",
            "excluded": "process startup, JSON, Python validation, thermo string/ctypes buffer construction",
            "count_only_status": "native diagnostic decomposition; not compared to a Rust count-only API",
        },
        "measurement_order": "Rust batch, then native .NET batch",
        "case_selection": "Blue unique, all fourteen score-3 layouts, ten deterministic spread layouts",
        "case_count": len(cases),
        "case_manifest_sha256": hashlib.sha256(manifest.encode("utf-8")).hexdigest(),
        "corpus": str(corpus),
        "corpus_sha256": sha256(corpus),
        "host": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "processor": platform.processor(),
            "logical_processors": os.cpu_count(),
        },
        "dotnet_sdk_version": command_text(args.dotnet, "--version"),
        "native_runtime": native_report["runtime"],
        "native_harness_sha256": {
            "project": sha256(HARNESS_PROJECT),
            "program": sha256(HARNESS_PROJECT.with_name("Program.cs")),
            "driver": sha256(Path(__file__)),
            "assembly": sha256(native_dll),
            "solver_assembly": sha256(native_solver_dll),
        },
        "rust_revision": git_text(ROOT, "rev-parse", "HEAD"),
        "rust_library": str(rust_library),
        "rust_library_sha256": sha256(rust_library),
        "rust_mode": "single-threaded in-process uncached FFI",
        "rust_warmup_rounds": args.rust_warmup_rounds,
        "rust_repeats_per_case": args.rust_repeats,
        "rust_batch_wall_seconds": rust_wall_seconds,
        "native_mode": (
            "persistent native .NET Release CLR/JIT process (the upstream-documented "
            "native mode, not NativeAOT); single-threaded solver; runtime-reported GC mode"
        ),
        "native_warmup_rounds": args.native_warmup_rounds,
        "native_repeats_per_case": args.native_repeats,
        "native_process_wall_seconds": native_wall_seconds,
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
