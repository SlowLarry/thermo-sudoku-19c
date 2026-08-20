#!/usr/bin/env python3
"""Independently verify a lazy thermo-topology proof-formula artifact set.

This verifier does not invoke or import the Rust exporter.  It validates the
complete solution-pair checkpoint, binds an active-cut manifest to an
append-only checkpoint prefix, reconstructs the directed-edge/cut ordering,
and compares the complete emitted DIMACS file byte-for-byte with an
independently generated base-plus-active formula.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from functools import lru_cache
import hashlib
import json
from pathlib import Path
import re
import sys
from typing import BinaryIO, Iterable, Iterator, Sequence


CHECKPOINT_HEADER = b"# thermo-global-cegis-v1"
ACTIVE_CUTS_HEADER = b"# thermo-topology-active-cuts-v1"
CNF_SCHEMA = "thermo-topology-cnf-v2"
CELLS = 81
DIGITS = 9
DIRECTED_EDGE_COUNT = 544
UNDIRECTED_EDGE_COUNT = DIRECTED_EDGE_COUNT // 2
COVER_LIMIT = 19
VARIABLE_COUNT = 7_226
BASE_CLAUSE_COUNT = 57_384

DIGIT_BASE = 1
EDGE_BASE = 730
OCCUPIED_BASE = 1_274
SEQUENTIAL_BASE = 1_355
SWAP_BASE = 2_875

FNV_OFFSET = 0xCBF29CE484222325
FNV_PRIME = 0x100000001B3
MASK64 = (1 << 64) - 1
MAX_U64 = MASK64

SYMMETRY_NONE = "none"
SYMMETRY_D4_COMPLEMENT_V1 = "d4-complement-v1"
SYMMETRY_MODES = {SYMMETRY_NONE, SYMMETRY_D4_COMPLEMENT_V1}

DECIMAL_RE = re.compile(rb"[0-9]+\Z")
HEX_RE = re.compile(rb"[0-9a-fA-F]+\Z")


class VerificationError(ValueError):
    """An input fails a structural, checksum, witness, or CNF check."""


@dataclass(frozen=True)
class DirectedEdge:
    lower: int
    upper: int


@dataclass(frozen=True)
class ActiveRecord:
    index: int
    first: bytes
    second: bytes


@dataclass(frozen=True)
class ActiveManifest:
    path: Path
    sha256: str
    cnf_schema: str
    symmetry_break: str
    edge_checksum: int
    directed_edges: int
    pool_pairs: int
    pool_unique_cuts: int
    pool_checksum: int
    active_checksum: int
    records: tuple[ActiveRecord, ...]


@dataclass(frozen=True)
class CheckpointAudit:
    path: Path
    sha256: str
    budget: int
    pairs: int
    unique_cuts: int
    checksum: int
    prefix_pairs: int
    prefix_unique_cuts: int
    prefix_checksum: int
    active_cuts: tuple[int, ...]


def fnv_byte(checksum: int, value: int) -> int:
    return ((checksum ^ value) * FNV_PRIME) & MASK64


def _fnv_grid(checksum: int, grid: bytes) -> int:
    for character in grid:
        checksum = fnv_byte(checksum, character - ord("0"))
    return checksum


def _fnv_pair(checksum: int, first: bytes, second: bytes) -> int:
    checksum = _fnv_grid(checksum, first)
    checksum = fnv_byte(checksum, 0xFE)
    checksum = _fnv_grid(checksum, second)
    return fnv_byte(checksum, 0xFF)


def directed_edges() -> tuple[DirectedEdge, ...]:
    edges: list[DirectedEdge] = []
    for left in range(CELLS):
        for right in range(left + 1, CELLS):
            row_distance = abs(left // 9 - right // 9)
            column_distance = abs(left % 9 - right % 9)
            if row_distance <= 1 and column_distance <= 1:
                edges.append(DirectedEdge(left, right))
                edges.append(DirectedEdge(right, left))
    if len(edges) != DIRECTED_EDGE_COUNT:
        raise AssertionError(f"reconstructed {len(edges)} directed edges")
    return tuple(edges)


EDGES = directed_edges()
ALL_EDGE_BITS = (1 << DIRECTED_EDGE_COUNT) - 1


def edges_checksum(edges: Sequence[DirectedEdge] = EDGES) -> int:
    checksum = FNV_OFFSET
    for edge in edges:
        checksum = fnv_byte(checksum, edge.lower)
        checksum = fnv_byte(checksum, edge.upper)
    return checksum


EDGE_CHECKSUM = edges_checksum()


def _validate_grid(grid: bytes) -> None:
    if len(grid) != CELLS or any(character < ord("1") or character > ord("9") for character in grid):
        raise VerificationError("expected exactly 81 ASCII digits 1-9")
    expected = 0x1FF

    def unit_mask(cells: Iterable[int]) -> int:
        mask = 0
        for cell in cells:
            bit = 1 << (grid[cell] - ord("1"))
            if mask & bit:
                return -1
            mask |= bit
        return mask

    for row in range(9):
        if unit_mask(range(row * 9, row * 9 + 9)) != expected:
            raise VerificationError(f"row {row + 1} is not 1-9")
    for column in range(9):
        if unit_mask(row * 9 + column for row in range(9)) != expected:
            raise VerificationError(f"column {column + 1} is not 1-9")
    for box in range(9):
        box_row = (box // 3) * 3
        box_column = (box % 3) * 3
        cells = (
            (box_row + position // 3) * 9 + box_column + position % 3
            for position in range(9)
        )
        if unit_mask(cells) != expected:
            raise VerificationError(f"box {box + 1} is not 1-9")


@lru_cache(maxsize=65_536)
def _validated_increasing_mask(grid: bytes) -> int:
    """Validate one solved grid and cache its increasing directed-edge mask."""
    _validate_grid(grid)
    mask = 0
    for edge_id, edge in enumerate(EDGES):
        if grid[edge.lower] < grid[edge.upper]:
            mask |= 1 << edge_id
    return mask


def pair_cut(first: bytes, second: bytes) -> int:
    """Return the 544-bit positive clause that distinguishes two solutions."""
    common_increases = _validated_increasing_mask(first) & _validated_increasing_mask(second)
    return ALL_EDGE_BITS ^ common_increases


def pair_clause(cut: int) -> tuple[int, ...]:
    literals: list[int] = []
    remaining = cut
    while remaining:
        least_bit = remaining & -remaining
        edge_id = least_bit.bit_length() - 1
        literals.append(EDGE_BASE + edge_id)
        remaining ^= least_bit
    return tuple(literals)


def _logical_line(raw: bytes) -> bytes:
    if raw.endswith(b"\n"):
        raw = raw[:-1]
        if raw.endswith(b"\r"):
            raw = raw[:-1]
    return raw


def _decimal(value: bytes, context: str) -> int:
    if not DECIMAL_RE.fullmatch(value):
        raise VerificationError(f"{context}: invalid decimal integer")
    parsed = int(value)
    if parsed > MAX_U64:
        raise VerificationError(f"{context}: integer exceeds 64 bits")
    return parsed


def _hex(value: bytes, context: str) -> int:
    if not HEX_RE.fullmatch(value):
        raise VerificationError(f"{context}: invalid hexadecimal integer")
    parsed = int(value, 16)
    if parsed > MAX_U64:
        raise VerificationError(f"{context}: integer exceeds 64 bits")
    return parsed


def _ascii(value: bytes, context: str) -> str:
    try:
        return value.decode("ascii")
    except UnicodeDecodeError as error:
        raise VerificationError(f"{context}: non-ASCII text") from error


def _parse_footer(line: bytes, prefix: bytes, context: str) -> tuple[int, int]:
    suffix = line[len(prefix) :]
    try:
        count, checksum = suffix.split(b" fnv1a64=", 1)
    except ValueError as error:
        raise VerificationError(f"{context}: malformed footer") from error
    return _decimal(count, context), _hex(checksum, context)


def parse_active_manifest(
    path: Path,
    expected_symmetry: str | None = None,
) -> ActiveManifest:
    """Validate the manifest's own schema, records, and order-sensitive hash."""
    digest = hashlib.sha256()
    try:
        handle = path.open("rb")
    except OSError as error:
        raise VerificationError(f"cannot read {path}: {error}") from error

    metadata: dict[bytes, bytes] = {}
    records: list[ActiveRecord] = []
    seen_indices: set[int] = set()
    footer: tuple[int, int] | None = None
    data_started = False
    active_checksum = FNV_OFFSET
    with handle:
        raw_header = handle.readline()
        digest.update(raw_header)
        if _logical_line(raw_header) != ACTIVE_CUTS_HEADER:
            raise VerificationError("wrong or missing active-cut manifest schema header")
        for line_number, raw in enumerate(handle, start=2):
            digest.update(raw)
            line = _logical_line(raw)
            if not line:
                raise VerificationError(f"manifest line {line_number}: blank lines are not allowed")
            footer_prefix = b"# end active_cuts="
            if line.startswith(footer_prefix):
                if footer is not None:
                    raise VerificationError(f"manifest line {line_number}: duplicate footer")
                footer = _parse_footer(line, footer_prefix, f"manifest line {line_number}")
                continue
            if footer is not None:
                raise VerificationError(f"manifest line {line_number}: data after footer")
            if line.startswith(b"# "):
                if data_started:
                    raise VerificationError(f"manifest line {line_number}: metadata after records")
                try:
                    key, value = line[2:].split(b"=", 1)
                except ValueError as error:
                    raise VerificationError(f"manifest line {line_number}: malformed metadata") from error
                if key in metadata:
                    raise VerificationError(f"manifest line {line_number}: duplicate metadata {_ascii(key, 'key')!r}")
                metadata[key] = value
                continue
            if line.startswith(b"#"):
                raise VerificationError(f"manifest line {line_number}: unexpected comment")

            data_started = True
            fields = line.split(b"|")
            if len(fields) != 3:
                raise VerificationError(f"manifest line {line_number}: malformed active-cut witness")
            index = _decimal(fields[0], f"manifest line {line_number} cut index")
            first, second = fields[1], fields[2]
            try:
                _validated_increasing_mask(first)
                _validated_increasing_mask(second)
            except VerificationError as error:
                raise VerificationError(f"manifest line {line_number}: invalid witness grid: {error}") from error
            if first >= second:
                raise VerificationError(f"manifest line {line_number}: witnesses are not canonically ordered")
            if index in seen_indices:
                raise VerificationError(f"manifest line {line_number}: duplicate active cut index {index}")
            seen_indices.add(index)
            record = ActiveRecord(index, first, second)
            records.append(record)
            for byte in index.to_bytes(8, "little"):
                active_checksum = fnv_byte(active_checksum, byte)
            active_checksum = _fnv_pair(active_checksum, first, second)

    expected_keys = {
        b"cnf_schema",
        b"symmetry_break",
        b"edge_order_fnv1a64",
        b"directed_edges",
        b"pool_pairs",
        b"pool_unique_cuts",
        b"pool_fnv1a64",
        b"active_cuts",
        b"fnv1a64",
    }
    if set(metadata) != expected_keys:
        found = sorted(_ascii(key, "manifest metadata key") for key in metadata)
        expected = sorted(_ascii(key, "manifest metadata key") for key in expected_keys)
        raise VerificationError(f"manifest metadata keys are {found}, expected {expected}")

    schema = _ascii(metadata[b"cnf_schema"], "manifest CNF schema")
    symmetry = _ascii(metadata[b"symmetry_break"], "manifest symmetry mode")
    if schema != CNF_SCHEMA:
        raise VerificationError(f"manifest CNF schema is {schema!r}, expected {CNF_SCHEMA!r}")
    if symmetry not in SYMMETRY_MODES:
        raise VerificationError(f"manifest symmetry mode is unsupported: {symmetry!r}")
    if expected_symmetry is not None and symmetry != expected_symmetry:
        raise VerificationError(f"manifest symmetry mode is {symmetry!r}, expected {expected_symmetry!r}")

    edge_checksum = _hex(metadata[b"edge_order_fnv1a64"], "manifest edge checksum")
    edge_count = _decimal(metadata[b"directed_edges"], "manifest directed-edge count")
    pool_pairs = _decimal(metadata[b"pool_pairs"], "manifest pool-pair count")
    pool_cuts = _decimal(metadata[b"pool_unique_cuts"], "manifest pool-cut count")
    pool_checksum = _hex(metadata[b"pool_fnv1a64"], "manifest pool checksum")
    declared_active = _decimal(metadata[b"active_cuts"], "manifest active-cut count")
    declared_checksum = _hex(metadata[b"fnv1a64"], "manifest active checksum")
    if edge_count != DIRECTED_EDGE_COUNT or edge_checksum != EDGE_CHECKSUM:
        raise VerificationError(
            "manifest directed-edge count/order does not match the independently reconstructed king-edge order"
        )
    if declared_active != len(records):
        raise VerificationError(f"manifest declares {declared_active} active cuts, found {len(records)}")
    if declared_checksum != active_checksum:
        raise VerificationError(
            f"manifest active checksum is {declared_checksum:016x}, computed {active_checksum:016x}"
        )
    if footer != (len(records), active_checksum):
        raise VerificationError(
            f"manifest footer is {footer!r}, expected ({len(records)}, {active_checksum:016x})"
        )
    return ActiveManifest(
        path=path.resolve(),
        sha256=digest.hexdigest(),
        cnf_schema=schema,
        symmetry_break=symmetry,
        edge_checksum=edge_checksum,
        directed_edges=edge_count,
        pool_pairs=pool_pairs,
        pool_unique_cuts=pool_cuts,
        pool_checksum=pool_checksum,
        active_checksum=active_checksum,
        records=tuple(records),
    )


