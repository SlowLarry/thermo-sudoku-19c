#!/usr/bin/env python3
"""Independent structural verifier for thermo-global-cegis checkpoints."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

HEADER = "# thermo-global-cegis-v1"
FNV_OFFSET = 0xCBF29CE484222325
FNV_PRIME = 0x100000001B3
MASK64 = (1 << 64) - 1


class VerificationError(ValueError):
    pass


def fnv_byte(value: int, checksum: int) -> int:
    return ((checksum ^ value) * FNV_PRIME) & MASK64


def parse_grid(text: str, context: str) -> tuple[int, ...]:
    if len(text) != 81 or any(character not in "123456789" for character in text):
        raise VerificationError(f"{context}: expected exactly 81 digits 1-9")
    grid = tuple(ord(character) - ord("0") for character in text)
    expected = set(range(1, 10))
    for row in range(9):
        if set(grid[row * 9 : row * 9 + 9]) != expected:
            raise VerificationError(f"{context}: row {row + 1} is not 1-9")
    for column in range(9):
        if {grid[row * 9 + column] for row in range(9)} != expected:
            raise VerificationError(f"{context}: column {column + 1} is not 1-9")
    for box_row in range(3):
        for box_column in range(3):
            cells = {
                grid[(box_row * 3 + dr) * 9 + box_column * 3 + dc]
                for dr in range(3)
                for dc in range(3)
            }
            if cells != expected:
                raise VerificationError(
                    f"{context}: box ({box_row + 1},{box_column + 1}) is not 1-9"
                )
    return grid


def verify(path: Path, required_budget: int | None) -> dict[str, object]:
    try:
        handle = path.open("r", encoding="ascii", newline=None)
    except OSError as error:
        raise VerificationError(f"cannot read {path}: {error}") from error

    with handle:
        first = handle.readline().rstrip("\r\n")
        if first != HEADER:
            raise VerificationError("wrong or missing schema header")

        metadata: dict[str, str] = {}
        pairs = 0
        checksum = FNV_OFFSET
        seen: set[str] = set()
        footer: tuple[int, int] | None = None
        data_started = False

        for line_number, raw_line in enumerate(handle, start=2):
            line = raw_line.rstrip("\r\n")
            if not line:
                raise VerificationError(f"line {line_number}: blank lines are not allowed")
            if line.startswith("# end pairs="):
                if footer is not None:
                    raise VerificationError(f"line {line_number}: duplicate footer")
                suffix = line.removeprefix("# end pairs=")
                try:
                    count_text, hash_text = suffix.split(" fnv1a64=", 1)
                    footer = (int(count_text), int(hash_text, 16))
                except (ValueError, TypeError) as error:
                    raise VerificationError(f"line {line_number}: malformed footer") from error
                continue
            if footer is not None:
                raise VerificationError(f"line {line_number}: data after footer")
            if line.startswith("# "):
                if data_started:
                    raise VerificationError(f"line {line_number}: metadata after pair data")
                if "=" not in line:
                    raise VerificationError(f"line {line_number}: malformed metadata")
                key, value = line[2:].split("=", 1)
                if key in metadata:
                    raise VerificationError(f"line {line_number}: duplicate metadata {key!r}")
                metadata[key] = value
                continue

            data_started = True
            try:
                first_text, second_text = line.split("|", 1)
            except ValueError as error:
                raise VerificationError(f"line {line_number}: missing pair separator") from error
            if "|" in second_text:
                raise VerificationError(f"line {line_number}: too many pair separators")
            first_grid = parse_grid(first_text, f"line {line_number} first grid")
            second_grid = parse_grid(second_text, f"line {line_number} second grid")
            if first_text >= second_text:
                raise VerificationError(
                    f"line {line_number}: grids are not distinct and canonically ordered"
                )
            if line in seen:
                raise VerificationError(f"line {line_number}: duplicate grid pair")
            seen.add(line)
            for digit in first_grid:
                checksum = fnv_byte(digit, checksum)
            checksum = fnv_byte(0xFE, checksum)
            for digit in second_grid:
                checksum = fnv_byte(digit, checksum)
            checksum = fnv_byte(0xFF, checksum)
            pairs += 1

    expected_keys = {"budget", "directed_edges", "pairs", "fnv1a64"}
    if set(metadata) != expected_keys:
        raise VerificationError(
            f"metadata keys are {sorted(metadata)}, expected {sorted(expected_keys)}"
        )
    try:
        budget = int(metadata["budget"])
        directed_edges = int(metadata["directed_edges"])
        declared_pairs = int(metadata["pairs"])
        declared_checksum = int(metadata["fnv1a64"], 16)
    except ValueError as error:
        raise VerificationError("metadata contains a malformed number") from error
    if required_budget is not None and budget != required_budget:
        raise VerificationError(f"budget is {budget}, expected {required_budget}")
    if directed_edges != 544:
        raise VerificationError(f"directed edge count is {directed_edges}, expected 544")
    if declared_pairs != pairs:
        raise VerificationError(f"declared {declared_pairs} pairs, found {pairs}")
    if declared_checksum != checksum:
        raise VerificationError(
            f"declared checksum {declared_checksum:016x}, computed {checksum:016x}"
        )
    if footer != (pairs, checksum):
        raise VerificationError(
            f"footer is {footer!r}, expected ({pairs}, {checksum:016x})"
        )
    return {
        "valid": True,
        "path": str(path.resolve()),
        "budget": budget,
        "directed_edges": directed_edges,
        "pairs": pairs,
        "fnv1a64": f"{checksum:016x}",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("checkpoint", type=Path)
    parser.add_argument("--budget", type=int, default=16)
    parser.add_argument("--json", action="store_true")
    arguments = parser.parse_args()
    try:
        result = verify(arguments.checkpoint, arguments.budget)
    except VerificationError as error:
        if arguments.json:
            print(json.dumps({"valid": False, "error": str(error)}))
        else:
            print(f"INVALID: {error}", file=sys.stderr)
        return 1
    if arguments.json:
        print(json.dumps(result, indent=2))
    else:
        print(
            f"valid checkpoint: pairs={result['pairs']} budget={result['budget']} "
            f"fnv1a64={result['fnv1a64']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
