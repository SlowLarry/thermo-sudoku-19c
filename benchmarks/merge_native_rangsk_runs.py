"""Merge independent native-Rangsk benchmark runs without losing raw samples."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
from pathlib import Path

from quick_compare_native_rangsk import aggregate, summarize


def file_sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require_equal(reports: list[dict[str, object]], key: str) -> None:
    present = [key in report for report in reports]
    if not any(present):
        return
    if not all(present):
        raise RuntimeError(f"only some input reports contain {key}")
    first = reports[0][key]
    if any(report[key] != first for report in reports[1:]):
        raise RuntimeError(f"input reports disagree on {key}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("inputs", nargs="+", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if len(args.inputs) < 2:
        parser.error("provide at least two independent run files")

    paths = [path.expanduser().resolve() for path in args.inputs]
    reports = [json.loads(path.read_text(encoding="utf-8")) for path in paths]
    stable_keys = (
        "upstream_repository",
        "upstream_branch",
        "upstream_revision",
        "upstream_solver_tree",
        "upstream_tracked_changes",
        "upstream_solver_changes",
        "timing_scope",
        "case_selection",
        "case_count",
        "case_manifest_sha256",
        "corpus_sha256",
        "host",
        "dotnet_sdk_version",
        "native_runtime",
        "native_harness_sha256",
        "measurement_order",
        "rust_revision",
        "rust_library_sha256",
        "rust_mode",
        "rust_warmup_rounds",
        "rust_repeats_per_case",
        "native_mode",
        "native_warmup_rounds",
        "native_repeats_per_case",
    )
    for key in stable_keys:
        require_equal(reports, key)

    case_maps = [
        {row["name"]: row for row in report["cases"]} for report in reports
    ]
    first_rows = reports[0]["cases"]
    expected_names = [row["name"] for row in first_rows]
    if any(list(case_map) != expected_names for case_map in case_maps):
        raise RuntimeError("input reports disagree on case names or order")

    rows: list[dict[str, object]] = []
    for first in first_rows:
        name = first["name"]
        matching = [case_map[name] for case_map in case_maps]
        for key in ("group", "expected", "thermos"):
            if any(row[key] != first[key] for row in matching[1:]):
                raise RuntimeError(f"input reports disagree on {name}.{key}")

        rust_samples = [
            float(value)
            for row in matching
            for value in row["rust"]["total_ms"]["samples"]
        ]
        native_build = [
            float(value)
            for row in matching
            for value in row["native"]["build_ms"]["samples"]
        ]
        native_count = [
            float(value)
            for row in matching
            for value in row["native"]["count_ms"]["samples"]
        ]
        native_total = [
            float(value)
            for row in matching
            for value in row["native"]["total_ms"]["samples"]
        ]
        rust = {
            "observed": first["expected"],
            "total_ms": summarize(rust_samples),
        }
        native = {
            "observed": first["expected"],
            "build_ms": summarize(native_build),
            "count_ms": summarize(native_count),
            "total_ms": summarize(native_total),
        }
        rows.append(
            {
                "name": name,
                "group": first["group"],
                "expected": first["expected"],
                "thermos": first["thermos"],
                "rust": rust,
                "native": native,
                "build_plus_count_speedup": (
                    float(native["total_ms"]["median"])
                    / float(rust["total_ms"]["median"])
                ),
            }
        )

    merged = copy.deepcopy(reports[0])
    rust_repeats = int(merged.pop("rust_repeats_per_case"))
    native_repeats = int(merged.pop("native_repeats_per_case"))
    rust_warmups = int(merged.pop("rust_warmup_rounds"))
    native_warmups = int(merged.pop("native_warmup_rounds"))
    merged.pop("rust_batch_wall_seconds")
    merged.pop("native_process_wall_seconds")
    merged.update(
        {
            "independent_runs": len(reports),
            "merge_method": (
                "concatenate per-case phase samples from independent runs, then "
                "recompute medians and aggregate ratios from the merged samples"
            ),
            "source_runs": [
                {
                    "file": path.name,
                    "sha256": file_sha256(path),
                    "rust_batch_wall_seconds": report["rust_batch_wall_seconds"],
                    "native_process_wall_seconds": report[
                        "native_process_wall_seconds"
                    ],
                }
                for path, report in zip(paths, reports)
            ],
            "rust_warmup_rounds_per_run": rust_warmups,
            "rust_repeats_per_case_per_run": rust_repeats,
            "rust_repeats_per_case": rust_repeats * len(reports),
            "native_warmup_rounds_per_run": native_warmups,
            "native_repeats_per_case_per_run": native_repeats,
            "native_repeats_per_case": native_repeats * len(reports),
            "groups": {
                group: aggregate(rows, group)
                for group in ("blue", "score3", "spread", "all")
            },
            "cases": rows,
        }
    )
    output = json.dumps(merged, indent=2) + "\n"
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(output, encoding="utf-8")
    print(output, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
