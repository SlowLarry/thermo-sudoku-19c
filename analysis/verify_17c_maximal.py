#!/usr/bin/env python3
"""Independent structural/witness verifier for thermo-17c-maximal JSONL shards."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any


CORPUS_RECORDS = 49_158
SCHEMA = "thermo-17c-maximal-v2"
PREREQUISITE = "all lower path counts below min_paths excluded"
P3 = (
    (0, 1, 2),
    (0, 2, 1),
    (1, 0, 2),
    (1, 2, 0),
    (2, 0, 1),
    (2, 1, 0),
)
PARTITION_FIELDS = (
    "maximal_covers",
    "within_source_duplicates",
    "classified_layout_occurrences",
    "cut_screened_layouts",
    "solver_calls",
    "multiple_layouts",
    "unique_layouts",
)
TERMINAL_COUNTER_FIELDS = (
    "eligible_records",
    "mandatory_nodes",
    "mandatory_degree_prunes",
    "mandatory_spatial_prunes",
    "isolate_prunes",
    "reversal_prunes",
    "skeletons",
    "merge_nodes",
    "merge_spatial_prunes",
    "merge_structure_prunes",
    "noncanonical_prunes",
    "maximal_covers",
    "classified_layout_occurrences",
    "cut_screened_layouts",
    "solver_calls",
    "cut_cache_probes",
    "witness_cuts_retained",
    "witness_cuts_admitted",
    "multiple_layouts",
    "unique_layouts",
)


def axis_morphs() -> list[tuple[int, ...]]:
    result: list[tuple[int, ...]] = []
    for band_map in P3:
        for first in P3:
            for second in P3:
                for third in P3:
                    within = (first, second, third)
                    permutation = [0] * 9
                    for old_band in range(3):
                        for old_offset in range(3):
                            permutation[3 * old_band + old_offset] = (
                                3 * band_map[old_band] + within[old_band][old_offset]
                            )
                    result.append(tuple(permutation))
    assert len(result) == 1296
    return result


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1 << 20):
            digest.update(block)
    return digest.hexdigest()


def nonnegative_integer(record: dict[str, Any], key: str, context: str) -> int:
    value = record.get(key)
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ValueError(f"{context}: {key} must be a nonnegative integer")
    return value


def expected_partitions(min_paths: int, max_paths: int) -> list[str]:
    result: list[str] = []

    def enumerate_partitions(
        remaining: int, count: int, maximum: int, prefix: list[int]
    ) -> None:
        if count == 0:
            if remaining == 0:
                result.append("+".join(str(length) for length in prefix))
            return
        upper = min(maximum, remaining - 2 * (count - 1))
        for length in range(upper, 1, -1):
            prefix.append(length)
            enumerate_partitions(remaining - length, count - 1, length, prefix)
            prefix.pop()

    for count in range(max_paths, min_paths - 1, -1):
        enumerate_partitions(17, count, 9, [])
    return result


def validate_corpus_record(encoded: str, line_number: int) -> None:
    if len(encoded) != 81:
        raise ValueError(
            f"corpus line {line_number}: expected 81 ASCII cells, got {len(encoded)}"
        )
    if any(character not in ".0123456789" for character in encoded):
        raise ValueError(f"corpus line {line_number}: invalid cell character")
    if sum(character in "123456789" for character in encoded) != 17:
        raise ValueError(f"corpus line {line_number}: expected exactly 17 clues")


def eligible_for_max_paths(encoded: str, max_paths: int) -> bool:
    counts = [encoded.count(str(digit)) for digit in range(1, 10)]
    return all(1 <= count <= max_paths for count in counts)


def parse_paths(encoded: str) -> list[list[int]]:
    return [[int(cell) for cell in path.split(",")] for path in encoded.split("|")]


def parse_grid(encoded: str) -> list[int]:
    if len(encoded) != 81 or any(digit not in "123456789" for digit in encoded):
        raise ValueError("solution must contain exactly 81 digits 1..9")
    return [ord(digit) - ord("0") for digit in encoded]


def validate_grid(grid: list[int]) -> None:
    expected = set(range(1, 10))
    for row in range(9):
        if set(grid[9 * row : 9 * row + 9]) != expected:
            raise ValueError(f"invalid row {row + 1}")
    for column in range(9):
        if {grid[9 * row + column] for row in range(9)} != expected:
            raise ValueError(f"invalid column {column + 1}")
    for box_row in range(3):
        for box_column in range(3):
            cells = {
                grid[9 * (3 * box_row + dr) + 3 * box_column + dc]
                for dr in range(3)
                for dc in range(3)
            }
            if cells != expected:
                raise ValueError(f"invalid box {3 * box_row + box_column + 1}")


def validate_paths(paths: list[list[int]], partition: str) -> None:
    lengths = sorted((len(path) for path in paths), reverse=True)
    declared = [int(length) for length in partition.split("+")]
    if lengths != declared:
        raise ValueError(f"path lengths {lengths} do not match {declared}")
    cells = [cell for path in paths for cell in path]
    if (
        len(cells) != 17
        or len(set(cells)) != 17
        or any(not 0 <= cell < 81 for cell in cells)
    ):
        raise ValueError("paths must cover 17 distinct in-range cells")
    for path in paths:
        if not 2 <= len(path) <= 9:
            raise ValueError("thermometer length outside 2..9")
        for left, right in zip(path, path[1:]):
            if max(abs(left // 9 - right // 9), abs(left % 9 - right % 9)) != 1:
                raise ValueError(f"non-king step {left}->{right}")


def validate_candidate(
    record: dict[str, Any],
    corpus: list[str],
    morphs: list[tuple[int, ...]],
    start_line: int,
    end_line: int,
) -> str:
    line_number = nonnegative_integer(record, "source_line", "candidate")
    if not start_line <= line_number <= end_line:
        raise ValueError(f"source line {line_number} outside shard range")
    source = record.get("source_puzzle")
    if not isinstance(source, str) or source != corpus[line_number - 1]:
        raise ValueError(f"source puzzle mismatch at line {line_number}")
    if record.get("classification_source") != "solver":
        raise ValueError("candidate was not classified by the solver")
    if record.get("witness_cut_id") is not None:
        raise ValueError("cache-disabled run unexpectedly references a witness cut")

    multiplicity = record.get("multiplicity")
    if multiplicity not in {"multiple", "unique"}:
        raise ValueError("candidate multiplicity must be multiple or unique")
    count = nonnegative_integer(record, "count", "candidate")
    capped = record.get("capped")
    if not isinstance(capped, bool):
        raise ValueError("candidate capped must be boolean")
    if multiplicity == "multiple":
        if count != 2 or capped is not True:
            raise ValueError("candidate is not a cap-two multiple")
    elif count != 1 or capped is not False:
        raise ValueError("candidate is not an exact-count-one unique")

    partition = record.get("partition")
    if not isinstance(partition, str):
        raise ValueError("candidate partition must be a string")
    paths_encoded = record.get("paths")
    if not isinstance(paths_encoded, str):
        raise ValueError("candidate paths must be a string")
    paths = parse_paths(paths_encoded)
    validate_paths(paths, partition)

    first_encoded = record.get("first_solution")
    if not isinstance(first_encoded, str):
        raise ValueError("candidate is missing its first solution")
    first = parse_grid(first_encoded)
    validate_grid(first)
    grids = [first]
    if multiplicity == "multiple":
        second_encoded = record.get("second_solution")
        if not isinstance(second_encoded, str):
            raise ValueError("multiple candidate is missing its second solution")
        second = parse_grid(second_encoded)
        validate_grid(second)
        if first == second:
            raise ValueError("multiplicity witnesses are identical")
        grids.append(second)
    elif record.get("second_solution") is not None:
        raise ValueError("unique candidate unexpectedly has a second solution")
    for grid in grids:
        if any(
            not all(grid[left] < grid[right] for left, right in zip(path, path[1:]))
            for path in paths
        ):
            raise ValueError("solution violates a thermometer")

    row_morph = nonnegative_integer(record, "row_morph", "candidate")
    column_morph = nonnegative_integer(record, "column_morph", "candidate")
    if not 0 <= row_morph < 1296 or not 0 <= column_morph < 1296:
        raise ValueError("morph index outside 0..1295")
    digit_order = record.get("digit_order")
    if not isinstance(digit_order, str):
        raise ValueError("candidate digit_order must be a string")
    order = [int(digit) for digit in digit_order.split(",")]
    if sorted(order) != list(range(1, 10)):
        raise ValueError("digit order is not a permutation of 1..9")
    rank = {source_digit: position + 1 for position, source_digit in enumerate(order)}
    rows = morphs[row_morph]
    columns = morphs[column_morph]
    target_values: dict[int, int] = {}
    for cell, character in enumerate(source):
        if character in "123456789":
            transformed = 9 * rows[cell // 9] + columns[cell % 9]
            target_values[transformed] = rank[int(character)]
    if set(target_values) != {cell for path in paths for cell in path}:
        raise ValueError("morphed clue footprint differs from thermo footprint")
    for path in paths:
        if not all(
            target_values[left] < target_values[right]
            for left, right in zip(path, path[1:])
        ):
            raise ValueError("morphed target digits do not increase on a path")
    if multiplicity == "unique" and any(
        first[cell] != value for cell, value in target_values.items()
    ):
        raise ValueError("unique solution does not realize the morphed classic clues")
    return multiplicity


def validate_partition_summary(
    record: dict[str, Any], expected: set[str], context: str, deduplicate: bool
) -> str:
    partition = record.get("partition")
    if not isinstance(partition, str) or partition not in expected:
        raise ValueError(f"{context}: unexpected partition {partition!r}")
    counts = {
        key: nonnegative_integer(record, key, context) for key in PARTITION_FIELDS
    }
    if counts["maximal_covers"] != (
        counts["within_source_duplicates"]
        + counts["classified_layout_occurrences"]
    ):
        raise ValueError(f"{context}: maximal/deduplication accounting mismatch")
    if not deduplicate and counts["within_source_duplicates"] != 0:
        raise ValueError(f"{context}: duplicates reported with deduplication disabled")
    if counts["classified_layout_occurrences"] != (
        counts["cut_screened_layouts"] + counts["solver_calls"]
    ):
        raise ValueError(f"{context}: classification-source accounting mismatch")
    if counts["classified_layout_occurrences"] != (
        counts["multiple_layouts"] + counts["unique_layouts"]
    ):
        raise ValueError(f"{context}: multiplicity accounting mismatch")
    if counts["cut_screened_layouts"] != 0:
        raise ValueError(f"{context}: cache-disabled run reports screened layouts")
    return partition


def verify_artifacts(
    corpus_path: Path,
    artifacts: list[Path],
    expected_min_paths: int,
    expected_max_paths: int,
    summary_only: bool,
) -> dict[str, Any]:
    if not 1 <= expected_min_paths <= expected_max_paths <= 8:
        raise ValueError("expected path range must satisfy 1 <= min <= max <= 8")
    if not artifacts:
        raise ValueError("at least one artifact is required")

    corpus_bytes = corpus_path.read_bytes()
    try:
        corpus = corpus_bytes.decode("ascii").splitlines()
    except UnicodeDecodeError as error:
        raise ValueError("corpus is not ASCII") from error
    if len(corpus) != CORPUS_RECORDS:
        raise ValueError(f"expected {CORPUS_RECORDS:,} corpus lines, got {len(corpus)}")
    for line_number, encoded in enumerate(corpus, 1):
        validate_corpus_record(encoded, line_number)

    expected_partition_list = expected_partitions(
        expected_min_paths, expected_max_paths
    )
    expected_partition_set = set(expected_partition_list)
    morphs = axis_morphs()
    ranges: list[tuple[int, int]] = []
    terminals: list[dict[str, Any]] = []
    artifact_info: list[dict[str, Any]] = []
    total_candidates = {"multiple": 0, "unique": 0}
    all_ranges_exhausted = True
    common_deduplicate: bool | None = None
    resolved_artifacts: set[Path] = set()

    for artifact in artifacts:
        resolved = artifact.resolve()
        if resolved in resolved_artifacts:
            raise ValueError(f"duplicate artifact path: {artifact}")
        resolved_artifacts.add(resolved)
        stat_before = artifact.stat()
        artifact_candidates = {"multiple": 0, "unique": 0}
        partition_summaries: dict[str, dict[str, Any]] = {}
        terminal: dict[str, Any] | None = None
        header: dict[str, Any] | None = None
        saw_partition_summary = False

        with artifact.open("r", encoding="utf-8", newline="") as stream:
            first_line = stream.readline()
            if not first_line:
                raise ValueError(f"{artifact}: empty artifact")
            try:
                parsed_header = json.loads(first_line)
            except json.JSONDecodeError as error:
                raise ValueError(f"{artifact}:1: invalid JSON: {error}") from error
            if not isinstance(parsed_header, dict):
                raise ValueError(f"{artifact}: header is not an object")
            header = parsed_header
            if header.get("schema") != SCHEMA:
                raise ValueError(f"{artifact}: unexpected schema")
            if (
                header.get("min_paths") != expected_min_paths
                or header.get("max_paths") != expected_max_paths
            ):
                raise ValueError(f"{artifact}: unexpected path range")
            if header.get("max_eligible") is not None:
                raise ValueError(f"{artifact}: max_eligible must be null")
            if header.get("emit_multiples") is not (not summary_only):
                expected_emit = str(not summary_only).lower()
                raise ValueError(
                    f"{artifact}: emit_multiples must be {expected_emit}"
                )
            deduplicate = header.get("deduplicate_layouts")
            if not isinstance(deduplicate, bool):
                raise ValueError(f"{artifact}: deduplicate_layouts must be boolean")
            if common_deduplicate is None:
                common_deduplicate = deduplicate
            elif deduplicate != common_deduplicate:
                raise ValueError(f"{artifact}: inconsistent deduplication setting")
            if header.get("witness_cache_limit") != 0:
                raise ValueError(f"{artifact}: witness cache must be disabled")
            if header.get("prerequisite") != PREREQUISITE:
                raise ValueError(f"{artifact}: unexpected prerequisite declaration")
            start_line = nonnegative_integer(header, "start_line", str(artifact))
            end_line = nonnegative_integer(header, "end_line", str(artifact))
            if not 1 <= start_line <= end_line <= len(corpus):
                raise ValueError(f"{artifact}: invalid shard range {start_line}-{end_line}")
            ranges.append((start_line, end_line))

            for line_index, line in enumerate(stream, 2):
                if not line.strip():
                    raise ValueError(f"{artifact}:{line_index}: blank record")
                try:
                    record = json.loads(line)
                except json.JSONDecodeError as error:
                    raise ValueError(
                        f"{artifact}:{line_index}: invalid JSON: {error}"
                    ) from error
                if not isinstance(record, dict):
                    raise ValueError(f"{artifact}:{line_index}: record is not an object")
                kind = record.get("type")
                context = f"{artifact}:{line_index}"
                try:
                    if terminal is not None:
                        raise ValueError("record follows terminal summary")
                    if kind == "candidate":
                        if saw_partition_summary:
                            raise ValueError("candidate follows partition summaries")
                        multiplicity = validate_candidate(
                            record, corpus, morphs, start_line, end_line
                        )
                        if summary_only and multiplicity != "unique":
                            raise ValueError(
                                "summary-only artifact contains a multiple candidate"
                            )
                        if not summary_only and multiplicity != "multiple":
                            raise ValueError(
                                "full-witness artifact contains a unique candidate"
                            )
                        if record.get("partition") not in expected_partition_set:
                            raise ValueError("candidate partition is outside path range")
                        artifact_candidates[multiplicity] += 1
                    elif kind == "partition-summary":
                        saw_partition_summary = True
                        partition = validate_partition_summary(
                            record, expected_partition_set, context, deduplicate
                        )
                        if partition in partition_summaries:
                            raise ValueError(f"duplicate partition summary {partition}")
                        partition_summaries[partition] = record
                    elif kind == "summary":
                        terminal = record
                    elif kind == "witness-cut":
                        raise ValueError(
                            "cache-disabled artifact contains witness-cut record"
                        )
                    else:
                        raise ValueError(f"unknown record type {kind!r}")
                except Exception as error:
                    raise ValueError(f"{context}: {error}") from error

        assert header is not None
        if terminal is None:
            raise ValueError(f"{artifact}: missing terminal summary")
        if list(partition_summaries) != expected_partition_list:
            missing = sorted(expected_partition_set - partition_summaries.keys())
            extra = sorted(partition_summaries.keys() - expected_partition_set)
            raise ValueError(
                f"{artifact}: partition summaries differ from expected order/set "
                f"(missing={missing}, extra={extra})"
            )

        if terminal.get("complete") not in {True, False} or not isinstance(
            terminal.get("complete"), bool
        ):
            raise ValueError(f"{artifact}: terminal complete must be boolean")
        if nonnegative_integer(terminal, "records", str(artifact)) != len(corpus):
            raise ValueError(f"{artifact}: terminal corpus record count mismatch")
        terminal_counts = {
            key: nonnegative_integer(terminal, key, str(artifact))
            for key in TERMINAL_COUNTER_FIELDS
        }
        for key in (
            "maximal_covers",
            "classified_layout_occurrences",
            "cut_screened_layouts",
            "solver_calls",
            "multiple_layouts",
            "unique_layouts",
        ):
            partition_total = sum(
                int(summary[key]) for summary in partition_summaries.values()
            )
            if terminal_counts[key] != partition_total:
                raise ValueError(f"{artifact}: terminal/partition mismatch for {key}")
        if terminal_counts["classified_layout_occurrences"] != (
            terminal_counts["multiple_layouts"] + terminal_counts["unique_layouts"]
        ):
            raise ValueError(f"{artifact}: terminal multiplicity accounting mismatch")
        if terminal_counts["maximal_covers"] != (
            terminal_counts["classified_layout_occurrences"]
            + sum(
                int(summary["within_source_duplicates"])
                for summary in partition_summaries.values()
            )
        ):
            raise ValueError(f"{artifact}: terminal deduplication accounting mismatch")
        if terminal_counts["solver_calls"] != terminal_counts[
            "classified_layout_occurrences"
        ]:
            raise ValueError(f"{artifact}: cache-disabled solver accounting mismatch")
        for key in (
            "cut_screened_layouts",
            "cut_cache_probes",
            "witness_cuts_retained",
            "witness_cuts_admitted",
        ):
            if terminal_counts[key] != 0:
                raise ValueError(f"{artifact}: cache-disabled {key} must be zero")

        if summary_only:
            if artifact_candidates["multiple"] != 0:
                raise ValueError(f"{artifact}: multiple candidate record was emitted")
            if artifact_candidates["unique"] != terminal_counts["unique_layouts"]:
                raise ValueError(f"{artifact}: unique candidate/summary count mismatch")
        else:
            if artifact_candidates["unique"] != 0:
                raise ValueError(f"{artifact}: full-witness run unexpectedly found unique")
            if artifact_candidates["multiple"] != terminal_counts[
                "classified_layout_occurrences"
            ]:
                raise ValueError(f"{artifact}: candidate/summary count mismatch")
            if terminal_counts["unique_layouts"] != 0:
                raise ValueError(f"{artifact}: expected zero unique layouts")

        expected_eligible = sum(
            eligible_for_max_paths(corpus[line_number - 1], expected_max_paths)
            for line_number in range(start_line, end_line + 1)
        )
        exhausted = terminal_counts["eligible_records"] == expected_eligible
        if terminal_counts["eligible_records"] > expected_eligible:
            raise ValueError(f"{artifact}: eligible record count exceeds shard total")
        if not exhausted and terminal_counts["unique_layouts"] == 0:
            raise ValueError(f"{artifact}: shard stopped without a unique discovery")
        all_ranges_exhausted &= exhausted

        full_corpus_shard = start_line == 1 and end_line == len(corpus)
        if terminal["complete"] and (not full_corpus_shard or not exhausted):
            raise ValueError(f"{artifact}: invalid complete=true terminal status")
        if (
            full_corpus_shard
            and exhausted
            and terminal_counts["unique_layouts"] == 0
            and terminal["complete"] is not True
        ):
            raise ValueError(f"{artifact}: complete full-corpus run marked incomplete")
        if (not full_corpus_shard or not exhausted) and terminal["complete"] is not False:
            raise ValueError(f"{artifact}: partial/stopped shard marked complete")

        artifact_hash = sha256(artifact)
        stat_after = artifact.stat()
        if (
            stat_before.st_size != stat_after.st_size
            or stat_before.st_mtime_ns != stat_after.st_mtime_ns
        ):
            raise ValueError(f"{artifact}: artifact changed while it was verified")

        total_candidates["multiple"] += artifact_candidates["multiple"]
        total_candidates["unique"] += artifact_candidates["unique"]
        terminals.append(terminal)
        artifact_info.append(
            {
                "path": str(artifact),
                "bytes": stat_after.st_size,
                "sha256": artifact_hash,
                "candidate_records": sum(artifact_candidates.values()),
                "multiple_candidate_records": artifact_candidates["multiple"],
                "unique_candidate_records": artifact_candidates["unique"],
                "eligible_records": terminal_counts["eligible_records"],
                "expected_eligible_records": expected_eligible,
                "range": [start_line, end_line],
                "range_exhausted": exhausted,
            }
        )

    ranges.sort()
    expected_start = 1
    for start, end in ranges:
        if start != expected_start:
            raise ValueError(f"range gap/overlap before {start} (expected {expected_start})")
        expected_start = end + 1
    if expected_start != len(corpus) + 1:
        raise ValueError(f"ranges stop at {expected_start - 1}, expected {len(corpus)}")

    coverage_complete = all_ranges_exhausted
    result = {
        "status": "valid" if coverage_complete else "valid-unique-found-incomplete",
        "summary_only": summary_only,
        "coverage_complete": coverage_complete,
        "corpus_bytes": len(corpus_bytes),
        "corpus_sha256": hashlib.sha256(corpus_bytes).hexdigest(),
        "records": len(corpus),
        "eligible_records": sum(int(summary["eligible_records"]) for summary in terminals),
        "candidate_occurrences": sum(
            int(summary["classified_layout_occurrences"]) for summary in terminals
        ),
        "emitted_candidate_records": sum(total_candidates.values()),
        "multiple_layouts": sum(int(summary["multiple_layouts"]) for summary in terminals),
        "unique_layouts": sum(int(summary["unique_layouts"]) for summary in terminals),
        "expected_partitions_per_shard": len(expected_partition_list),
        "artifacts": artifact_info,
    }
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("corpus", type=Path)
    parser.add_argument("artifacts", nargs="+", type=Path)
    parser.add_argument("--expected-min-paths", type=int, required=True)
    parser.add_argument("--expected-max-paths", type=int, required=True)
    parser.add_argument(
        "--summary-only",
        action="store_true",
        help="verify emit_multiples=false artifacts while fully checking any unique record",
    )
    args = parser.parse_args()

    result = verify_artifacts(
        args.corpus,
        args.artifacts,
        args.expected_min_paths,
        args.expected_max_paths,
        args.summary_only,
    )
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
