#!/usr/bin/env python3
"""Run one exact maximal-forest path layer as durable, resumable chunks.

The Rust scanner deliberately remains single-threaded.  This launcher gives
several scanner processes small, dynamically assigned catalogue ranges.  A
range becomes complete only after its JSONL passes structural/accounting
checks and is atomically renamed from a temporary file.  Re-running the same
command skips those completed ranges, so killing a search loses at most the
currently active small chunks rather than a multi-thousand-record shard.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
import uuid
from typing import Any, Iterable


CORPUS_RECORDS = 49_158
SCANNER_SCHEMA = "thermo-17c-maximal-v2"
RUN_SCHEMA = "thermo-17c-maximal-chunk-run-v1"
PREREQUISITE = "all lower path counts below min_paths excluded"
PARTITION_FIELDS = (
    "maximal_covers",
    "within_source_duplicates",
    "classified_layout_occurrences",
    "cut_screened_layouts",
    "solver_calls",
    "multiple_layouts",
    "unique_layouts",
)
SUMMARY_FIELDS = (
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
    "early_noncanonical_prunes",
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


class RunError(RuntimeError):
    pass


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def atomic_json(path: Path, value: Any) -> None:
    temporary = path.with_name(f".{path.name}.{uuid.uuid4().hex}.tmp")
    with temporary.open("w", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, sort_keys=True, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)


def parse_corpus(path: Path) -> tuple[bytes, list[str]]:
    data = path.read_bytes()
    try:
        records = data.decode("ascii").splitlines()
    except UnicodeDecodeError as error:
        raise RunError("catalogue is not ASCII") from error
    if len(records) != CORPUS_RECORDS:
        raise RunError(
            f"expected {CORPUS_RECORDS:,} catalogue records, got {len(records):,}"
        )
    for line_number, encoded in enumerate(records, 1):
        if len(encoded) != 81 or any(cell not in ".0123456789" for cell in encoded):
            raise RunError(f"catalogue line {line_number} is not an 81-cell puzzle")
        if sum(cell in "123456789" for cell in encoded) != 17:
            raise RunError(f"catalogue line {line_number} does not have 17 clues")
    return data, records


def eligible(encoded: str, paths: int) -> bool:
    counts = [encoded.count(digit) for digit in "123456789"]
    return all(0 < count <= paths for count in counts)


def make_chunks(records: list[str], paths: int, eligible_per_chunk: int) -> list[dict[str, int]]:
    chunks: list[dict[str, int]] = []
    start = 1
    eligible_in_chunk = 0
    total_eligible = 0
    for line_number, encoded in enumerate(records, 1):
        if eligible(encoded, paths):
            eligible_in_chunk += 1
            total_eligible += 1
        if eligible_in_chunk == eligible_per_chunk:
            chunks.append(
                {
                    "index": len(chunks) + 1,
                    "start_line": start,
                    "end_line": line_number,
                    "eligible_records": eligible_in_chunk,
                }
            )
            start = line_number + 1
            eligible_in_chunk = 0
    if start <= len(records):
        chunks.append(
            {
                "index": len(chunks) + 1,
                "start_line": start,
                "end_line": len(records),
                "eligible_records": eligible_in_chunk,
            }
        )
    if sum(chunk["eligible_records"] for chunk in chunks) != total_eligible:
        raise AssertionError("chunk eligibility accounting failed")
    expected_start = 1
    for chunk in chunks:
        if chunk["start_line"] != expected_start:
            raise AssertionError("chunks are not contiguous")
        expected_start = chunk["end_line"] + 1
    if expected_start != len(records) + 1:
        raise AssertionError("chunks do not cover the catalogue")
    return chunks


def enumerate_partitions(
    remaining: int,
    count: int,
    maximum: int,
    prefix: tuple[int, ...] = (),
) -> Iterable[tuple[int, ...]]:
    if count == 0:
        if remaining == 0:
            yield prefix
        return
    upper = min(maximum, remaining - 2 * (count - 1))
    for length in range(upper, 1, -1):
        yield from enumerate_partitions(
            remaining - length, count - 1, length, prefix + (length,)
        )


def expected_partitions(paths: int) -> list[str]:
    return ["+".join(map(str, part)) for part in enumerate_partitions(17, paths, 9)]


def nonnegative(record: dict[str, Any], key: str, context: str) -> int:
    value = record.get(key)
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise RunError(f"{context}: {key} must be a nonnegative integer")
    return value


def artifact_name(chunk: dict[str, int]) -> str:
    return (
        f"chunk-{chunk['index']:05d}-lines-"
        f"{chunk['start_line']:05d}-{chunk['end_line']:05d}.jsonl"
    )


def validate_artifact(
    path: Path,
    chunk: dict[str, int],
    paths: int,
    record_count: int,
    corpus_records: list[str] | None = None,
) -> dict[str, Any]:
    data = path.read_bytes()
    if not data or not data.endswith(b"\n"):
        raise RunError(f"{path}: empty or truncated artifact")
    try:
        records = [json.loads(line) for line in data.decode("utf-8").splitlines()]
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RunError(f"{path}: invalid JSONL: {error}") from error
    if not records or not all(isinstance(record, dict) for record in records):
        raise RunError(f"{path}: artifact records must be JSON objects")
    header = records[0]
    expected_header = {
        "schema": SCANNER_SCHEMA,
        "min_paths": paths,
        "max_paths": paths,
        "start_line": chunk["start_line"],
        "end_line": chunk["end_line"],
        "max_eligible": None,
        "emit_multiples": False,
        "deduplicate_layouts": False,
        "witness_cache_limit": 0,
        "prerequisite": PREREQUISITE,
    }
    for key, expected in expected_header.items():
        if header.get(key) != expected:
            raise RunError(f"{path}: header {key} is {header.get(key)!r}, expected {expected!r}")

    partitions: dict[str, dict[str, Any]] = {}
    unique_candidates = 0
    terminal: dict[str, Any] | None = None
    for index, record in enumerate(records[1:], 2):
        kind = record.get("type")
        context = f"{path}:{index}"
        if terminal is not None:
            raise RunError(f"{context}: record follows terminal summary")
        if kind == "candidate":
            if record.get("multiplicity") != "unique":
                raise RunError(f"{context}: summary-only chunk emitted a multiple candidate")
            source_line = nonnegative(record, "source_line", context)
            if not chunk["start_line"] <= source_line <= chunk["end_line"]:
                raise RunError(f"{context}: candidate source line is outside its chunk")
            if (
                corpus_records is not None
                and record.get("source_puzzle") != corpus_records[source_line - 1]
            ):
                raise RunError(f"{context}: candidate source puzzle does not match the catalogue")
            if record.get("partition") not in expected_partitions(paths):
                raise RunError(f"{context}: candidate partition is outside its layer")
            unique_candidates += 1
        elif kind == "partition-summary":
            partition = record.get("partition")
            if not isinstance(partition, str) or partition in partitions:
                raise RunError(f"{context}: invalid or duplicate partition summary")
            for key in PARTITION_FIELDS:
                nonnegative(record, key, context)
            partitions[partition] = record
        elif kind == "summary":
            terminal = record
        else:
            raise RunError(f"{context}: unexpected record type {kind!r}")

    if terminal is None:
        raise RunError(f"{path}: missing terminal summary")
    if list(partitions) != expected_partitions(paths):
        raise RunError(f"{path}: partition summary order/set mismatch")
    if nonnegative(terminal, "records", str(path)) != record_count:
        raise RunError(f"{path}: terminal catalogue size mismatch")
    for key in SUMMARY_FIELDS:
        # Older frozen artifacts do not contain the new early-prune counter;
        # newly scheduled chunks do.  The scheduler requires it so a run
        # cannot silently mix pre- and post-optimization scanner binaries.
        nonnegative(terminal, key, str(path))
    for key in (
        "maximal_covers",
        "classified_layout_occurrences",
        "cut_screened_layouts",
        "solver_calls",
        "multiple_layouts",
        "unique_layouts",
    ):
        if terminal[key] != sum(partition[key] for partition in partitions.values()):
            raise RunError(f"{path}: terminal/partition mismatch for {key}")
    if terminal["classified_layout_occurrences"] != (
        terminal["multiple_layouts"] + terminal["unique_layouts"]
    ):
        raise RunError(f"{path}: multiplicity accounting mismatch")
    if unique_candidates != terminal["unique_layouts"]:
        raise RunError(f"{path}: unique candidate/summary mismatch")
    processed_eligible = terminal["eligible_records"]
    exhausted = processed_eligible == chunk["eligible_records"]
    if not exhausted and (
        terminal["unique_layouts"] == 0
        or not 0 < processed_eligible <= chunk["eligible_records"]
    ):
        raise RunError(f"{path}: terminal eligible-record count mismatch")
    expected_complete = (
        chunk["start_line"] == 1
        and chunk["end_line"] == record_count
        and exhausted
        and terminal["unique_layouts"] == 0
    )
    if terminal.get("complete") is not expected_complete:
        raise RunError(f"{path}: terminal complete flag mismatch")
    return {
        "terminal": terminal,
        "partitions": partitions,
        "bytes": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
        "exhausted": exhausted,
    }


class ActiveProcesses:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.processes: set[subprocess.Popen[str]] = set()

    def add(self, process: subprocess.Popen[str]) -> None:
        with self.lock:
            self.processes.add(process)

    def discard(self, process: subprocess.Popen[str]) -> None:
        with self.lock:
            self.processes.discard(process)

    def terminate_all(self) -> None:
        with self.lock:
            processes = list(self.processes)
        for process in processes:
            if process.poll() is None:
                process.terminate()


def run_chunk(
    binary: Path,
    corpus: Path,
    output_dir: Path,
    chunk: dict[str, int],
    paths: int,
    corpus_records: list[str],
    active: ActiveProcesses,
) -> tuple[Path, dict[str, Any]]:
    final_path = output_dir / artifact_name(chunk)
    temporary = output_dir / f".{final_path.name}.{uuid.uuid4().hex}.partial"
    command = [
        str(binary),
        "--input",
        str(corpus),
        "--output",
        str(temporary),
        "--min-paths",
        str(paths),
        "--max-paths",
        str(paths),
        "--start-line",
        str(chunk["start_line"]),
        "--end-line",
        str(chunk["end_line"]),
        "--stop-on-first",
        "--progress-every",
        "0",
    ]
    flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
        creationflags=flags,
    )
    active.add(process)
    try:
        stdout, stderr = process.communicate()
    finally:
        active.discard(process)
    if process.returncode != 0:
        raise RunError(
            f"chunk {chunk['index']} scanner exit {process.returncode}; "
            f"stdout={stdout[-2000:]!r}; stderr={stderr[-4000:]!r}; "
            f"partial={temporary}"
        )
    try:
        result = validate_artifact(
            temporary, chunk, paths, len(corpus_records), corpus_records
        )
    except Exception:
        raise RunError(
            f"chunk {chunk['index']} produced an invalid partial artifact at {temporary}"
        ) from None
    os.replace(temporary, final_path)
    return final_path, result


def aggregate(
    output_dir: Path,
    chunks: list[dict[str, int]],
    paths: int,
    corpus_records: list[str],
) -> dict[str, Any]:
    totals = {key: 0 for key in SUMMARY_FIELDS}
    partition_totals = {
        partition: {key: 0 for key in PARTITION_FIELDS}
        for partition in expected_partitions(paths)
    }
    artifact_digest = hashlib.sha256()
    artifact_bytes = 0
    completed = 0
    for chunk in chunks:
        path = output_dir / artifact_name(chunk)
        if not path.exists():
            continue
        result = validate_artifact(path, chunk, paths, len(corpus_records), corpus_records)
        completed += int(result["exhausted"])
        artifact_bytes += result["bytes"]
        artifact_digest.update(path.name.encode("ascii"))
        artifact_digest.update(b"\0")
        artifact_digest.update(bytes.fromhex(result["sha256"]))
        for key in SUMMARY_FIELDS:
            totals[key] += result["terminal"][key]
        for partition, summary in result["partitions"].items():
            for key in PARTITION_FIELDS:
                partition_totals[partition][key] += summary[key]
    return {
        "schema": RUN_SCHEMA,
        "paths": paths,
        "complete": completed == len(chunks),
        "chunks": len(chunks),
        "completed_chunks": completed,
        "artifact_bytes": artifact_bytes,
        "artifact_set_sha256": artifact_digest.hexdigest(),
        "totals": totals,
        "partitions": partition_totals,
    }


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--paths", type=int, choices=range(4, 8), required=True)
    parser.add_argument("--workers", type=int, default=min(4, os.cpu_count() or 1))
    parser.add_argument(
        "--max-new-chunks",
        type=int,
        help="process at most this many pending chunks in this invocation",
    )
    parser.add_argument(
        "--eligible-per-chunk",
        type=int,
        default=16,
        help="durable work-unit size; default: 16 eligible catalogue records",
    )
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if (
        args.workers <= 0
        or args.eligible_per_chunk <= 0
        or args.max_new_chunks is not None
        and args.max_new_chunks <= 0
    ):
        raise RunError("workers, chunk size, and max-new-chunks must be positive")
    corpus = args.corpus.resolve()
    binary = args.binary.resolve()
    output_dir = args.output_dir.resolve()
    if not corpus.is_file() or not binary.is_file():
        raise RunError("corpus and binary must be existing files")
    corpus_data, records = parse_corpus(corpus)
    chunks = make_chunks(records, args.paths, args.eligible_per_chunk)
    identity = {
        "schema": RUN_SCHEMA,
        "corpus_bytes": len(corpus_data),
        "corpus_sha256": hashlib.sha256(corpus_data).hexdigest(),
        "records": len(records),
        "binary_bytes": binary.stat().st_size,
        "binary_sha256": sha256(binary),
        "paths": args.paths,
        "eligible_per_chunk": args.eligible_per_chunk,
        "eligible_records": sum(chunk["eligible_records"] for chunk in chunks),
        "chunks": len(chunks),
        "scanner_options": {
            "min_paths": args.paths,
            "max_paths": args.paths,
            "stop_on_first": True,
            "emit_multiples": False,
            "deduplicate_layouts": False,
            "witness_cache_limit": 0,
        },
    }
    if args.dry_run:
        print(json.dumps(identity, sort_keys=True))
        return 0

    output_dir.mkdir(parents=True, exist_ok=True)
    identity_path = output_dir / "run-identity.json"
    if identity_path.exists():
        existing = json.loads(identity_path.read_text(encoding="utf-8"))
        if existing != identity:
            raise RunError(
                "output directory belongs to a different corpus, binary, layer, or chunking"
            )
    else:
        atomic_json(identity_path, identity)

    completed: dict[int, tuple[Path, dict[str, Any]]] = {}
    pending: list[dict[str, int]] = []
    for chunk in chunks:
        path = output_dir / artifact_name(chunk)
        if path.exists():
            completed[chunk["index"]] = (
                path,
                validate_artifact(path, chunk, args.paths, len(records), records),
            )
        else:
            pending.append(chunk)

    pending_total = len(pending)
    if args.max_new_chunks is not None:
        pending = pending[: args.max_new_chunks]

    print(
        json.dumps(
            {
                "event": "resume",
                "paths": args.paths,
                "chunks": len(chunks),
                "completed": len(completed),
                "pending": pending_total,
                "selected_this_invocation": len(pending),
                "eligible_records": identity["eligible_records"],
                "workers": args.workers,
            },
            sort_keys=True,
        ),
        flush=True,
    )
    started = time.monotonic()
    active = ActiveProcesses()
    unique_found = any(
        result["terminal"]["unique_layouts"] != 0 for _, result in completed.values()
    )
    pending_iterator = iter(pending)
    futures: dict[concurrent.futures.Future[tuple[Path, dict[str, Any]]], dict[str, int]] = {}
    executor = concurrent.futures.ThreadPoolExecutor(max_workers=args.workers)
    try:
        if not unique_found:
            for _ in range(min(args.workers, len(pending))):
                chunk = next(pending_iterator)
                future = executor.submit(
                    run_chunk,
                    binary,
                    corpus,
                    output_dir,
                    chunk,
                    args.paths,
                    records,
                    active,
                )
                futures[future] = chunk
        while futures:
            done, _ = concurrent.futures.wait(
                futures, return_when=concurrent.futures.FIRST_COMPLETED
            )
            for future in done:
                chunk = futures.pop(future)
                path, result = future.result()
                completed[chunk["index"]] = (path, result)
                if result["terminal"]["unique_layouts"] != 0:
                    unique_found = True
                print(
                    json.dumps(
                        {
                            "event": "chunk-complete",
                            "chunk": chunk["index"],
                            "completed": len(completed),
                            "total": len(chunks),
                            "source_lines": [chunk["start_line"], chunk["end_line"]],
                            "eligible_records": chunk["eligible_records"],
                            "classified": result["terminal"][
                                "classified_layout_occurrences"
                            ],
                            "unique": result["terminal"]["unique_layouts"],
                            "elapsed_seconds": round(time.monotonic() - started, 3),
                        },
                        sort_keys=True,
                    ),
                    flush=True,
                )
                if not unique_found:
                    try:
                        next_chunk = next(pending_iterator)
                    except StopIteration:
                        pass
                    else:
                        next_future = executor.submit(
                            run_chunk,
                            binary,
                            corpus,
                            output_dir,
                            next_chunk,
                            args.paths,
                            records,
                            active,
                        )
                        futures[next_future] = next_chunk
    except BaseException:
        active.terminate_all()
        raise
    finally:
        executor.shutdown(wait=True, cancel_futures=True)

    summary = aggregate(output_dir, chunks, args.paths, records)
    summary.update(
        {
            "corpus_sha256": identity["corpus_sha256"],
            "binary_sha256": identity["binary_sha256"],
            "elapsed_seconds_this_invocation": round(time.monotonic() - started, 3),
            "stopped_after_unique": unique_found,
        }
    )
    atomic_json(output_dir / "summary.json", summary)
    print(json.dumps(summary, sort_keys=True), flush=True)
    return 3 if unique_found else (0 if summary["complete"] else 2)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RunError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(2) from error
