#!/usr/bin/env python3
"""Verify a ``thermo-two-cell-v1`` line certificate without a Sudoku solver.

The certificate is a witness certificate for the following narrow claim: every
legal, directed, disjoint two-cell thermometer that can be added to the stated
base layout has at least two solutions.  Two valid solution grids per edge are
enough to prove that claim, so verification needs no search.

Records labelled ``0`` or ``1`` are parsed and checked against the supplied
witness pool, but their upper bounds cannot be proved by this certificate
format.  Consequently, such a certificate is valid evidence but is not an
exclusion proof.  Use ``--require-exclusion`` when that distinction matters.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import unittest
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Iterable, Sequence


VERSION = "thermo-two-cell-v1"
CELL_COUNT = 81
GRID_SIDE = 9
_UNSIGNED_INTEGER = re.compile(r"(?:0|[1-9][0-9]*)\Z")


class CertificateError(ValueError):
    """Raised when a certificate is malformed or a claimed fact is false."""


@dataclass(frozen=True)
class ExtensionClaim:
    bulb: int
    tip: int
    label: str
    first_witness: int | None
    second_witness: int | None
    line_number: int


@dataclass(frozen=True)
class Certificate:
    givens: tuple[int, ...]
    thermometers: tuple[tuple[int, ...], ...]
    witness_complete: bool
    witnesses: dict[int, tuple[int, ...]]
    extensions: tuple[ExtensionClaim, ...]


@dataclass(frozen=True)
class VerificationReport:
    version: str
    base_covered_cells: int
    candidate_extensions: int
    witness_solutions: int
    multiple_extensions: int
    unproved_exact_extensions: int
    witness_complete: bool
    extension_coverage_complete: bool
    exclusion_proved: bool


def _error(line_number: int | None, message: str) -> CertificateError:
    prefix = "certificate" if line_number is None else f"line {line_number}"
    return CertificateError(f"{prefix}: {message}")


def _parse_uint(text: str, line_number: int, what: str) -> int:
    if not _UNSIGNED_INTEGER.fullmatch(text):
        raise _error(line_number, f"invalid {what}: {text!r}")
    return int(text)


def _parse_witness_reference(text: str, line_number: int) -> int | None:
    if text == "-":
        return None
    return _parse_uint(text, line_number, "witness reference")


def _parse_givens(text: str, line_number: int) -> tuple[int, ...]:
    if len(text) != CELL_COUNT:
        raise _error(
            line_number,
            f"base_givens has length {len(text)}; expected {CELL_COUNT}",
        )
    if not re.fullmatch(r"[0-9.]{81}", text):
        raise _error(line_number, "base_givens must contain only '.', '0', or '1'..'9'")
    return tuple(0 if char in ".0" else int(char) for char in text)


def _parse_solution(text: str, line_number: int) -> tuple[int, ...]:
    if len(text) != CELL_COUNT or not re.fullmatch(r"[1-9]{81}", text):
        raise _error(line_number, "witness grid must contain exactly 81 digits in '1'..'9'")
    return tuple(int(char) for char in text)


def _parse_thermometers(text: str, line_number: int) -> tuple[tuple[int, ...], ...]:
    if text == "":
        return ()
    paths: list[tuple[int, ...]] = []
    for path_index, encoded_path in enumerate(text.split("|")):
        if encoded_path == "":
            raise _error(line_number, f"thermometer {path_index} is empty")
        cells = tuple(
            _parse_uint(cell, line_number, f"cell in thermometer {path_index}")
            for cell in encoded_path.split(",")
        )
        paths.append(cells)
    return tuple(paths)


def parse_certificate(text: str) -> Certificate:
    """Parse the certificate section of complete CLI output.

    The Rust CLI prints timing and count records before the version marker.
    Those preamble records are deliberately ignored.  Once the marker is seen,
    unknown or duplicate records are rejected.
    """

    records = list(enumerate(text.splitlines(), start=1))
    marker_lines = [
        number
        for number, raw in records
        if raw.strip().startswith("certificate_version=")
    ]
    if not marker_lines:
        raise _error(None, "missing certificate_version record")
    if len(marker_lines) != 1:
        raise _error(None, "expected exactly one certificate_version record")

    marker_line = marker_lines[0]
    scalars: dict[str, tuple[str, int]] = {}
    witnesses: dict[int, tuple[int, ...]] = {}
    extensions: list[ExtensionClaim] = []
    extension_keys: set[tuple[int, int]] = set()

    for line_number, raw in records:
        if line_number < marker_line:
            continue
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        key, separator, value = line.partition("=")
        if not separator:
            raise _error(line_number, "expected a key=value record")

        if key in {"certificate_version", "base_givens", "base_thermos", "witness_complete"}:
            if key in scalars:
                raise _error(line_number, f"duplicate {key} record")
            scalars[key] = (value, line_number)
            continue

        if key == "witness":
            fields = value.split(",", maxsplit=1)
            if len(fields) != 2:
                raise _error(line_number, "witness must be INDEX,GRID")
            index = _parse_uint(fields[0], line_number, "witness index")
            if index in witnesses:
                raise _error(line_number, f"duplicate witness index {index}")
            witnesses[index] = _parse_solution(fields[1], line_number)
            continue

        if key == "extension":
            fields = value.split(",")
            if len(fields) != 5:
                raise _error(
                    line_number,
                    "extension must be BULB,TIP,LABEL,FIRST_WITNESS,SECOND_WITNESS",
                )
            bulb = _parse_uint(fields[0], line_number, "bulb cell")
            tip = _parse_uint(fields[1], line_number, "tip cell")
            label = fields[2]
            if label not in {"0", "1", "2+"}:
                raise _error(line_number, f"invalid extension label {label!r}")
            edge = (bulb, tip)
            if edge in extension_keys:
                raise _error(line_number, f"duplicate extension {bulb}->{tip}")
            extension_keys.add(edge)
            extensions.append(
                ExtensionClaim(
                    bulb=bulb,
                    tip=tip,
                    label=label,
                    first_witness=_parse_witness_reference(fields[3], line_number),
                    second_witness=_parse_witness_reference(fields[4], line_number),
                    line_number=line_number,
                )
            )
            continue

        raise _error(line_number, f"unknown certificate record {key!r}")

    for key in ("certificate_version", "base_givens", "base_thermos", "witness_complete"):
        if key not in scalars:
            raise _error(None, f"missing {key} record")

    version, version_line = scalars["certificate_version"]
    if version != VERSION:
        raise _error(version_line, f"unsupported certificate version {version!r}")

    complete_text, complete_line = scalars["witness_complete"]
    if complete_text not in {"true", "false"}:
        raise _error(complete_line, "witness_complete must be 'true' or 'false'")

    givens_text, givens_line = scalars["base_givens"]
    thermos_text, thermos_line = scalars["base_thermos"]
    return Certificate(
        givens=_parse_givens(givens_text, givens_line),
        thermometers=_parse_thermometers(thermos_text, thermos_line),
        witness_complete=complete_text == "true",
        witnesses=witnesses,
        extensions=tuple(extensions),
    )


def _king_adjacent(first: int, second: int) -> bool:
    first_row, first_column = divmod(first, GRID_SIDE)
    second_row, second_column = divmod(second, GRID_SIDE)
    return (
        first != second
        and abs(first_row - second_row) <= 1
        and abs(first_column - second_column) <= 1
    )


def _validate_geometry(paths: Sequence[Sequence[int]]) -> set[int]:
    occupied: set[int] = set()
    for path_index, path in enumerate(paths):
        if not 2 <= len(path) <= 9:
            raise _error(
                None,
                f"thermometer {path_index} has length {len(path)}; expected 2..9",
            )
        local: set[int] = set()
        for position, cell in enumerate(path):
            if not 0 <= cell < CELL_COUNT:
                raise _error(
                    None,
                    f"thermometer {path_index}, position {position}: cell {cell} is outside 0..80",
                )
            if cell in local:
                raise _error(None, f"thermometer {path_index} repeats cell {cell}")
            if cell in occupied:
                raise _error(None, f"cell {cell} occurs in multiple thermometers")
            local.add(cell)
        for first, second in zip(path, path[1:]):
            if not _king_adjacent(first, second):
                raise _error(
                    None,
                    f"thermometer {path_index} has non-adjacent step {first}->{second}",
                )
        occupied.update(local)
    return occupied


def _houses() -> Iterable[tuple[int, ...]]:
    for row in range(GRID_SIDE):
        yield tuple(row * GRID_SIDE + column for column in range(GRID_SIDE))
    for column in range(GRID_SIDE):
        yield tuple(row * GRID_SIDE + column for row in range(GRID_SIDE))
    for box_row in range(0, GRID_SIDE, 3):
        for box_column in range(0, GRID_SIDE, 3):
            yield tuple(
                (box_row + row) * GRID_SIDE + box_column + column
                for row in range(3)
                for column in range(3)
            )


def _validate_partial_grid(givens: Sequence[int]) -> None:
    for house_index, house in enumerate(_houses()):
        nonzero = [givens[cell] for cell in house if givens[cell] != 0]
        if len(nonzero) != len(set(nonzero)):
            raise _error(None, f"base_givens repeats a digit in house {house_index}")


def _validate_solution(
    witness_index: int,
    grid: Sequence[int],
    givens: Sequence[int],
    paths: Sequence[Sequence[int]],
) -> None:
    target = set(range(1, 10))
    for house_index, house in enumerate(_houses()):
        if {grid[cell] for cell in house} != target:
            raise _error(
                None,
                f"witness {witness_index} is not a classic Sudoku solution in house {house_index}",
            )
    for cell, given in enumerate(givens):
        if given and grid[cell] != given:
            raise _error(
                None,
                f"witness {witness_index} violates given {given} at cell {cell}",
            )
    for path_index, path in enumerate(paths):
        if any(grid[first] >= grid[second] for first, second in zip(path, path[1:])):
            raise _error(
                None,
                f"witness {witness_index} does not increase on thermometer {path_index}",
            )


def _expected_extensions(occupied: set[int]) -> set[tuple[int, int]]:
    uncovered = [cell for cell in range(CELL_COUNT) if cell not in occupied]
    return {
        (bulb, tip)
        for bulb in uncovered
        for tip in uncovered
        if _king_adjacent(bulb, tip)
    }


def verify_certificate(certificate: Certificate) -> VerificationReport:
    """Validate all independently checkable statements in a certificate."""

    occupied = _validate_geometry(certificate.thermometers)
    _validate_partial_grid(certificate.givens)

    # Given digits at two positions on one thermometer must respect their
    # transitive order even if unknown cells lie between them.
    for path_index, path in enumerate(certificate.thermometers):
        for earlier_position, earlier_cell in enumerate(path):
            earlier = certificate.givens[earlier_cell]
            if not earlier:
                continue
            for later_cell in path[earlier_position + 1 :]:
                later = certificate.givens[later_cell]
                if later and earlier >= later:
                    raise _error(
                        None,
                        f"base_givens contradict thermometer {path_index}: "
                        f"cell {earlier_cell} is not below cell {later_cell}",
                    )

    witness_indices = sorted(certificate.witnesses)
    if witness_indices != list(range(len(witness_indices))):
        raise _error(None, "witness indices must be contiguous from zero")
    witness_grids = list(certificate.witnesses.values())
    if len(witness_grids) != len(set(witness_grids)):
        raise _error(None, "witness indices must identify distinct solution grids")
    for index, grid in certificate.witnesses.items():
        _validate_solution(index, grid, certificate.givens, certificate.thermometers)

    expected = _expected_extensions(occupied)
    claimed = {(claim.bulb, claim.tip) for claim in certificate.extensions}
    missing = sorted(expected - claimed)
    extra = sorted(claimed - expected)
    if missing or extra:
        details: list[str] = []
        if missing:
            details.append(
                f"missing {len(missing)} legal extension(s), first {missing[0][0]}->{missing[0][1]}"
            )
        if extra:
            details.append(
                f"contains {len(extra)} illegal extension(s), first {extra[0][0]}->{extra[0][1]}"
            )
        raise _error(None, "; ".join(details))

    multiple_count = 0
    unproved_count = 0
    for claim in certificate.extensions:
        first = claim.first_witness
        second = claim.second_witness
        referenced = [reference for reference in (first, second) if reference is not None]
        for reference in referenced:
            if reference not in certificate.witnesses:
                raise _error(
                    claim.line_number,
                    f"extension {claim.bulb}->{claim.tip} references absent witness {reference}",
                )

        satisfying = {
            index
            for index, grid in certificate.witnesses.items()
            if grid[claim.bulb] < grid[claim.tip]
        }
        if claim.label == "0":
            unproved_count += 1
            if first is not None or second is not None:
                raise _error(claim.line_number, "a label-0 extension must use '-,-'")
            if satisfying:
                index = min(satisfying)
                raise _error(
                    claim.line_number,
                    f"label-0 claim is contradicted by supplied witness {index}",
                )
        elif claim.label == "1":
            unproved_count += 1
            if first is None or second is not None:
                raise _error(
                    claim.line_number,
                    "a label-1 extension must have exactly one witness reference",
                )
            if first not in satisfying:
                raise _error(
                    claim.line_number,
                    f"witness {first} does not satisfy {claim.bulb}<{claim.tip}",
                )
            contradictory = satisfying - {first}
            if contradictory:
                index = min(contradictory)
                raise _error(
                    claim.line_number,
                    f"label-1 claim is contradicted by supplied witness {index}",
                )
        else:
            multiple_count += 1
            if first is None or second is None:
                raise _error(
                    claim.line_number,
                    "a label-2+ extension requires two witness references",
                )
            if first == second:
                raise _error(
                    claim.line_number,
                    "a label-2+ extension requires distinct witness indices",
                )
            for reference in (first, second):
                if reference not in satisfying:
                    raise _error(
                        claim.line_number,
                        f"witness {reference} does not satisfy {claim.bulb}<{claim.tip}",
                    )

    derived_complete = unproved_count == 0
    if certificate.witness_complete != derived_complete:
        raise _error(
            None,
            "witness_complete disagrees with the extension records "
            f"(expected {'true' if derived_complete else 'false'})",
        )

    return VerificationReport(
        version=VERSION,
        base_covered_cells=len(occupied),
        candidate_extensions=len(expected),
        witness_solutions=len(certificate.witnesses),
        multiple_extensions=multiple_count,
        unproved_exact_extensions=unproved_count,
        witness_complete=certificate.witness_complete,
        extension_coverage_complete=True,
        exclusion_proved=derived_complete,
    )


def verify_text(text: str) -> VerificationReport:
    return verify_certificate(parse_certificate(text))


def _pattern_solution() -> tuple[int, ...]:
    return tuple((row * 3 + row // 3 + column) % 9 + 1 for row in range(9) for column in range(9))


def _transpose(grid: Sequence[int]) -> tuple[int, ...]:
    return tuple(grid[column * 9 + row] for row in range(9) for column in range(9))


def _complete_test_certificate() -> str:
    first = _pattern_solution()
    second = tuple(10 - digit for digit in first)
    third = _transpose(first)
    fourth = tuple(10 - digit for digit in third)
    grids = (first, second, third, fourth)
    lines = [
        "mode=screen-two-cell",
        f"certificate_version={VERSION}",
        f"base_givens={'0' * 81}",
        "base_thermos=",
        "witness_complete=true",
    ]
    for index, grid in enumerate(grids):
        lines.append(f"witness={index},{''.join(map(str, grid))}")
    for bulb, tip in sorted(_expected_extensions(set())):
        satisfying = [index for index, grid in enumerate(grids) if grid[bulb] < grid[tip]]
        if len(satisfying) < 2:  # pragma: no cover - guards the test fixture itself
            raise AssertionError(f"fixture lacks two witnesses for {bulb}->{tip}")
        lines.append(f"extension={bulb},{tip},2+,{satisfying[0]},{satisfying[1]}")
    return "\n".join(lines) + "\n"


class _VerifierSelfTests(unittest.TestCase):
    def test_complete_witness_certificate_proves_exclusion(self) -> None:
        report = verify_text(_complete_test_certificate())
        self.assertEqual(report.candidate_extensions, 544)
        self.assertEqual(report.multiple_extensions, 544)
        self.assertTrue(report.exclusion_proved)

    def test_incomplete_edge_coverage_is_rejected(self) -> None:
        lines = _complete_test_certificate().splitlines()
        with self.assertRaisesRegex(CertificateError, "missing 1 legal extension"):
            verify_text("\n".join(lines[:-1]))

    def test_same_witness_twice_is_rejected(self) -> None:
        text = _complete_test_certificate()
        text = re.sub(
            r"extension=(\d+),(\d+),2\+,(\d+),(\d+)",
            lambda match: (
                f"extension={match.group(1)},{match.group(2)},2+,"
                f"{match.group(3)},{match.group(3)}"
            ),
            text,
            count=1,
        )
        with self.assertRaisesRegex(CertificateError, "distinct witness indices"):
            verify_text(text)

    def test_invalid_sudoku_witness_is_rejected(self) -> None:
        text = _complete_test_certificate()
        text = re.sub(r"(witness=0,)\d", r"\g<1>9", text, count=1)
        with self.assertRaisesRegex(CertificateError, "not a classic Sudoku solution"):
            verify_text(text)

    def test_false_zero_claim_is_rejected_from_pool(self) -> None:
        text = _complete_test_certificate()
        text = re.sub(
            r"extension=(\d+),(\d+),2\+,(\d+),(\d+)",
            r"extension=\1,\2,0,-,-",
            text,
            count=1,
        ).replace("witness_complete=true", "witness_complete=false")
        with self.assertRaisesRegex(CertificateError, "label-0 claim is contradicted"):
            verify_text(text)

    def test_overlapping_base_thermometers_are_rejected(self) -> None:
        text = "\n".join(
            (
                f"certificate_version={VERSION}",
                f"base_givens={'0' * 81}",
                "base_thermos=0,1|1,2",
                "witness_complete=false",
            )
        )
        with self.assertRaisesRegex(CertificateError, "multiple thermometers"):
            verify_text(text)


def _run_self_tests() -> bool:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(_VerifierSelfTests)
    return unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful()


def _read_input(path: str) -> str:
    if path == "-":
        return sys.stdin.read()
    return Path(path).read_text(encoding="utf-8")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("certificate", nargs="?", help="certificate file, or '-' for stdin")
    parser.add_argument(
        "--require-exclusion",
        action="store_true",
        help="fail unless every legal extension has two independently checked witnesses",
    )
    parser.add_argument("--json", action="store_true", help="print the report as JSON")
    parser.add_argument("--self-test", action="store_true", help="run built-in verifier tests")
    args = parser.parse_args(argv)

    if args.self_test:
        if args.certificate is not None:
            parser.error("certificate cannot be combined with --self-test")
        return 0 if _run_self_tests() else 1
    if args.certificate is None:
        parser.error("certificate is required unless --self-test is used")

    try:
        report = verify_text(_read_input(args.certificate))
    except (CertificateError, OSError) as error:
        print(f"verification failed: {error}", file=sys.stderr)
        return 1

    if args.json:
        print(json.dumps(asdict(report), sort_keys=True))
    else:
        print("certificate_valid=true")
        for key, value in asdict(report).items():
            if key == "version":
                continue
            rendered = str(value).lower() if isinstance(value, bool) else value
            print(f"{key}={rendered}")

    if args.require_exclusion and not report.exclusion_proved:
        print(
            "verification failed: certificate contains unproved 0/1 classifications",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
