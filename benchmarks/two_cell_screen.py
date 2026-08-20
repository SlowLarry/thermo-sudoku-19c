"""Benchmark collective-prefix strategies for every legal two-cell extension."""

from __future__ import annotations

import argparse
import hashlib
import json
import statistics
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from thermo_search.thermo_anneal import GeometryError, Layout, parse_layout_text  # noqa: E402


def score_three_bases(corpus: Path) -> list[tuple[int, Layout]]:
    records: list[tuple[int, Layout]] = []
    seen: set[Layout] = set()
    for line_number, line in enumerate(corpus.read_text(encoding="utf-8").splitlines(), 1):
        declared_text, separator, layout_text = line.partition(";")
        if not separator or int(declared_text) != 3:
            continue
        try:
            layout = parse_layout_text(layout_text)
        except (GeometryError, ValueError):
            continue
        base = tuple(path for path in layout if len(path) != 2)
        if sorted(map(len, base)) != [8, 9] or base in seen:
            continue
        seen.add(base)
        records.append((line_number, base))
    if len(records) != 14:
        raise RuntimeError(f"expected fourteen distinct score-3 bases, found {len(records)}")
    return records


def compact_paths(layout: Layout) -> str:
    return "|".join(",".join(map(str, path)) for path in layout)


def run_once(executable: Path, base: Layout, mode: str) -> dict[str, object]:
    command = [str(executable), "--thermos", compact_paths(base), "--screen-two-cell"]
    if mode == "collective":
        command.append("--collective-only")
    else:
        command.extend(("--collective-prefix", mode.removeprefix("hybrid-")))
    completed = subprocess.run(
        command,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=300,
    )
    fields: dict[str, object] = {}
    sub_two_extensions: list[tuple[int, int, str]] = []
    for line in completed.stdout.splitlines():
        key, separator, value = line.partition("=")
        if not separator:
            continue
        if key == "extension":
            bulb, tip, label = value.split(",")
            sub_two_extensions.append((int(bulb), int(tip), label))
            continue
        if value in ("true", "false"):
            fields[key] = value == "true"
        else:
            try:
                fields[key] = int(value)
            except ValueError:
                fields[key] = value
    if fields.get("candidate_edges") != 370:
        raise RuntimeError(f"unexpected candidate count: {fields}")
    if fields.get("unique_extensions") != 0:
        raise RuntimeError(f"unique extension reported: {fields}")
    fields["sub_two_extensions"] = tuple(sub_two_extensions)
    return fields


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest().upper()


def classification_digest(classification: tuple[tuple[int, int, str], ...]) -> str:
    encoded = "".join(
        f"{bulb},{tip},{label}\n" for bulb, tip, label in classification
    ).encode("ascii")
    return hashlib.sha256(encoded).hexdigest().upper()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--executable",
        type=Path,
        default=ROOT / "thermo-sudoku-rs" / "target" / "release" / "thermo-sudoku-cli.exe",
    )
    parser.add_argument(
        "--corpus", type=Path, default=ROOT / "sources" / "min_thermos_9_8_2.txt"
    )
    parser.add_argument("--prefixes", default="0,1,2,4,8,16,32,64,128,256")
    parser.add_argument("--repeats", type=int, default=7)
    parser.add_argument("--include-collective", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.repeats < 1:
        parser.error("--repeats must be positive")
    executable = args.executable.resolve()
    if not executable.is_file():
        parser.error(f"solver executable not found: {executable}")
    prefixes = [int(value) for value in args.prefixes.split(",")]
    if any(value < 0 for value in prefixes):
        parser.error("prefix limits must be non-negative")
    modes = [f"hybrid-{prefix}" for prefix in prefixes]
    if args.include_collective:
        modes.append("collective")

    bases = score_three_bases(args.corpus)
    rows: list[dict[str, object]] = []
    for line_number, base in bases:
        expected_classification: tuple[tuple[int, int, str], ...] | None = None
        for mode in modes:
            run_once(executable, base, mode)  # warm process and filesystem pages
            samples = [run_once(executable, base, mode) for _ in range(args.repeats)]
            for sample in samples:
                classification = sample["sub_two_extensions"]
                assert isinstance(classification, tuple)
                if expected_classification is None:
                    expected_classification = classification
                elif classification != expected_classification:
                    raise RuntimeError(
                        f"classification mismatch on corpus line {line_number}, mode {mode}"
                    )
            elapsed = [int(sample["elapsed_us"]) for sample in samples]
            representative = samples[0]
            classification = representative["sub_two_extensions"]
            assert isinstance(classification, tuple)
            row = {
                "line": line_number,
                "mode": mode,
                "median_us": statistics.median(elapsed),
                "min_us": min(elapsed),
                "max_us": max(elapsed),
                "collective_solutions_visited": representative[
                    "collective_solutions_visited"
                ],
                "fallback_searches": representative["fallback_searches"],
                "zero_extensions": representative["zero_extensions"],
                "multiple_extensions": representative["multiple_extensions"],
                "witness_solutions": representative["witness_solutions"],
                "classification_sha256": classification_digest(classification),
            }
            rows.append(row)
            print(
                f"line {line_number:>2} {mode:>12}: {row['median_us']:>8.1f} us, "
                f"fallback {row['fallback_searches']}",
                file=sys.stderr,
                flush=True,
            )

    aggregate = {}
    for mode in modes:
        selected = [row for row in rows if row["mode"] == mode]
        aggregate[mode] = {
            "bases": len(selected),
            "sum_of_case_medians_us": sum(float(row["median_us"]) for row in selected),
            "median_case_us": statistics.median(float(row["median_us"]) for row in selected),
            "max_case_us": max(float(row["median_us"]) for row in selected),
            "sum_fallback_searches": sum(int(row["fallback_searches"]) for row in selected),
        }
    report = {
        "schema": 2,
        "executable": str(executable),
        "executable_sha256": sha256_file(executable),
        "corpus": str(args.corpus.resolve()),
        "corpus_sha256": sha256_file(args.corpus),
        "base_count": len(bases),
        "repeats": args.repeats,
        "modes": modes,
        "aggregate": aggregate,
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
