import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


MODULE_PATH = Path(__file__).with_name("run_17c_maximal_chunks.py")
SPEC = importlib.util.spec_from_file_location("run_17c_maximal_chunks", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
chunks = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(chunks)


class ChunkSchedulerTests(unittest.TestCase):
    def test_chunks_are_contiguous_and_cap_eligible_work(self) -> None:
        eligible = "12345678912345678" + "." * 64
        ineligible = "11111111111111111" + "." * 64
        records = [ineligible, eligible, ineligible, eligible, eligible, ineligible]
        result = chunks.make_chunks(records, paths=4, eligible_per_chunk=2)
        self.assertEqual(
            result,
            [
                {
                    "index": 1,
                    "start_line": 1,
                    "end_line": 4,
                    "eligible_records": 2,
                },
                {
                    "index": 2,
                    "start_line": 5,
                    "end_line": 6,
                    "eligible_records": 1,
                },
            ],
        )

    def test_partition_lists_match_the_rust_layer_counts(self) -> None:
        self.assertEqual(len(chunks.expected_partitions(4)), 16)
        self.assertEqual(len(chunks.expected_partitions(5)), 13)
        self.assertEqual(len(chunks.expected_partitions(6)), 7)
        self.assertEqual(len(chunks.expected_partitions(7)), 3)

    def synthetic_artifact(self, path: Path, chunk: dict[str, int]) -> None:
        records = [
            {
                "schema": chunks.SCANNER_SCHEMA,
                "min_paths": 7,
                "max_paths": 7,
                "start_line": chunk["start_line"],
                "end_line": chunk["end_line"],
                "max_eligible": None,
                "emit_multiples": False,
                "deduplicate_layouts": False,
                "witness_cache_limit": 0,
                "prerequisite": chunks.PREREQUISITE,
            }
        ]
        for partition in chunks.expected_partitions(7):
            summary = {"type": "partition-summary", "partition": partition}
            summary.update({key: 0 for key in chunks.PARTITION_FIELDS})
            records.append(summary)
        terminal = {
            "type": "summary",
            "complete": False,
            "records": chunks.CORPUS_RECORDS,
        }
        terminal.update({key: 0 for key in chunks.SUMMARY_FIELDS})
        terminal["eligible_records"] = chunk["eligible_records"]
        records.append(terminal)
        path.write_text(
            "".join(json.dumps(record) + "\n" for record in records),
            encoding="utf-8",
            newline="",
        )

    def test_complete_chunk_is_accepted_and_truncation_is_rejected(self) -> None:
        chunk = {
            "index": 1,
            "start_line": 10,
            "end_line": 20,
            "eligible_records": 0,
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / chunks.artifact_name(chunk)
            self.synthetic_artifact(path, chunk)
            result = chunks.validate_artifact(path, chunk, 7, chunks.CORPUS_RECORDS)
            self.assertEqual(result["terminal"]["unique_layouts"], 0)
            path.write_bytes(path.read_bytes()[:-1])
            with self.assertRaisesRegex(chunks.RunError, "truncated"):
                chunks.validate_artifact(path, chunk, 7, chunks.CORPUS_RECORDS)

    def test_unique_stop_is_preserved_as_an_unexhausted_witness_chunk(self) -> None:
        chunk = {
            "index": 1,
            "start_line": 10,
            "end_line": 20,
            "eligible_records": 4,
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / chunks.artifact_name(chunk)
            self.synthetic_artifact(path, {**chunk, "eligible_records": 1})
            records = [json.loads(line) for line in path.read_text().splitlines()]
            partition = records[1]
            for key in (
                "maximal_covers",
                "classified_layout_occurrences",
                "solver_calls",
                "unique_layouts",
            ):
                partition[key] = 1
            terminal = records[-1]
            for key in (
                "maximal_covers",
                "classified_layout_occurrences",
                "solver_calls",
                "unique_layouts",
            ):
                terminal[key] = 1
            records.insert(
                1,
                {
                    "type": "candidate",
                    "multiplicity": "unique",
                    "source_line": 10,
                    "source_puzzle": "ignored without corpus_records",
                    "partition": partition["partition"],
                },
            )
            path.write_text(
                "".join(json.dumps(record) + "\n" for record in records),
                encoding="utf-8",
                newline="",
            )
            result = chunks.validate_artifact(path, chunk, 7, chunks.CORPUS_RECORDS)
            self.assertFalse(result["exhausted"])
            self.assertEqual(result["terminal"]["unique_layouts"], 1)


if __name__ == "__main__":
    unittest.main()
