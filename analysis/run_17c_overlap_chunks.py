#!/usr/bin/env python3
"""Run the exact generalized 17-cell scan as durable dynamic chunks.

Each Rust process owns a small, independent catalogue interval and writes to a
unique temporary JSONL.  The launcher validates the terminal accounting before
atomically publishing the chunk.  Re-running the same command skips validated
chunks, while a unique result stops new scheduling and terminates outstanding
workers.  An OS advisory lock permits only one launcher to mutate an output
directory at a time and is released automatically if the launcher crashes.
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
from typing import Any


CORPUS_RECORDS = 49_158
CORPUS_FNV1A64 = 0x96BAF249978384BB
SCANNER_SCHEMA = "thermo-17c-overlap-v1"
RUN_SCHEMA = "thermo-17c-overlap-chunk-run-v1"
FNV_OFFSET = 0xCBF29CE484222325
FNV_PRIME = 0x100000001B3

COUNT_FIELDS = (
    "records_in_range",
    "records_missing_digits",
    "row_axis_classes_raw",
    "row_axis_classes_maximal",
    "column_axis_classes_raw",
    "column_axis_classes_maximal",
    "network_intersections_scanned",
    "network_classes_raw",
    "network_classes_retained",
    "coordinate_morph_pairs_in_scope",
    "retained_exact_mask_morph_pairs",
    "coverage_pruned_classes",
    "no_hamiltonian_classes",
    "canonical_digit_orders",
    "hamiltonian_orders",
    "structurally_pruned_orders",
    "duplicate_orientation_orders",
    "raw_candidate_orientations",
    "unique_poset_closures",
    "duplicate_poset_closures",
    "maximal_poset_closures",
    "dominated_poset_closures",
    "candidate_units",
    "classified_units",
    "zero",
    "unique",
    "multiple",
    "exact_counts",
    "capped_counts",
    "observed_solution_count_sum",
    "solver_nodes",
    "solver_branches",
    "solver_propagation_rounds",
    "solver_comparison_revisions",
)


class RunError(RuntimeError):
    pass


class OutputDirectoryLock:
    """Crash-released advisory lock for one launcher per output directory."""

    def __init__(self, path: Path, stream: Any) -> None:
        self.path = path
        self._stream = stream

    @classmethod
    def acquire(cls, output_dir: Path) -> "OutputDirectoryLock":
        path = output_dir / ".thermo-17c-overlap-runner.lock"
        try:
            stream = path.open("a+b", buffering=0)
        except OSError as error:
            raise RunError(f"cannot open output-directory lock {path}: {error}") from error
        try:
            if os.name == "nt":
                import msvcrt

                stream.seek(0, os.SEEK_END)
                if stream.tell() == 0:
                    stream.write(b"\0")
                stream.seek(0)
                msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl

                fcntl.flock(stream.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except (OSError, IOError) as error:
            stream.close()
            raise RunError(
                f"another launcher holds the output-directory lock {path}"
            ) from error
        return cls(path, stream)

    def close(self) -> None:
        stream, self._stream = self._stream, None
        if stream is None:
            return
        try:
            if os.name == "nt":
                import msvcrt

                stream.seek(0)
                msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                import fcntl

                fcntl.flock(stream.fileno(), fcntl.LOCK_UN)
        finally:
            stream.close()

    def __del__(self) -> None:
        self.close()


def fnv1a64(data: bytes) -> int:
    value = FNV_OFFSET
    for byte in data:
        value ^= byte
        value = value * FNV_PRIME & 0xFFFF_FFFF_FFFF_FFFF
    return value


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
    if fnv1a64(data) != CORPUS_FNV1A64:
        raise RunError("catalogue FNV-1a64 does not match the audited corpus")
    for line_number, encoded in enumerate(records, 1):
        if len(encoded) != 81 or any(cell not in ".123456789" for cell in encoded):
            raise RunError(f"catalogue line {line_number} is not an 81-cell puzzle")
        if sum(cell != "." for cell in encoded) != 17:
            raise RunError(f"catalogue line {line_number} does not have 17 clues")
    return data, records


def eligible(encoded: str) -> bool:
    return all(digit in encoded for digit in "123456789")


def make_chunks(records: list[str], eligible_per_chunk: int) -> list[dict[str, int]]:
    chunks: list[dict[str, int]] = []
    start = 1
    count = 0
    for line_number, encoded in enumerate(records, 1):
        count += int(eligible(encoded))
        if count == eligible_per_chunk:
            chunks.append(
                {
                    "index": len(chunks) + 1,
                    "start_line": start,
                    "end_line": line_number,
                    "eligible_records": count,
                }
            )
            start = line_number + 1
            count = 0
    if start <= len(records):
        chunks.append(
            {
                "index": len(chunks) + 1,
                "start_line": start,
                "end_line": len(records),
                "eligible_records": count,
            }
        )
    expected_start = 1
    for chunk in chunks:
        if chunk["start_line"] != expected_start:
            raise AssertionError("chunk ranges are not contiguous")
        expected_start = chunk["end_line"] + 1
    if expected_start != len(records) + 1:
        raise AssertionError("chunk ranges do not cover the corpus")
    return chunks


def artifact_name(chunk: dict[str, int]) -> str:
    return (
        f"chunk-{chunk['index']:05d}-lines-"
        f"{chunk['start_line']:05d}-{chunk['end_line']:05d}.jsonl"
    )


def nonnegative(record: dict[str, Any], key: str, context: str) -> int:
    value = record.get(key)
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise RunError(f"{context}: {key} must be a nonnegative integer")
    return value


def validate_artifact(
    path: Path,
    chunk: dict[str, int],
    records: list[str],
    algorithm_revision: str | None,
) -> dict[str, Any]:
    data = path.read_bytes()
    if not data or not data.endswith(b"\n"):
        raise RunError(f"{path}: empty or truncated JSONL")
    try:
        entries = [json.loads(line) for line in data.decode("utf-8").splitlines()]
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise RunError(f"{path}: invalid JSONL: {error}") from error
    if len(entries) < 2 or not all(isinstance(entry, dict) for entry in entries):
        raise RunError(f"{path}: missing header or summary")
    header = entries[0]
    actual_revision = header.get("algorithm_revision")
    if not isinstance(actual_revision, str) or not actual_revision:
        raise RunError(f"{path}: header has no algorithm revision")
    if algorithm_revision and actual_revision != algorithm_revision:
        raise RunError(
            f"{path}: algorithm revision is {actual_revision!r}, "
            f"expected {algorithm_revision!r}"
        )
    expected_header = {
        "type": "header",
        "schema": SCANNER_SCHEMA,
        "algorithm_revision": actual_revision,
        "input_fnv1a64": f"{CORPUS_FNV1A64:016x}",
        "records": len(records),
        "expected_records": len(records),
        "expected_fnv1a64": f"{CORPUS_FNV1A64:016x}",
        "corpus_is_complete_assertion": True,
        "mode": "exact",
        "start_line": chunk["start_line"],
        "end_line": chunk["end_line"],
        "max_units": None,
        "solution_cap": 2,
        "emit_cases": False,
    }
    for key, expected in expected_header.items():
        if header.get(key) != expected:
            raise RunError(
                f"{path}: header {key} is {header.get(key)!r}, expected {expected!r}"
            )

    cases: list[dict[str, Any]] = []
    summary: dict[str, Any] | None = None
    for index, entry in enumerate(entries[1:], 2):
        if summary is not None:
            raise RunError(f"{path}:{index}: record follows terminal summary")
        kind = entry.get("type")
        if kind == "case":
            line = nonnegative(entry, "line", f"{path}:{index}")
            if not chunk["start_line"] <= line <= chunk["end_line"]:
                raise RunError(f"{path}:{index}: case line is outside chunk")
            if entry.get("source") != records[line - 1]:
                raise RunError(f"{path}:{index}: case source does not match catalogue")
            if entry.get("multiplicity") not in {"zero", "unique"}:
                raise RunError(f"{path}:{index}: unexpected emitted multiple case")
            cases.append(entry)
        elif kind == "summary":
            summary = entry
        else:
            raise RunError(f"{path}:{index}: unexpected record type {kind!r}")
    if summary is None:
        raise RunError(f"{path}: missing terminal summary")
    for key in COUNT_FIELDS:
        nonnegative(summary, key, str(path))
    if summary.get("schema") != SCANNER_SCHEMA:
        raise RunError(f"{path}: summary schema mismatch")
    if summary.get("algorithm_revision") != actual_revision:
        raise RunError(f"{path}: summary algorithm revision mismatch")
    if summary.get("mode") != "exact" or summary.get("limit_hit") is not False:
        raise RunError(f"{path}: exact unlimited run metadata mismatch")
    fingerprint = header.get("fingerprint")
    if (
        not isinstance(fingerprint, str)
        or len(fingerprint) != 16
        or any(character not in "0123456789abcdef" for character in fingerprint)
        or summary.get("fingerprint") != fingerprint
    ):
        raise RunError(f"{path}: header/summary fingerprint mismatch")
    if summary["classified_units"] != (
        summary["zero"] + summary["unique"] + summary["multiple"]
    ):
        raise RunError(f"{path}: multiplicity accounting mismatch")
    if summary["classified_units"] != summary["exact_counts"] + summary["capped_counts"]:
        raise RunError(f"{path}: exact/capped accounting mismatch")
    if summary["candidate_units"] != summary["classified_units"]:
        raise RunError(f"{path}: candidate/classification accounting mismatch")
    if summary["zero"] != 0:
        raise RunError(f"{path}: target-true network unexpectedly has no solution")
    if summary["exact_counts"] != summary["unique"]:
        raise RunError(f"{path}: cap-two exact-count accounting mismatch")
    if summary["capped_counts"] != summary["multiple"]:
        raise RunError(f"{path}: cap-two capped-count accounting mismatch")
    if summary["observed_solution_count_sum"] != (
        summary["unique"] + 2 * summary["multiple"]
    ):
        raise RunError(f"{path}: cap-two solution-sum accounting mismatch")
    emitted_zero = sum(case.get("multiplicity") == "zero" for case in cases)
    emitted_unique = sum(case.get("multiplicity") == "unique" for case in cases)
    if emitted_zero != summary["zero"] or emitted_unique != summary["unique"]:
        raise RunError(f"{path}: emitted exceptional-case accounting mismatch")

    stopped = summary.get("stopped_on_unique") is True
    exhausted = summary.get("exact_requested_range_complete") is True
    if stopped == exhausted or stopped != (summary["unique"] != 0):
        raise RunError(f"{path}: completion/unique-stop flags are inconsistent")
    expected_lines = chunk["end_line"] - chunk["start_line"] + 1
    if exhausted:
        if summary["records_in_range"] != expected_lines:
            raise RunError(f"{path}: completed range record count mismatch")
        missing = sum(
            not eligible(record)
            for record in records[chunk["start_line"] - 1 : chunk["end_line"]]
        )
        if summary["records_missing_digits"] != missing:
            raise RunError(f"{path}: missing-digit accounting mismatch")
    elif not 0 < summary["records_in_range"] <= expected_lines:
        raise RunError(f"{path}: stopped range record count is invalid")

    return {
        "summary": summary,
        "bytes": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
        "exhausted": exhausted,
        "unique": summary["unique"] != 0,
        "algorithm_revision": actual_revision,
    }


class ActiveProcesses:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._processes: set[subprocess.Popen[str]] = set()

    def add(self, process: subprocess.Popen[str]) -> None:
        with self._lock:
            self._processes.add(process)

    def discard(self, process: subprocess.Popen[str]) -> None:
        with self._lock:
            self._processes.discard(process)

    def terminate_all(self) -> None:
        with self._lock:
            processes = tuple(self._processes)
        for process in processes:
            if process.poll() is None:
                process.terminate()


def run_chunk(
    binary: Path,
    corpus: Path,
    output_dir: Path,
    chunk: dict[str, int],
    records: list[str],
    algorithm_revision: str,
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
        "--mode",
        "exact",
        "--start-line",
        str(chunk["start_line"]),
        "--end-line",
        str(chunk["end_line"]),
        "--solution-cap",
        "2",
        "--stop-on-first",
        "--progress-every",
        "0",
        "--expected-records",
        str(len(records)),
        "--expected-fnv64",
        f"{CORPUS_FNV1A64:016x}",
        "--corpus-is-complete",
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
        result = validate_artifact(temporary, chunk, records, algorithm_revision)
    except Exception as error:
        raise RunError(
            f"chunk {chunk['index']} left invalid partial artifact {temporary}: {error}"
        ) from error
    os.replace(temporary, final_path)
    return final_path, result


def aggregate(
    output_dir: Path,
    chunks: list[dict[str, int]],
    records: list[str],
    algorithm_revision: str,
) -> dict[str, Any]:
    totals = {key: 0 for key in COUNT_FIELDS}
    artifact_digest = hashlib.sha256()
    artifact_bytes = 0
    exhausted = 0
    unique = False
    for chunk in chunks:
        path = output_dir / artifact_name(chunk)
        if not path.exists():
            continue
        result = validate_artifact(path, chunk, records, algorithm_revision)
        exhausted += int(result["exhausted"])
        unique |= result["unique"]
        artifact_bytes += result["bytes"]
        artifact_digest.update(path.name.encode("ascii"))
        artifact_digest.update(b"\0")
        artifact_digest.update(bytes.fromhex(result["sha256"]))
        for key in COUNT_FIELDS:
            totals[key] += result["summary"][key]
    return {
        "schema": RUN_SCHEMA,
        "algorithm_revision": algorithm_revision,
        "complete": exhausted == len(chunks),
        "unique_found": unique,
        "chunks": len(chunks),
        "completed_chunks": exhausted,
        "artifact_bytes": artifact_bytes,
        "artifact_set_sha256": artifact_digest.hexdigest(),
        "totals": totals,
    }


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--workers", type=int, default=min(4, os.cpu_count() or 1))
    parser.add_argument("--eligible-per-chunk", type=int, default=16)
    parser.add_argument("--max-new-chunks", type=int)
    parser.add_argument("--dry-run", action="store_true")
    return parser.parse_args(argv)


def read_revision(path: Path) -> str:
    entries = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]
    revision = entries[0].get("algorithm_revision") if entries else None
    if not isinstance(revision, str) or not revision:
        raise RunError(f"{path}: missing algorithm revision")
    return revision


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
        raise RunError("corpus and scanner binary must be existing files")
    if output_dir == corpus or output_dir in corpus.parents:
        raise RunError("output directory must not contain the catalogue")
    if output_dir == binary or output_dir in binary.parents:
        raise RunError("output directory must not contain the scanner binary")
    corpus_data, records = parse_corpus(corpus)
    chunks = make_chunks(records, args.eligible_per_chunk)
    binary_hash = sha256(binary)
    corpus_hash = hashlib.sha256(corpus_data).hexdigest()
    identity_path = output_dir / "run-identity.json"
    summary_path = output_dir / "summary.json"
    lock_path = output_dir / ".thermo-17c-overlap-runner.lock"
    for reserved in (identity_path, summary_path, lock_path):
        if reserved.exists() and (
            os.path.samefile(reserved, corpus) or os.path.samefile(reserved, binary)
        ):
            raise RunError(f"reserved output aliases an input file: {reserved}")
    algorithm_revision = ""
    dry_identity = {
        "schema": RUN_SCHEMA,
        "corpus_sha256": corpus_hash,
        "binary_sha256": binary_hash,
        "eligible_records": sum(chunk["eligible_records"] for chunk in chunks),
        "chunks": len(chunks),
    }
    if args.dry_run:
        print(json.dumps(dry_identity, sort_keys=True))
        return 0
    output_dir.mkdir(parents=True, exist_ok=True)
    # Keep this object alive for the complete mutating run. Closing it (or a
    # process crash) releases the OS lock; the harmless lock file may remain.
    _run_lock = OutputDirectoryLock.acquire(output_dir)
    if identity_path.exists():
        identity = json.loads(identity_path.read_text(encoding="utf-8"))
        if not isinstance(identity.get("algorithm_revision"), str) or not identity[
            "algorithm_revision"
        ]:
            raise RunError("run identity has no scanner algorithm revision")
        expected_identity = {
            "schema": RUN_SCHEMA,
            "corpus_bytes": len(corpus_data),
            "corpus_sha256": corpus_hash,
            "corpus_fnv1a64": f"{CORPUS_FNV1A64:016x}",
            "records": len(records),
            "binary_bytes": binary.stat().st_size,
            "binary_sha256": binary_hash,
            "eligible_per_chunk": args.eligible_per_chunk,
            "eligible_records": sum(chunk["eligible_records"] for chunk in chunks),
            "chunks": len(chunks),
            "algorithm_revision": identity.get("algorithm_revision"),
        }
        if identity != expected_identity:
            raise RunError("output directory belongs to a different exact run")
        algorithm_revision = identity["algorithm_revision"]
    completed: dict[int, tuple[Path, dict[str, Any]]] = {}
    pending: list[dict[str, int]] = []
    existing_paths = [
        output_dir / artifact_name(chunk)
        for chunk in chunks
        if (output_dir / artifact_name(chunk)).exists()
    ]
    for path in existing_paths:
        if os.path.samefile(path, corpus) or os.path.samefile(path, binary):
            raise RunError(f"chunk artifact aliases an input file: {path}")
    if not algorithm_revision and existing_paths:
        algorithm_revision = read_revision(existing_paths[0])
    for chunk in chunks:
        path = output_dir / artifact_name(chunk)
        if path.exists():
            completed[chunk["index"]] = (
                path,
                validate_artifact(path, chunk, records, algorithm_revision),
            )
        else:
            pending.append(chunk)

    if algorithm_revision and not identity_path.exists():
        atomic_json(
            identity_path,
            {
                "schema": RUN_SCHEMA,
                "corpus_bytes": len(corpus_data),
                "corpus_sha256": corpus_hash,
                "corpus_fnv1a64": f"{CORPUS_FNV1A64:016x}",
                "records": len(records),
                "binary_bytes": binary.stat().st_size,
                "binary_sha256": binary_hash,
                "eligible_per_chunk": args.eligible_per_chunk,
                "eligible_records": sum(
                    item["eligible_records"] for item in chunks
                ),
                "chunks": len(chunks),
                "algorithm_revision": algorithm_revision,
            },
        )

    if args.max_new_chunks is not None:
        pending = pending[: args.max_new_chunks]
    print(
        json.dumps(
            {
                "event": "resume",
                "chunks": len(chunks),
                "completed": len(completed),
                "selected_this_invocation": len(pending),
                "workers": args.workers,
            },
            sort_keys=True,
        ),
        flush=True,
    )

    started = time.monotonic()
    active = ActiveProcesses()
    unique_found = any(result["unique"] for _, result in completed.values())
    iterator = iter(pending)
    futures: dict[
        concurrent.futures.Future[tuple[Path, dict[str, Any]]], dict[str, int]
    ] = {}
    executor = concurrent.futures.ThreadPoolExecutor(max_workers=args.workers)
    try:
        if not unique_found:
            for _ in range(min(args.workers, len(pending))):
                chunk = next(iterator)
                future = executor.submit(
                    run_chunk,
                    binary,
                    corpus,
                    output_dir,
                    chunk,
                    records,
                    algorithm_revision,
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
                if not algorithm_revision:
                    algorithm_revision = result["algorithm_revision"]
                    result = validate_artifact(path, chunk, records, algorithm_revision)
                    identity = {
                        "schema": RUN_SCHEMA,
                        "corpus_bytes": len(corpus_data),
                        "corpus_sha256": corpus_hash,
                        "corpus_fnv1a64": f"{CORPUS_FNV1A64:016x}",
                        "records": len(records),
                        "binary_bytes": binary.stat().st_size,
                        "binary_sha256": binary_hash,
                        "eligible_per_chunk": args.eligible_per_chunk,
                        "eligible_records": sum(
                            item["eligible_records"] for item in chunks
                        ),
                        "chunks": len(chunks),
                        "algorithm_revision": algorithm_revision,
                    }
                    atomic_json(identity_path, identity)
                elif result["algorithm_revision"] != algorithm_revision:
                    raise RunError(
                        "completed chunk used a different scanner algorithm revision"
                    )
                completed[chunk["index"]] = (path, result)
                unique_found |= result["unique"]
                print(
                    json.dumps(
                        {
                            "event": "chunk-complete",
                            "chunk": chunk["index"],
                            "completed": len(completed),
                            "total": len(chunks),
                            "source_lines": [chunk["start_line"], chunk["end_line"]],
                            "eligible_records": chunk["eligible_records"],
                            "classified": result["summary"]["classified_units"],
                            "unique": result["summary"]["unique"],
                            "elapsed_seconds": round(time.monotonic() - started, 3),
                        },
                        sort_keys=True,
                    ),
                    flush=True,
                )
                if unique_found:
                    active.terminate_all()
                    break
                try:
                    next_chunk = next(iterator)
                except StopIteration:
                    pass
                else:
                    next_future = executor.submit(
                        run_chunk,
                        binary,
                        corpus,
                        output_dir,
                        next_chunk,
                        records,
                        algorithm_revision,
                        active,
                    )
                    futures[next_future] = next_chunk
            if unique_found:
                break
    except BaseException:
        active.terminate_all()
        raise
    finally:
        executor.shutdown(wait=True, cancel_futures=True)

    summary = aggregate(output_dir, chunks, records, algorithm_revision)
    summary.update(
        {
            "corpus_sha256": corpus_hash,
            "binary_sha256": binary_hash,
            "elapsed_seconds_this_invocation": round(time.monotonic() - started, 3),
            "stopped_after_unique": unique_found,
        }
    )
    atomic_json(summary_path, summary)
    print(json.dumps(summary, sort_keys=True), flush=True)
    exit_code = 3 if unique_found else (0 if summary["complete"] else 2)
    _run_lock.close()
    return exit_code


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RunError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(2) from error