def audit_checkpoint(
    path: Path,
    manifest: ActiveManifest,
    required_budget: int | None = 16,
) -> CheckpointAudit:
    """Stream and validate the full pair corpus while reconstructing stable cuts."""
    digest = hashlib.sha256()
    try:
        handle = path.open("rb")
    except OSError as error:
        raise VerificationError(f"cannot read {path}: {error}") from error

    metadata: dict[bytes, bytes] = {}
    footer: tuple[int, int] | None = None
    data_started = False
    pair_count = 0
    unique_cut_count = 0
    checksum = FNV_OFFSET
    seen_pairs: set[bytes] = set()
    seen_cuts: set[int] = set()
    active_by_index = {record.index: record for record in manifest.records}
    resolved_active: dict[int, int] = {}
    prefix_checksum: int | None = FNV_OFFSET if manifest.pool_pairs == 0 else None
    prefix_unique_cuts: int | None = 0 if manifest.pool_pairs == 0 else None

    with handle:
        raw_header = handle.readline()
        digest.update(raw_header)
        if _logical_line(raw_header) != CHECKPOINT_HEADER:
            raise VerificationError("wrong or missing checkpoint schema header")
        for line_number, raw in enumerate(handle, start=2):
            digest.update(raw)
            line = _logical_line(raw)
            if not line:
                raise VerificationError(f"checkpoint line {line_number}: blank lines are not allowed")
            footer_prefix = b"# end pairs="
            if line.startswith(footer_prefix):
                if footer is not None:
                    raise VerificationError(f"checkpoint line {line_number}: duplicate footer")
                footer = _parse_footer(line, footer_prefix, f"checkpoint line {line_number}")
                continue
            if footer is not None:
                raise VerificationError(f"checkpoint line {line_number}: data after footer")
            if line.startswith(b"# "):
                if data_started:
                    raise VerificationError(f"checkpoint line {line_number}: metadata after pair data")
                try:
                    key, value = line[2:].split(b"=", 1)
                except ValueError as error:
                    raise VerificationError(f"checkpoint line {line_number}: malformed metadata") from error
                if key in metadata:
                    raise VerificationError(f"checkpoint line {line_number}: duplicate metadata {_ascii(key, 'key')!r}")
                metadata[key] = value
                continue
            if line.startswith(b"#"):
                raise VerificationError(f"checkpoint line {line_number}: unexpected comment")

            data_started = True
            if line.count(b"|") != 1:
                raise VerificationError(f"checkpoint line {line_number}: expected one pair separator")
            first, second = line.split(b"|", 1)
            if first >= second:
                raise VerificationError(
                    f"checkpoint line {line_number}: grids are not distinct and canonically ordered"
                )
            try:
                first_mask = _validated_increasing_mask(first)
                second_mask = _validated_increasing_mask(second)
            except VerificationError as error:
                raise VerificationError(f"checkpoint line {line_number}: invalid solved grid: {error}") from error
            if line in seen_pairs:
                raise VerificationError(f"checkpoint line {line_number}: duplicate grid pair")
            seen_pairs.add(line)

            checksum = _fnv_pair(checksum, first, second)
            cut = ALL_EDGE_BITS ^ (first_mask & second_mask)
            if cut not in seen_cuts:
                seen_cuts.add(cut)
                cut_index = unique_cut_count
                unique_cut_count += 1
                record = active_by_index.get(cut_index)
                if record is not None:
                    if (first, second) != (record.first, record.second):
                        raise VerificationError(
                            f"manifest witness for active cut {cut_index} is not its first checkpoint pair"
                        )
                    if pair_cut(record.first, record.second) != cut:
                        raise VerificationError(f"manifest witness does not reconstruct active cut {cut_index}")
                    resolved_active[cut_index] = cut
            pair_count += 1
            if pair_count == manifest.pool_pairs:
                prefix_checksum = checksum
                prefix_unique_cuts = unique_cut_count

    expected_keys = {b"budget", b"directed_edges", b"pairs", b"fnv1a64"}
    if set(metadata) != expected_keys:
        found = sorted(_ascii(key, "checkpoint metadata key") for key in metadata)
        expected = sorted(_ascii(key, "checkpoint metadata key") for key in expected_keys)
        raise VerificationError(f"checkpoint metadata keys are {found}, expected {expected}")
    budget = _decimal(metadata[b"budget"], "checkpoint budget")
    edge_count = _decimal(metadata[b"directed_edges"], "checkpoint directed-edge count")
    declared_pairs = _decimal(metadata[b"pairs"], "checkpoint pair count")
    declared_checksum = _hex(metadata[b"fnv1a64"], "checkpoint checksum")
    if required_budget is not None and budget != required_budget:
        raise VerificationError(f"checkpoint budget is {budget}, expected {required_budget}")
    if edge_count != DIRECTED_EDGE_COUNT:
        raise VerificationError(f"checkpoint directed-edge count is {edge_count}, expected {DIRECTED_EDGE_COUNT}")
    if declared_pairs != pair_count:
        raise VerificationError(f"checkpoint declares {declared_pairs} pairs, found {pair_count}")
    if declared_checksum != checksum:
        raise VerificationError(
            f"checkpoint checksum is {declared_checksum:016x}, computed {checksum:016x}"
        )
    if footer != (pair_count, checksum):
        raise VerificationError(
            f"checkpoint footer is {footer!r}, expected ({pair_count}, {checksum:016x})"
        )

    if prefix_checksum is None or prefix_unique_cuts is None:
        raise VerificationError(
            f"manifest pool prefix has {manifest.pool_pairs} pairs, beyond checkpoint length {pair_count}"
        )
    if prefix_checksum != manifest.pool_checksum:
        raise VerificationError(
            f"manifest prefix checksum is {manifest.pool_checksum:016x}, computed {prefix_checksum:016x}"
        )
    if prefix_unique_cuts != manifest.pool_unique_cuts:
        raise VerificationError(
            f"manifest prefix declares {manifest.pool_unique_cuts} unique cuts, computed {prefix_unique_cuts}"
        )
    out_of_prefix = [record.index for record in manifest.records if record.index >= manifest.pool_unique_cuts]
    if out_of_prefix:
        raise VerificationError(f"active cut indices are outside the manifest pool prefix: {out_of_prefix[:8]}")
    unresolved = [record.index for record in manifest.records if record.index not in resolved_active]
    if unresolved:
        raise VerificationError(f"active cut witnesses were not resolved from the checkpoint: {unresolved[:8]}")

    return CheckpointAudit(
        path=path.resolve(),
        sha256=digest.hexdigest(),
        budget=budget,
        pairs=pair_count,
        unique_cuts=unique_cut_count,
        checksum=checksum,
        prefix_pairs=manifest.pool_pairs,
        prefix_unique_cuts=prefix_unique_cuts,
        prefix_checksum=prefix_checksum,
        active_cuts=tuple(resolved_active[record.index] for record in manifest.records),
    )


