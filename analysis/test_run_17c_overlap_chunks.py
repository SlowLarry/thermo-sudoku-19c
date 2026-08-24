from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


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


if __name__ == "__main__":
    unittest.main()
