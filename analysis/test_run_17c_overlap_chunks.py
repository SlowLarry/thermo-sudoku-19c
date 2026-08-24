from __future__ import annotations

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest import mock


MODULE_PATH = Path(__file__).with_name("run_17c_overlap_chunks.py")
SPEC = importlib.util.spec_from_file_location("run_17c_overlap_chunks", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


def summary(**overrides):
    record = {key: 0 for key in RUNNER.COUNT_FIELDS}
    record.update(
        {
            "type": "summary",
            "schema": RUNNER.SCANNER_SCHEMA,
            "algorithm_revision": "test-revision",
            "fingerprint": "0123456789abcdef",
            "mode": "exact",
            "scope_exhausted": True,
            "exact_requested_range_complete": True,
            "limit_hit": False,
            "stopped_on_unique": False,
        }
    )
    record.update(overrides)
    return record


def artifact_records(chunk, records, *, revision="test-revision", classified=1):
    header = {
        "type": "header",
        "schema": RUNNER.SCANNER_SCHEMA,
        "algorithm_revision": revision,
        "fingerprint": "0123456789abcdef",
        "input_fnv1a64": f"{RUNNER.CORPUS_FNV1A64:016x}",
        "records": len(records),
        "expected_records": len(records),
        "expected_fnv1a64": f"{RUNNER.CORPUS_FNV1A64:016x}",
        "corpus_is_complete_assertion": True,
        "mode": "exact",
        "start_line": chunk["start_line"],
        "end_line": chunk["end_line"],
        "max_units": None,
        "solution_cap": 2,
        "emit_cases": False,
    }
    start = chunk["start_line"]
    end = chunk["end_line"]
    terminal = summary(
        algorithm_revision=revision,
        records_in_range=end - start + 1,
        records_missing_digits=sum(
            not RUNNER.eligible(record) for record in records[start - 1 : end]
        ),
        candidate_units=classified,
        classified_units=classified,
        multiple=classified,
        capped_counts=classified,
        observed_solution_count_sum=2 * classified,
    )
    return header, terminal


def write_complete_artifact(path, chunk, records, *, classified=1):
    path.write_text(
        "\n".join(
            json.dumps(record)
            for record in artifact_records(chunk, records, classified=classified)
        )
        + "\n",
        encoding="utf-8",
    )


class ChunkTests(unittest.TestCase):
    def test_output_directory_lock_is_exclusive_and_reacquirable(self):
        with tempfile.TemporaryDirectory() as directory:
            output_dir = Path(directory)
            first = RUNNER.OutputDirectoryLock.acquire(output_dir)
            with self.assertRaisesRegex(RUNNER.RunError, "another launcher"):
                RUNNER.OutputDirectoryLock.acquire(output_dir)
            first.close()

            second = RUNNER.OutputDirectoryLock.acquire(output_dir)
            second.close()

    def test_split_child_scanner_failure_uses_stable_task_label(self):
        class FailedProcess:
            returncode = 9

            def communicate(self):
                return "", "failed"

        child = {
            "part": 2,
            "start_line": 3,
            "end_line": 3,
            "eligible_records": 1,
        }
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with mock.patch.object(RUNNER.subprocess, "Popen", return_value=FailedProcess()):
                with self.assertRaisesRegex(
                    RUNNER.RunError, "chunk 7 part 2 scanner exit 9"
                ):
                    RUNNER.run_chunk(
                        root / "scanner.exe",
                        root / "corpus.txt",
                        root,
                        child,
                        ["123456789" + "." * 72] * 3,
                        "test-revision",
                        RUNNER.ActiveProcesses(),
                        "child.jsonl",
                        "chunk 7 part 2",
                    )

    def test_chunks_are_contiguous_and_count_eligible_records(self):
        records = ["123456789" + "." * 72, "1" * 81, "987654321" + "." * 72]
        chunks = RUNNER.make_chunks(records, 1)
        self.assertEqual(
            chunks,
            [
                {"index": 1, "start_line": 1, "end_line": 1, "eligible_records": 1},
                {"index": 2, "start_line": 2, "end_line": 3, "eligible_records": 1},
            ],
        )

    def test_valid_complete_artifact(self):
        records = ["123456789" + "." * 72]
        chunk = {"index": 1, "start_line": 1, "end_line": 1, "eligible_records": 1}
        header = {
            "type": "header",
            "schema": RUNNER.SCANNER_SCHEMA,
            "algorithm_revision": "test-revision",
            "fingerprint": "0123456789abcdef",
            "input_fnv1a64": f"{RUNNER.CORPUS_FNV1A64:016x}",
            "records": 1,
            "expected_records": 1,
            "expected_fnv1a64": f"{RUNNER.CORPUS_FNV1A64:016x}",
            "corpus_is_complete_assertion": True,
            "mode": "exact",
            "start_line": 1,
            "end_line": 1,
            "max_units": None,
            "solution_cap": 2,
            "emit_cases": False,
        }
        terminal = summary(
            records_in_range=1,
            candidate_units=3,
            classified_units=3,
            multiple=3,
            capped_counts=3,
            observed_solution_count_sum=6,
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "chunk.jsonl"
            path.write_text(
                json.dumps(header) + "\n" + json.dumps(terminal) + "\n",
                encoding="utf-8",
            )
            result = RUNNER.validate_artifact(
                path, chunk, records, "test-revision"
            )
        self.assertTrue(result["exhausted"])
        self.assertFalse(result["unique"])

    def test_unique_stop_requires_emitted_unique_case(self):
        records = ["123456789" + "." * 72]
        chunk = {"index": 1, "start_line": 1, "end_line": 1, "eligible_records": 1}
        header = {
            "type": "header",
            "schema": RUNNER.SCANNER_SCHEMA,
            "algorithm_revision": "test-revision",
            "fingerprint": "0123456789abcdef",
            "input_fnv1a64": f"{RUNNER.CORPUS_FNV1A64:016x}",
            "records": 1,
            "expected_records": 1,
            "expected_fnv1a64": f"{RUNNER.CORPUS_FNV1A64:016x}",
            "corpus_is_complete_assertion": True,
            "mode": "exact",
            "start_line": 1,
            "end_line": 1,
            "max_units": None,
            "solution_cap": 2,
            "emit_cases": False,
        }
        case = {
            "type": "case",
            "line": 1,
            "source": records[0],
            "multiplicity": "unique",
        }
        terminal = summary(
            scope_exhausted=False,
            exact_requested_range_complete=False,
            stopped_on_unique=True,
            records_in_range=1,
            candidate_units=1,
            classified_units=1,
            unique=1,
            exact_counts=1,
            observed_solution_count_sum=1,
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "chunk.jsonl"
            path.write_text(
                "\n".join(map(json.dumps, (header, case, terminal))) + "\n",
                encoding="utf-8",
            )
            result = RUNNER.validate_artifact(
                path, chunk, records, "test-revision"
            )
        self.assertTrue(result["unique"])
        self.assertFalse(result["exhausted"])

    def test_bad_accounting_is_rejected(self):
        records = ["123456789" + "." * 72]
        chunk = {"index": 1, "start_line": 1, "end_line": 1, "eligible_records": 1}
        header = {
            "type": "header",
            "schema": RUNNER.SCANNER_SCHEMA,
            "algorithm_revision": "test-revision",
            "fingerprint": "0123456789abcdef",
            "input_fnv1a64": f"{RUNNER.CORPUS_FNV1A64:016x}",
            "records": 1,
            "expected_records": 1,
            "expected_fnv1a64": f"{RUNNER.CORPUS_FNV1A64:016x}",
            "corpus_is_complete_assertion": True,
            "mode": "exact",
            "start_line": 1,
            "end_line": 1,
            "max_units": None,
            "solution_cap": 2,
            "emit_cases": False,
        }
        terminal = summary(records_in_range=1, classified_units=1)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "chunk.jsonl"
            path.write_text(
                json.dumps(header) + "\n" + json.dumps(terminal) + "\n",
                encoding="utf-8",
            )
            with self.assertRaises(RUNNER.RunError):
                RUNNER.validate_artifact(path, chunk, records, "test-revision")

    def test_split_children_are_contiguous_and_have_one_eligible_record(self):
        eligible = "123456789" + "." * 72
        ineligible = "1" * 81
        records = [ineligible, eligible, ineligible, ineligible, eligible, ineligible, eligible]
        parent = {
            "index": 7,
            "start_line": 1,
            "end_line": 7,
            "eligible_records": 3,
        }

        children = RUNNER.make_split_children(parent, records)

        self.assertEqual(
            children,
            [
                {
                    "part": 1,
                    "start_line": 1,
                    "end_line": 2,
                    "eligible_records": 1,
                },
                {
                    "part": 2,
                    "start_line": 3,
                    "end_line": 5,
                    "eligible_records": 1,
                },
                {
                    "part": 3,
                    "start_line": 6,
                    "end_line": 7,
                    "eligible_records": 1,
                },
            ],
        )
        broken = [dict(child) for child in children]
        broken[1]["start_line"] += 1
        with self.assertRaisesRegex(RUNNER.RunError, "gap or overlap"):
            RUNNER.validate_split_partition(parent, broken, records)

    def test_split_manifest_is_exact_and_revision_bound(self):
        eligible = "123456789" + "." * 72
        records = [eligible, "1" * 81, eligible]
        parent = {
            "index": 4,
            "start_line": 1,
            "end_line": 3,
            "eligible_records": 2,
        }
        children = RUNNER.make_split_children(parent, records)
        value = RUNNER.split_manifest_value(
            parent,
            children,
            "a" * 64,
            "b" * 64,
            16,
            "test-revision",
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / RUNNER.split_manifest_name(parent)
            RUNNER.atomic_json(path, value)
            self.assertEqual(
                RUNNER.validate_split_manifest(
                    path,
                    parent,
                    children,
                    "a" * 64,
                    "b" * 64,
                    16,
                    "test-revision",
                ),
                "test-revision",
            )
            value["children"][0]["end_line"] += 1
            RUNNER.atomic_json(path, value)
            with self.assertRaisesRegex(RUNNER.RunError, "exact parent partition"):
                RUNNER.validate_split_manifest(
                    path,
                    parent,
                    children,
                    "a" * 64,
                    "b" * 64,
                    16,
                    "test-revision",
                )

    def test_hierarchical_aggregate_accepts_complete_child_partition(self):
        eligible = "123456789" + "." * 72
        records = ["1" * 81, eligible, "1" * 81, "1" * 81, eligible]
        parent = {
            "index": 1,
            "start_line": 1,
            "end_line": 5,
            "eligible_records": 2,
        }
        children = RUNNER.make_split_children(parent, records)
        with tempfile.TemporaryDirectory() as directory:
            output_dir = Path(directory)
            for child in children:
                write_complete_artifact(
                    output_dir / RUNNER.split_artifact_name(parent, child),
                    child,
                    records,
                )

            result = RUNNER.aggregate(
                output_dir,
                [parent],
                records,
                "test-revision",
                {1: children},
            )

        self.assertTrue(result["complete"])
        self.assertEqual(result["completed_chunks"], 1)
        self.assertEqual(result["completed_artifacts"], 2)
        self.assertEqual(result["totals"]["multiple"], 2)

    def test_hierarchical_aggregate_rejects_parent_even_before_children_exist(self):
        eligible = "123456789" + "." * 72
        records = [eligible, eligible]
        parent = {
            "index": 1,
            "start_line": 1,
            "end_line": 2,
            "eligible_records": 2,
        }
        children = RUNNER.make_split_children(parent, records)
        with tempfile.TemporaryDirectory() as directory:
            output_dir = Path(directory)
            write_complete_artifact(
                output_dir / RUNNER.artifact_name(parent), parent, records
            )
            with self.assertRaisesRegex(RUNNER.RunError, "committed as split"):
                RUNNER.aggregate(
                    output_dir,
                    [parent],
                    records,
                    "test-revision",
                    {1: children},
                )

    def test_hierarchical_aggregate_keeps_incomplete_children_incomplete(self):
        eligible = "123456789" + "." * 72
        records = [eligible, "1" * 81, eligible]
        parent = {
            "index": 1,
            "start_line": 1,
            "end_line": 3,
            "eligible_records": 2,
        }
        children = RUNNER.make_split_children(parent, records)
        with tempfile.TemporaryDirectory() as directory:
            output_dir = Path(directory)
            write_complete_artifact(
                output_dir / RUNNER.split_artifact_name(parent, children[0]),
                children[0],
                records,
            )
            result = RUNNER.aggregate(
                output_dir,
                [parent],
                records,
                "test-revision",
                {1: children},
            )

        self.assertFalse(result["complete"])
        self.assertEqual(result["completed_chunks"], 0)
        self.assertEqual(result["completed_artifacts"], 1)

    def test_bad_bootstrap_parent_cannot_commit_identity_or_split_manifest(self):
        eligible = "123456789" + "." * 72
        records = [eligible] * 4
        parent = {
            "index": 1,
            "start_line": 1,
            "end_line": 2,
            "eligible_records": 2,
        }
        header, _ = artifact_records(parent, records)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            corpus = root / "corpus.txt"
            binary = root / "scanner.exe"
            output_dir = root / "run"
            output_dir.mkdir()
            corpus.write_bytes(b"fixture")
            binary.write_bytes(b"scanner")
            (output_dir / RUNNER.artifact_name(parent)).write_text(
                json.dumps(header) + "\n", encoding="utf-8"
            )
            arguments = [
                "--corpus",
                str(corpus),
                "--binary",
                str(binary),
                "--output-dir",
                str(output_dir),
                "--workers",
                "1",
                "--eligible-per-chunk",
                "2",
                "--split-chunk",
                "2",
            ]
            with mock.patch.object(
                RUNNER, "parse_corpus", return_value=(b"fixture", records)
            ):
                with self.assertRaisesRegex(RUNNER.RunError, "missing header or summary"):
                    RUNNER.main(arguments)

            self.assertFalse((output_dir / "run-identity.json").exists())
            self.assertFalse((output_dir / "split-chunk-00002.json").exists())

    def test_main_resumes_persisted_split_with_one_split_worker_lane(self):
        eligible = "123456789" + "." * 72
        records = [eligible] * 6
        lane_lock = threading.Lock()
        active_split = 0
        maximum_active_split = 0

        def fake_run_chunk(
            binary,
            corpus,
            output_dir,
            chunk,
            supplied_records,
            algorithm_revision,
            active,
            final_name=None,
            task_label=None,
        ):
            del binary, corpus, active, task_label
            nonlocal active_split, maximum_active_split
            is_split = "-part-" in final_name
            if is_split:
                with lane_lock:
                    active_split += 1
                    maximum_active_split = max(maximum_active_split, active_split)
            try:
                time.sleep(0.01)
                path = output_dir / final_name
                write_complete_artifact(path, chunk, supplied_records)
                result = RUNNER.validate_artifact(
                    path,
                    chunk,
                    supplied_records,
                    algorithm_revision or "test-revision",
                )
                return path, result
            finally:
                if is_split:
                    with lane_lock:
                        active_split -= 1

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            corpus = root / "corpus.txt"
            binary = root / "scanner.exe"
            output_dir = root / "run"
            corpus.write_bytes(b"fixture")
            binary.write_bytes(b"scanner")
            common = [
                "--corpus",
                str(corpus),
                "--binary",
                str(binary),
                "--output-dir",
                str(output_dir),
                "--workers",
                "2",
                "--eligible-per-chunk",
                "2",
            ]
            with (
                mock.patch.object(
                    RUNNER, "parse_corpus", return_value=(b"fixture", records)
                ),
                mock.patch.object(RUNNER, "run_chunk", side_effect=fake_run_chunk),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                self.assertEqual(RUNNER.main(common + ["--max-new-chunks", "1"]), 2)
                self.assertEqual(
                    RUNNER.main(
                        common
                        + [
                            "--split-chunk",
                            "2",
                            "--split-workers",
                            "1",
                        ]
                    ),
                    0,
                )
                # The committed manifest is enough to resume the hierarchy;
                # the explicit CLI split need not be repeated.
                self.assertEqual(RUNNER.main(common), 0)
                orphan = (
                    output_dir
                    / "chunk-00001-part-0001-lines-00001-00001.jsonl"
                )
                orphan.write_text("{}\n", encoding="utf-8")
                with self.assertRaisesRegex(RUNNER.RunError, "orphan split artifact"):
                    RUNNER.main(common)

            manifest = json.loads(
                (output_dir / "split-chunk-00002.json").read_text(encoding="utf-8")
            )
            terminal = json.loads(
                (output_dir / "summary.json").read_text(encoding="utf-8")
            )

        self.assertEqual(manifest["algorithm_revision"], "test-revision")
        self.assertEqual(maximum_active_split, 1)
        self.assertTrue(terminal["complete"])
        self.assertEqual(terminal["completed_chunks"], 3)
        self.assertEqual(terminal["split_parent_chunks"], [2])
        self.assertIn("split_manifest_set_sha256", terminal)


if __name__ == "__main__":
    unittest.main()
