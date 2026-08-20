#!/usr/bin/env python3
"""Adversarial tests for verify_topology_active_cnf.py."""

from __future__ import annotations

import shutil
import sys
import tempfile
from pathlib import Path
import unittest


sys.path.insert(0, str(Path(__file__).resolve().parent))
import verify_topology_active_cnf as verifier  # noqa: E402


CANONICAL = b"123456789456789123789123456214365897365897214897214365531642978642978531978531642"


def swap_symbols(grid: bytes, left: int, right: int) -> bytes:
    result = bytearray(grid)
    left_byte = ord("0") + left
    right_byte = ord("0") + right
    for index, value in enumerate(result):
        if value == left_byte:
            result[index] = right_byte
        elif value == right_byte:
            result[index] = left_byte
    return bytes(result)


def canonical_pair(first: bytes, second: bytes) -> tuple[bytes, bytes]:
    return (first, second) if first < second else (second, first)


PAIR_ONE = canonical_pair(CANONICAL, swap_symbols(CANONICAL, 1, 2))
PAIR_TWO = canonical_pair(CANONICAL, swap_symbols(CANONICAL, 3, 4))


def pairs_checksum(pairs: list[tuple[bytes, bytes]]) -> int:
    checksum = verifier.FNV_OFFSET
    for first, second in pairs:
        checksum = verifier._fnv_pair(checksum, first, second)
    return checksum


def unique_cut_witnesses(
    pairs: list[tuple[bytes, bytes]],
) -> list[tuple[bytes, bytes]]:
    seen: set[int] = set()
    witnesses: list[tuple[bytes, bytes]] = []
    for pair in pairs:
        cut = verifier.pair_cut(*pair)
        if cut not in seen:
            seen.add(cut)
            witnesses.append(pair)
    return witnesses


def write_checkpoint(path: Path, pairs: list[tuple[bytes, bytes]]) -> None:
    checksum = pairs_checksum(pairs)
    lines = [
        verifier.CHECKPOINT_HEADER,
        b"# budget=16",
        f"# directed_edges={verifier.DIRECTED_EDGE_COUNT}".encode("ascii"),
        f"# pairs={len(pairs)}".encode("ascii"),
        f"# fnv1a64={checksum:016x}".encode("ascii"),
        *(first + b"|" + second for first, second in pairs),
        f"# end pairs={len(pairs)} fnv1a64={checksum:016x}".encode("ascii"),
    ]
    path.write_bytes(b"\n".join(lines) + b"\n")


def active_checksum(records: list[tuple[int, bytes, bytes]]) -> int:
    checksum = verifier.FNV_OFFSET
    for index, first, second in records:
        for byte in index.to_bytes(8, "little"):
            checksum = verifier.fnv_byte(checksum, byte)
        checksum = verifier._fnv_pair(checksum, first, second)
    return checksum


def write_manifest(
    path: Path,
    checkpoint_pairs: list[tuple[bytes, bytes]],
    pool_pairs: int,
    records: list[tuple[int, bytes, bytes]],
    *,
    pool_unique_cuts: int | None = None,
    symmetry: str = verifier.SYMMETRY_NONE,
) -> None:
    prefix = checkpoint_pairs[:pool_pairs]
    prefix_cuts = unique_cut_witnesses(prefix)
    declared_pool_cuts = len(prefix_cuts) if pool_unique_cuts is None else pool_unique_cuts
    prefix_checksum = pairs_checksum(prefix)
    checksum = active_checksum(records)
    lines = [
        verifier.ACTIVE_CUTS_HEADER,
        f"# cnf_schema={verifier.CNF_SCHEMA}".encode("ascii"),
        f"# symmetry_break={symmetry}".encode("ascii"),
        f"# edge_order_fnv1a64={verifier.EDGE_CHECKSUM:016x}".encode("ascii"),
        f"# directed_edges={verifier.DIRECTED_EDGE_COUNT}".encode("ascii"),
        f"# pool_pairs={pool_pairs}".encode("ascii"),
        f"# pool_unique_cuts={declared_pool_cuts}".encode("ascii"),
        f"# pool_fnv1a64={prefix_checksum:016x}".encode("ascii"),
        f"# active_cuts={len(records)}".encode("ascii"),
        f"# fnv1a64={checksum:016x}".encode("ascii"),
        *(f"{index}|".encode("ascii") + first + b"|" + second for index, first, second in records),
        f"# end active_cuts={len(records)} fnv1a64={checksum:016x}".encode("ascii"),
    ]
    path.write_bytes(b"\n".join(lines) + b"\n")


class ArtifactVerifierTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.temporary = tempfile.TemporaryDirectory(prefix="topology-verifier-")
        cls.root = Path(cls.temporary.name)
        cls.pairs = [PAIR_ONE, PAIR_TWO]
        cls.checkpoint = cls.root / "valid.checkpoint"
        cls.manifest = cls.root / "valid.active"
        cls.cnf = cls.root / "valid.cnf"
        write_checkpoint(cls.checkpoint, cls.pairs)
        witnesses = unique_cut_witnesses(cls.pairs)
        if len(witnesses) != 2:
            raise AssertionError("test pairs unexpectedly have duplicate cuts")
        write_manifest(cls.manifest, cls.pairs, 1, [(0, *witnesses[0])])
        parsed_manifest = verifier.parse_active_manifest(cls.manifest)
        audited_checkpoint = verifier.audit_checkpoint(cls.checkpoint, parsed_manifest)
        with cls.cnf.open("wb") as handle:
            handle.writelines(verifier.expected_cnf_lines(audited_checkpoint, parsed_manifest))

    @classmethod
    def tearDownClass(cls) -> None:
        cls.temporary.cleanup()

    def copy(self, source: Path, name: str) -> Path:
        destination = self.root / name
        shutil.copyfile(source, destination)
        return destination

    def test_valid_descendant_checkpoint_and_exact_cnf(self) -> None:
        result = verifier.verify_artifacts(self.checkpoint, self.manifest, self.cnf)
        self.assertTrue(result["valid"])
        self.assertEqual(result["checkpoint"]["pairs"], 2)
        self.assertEqual(result["manifest"]["pool_prefix_pairs"], 1)
        self.assertTrue(result["cnf"]["exact_independent_reemission_match"])

    def test_checkpoint_checksum_corruption_is_rejected(self) -> None:
        path = self.copy(self.checkpoint, "bad-checksum.checkpoint")
        contents = path.read_bytes()
        marker = b"# fnv1a64="
        position = contents.index(marker) + len(marker)
        replacement = b"0" if contents[position : position + 1] != b"0" else b"1"
        contents = contents[:position] + replacement + contents[position + 1 :]
        path.write_bytes(contents)
        with self.assertRaisesRegex(verifier.VerificationError, "checkpoint checksum"):
            verifier.verify_artifacts(path, self.manifest, self.cnf)

    def test_duplicate_pair_is_rejected_even_with_fresh_metadata(self) -> None:
        path = self.root / "duplicate.checkpoint"
        write_checkpoint(path, [PAIR_ONE, PAIR_ONE])
        with self.assertRaisesRegex(verifier.VerificationError, "duplicate grid pair"):
            verifier.verify_artifacts(path, self.manifest, self.cnf)

    def test_invalid_sudoku_is_rejected_before_metadata_can_authorize_it(self) -> None:
        path = self.copy(self.checkpoint, "invalid-grid.checkpoint")
        invalid = bytearray(PAIR_ONE[1])
        invalid[1] = invalid[2]
        path.write_bytes(path.read_bytes().replace(PAIR_ONE[1], bytes(invalid), 1))
        with self.assertRaisesRegex(verifier.VerificationError, "invalid solved grid"):
            verifier.verify_artifacts(path, self.manifest, self.cnf)

    def test_wrong_first_cut_witness_is_rejected_with_valid_manifest_hash(self) -> None:
        path = self.root / "wrong-witness.active"
        write_manifest(path, self.pairs, 2, [(0, *PAIR_TWO)])
        with self.assertRaisesRegex(verifier.VerificationError, "not its first checkpoint pair"):
            verifier.verify_artifacts(self.checkpoint, path, self.cnf)

    def test_duplicate_active_index_is_rejected(self) -> None:
        path = self.root / "duplicate-index.active"
        write_manifest(path, self.pairs, 1, [(0, *PAIR_ONE), (0, *PAIR_ONE)])
        with self.assertRaisesRegex(verifier.VerificationError, "duplicate active cut index"):
            verifier.verify_artifacts(self.checkpoint, path, self.cnf)

    def test_manifest_checksum_corruption_is_rejected(self) -> None:
        path = self.copy(self.manifest, "bad-active-checksum.active")
        contents = path.read_bytes()
        marker = b"# fnv1a64="
        position = contents.index(marker) + len(marker)
        replacement = b"0" if contents[position : position + 1] != b"0" else b"1"
        path.write_bytes(contents[:position] + replacement + contents[position + 1 :])
        with self.assertRaisesRegex(verifier.VerificationError, "manifest active checksum"):
            verifier.verify_artifacts(self.checkpoint, path, self.cnf)

    def test_wrong_prefix_cut_count_is_rejected(self) -> None:
        path = self.root / "bad-prefix.active"
        write_manifest(path, self.pairs, 1, [(0, *PAIR_ONE)], pool_unique_cuts=2)
        with self.assertRaisesRegex(verifier.VerificationError, "prefix declares 2 unique cuts"):
            verifier.verify_artifacts(self.checkpoint, path, self.cnf)

    def test_cnf_clause_tampering_is_rejected(self) -> None:
        path = self.copy(self.cnf, "tampered.cnf")
        contents = path.read_bytes()
        final_newline = contents.rfind(b"\n", 0, len(contents) - 1)
        path.write_bytes(contents[: final_newline + 1] + b"1 0\n")
        with self.assertRaisesRegex(verifier.VerificationError, "CNF mismatch"):
            verifier.verify_artifacts(self.checkpoint, self.manifest, path)

    def test_explicit_symmetry_expectation_is_enforced(self) -> None:
        with self.assertRaisesRegex(verifier.VerificationError, "symmetry mode"):
            verifier.verify_artifacts(
                self.checkpoint,
                self.manifest,
                self.cnf,
                expected_symmetry=verifier.SYMMETRY_D4_COMPLEMENT_V1,
            )


if __name__ == "__main__":
    unittest.main()
