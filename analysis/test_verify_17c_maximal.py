from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from analysis import verify_17c_maximal as verifier


PUZZLE = "12345678912345678" + "." * 64
SOLUTION = (
    "123456789"
    "456789123"
    "789123456"
    "234567891"
    "567891234"
    "891234567"
    "345678912"
    "678912345"
    "912345678"
)


def zero_partition_summary(partition: str) -> dict[str, object]:
    return {
        "type": "partition-summary",
        "partition": partition,
        "maximal_covers": 0,
        "within_source_duplicates": 0,
        "classified_layout_occurrences": 0,
        "cut_screened_layouts": 0,
        "solver_calls": 0,
        "multiple_layouts": 0,
        "unique_layouts": 0,
    }


def terminal(records: int, eligible: int, complete: bool) -> dict[str, object]:
    result: dict[str, object] = {
        "type": "summary",
        "complete": complete,
        "records": records,
    }
    result.update({key: 0 for key in verifier.TERMINAL_COUNTER_FIELDS})
    result["eligible_records"] = eligible
    return result


def write_summary_artifact(
    path: Path,
    start_line: int,
    end_line: int,
    records: int,
    *,
    emit_multiples: bool = False,
) -> None:
    header = {
        "schema": verifier.SCHEMA,
        "min_paths": 4,
        "max_paths": 7,
        "start_line": start_line,
        "end_line": end_line,
        "max_eligible": None,
        "emit_multiples": emit_multiples,
        "deduplicate_layouts": False,
        "witness_cache_limit": 0,
        "prerequisite": verifier.PREREQUISITE,
    }
    records_to_write = [header]
    records_to_write.extend(
        zero_partition_summary(partition)
        for partition in verifier.expected_partitions(4, 7)
    )
    records_to_write.append(terminal(records, end_line - start_line + 1, False))
    path.write_text(
        "".join(json.dumps(record) + "\n" for record in records_to_write),
        encoding="utf-8",
    )


class SummaryOnlyVerifierTests(unittest.TestCase):
    def test_expected_k4_to_k7_partitions(self) -> None:
        partitions = verifier.expected_partitions(4, 7)
        self.assertEqual(39, len(partitions))
        self.assertEqual("5+2+2+2+2+2+2", partitions[0])
        self.assertEqual("5+4+4+4", partitions[-1])
        self.assertEqual(len(partitions), len(set(partitions)))

    def test_two_partial_shards_form_complete_summary_only_coverage(self) -> None:
        with tempfile.TemporaryDirectory() as directory_name:
            directory = Path(directory_name)
            corpus = directory / "corpus.txt"
            corpus.write_text(PUZZLE + "\n" + PUZZLE + "\n", encoding="ascii")
            first = directory / "first.jsonl"
            second = directory / "second.jsonl"
            write_summary_artifact(first, 1, 1, 2)
            write_summary_artifact(second, 2, 2, 2)

            with patch.object(verifier, "CORPUS_RECORDS", 2):
                result = verifier.verify_artifacts(corpus, [first, second], 4, 7, True)

            self.assertEqual("valid", result["status"])
            self.assertTrue(result["coverage_complete"])
            self.assertEqual(2, result["eligible_records"])
            self.assertEqual(39, result["expected_partitions_per_shard"])
            self.assertEqual(64, len(result["artifacts"][0]["sha256"]))

    def test_summary_only_rejects_emit_multiples(self) -> None:
        with tempfile.TemporaryDirectory() as directory_name:
            directory = Path(directory_name)
            corpus = directory / "corpus.txt"
            corpus.write_text(PUZZLE + "\n", encoding="ascii")
            artifact = directory / "artifact.jsonl"
            write_summary_artifact(artifact, 1, 1, 1, emit_multiples=True)

            with patch.object(verifier, "CORPUS_RECORDS", 1):
                with self.assertRaisesRegex(ValueError, "emit_multiples must be false"):
                    verifier.verify_artifacts(corpus, [artifact], 4, 7, True)

    def test_unique_candidate_gets_full_structural_witness_check(self) -> None:
        source_cells = ["."] * 81
        paths = [
            [0, 1, 2, 3, 4],
            [5, 6, 7, 8],
            [9, 10, 11, 12],
            [21, 22, 23, 24],
        ]
        for cell in (cell for path in paths for cell in path):
            source_cells[cell] = SOLUTION[cell]
        source = "".join(source_cells)
        candidate = {
            "type": "candidate",
            "classification_source": "solver",
            "witness_cut_id": None,
            "multiplicity": "unique",
            "count": 1,
            "capped": False,
            "source_line": 1,
            "source_puzzle": source,
            "partition": "5+4+4+4",
            "row_morph": 0,
            "column_morph": 0,
            "digit_order": "1,2,3,4,5,6,7,8,9",
            "paths": "0,1,2,3,4|5,6,7,8|9,10,11,12|21,22,23,24",
            "first_solution": SOLUTION,
            "second_solution": None,
        }

        self.assertEqual(
            "unique",
            verifier.validate_candidate(
                candidate, [source], verifier.axis_morphs(), 1, 1
            ),
        )
        candidate["first_solution"] = SOLUTION.translate(str.maketrans("12", "21"))
        with self.assertRaisesRegex(
            ValueError, "invalid row|violates a thermometer|does not realize"
        ):
            verifier.validate_candidate(candidate, [source], verifier.axis_morphs(), 1, 1)


if __name__ == "__main__":
    unittest.main()