def digit_var(cell: int, digit_index: int) -> int:
    return DIGIT_BASE + cell * DIGITS + digit_index


def edge_var(edge: int) -> int:
    return EDGE_BASE + edge


def occupied_var(cell: int) -> int:
    return OCCUPIED_BASE + cell


def sequential_var(prefix: int, count: int) -> int:
    return SEQUENTIAL_BASE + prefix * COVER_LIMIT + count


def swap_var(digit_index: int, edge: int) -> int:
    return SWAP_BASE + digit_index * DIRECTED_EDGE_COUNT + edge


def _at_most_one(variables: Sequence[int]) -> Iterator[tuple[int, ...]]:
    for left in range(len(variables)):
        for right in range(left + 1, len(variables)):
            yield (-variables[left], -variables[right])


def _exactly_one(variables: Sequence[int]) -> Iterator[tuple[int, ...]]:
    yield tuple(variables)
    yield from _at_most_one(variables)


def _less_or_equal_digit(left: int, right: int) -> Iterator[tuple[int, ...]]:
    for left_digit in range(DIGITS):
        for right_digit in range(left_digit):
            yield (-digit_var(left, left_digit), -digit_var(right, right_digit))


def base_clauses(symmetry_break: str) -> Iterator[tuple[int, ...]]:
    """Reconstruct the topology master independently in deterministic order."""
    if symmetry_break not in SYMMETRY_MODES:
        raise VerificationError(f"unsupported symmetry mode {symmetry_break!r}")

    # Classic Sudoku: one digit per cell and one occurrence per row, column,
    # and box. Each exactly-one uses one positive clause plus pairwise AMO.
    for cell in range(CELLS):
        yield from _exactly_one(tuple(digit_var(cell, digit) for digit in range(DIGITS)))
    for digit in range(DIGITS):
        for row in range(9):
            yield from _exactly_one(tuple(digit_var(row * 9 + column, digit) for column in range(9)))
        for column in range(9):
            yield from _exactly_one(tuple(digit_var(row * 9 + column, digit) for row in range(9)))
        for box in range(9):
            box_row = (box // 3) * 3
            box_column = (box % 3) * 3
            variables = tuple(
                digit_var((box_row + position // 3) * 9 + box_column + position % 3, digit)
                for position in range(9)
            )
            yield from _exactly_one(variables)

    # Every selected directed king edge imposes a strict digit increase.
    for edge_id, edge in enumerate(EDGES):
        selected = edge_var(edge_id)
        for lower_digit in range(DIGITS):
            for upper_digit in range(lower_digit + 1):
                yield (
                    -selected,
                    -digit_var(edge.lower, lower_digit),
                    -digit_var(edge.upper, upper_digit),
                )

    incoming: list[list[int]] = [[] for _ in range(CELLS)]
    outgoing: list[list[int]] = [[] for _ in range(CELLS)]
    incident: list[list[int]] = [[] for _ in range(CELLS)]
    for edge_id, edge in enumerate(EDGES):
        selected = edge_var(edge_id)
        outgoing[edge.lower].append(selected)
        incoming[edge.upper].append(selected)
        incident[edge.lower].append(selected)
        incident[edge.upper].append(selected)
        yield (-selected, occupied_var(edge.lower))
        yield (-selected, occupied_var(edge.upper))
    for cell in range(CELLS):
        yield tuple([-occupied_var(cell), *incident[cell]])
        yield from _at_most_one(incoming[cell])
        yield from _at_most_one(outgoing[cell])
    for undirected in range(UNDIRECTED_EDGE_COUNT):
        yield (-edge_var(2 * undirected), -edge_var(2 * undirected + 1))

    # Sinz sequential counter for at most 19 occupied cells.
    yield (-occupied_var(0), sequential_var(0, 0))
    for cell in range(1, CELLS - 1):
        yield (-occupied_var(cell), sequential_var(cell, 0))
        yield (-sequential_var(cell - 1, 0), sequential_var(cell, 0))
        for count in range(1, COVER_LIMIT):
            yield (
                -occupied_var(cell),
                -sequential_var(cell - 1, count - 1),
                sequential_var(cell, count),
            )
            yield (-sequential_var(cell - 1, count), sequential_var(cell, count))
    for cell in range(1, CELLS):
        yield (-occupied_var(cell), -sequential_var(cell - 1, COVER_LIMIT - 1))

    # Each adjacent digit-symbol swap must be detected on a selected edge.
    for digit_index in range(8):
        witnesses: list[int] = []
        for edge_id, edge in enumerate(EDGES):
            witness = swap_var(digit_index, edge_id)
            witnesses.append(witness)
            yield (-witness, edge_var(edge_id))
            yield (-witness, digit_var(edge.lower, digit_index))
            yield (-witness, digit_var(edge.upper, digit_index + 1))
        yield tuple(witnesses)

    if symmetry_break == SYMMETRY_D4_COMPLEMENT_V1:
        for corner in (8, 72, 80):
            yield from _less_or_equal_digit(0, corner)
        yield from _less_or_equal_digit(1, 9)
        for digit_index in range(5, DIGITS):
            yield (-digit_var(0, digit_index),)


@lru_cache(maxsize=len(SYMMETRY_MODES))
def base_clause_count(symmetry_break: str) -> int:
    count = sum(1 for _ in base_clauses(symmetry_break))
    expected = BASE_CLAUSE_COUNT + (148 if symmetry_break == SYMMETRY_D4_COMPLEMENT_V1 else 0)
    if count != expected:
        raise AssertionError(f"independent base generated {count} clauses, expected {expected}")
    return count


def _clause_line(clause: Sequence[int]) -> bytes:
    if clause:
        return (" ".join(str(literal) for literal in clause) + " 0\n").encode("ascii")
    return b"0\n"


def expected_cnf_lines(
    checkpoint: CheckpointAudit,
    manifest: ActiveManifest,
) -> Iterator[bytes]:
    base_count = base_clause_count(manifest.symmetry_break)
    clause_count = base_count + len(manifest.records)
    header = (
        f"c {CNF_SCHEMA}\n",
        "c cut_pool_mode lazy-active-v1\n",
        "c model classic-sudoku plus disjoint-directed-king-paths\n",
        f"c covered_cells_at_most {COVER_LIMIT}\n",
        "c diagonal_crossings_without_shared_cells allowed\n",
        f"c symmetry_break {manifest.symmetry_break}\n",
        f"c checkpoint_budget {checkpoint.budget}\n",
        f"c checkpoint_pairs {checkpoint.pairs}\n",
        f"c full_unique_pair_cuts {checkpoint.unique_cuts}\n",
        f"c active_pair_cuts {len(manifest.records)}\n",
        f"c checkpoint_fnv1a64 {checkpoint.checksum:016x}\n",
        f"c active_fnv1a64 {manifest.active_checksum:016x}\n",
        "c digit_variables 1 729\n",
        "c edge_variables 730 1273\n",
        "c occupied_variables 1274 1354\n",
        "c sequential_variables 1355 2874\n",
        "c swap_witness_variables 2875 7226\n",
        "c edge_order lexicographic unordered cell pair then forward and reverse\n",
        f"c edge_order_fnv1a64 {EDGE_CHECKSUM:016x}\n",
        "c digit_var 1+9*cell+(digit-1)\n",
        "c edge_var 730+edge_id\n",
        "c occupied_var 1274+cell\n",
        "c sequential_var 1355+19*prefix+count\n",
        "c swap_var 2875+544*(digit-1)+edge_id\n",
        f"p cnf {VARIABLE_COUNT} {clause_count}\n",
    )
    for line in header:
        yield line.encode("ascii")
    for clause in base_clauses(manifest.symmetry_break):
        yield _clause_line(clause)
    for cut in checkpoint.active_cuts:
        yield _clause_line(pair_clause(cut))


def _short_line(line: bytes, limit: int = 200) -> str:
    suffix = "..." if len(line) > limit else ""
    return line[:limit].decode("ascii", errors="backslashreplace").rstrip("\r\n") + suffix


def verify_cnf_exact(
    path: Path,
    checkpoint: CheckpointAudit,
    manifest: ActiveManifest,
) -> dict[str, object]:
    """Compare every comment, DIMACS header, base clause, and active clause."""
    actual_hash = hashlib.sha256()
    expected_hash = hashlib.sha256()
    expected_line_count = 0
    try:
        handle: BinaryIO = path.open("rb")
    except OSError as error:
        raise VerificationError(f"cannot read {path}: {error}") from error
    with handle:
        for line_number, expected in enumerate(expected_cnf_lines(checkpoint, manifest), start=1):
            expected_line_count = line_number
            expected_hash.update(expected)
            actual = handle.readline()
            if not actual:
                raise VerificationError(f"CNF ended before expected line {line_number}")
            actual_hash.update(actual)
            if actual != expected:
                raise VerificationError(
                    f"CNF mismatch at line {line_number}: expected {_short_line(expected)!r}, "
                    f"found {_short_line(actual)!r}"
                )
        extra = handle.readline()
        if extra:
            raise VerificationError(
                f"CNF has data after expected line {expected_line_count}: {_short_line(extra)!r}"
            )
    if actual_hash.digest() != expected_hash.digest():
        raise AssertionError("equal line streams unexpectedly produced different SHA-256 hashes")
    return {
        "path": str(path.resolve()),
        "sha256": actual_hash.hexdigest(),
        "variables": VARIABLE_COUNT,
        "base_clauses": base_clause_count(manifest.symmetry_break),
        "active_clauses": len(manifest.records),
        "clauses": base_clause_count(manifest.symmetry_break) + len(manifest.records),
        "exact_independent_reemission_match": True,
    }


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            while block := handle.read(1 << 20):
                digest.update(block)
    except OSError as error:
        raise VerificationError(f"cannot re-read {path}: {error}") from error
    return digest.hexdigest()


def verify_artifacts(
    checkpoint_path: Path,
    manifest_path: Path,
    cnf_path: Path,
    required_budget: int | None = 16,
    expected_symmetry: str | None = None,
) -> dict[str, object]:
    manifest = parse_active_manifest(manifest_path, expected_symmetry)
    checkpoint = audit_checkpoint(checkpoint_path, manifest, required_budget)
    cnf = verify_cnf_exact(cnf_path, checkpoint, manifest)
    for label, path, expected_hash in (
        ("checkpoint", checkpoint_path, checkpoint.sha256),
        ("active manifest", manifest_path, manifest.sha256),
        ("CNF", cnf_path, str(cnf["sha256"])),
    ):
        if _sha256_file(path) != expected_hash:
            raise VerificationError(f"{label} changed while it was being verified")
    return {
        "valid": True,
        "scope": "lazy base-plus-active topology CNF",
        "cnf_schema": manifest.cnf_schema,
        "symmetry_break": manifest.symmetry_break,
        "edge_order_fnv1a64": f"{manifest.edge_checksum:016x}",
        "checkpoint": {
            "path": str(checkpoint.path),
            "sha256": checkpoint.sha256,
            "budget": checkpoint.budget,
            "pairs": checkpoint.pairs,
            "unique_pair_cuts": checkpoint.unique_cuts,
            "fnv1a64": f"{checkpoint.checksum:016x}",
        },
        "manifest": {
            "path": str(manifest.path),
            "sha256": manifest.sha256,
            "pool_prefix_pairs": manifest.pool_pairs,
            "pool_prefix_unique_cuts": manifest.pool_unique_cuts,
            "pool_prefix_fnv1a64": f"{manifest.pool_checksum:016x}",
            "active_cuts": len(manifest.records),
            "fnv1a64": f"{manifest.active_checksum:016x}",
            "all_witnesses_match_first_checkpoint_cut_occurrence": True,
        },
        "cnf": cnf,
    }


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("checkpoint", type=Path, help="full thermo-global-cegis-v1 pair checkpoint")
    parser.add_argument("active_cuts", type=Path, help="thermo-topology-active-cuts-v1 manifest")
    parser.add_argument("cnf", type=Path, help="lazy base-plus-active DIMACS file")
    parser.add_argument("--budget", type=int, default=16, help="required checkpoint budget (default: 16)")
    parser.add_argument(
        "--expected-symmetry-break",
        choices=sorted(SYMMETRY_MODES),
        help="optionally require a particular manifest/CNF symmetry mode",
    )
    parser.add_argument("--json", action="store_true", help="print machine-readable evidence")
    arguments = parser.parse_args(argv)
    try:
        result = verify_artifacts(
            arguments.checkpoint,
            arguments.active_cuts,
            arguments.cnf,
            required_budget=arguments.budget,
            expected_symmetry=arguments.expected_symmetry_break,
        )
    except VerificationError as error:
        if arguments.json:
            print(json.dumps({"valid": False, "error": str(error)}))
        else:
            print(f"INVALID: {error}", file=sys.stderr)
        return 1
    if arguments.json:
        print(json.dumps(result, indent=2))
    else:
        checkpoint = result["checkpoint"]
        manifest = result["manifest"]
        cnf = result["cnf"]
        print(
            "valid lazy topology artifacts: "
            f"pairs={checkpoint['pairs']} unique_cuts={checkpoint['unique_pair_cuts']} "
            f"active={manifest['active_cuts']} clauses={cnf['clauses']} "
            f"symmetry={result['symmetry_break']} cnf_sha256={cnf['sha256']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
