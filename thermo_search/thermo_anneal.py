"""Reproducible exploratory search for classic Sudoku thermometer layouts.

This is a maintained replacement for ``sources/min thermo new.ipynb``.  The
synced notebook remains untouched.  Layouts use zero-based row-major cells and
each path is ordered from bulb to tip.

The default geometry permits orthogonal and diagonal king moves, forbids a
repeated cell, and requires different thermometers to be cell-disjoint.
"""

from __future__ import annotations

import argparse
import ast
import ctypes
import json
import math
import os
import random
import subprocess
import sys
import time
from collections import OrderedDict
from dataclasses import asdict, dataclass
from enum import Enum
from functools import lru_cache
from pathlib import Path
from typing import Iterable, Protocol, Sequence

Cell = int
Thermo = tuple[Cell, ...]
Layout = tuple[Thermo, ...]
Mutation = tuple[int, int, int]  # thermometer index, path position, replacement cell
MUTATION_CACHE_SIZE = 10_000
COUNT_CACHE_SIZE = 20_000
MAX_COUNT_LIMIT = (1 << 63) - 1


class GeometryError(ValueError):
    """The supplied thermometer geometry is outside the fixed search model."""


class NoMutationError(RuntimeError):
    """A layout has no legal one-cell mutation."""


class SolverError(RuntimeError):
    """A solver backend failed or returned an unexpected response."""


class Multiplicity(str, Enum):
    ZERO = "0"
    UNIQUE = "1"
    MULTIPLE = "2+"


@dataclass(frozen=True)
class CountResult:
    count: int
    exact: bool
    duration_seconds: float

    @property
    def multiplicity(self) -> Multiplicity:
        if self.count == 0:
            return Multiplicity.ZERO
        if self.count == 1 and self.exact:
            return Multiplicity.UNIQUE
        return Multiplicity.MULTIPLE


class SolverBackend(Protocol):
    def count(self, layout: Layout, *, cap: int) -> CountResult: ...

    def description(self) -> str: ...


def neighbors(cell: Cell, *, diagonal: bool = True) -> tuple[Cell, ...]:
    if not 0 <= cell < 81:
        raise GeometryError(f"cell {cell} is outside 0..80")
    row, col = divmod(cell, 9)
    result: list[int] = []
    for dr in (-1, 0, 1):
        for dc in (-1, 0, 1):
            if dr == dc == 0:
                continue
            if not diagonal and dr != 0 and dc != 0:
                continue
            rr, cc = row + dr, col + dc
            if 0 <= rr < 9 and 0 <= cc < 9:
                result.append(9 * rr + cc)
    return tuple(sorted(result))


def normalize_layout(raw: Iterable[Iterable[int]]) -> Layout:
    try:
        paths = tuple(raw)
    except TypeError as error:
        raise GeometryError("layout must be an iterable of integer paths") from error
    normalized: list[Thermo] = []
    for thermo_index, path in enumerate(paths):
        if isinstance(path, (str, bytes, set, frozenset)):
            raise GeometryError(
                f"thermometer {thermo_index} must be an ordered integer sequence"
            )
        try:
            cells = tuple(path)
        except TypeError as error:
            raise GeometryError(
                f"thermometer {thermo_index} must be an ordered integer sequence"
            ) from error
        for position, cell in enumerate(cells):
            if type(cell) is not int:
                raise GeometryError(
                    f"thermometer {thermo_index}, position {position}: "
                    "cell must be an integer"
                )
        normalized.append(cells)
    return tuple(normalized)


def canonical_layout(layout: Layout) -> Layout:
    """Normalize thermometer order while preserving every bulb-to-tip order."""

    return tuple(sorted(layout, key=lambda path: (-len(path), path)))


def validate_layout(
    layout: Layout,
    *,
    disjoint: bool = True,
    diagonal: bool = True,
) -> None:
    occupied: set[int] = set()
    for thermo_index, path in enumerate(layout):
        if not 2 <= len(path) <= 9:
            raise GeometryError(
                f"thermometer {thermo_index} has length {len(path)}; expected 2..9"
            )
        local: set[int] = set()
        for position, cell in enumerate(path):
            if not 0 <= cell < 81:
                raise GeometryError(
                    f"thermometer {thermo_index}, position {position}: "
                    f"cell {cell} is outside 0..80"
                )
            if cell in local:
                raise GeometryError(f"thermometer {thermo_index} repeats cell {cell}")
            if disjoint and cell in occupied:
                raise GeometryError(f"cell {cell} occurs in multiple thermometers")
            local.add(cell)
        for left, right in zip(path, path[1:]):
            if right not in neighbors(left, diagonal=diagonal):
                kind = "orthogonally " if not diagonal else ""
                raise GeometryError(
                    f"thermometer {thermo_index} has cells {left}->{right}, "
                    f"which are not {kind}adjacent"
                )
        occupied.update(local)


@lru_cache(maxsize=MUTATION_CACHE_SIZE)
def legal_mutation_descriptors(
    layout: Layout, *, diagonal: bool = True
) -> tuple[Mutation, ...]:
    """Return compact legal one-cell moves in deterministic order."""

    validate_layout(layout, diagonal=diagonal)
    mutations: list[Mutation] = []
    for thermo_index, path in enumerate(layout):
        occupied_elsewhere = {
            cell
            for other_index, other in enumerate(layout)
            if other_index != thermo_index
            for cell in other
        }
        for position, old_cell in enumerate(path):
            if position == 0:
                options = set(neighbors(path[1], diagonal=diagonal))
            elif position == len(path) - 1:
                options = set(neighbors(path[-2], diagonal=diagonal))
            else:
                options = set(neighbors(path[position - 1], diagonal=diagonal))
                options.intersection_update(
                    neighbors(path[position + 1], diagonal=diagonal)
                )

            forbidden = occupied_elsewhere | (set(path) - {old_cell}) | {old_cell}
            for replacement in sorted(options - forbidden):
                mutations.append((thermo_index, position, replacement))
    return tuple(mutations)


def apply_mutation(layout: Layout, mutation: Mutation) -> Layout:
    thermo_index, position, replacement = mutation
    changed_path = list(layout[thermo_index])
    changed_path[position] = replacement
    changed_layout = list(layout)
    changed_layout[thermo_index] = tuple(changed_path)
    return tuple(changed_layout)


def legal_mutations(layout: Layout, *, diagonal: bool = True) -> tuple[Layout, ...]:
    """Materialize every legal neighbor; search code uses compact descriptors."""

    return tuple(
        apply_mutation(layout, move)
        for move in legal_mutation_descriptors(layout, diagonal=diagonal)
    )


def mutate(layout: Layout, rng: random.Random, *, diagonal: bool = True) -> Layout:
    choices = legal_mutation_descriptors(layout, diagonal=diagonal)
    if not choices:
        raise NoMutationError("layout has no legal one-cell mutation")
    return apply_mutation(layout, choices[rng.randrange(len(choices))])


def idx2str(path: Sequence[int]) -> str:
    normalized = normalize_layout([path])[0]
    if not normalized:
        raise GeometryError("a solver thermometer cannot be empty")
    return "".join(f"R{cell // 9 + 1}C{cell % 9 + 1}" for cell in normalized)


class ConsoleSolver:
    """Adapter for Rangsk's SudokuSolverConsole executable."""

    def __init__(
        self,
        executable: str | os.PathLike[str],
        *,
        timeout_seconds: float = 300.0,
        extra_constraints: Sequence[str] = (),
    ) -> None:
        self.executable = Path(executable).expanduser().resolve()
        if not self.executable.is_file():
            raise SolverError(f"solver executable does not exist: {self.executable}")
        self.timeout_seconds = timeout_seconds
        self.extra_constraints = tuple(extra_constraints)
        self._cache: OrderedDict[tuple[Layout, int], CountResult] = OrderedDict()

    def description(self) -> str:
        extras = json.dumps(self.extra_constraints, separators=(",", ":"))
        return (
            f"SudokuSolverConsole:{self.executable};"
            f"extra_constraints={extras}"
        )

    def count(self, layout: Layout, *, cap: int) -> CountResult:
        if not 2 <= cap <= MAX_COUNT_LIMIT:
            raise ValueError(
                f"cap must be between 2 and {MAX_COUNT_LIMIT}"
            )
        validate_layout(layout)
        key = (canonical_layout(layout), cap)
        if cached := self._cache.get(key):
            self._cache.move_to_end(key)
            return cached

        arguments = [
            str(self.executable),
            "--json",
            "--blank=9",
            "--solutioncount",
            f"--maxcount={cap}",
            "--hide-banner",
        ]
        arguments.extend(f"--constraint={name}" for name in self.extra_constraints)
        arguments.extend(
            f"--constraint=thermo:{idx2str(path)}" for path in canonical_layout(layout)
        )
        started = time.perf_counter()
        try:
            completed = subprocess.run(
                arguments,
                check=False,
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=self.timeout_seconds,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise SolverError(f"solver invocation failed: {error}") from error
        duration = time.perf_counter() - started
        if completed.returncode != 0:
            raise SolverError(
                f"solver exited with code {completed.returncode}: {completed.stderr.strip()}"
            )
        try:
            response = json.loads(completed.stdout)
        except json.JSONDecodeError as error:
            raise SolverError(
                f"solver returned malformed JSON: {completed.stdout[:500]!r}"
            ) from error

        if response.get("type") == "solutionCount":
            count = int(response["count"])
            result = CountResult(count=count, exact=count < cap, duration_seconds=duration)
        elif response.get("type") == "error" and "no solutions" in str(
            response.get("error", "")
        ).lower():
            result = CountResult(count=0, exact=True, duration_seconds=duration)
        else:
            raise SolverError(f"unexpected solver response: {response!r}")
        self._cache[key] = result
        if len(self._cache) > COUNT_CACHE_SIZE:
            self._cache.popitem(last=False)
        return result


class RustSolver:
    """In-process adapter for ``thermo-sudoku-rs`` through its C ABI."""

    def __init__(self, library: str | os.PathLike[str] | None = None) -> None:
        if library is None:
            root = Path(__file__).resolve().parents[1]
            names = ("thermo_sudoku.dll", "libthermo_sudoku.so", "libthermo_sudoku.dylib")
            candidates = [root / "thermo-sudoku-rs" / "target" / "release" / name for name in names]
            library_path = next((path for path in candidates if path.is_file()), candidates[0])
        else:
            library_path = Path(library).expanduser().resolve()
        if not library_path.is_file():
            raise SolverError(
                f"Rust solver library does not exist: {library_path}. "
                "Build it with `cargo build --release --manifest-path "
                "thermo-sudoku-rs/Cargo.toml`."
            )
        self.library_path = library_path
        self._library = ctypes.CDLL(str(library_path))
        self._count = self._library.thermo_sudoku_count_up_to
        self._count.argtypes = [
            ctypes.POINTER(ctypes.c_uint8),
            ctypes.POINTER(ctypes.c_uint8),
            ctypes.POINTER(ctypes.c_uint16),
            ctypes.c_size_t,
            ctypes.c_uint64,
            ctypes.POINTER(ctypes.c_uint8),
        ]
        self._count.restype = ctypes.c_int64
        self._cache: OrderedDict[tuple[Layout, int], CountResult] = OrderedDict()

    def description(self) -> str:
        return f"thermo-sudoku-rs:{self.library_path};constraints=classic"

    def count(self, layout: Layout, *, cap: int) -> CountResult:
        if not 2 <= cap <= MAX_COUNT_LIMIT:
            raise ValueError(
                f"cap must be between 2 and {MAX_COUNT_LIMIT}"
            )
        validate_layout(layout)
        normalized = canonical_layout(layout)
        key = (normalized, cap)
        if cached := self._cache.get(key):
            self._cache.move_to_end(key)
            return cached

        flat = [cell for path in normalized for cell in path]
        offsets = [0]
        for path in normalized:
            offsets.append(offsets[-1] + len(path))
        cells_array = (ctypes.c_uint8 * max(1, len(flat)))(*(flat or [0]))
        offsets_array = (ctypes.c_uint16 * len(offsets))(*offsets)
        solution_array = (ctypes.c_uint8 * 81)()

        started = time.perf_counter()
        count = int(
            self._count(
                None,
                cells_array,
                offsets_array,
                len(normalized),
                cap,
                solution_array,
            )
        )
        duration = time.perf_counter() - started
        if count < 0:
            raise SolverError(f"Rust solver rejected the layout (error code {count})")
        result = CountResult(count=count, exact=count < cap, duration_seconds=duration)
        self._cache[key] = result
        if len(self._cache) > COUNT_CACHE_SIZE:
            self._cache.popitem(last=False)
        return result


@dataclass(frozen=True)
class AnnealConfig:
    start_temperature: float = 200.0
    min_temperature: float = 0.1
    alpha: float = 0.95
    steps_per_temperature: int = 50
    reheats: int = 1
    initial_cap: int = 1_000
    cap_factor: float = 4.0
    min_cap: int = 2
    max_cap: int = 50_000
    diagonal: bool = True
    use_hastings_correction: bool = True
    print_threshold: int | None = 100

    def validate(self) -> None:
        floats = {
            "start_temperature": self.start_temperature,
            "min_temperature": self.min_temperature,
            "alpha": self.alpha,
            "cap_factor": self.cap_factor,
        }
        for name, value in floats.items():
            if not math.isfinite(value):
                raise ValueError(f"{name} must be finite")
        if not self.start_temperature > self.min_temperature > 0:
            raise ValueError("require start_temperature > min_temperature > 0")
        if not 0 < self.alpha < 1:
            raise ValueError("alpha must be between 0 and 1")
        if self.steps_per_temperature <= 0 or self.reheats <= 0:
            raise ValueError("steps_per_temperature and reheats must be positive")
        if self.min_cap < 2 or not self.min_cap <= self.initial_cap <= self.max_cap:
            raise ValueError("require 2 <= min_cap <= initial_cap <= max_cap")
        if self.max_cap > MAX_COUNT_LIMIT:
            raise ValueError(f"max_cap must not exceed {MAX_COUNT_LIMIT}")
        if self.cap_factor < 1:
            raise ValueError("cap_factor must be at least 1")


@dataclass(frozen=True)
class ScoredLayout:
    layout: Layout
    count: int
    exact: bool

    @property
    def objective(self) -> float:
        if self.count <= 0:
            return math.inf
        return math.log(self.count)


@dataclass(frozen=True)
class AnnealResult:
    best: ScoredLayout
    current: ScoredLayout
    found_unique: bool
    proposals: int
    accepted: int
    seed: int


class JsonlRunLogger:
    def __init__(self, path: str | os.PathLike[str]) -> None:
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._file = self.path.open("x", encoding="utf-8", buffering=1)

    def write(self, kind: str, **payload: object) -> None:
        record = {"type": kind, "timestamp": time.time(), **payload}
        self._file.write(json.dumps(record, separators=(",", ":")) + "\n")
        self._file.flush()

    def close(self) -> None:
        self._file.close()

    def __enter__(self) -> "JsonlRunLogger":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()


def anneal(
    initial: Layout,
    solver: SolverBackend,
    config: AnnealConfig,
    *,
    seed: int,
    logger: JsonlRunLogger | None = None,
) -> AnnealResult:
    """Run bounded, deterministic simulated annealing.

    Counts equal to a positive cap are explicit lower bounds.  A count of one
    remains exact because every cap used here is at least two.
    """

    config.validate()
    validate_layout(initial, diagonal=config.diagonal)
    rng = random.Random(seed)

    initial_count = solver.count(initial, cap=config.initial_cap)
    if initial_count.count == 0:
        raise ValueError("initial layout has no solutions")
    current = ScoredLayout(initial, initial_count.count, initial_count.exact)
    best = current
    proposals = 0
    accepted = 0

    if logger:
        logger.write(
            "run_start",
            seed=seed,
            solver=solver.description(),
            config=asdict(config),
            initial_layout=initial,
            initial_count=initial_count.count,
            initial_exact=initial_count.exact,
        )
    if current.count == 1 and current.exact:
        result = AnnealResult(current, current, True, 0, 0, seed)
        if logger:
            logger.write("unique", reason="initial", result=_result_json(result))
        return result

    for reheat in range(config.reheats):
        temperature = config.start_temperature
        while temperature > config.min_temperature:
            for _ in range(config.steps_per_temperature):
                moves = legal_mutation_descriptors(
                    current.layout, diagonal=config.diagonal
                )
                if not moves:
                    result = AnnealResult(best, current, False, proposals, accepted, seed)
                    if logger:
                        logger.write("run_end", reason="rigid", result=_result_json(result))
                    return result
                candidate_layout = apply_mutation(
                    current.layout, moves[rng.randrange(len(moves))]
                )
                proposals += 1

                # Compare every non-exact score at one common cap.  Otherwise a
                # 1,000+ current state could incorrectly look better than a
                # candidate whose exact count is 2,000 merely because the
                # candidate was evaluated with a larger cap.
                exact_floor = max(
                    (
                        scored.count + 1
                        for scored in (current, best)
                        if scored.exact
                    ),
                    default=config.min_cap,
                )
                if current.count >= config.max_cap / config.cap_factor:
                    scaled_cap = config.max_cap
                else:
                    scaled_cap = math.ceil(current.count * config.cap_factor)
                comparison_cap = max(
                    config.min_cap,
                    min(
                        config.max_cap,
                        max(
                            exact_floor,
                            scaled_cap,
                        ),
                    ),
                )
                if not current.exact:
                    refreshed = solver.count(current.layout, cap=comparison_cap)
                    current = ScoredLayout(
                        current.layout, refreshed.count, refreshed.exact
                    )
                if best.layout == current.layout:
                    best = current
                elif not best.exact:
                    refreshed = solver.count(best.layout, cap=comparison_cap)
                    best = ScoredLayout(best.layout, refreshed.count, refreshed.exact)

                candidate_count = solver.count(candidate_layout, cap=comparison_cap)
                if candidate_count.count == 0:
                    continue
                candidate = ScoredLayout(
                    candidate_layout, candidate_count.count, candidate_count.exact
                )
                if candidate.count == 1 and candidate.exact:
                    accepted += 1
                    current = candidate
                    best = candidate
                    result = AnnealResult(best, current, True, proposals, accepted, seed)
                    if logger:
                        logger.write(
                            "unique",
                            reheat=reheat,
                            temperature=temperature,
                            proposal=proposals,
                            comparison_cap=comparison_cap,
                            layout=candidate.layout,
                            result=_result_json(result),
                        )
                    return result

                # Best means best evaluated state, even if the Markov-chain
                # acceptance step below rejects it (for example after the
                # Hastings degree correction).
                if (candidate.count, not candidate.exact) < (
                    best.count,
                    not best.exact,
                ):
                    best = candidate

                delta = candidate.objective - current.objective
                if delta <= 0:
                    probability = 1.0
                else:
                    probability = math.exp(-delta / temperature)
                if config.use_hastings_correction:
                    reverse_degree = len(
                        legal_mutation_descriptors(
                            candidate.layout, diagonal=config.diagonal
                        )
                    )
                    if reverse_degree == 0:
                        probability = 0.0
                    else:
                        probability *= len(moves) / reverse_degree
                probability = min(1.0, probability)

                if rng.random() < probability:
                    current = candidate
                    accepted += 1
                    if config.print_threshold is not None and candidate.count <= config.print_threshold:
                        qualifier = "" if candidate.exact else "+"
                        print(
                            f"accepted {canonical_layout(candidate.layout)} with "
                            f"{candidate.count}{qualifier} solutions at "
                            f"T={temperature:.6g}, p={probability:.3f}"
                        )
                    if logger:
                        logger.write(
                            "accepted",
                            reheat=reheat,
                            temperature=temperature,
                            proposal=proposals,
                            comparison_cap=comparison_cap,
                            probability=probability,
                            layout=candidate.layout,
                            count=candidate.count,
                            exact=candidate.exact,
                            best_count=best.count,
                            best_exact=best.exact,
                        )
            # Cool after every fixed proposal block, whether moves were accepted or not.
            temperature *= config.alpha

    result = AnnealResult(best, current, False, proposals, accepted, seed)
    if logger:
        logger.write("run_end", reason="schedule_complete", result=_result_json(result))
    return result


def _result_json(result: AnnealResult) -> dict[str, object]:
    return {
        "best_layout": result.best.layout,
        "best_count": result.best.count,
        "best_exact": result.best.exact,
        "current_layout": result.current.layout,
        "current_count": result.current.count,
        "current_exact": result.current.exact,
        "found_unique": result.found_unique,
        "proposals": result.proposals,
        "accepted": result.accepted,
        "seed": result.seed,
    }


def parse_layout_text(text: str) -> Layout:
    try:
        raw = ast.literal_eval(text)
    except (SyntaxError, ValueError) as error:
        raise GeometryError("layout is not a valid Python/JSON nested sequence") from error
    layout = normalize_layout(raw)
    validate_layout(layout)
    return layout


def read_legacy_layout(path: str | os.PathLike[str], line_number: int) -> Layout:
    if line_number <= 0:
        raise ValueError("line number is one-based and must be positive")
    lines = Path(path).read_text(encoding="utf-8").splitlines()
    try:
        line = lines[line_number - 1]
    except IndexError as error:
        raise ValueError(f"file has only {len(lines)} lines") from error
    _, separator, text = line.partition(";")
    if not separator:
        raise ValueError("legacy result line has no semicolon")
    return parse_layout_text(text)


def _add_layout_source(parser: argparse.ArgumentParser) -> None:
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--layout", help="nested list/tuple of zero-based paths")
    group.add_argument("--input", type=Path, help="legacy count;layout result file")
    parser.add_argument("--line", type=int, default=1, help="one-based --input line")


def _add_backend(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--backend", choices=("rust", "console"), default="rust")
    parser.add_argument("--rust-library", type=Path)
    parser.add_argument("--solver", type=Path, default=os.environ.get("SUDOKU_SOLVER"))
    parser.add_argument("--extra-constraint", action="append", default=[])


def _load_layout(args: argparse.Namespace) -> Layout:
    if args.layout is not None:
        return parse_layout_text(args.layout)
    return read_legacy_layout(args.input, args.line)


def _load_backend(args: argparse.Namespace) -> SolverBackend:
    if args.backend == "rust":
        if args.extra_constraint:
            raise ValueError("the Rust backend currently supports classic Sudoku only")
        return RustSolver(args.rust_library)
    if args.solver is None:
        raise ValueError("--solver or SUDOKU_SOLVER is required for the console backend")
    return ConsoleSolver(args.solver, extra_constraints=args.extra_constraint)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    check = subparsers.add_parser("check", help="count one layout up to a cap")
    _add_layout_source(check)
    _add_backend(check)
    check.add_argument("--cap", type=int, default=2)

    corpus = subparsers.add_parser(
        "validate-corpus", help="recount every record in a legacy result file"
    )
    corpus.add_argument("--input", type=Path, required=True)
    _add_backend(corpus)

    search = subparsers.add_parser("anneal", help="run bounded simulated annealing")
    _add_layout_source(search)
    _add_backend(search)
    search.add_argument("--output", type=Path)
    search.add_argument("--seed", type=int, required=True)
    search.add_argument("--temperature", type=float, default=200.0)
    search.add_argument("--min-temperature", type=float, default=0.1)
    search.add_argument("--alpha", type=float, default=0.95)
    search.add_argument("--steps-per-temperature", type=int, default=50)
    search.add_argument("--reheats", type=int, default=1)
    search.add_argument("--initial-cap", type=int, default=1_000)
    search.add_argument("--cap-factor", type=float, default=4.0)
    search.add_argument("--min-cap", type=int, default=2)
    search.add_argument("--max-cap", type=int, default=50_000)
    search.add_argument("--orthogonal-only", action="store_true")
    search.add_argument("--no-hastings", action="store_true")
    search.add_argument("--print-threshold", type=int, default=100)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        backend = _load_backend(args)
        if args.command == "validate-corpus":
            matched = mismatched = invalid = 0
            details: list[dict[str, object]] = []
            started = time.perf_counter()
            for line_number, line in enumerate(
                args.input.read_text(encoding="utf-8").splitlines(), 1
            ):
                declared_text, separator, layout_text = line.partition(";")
                if not separator:
                    invalid += 1
                    details.append({"line": line_number, "error": "missing semicolon"})
                    continue
                try:
                    declared = int(declared_text)
                    if declared < 0:
                        raise ValueError("declared solution count must be non-negative")
                    layout = parse_layout_text(layout_text)
                except (GeometryError, ValueError) as error:
                    invalid += 1
                    details.append({"line": line_number, "error": str(error)})
                    continue
                counted = backend.count(layout, cap=max(2, declared + 1))
                if counted.exact and counted.count == declared:
                    matched += 1
                else:
                    mismatched += 1
                    details.append(
                        {
                            "line": line_number,
                            "declared": declared,
                            "counted": counted.count,
                            "exact": counted.exact,
                        }
                    )
            print(
                json.dumps(
                    {
                        "matched": matched,
                        "mismatched": mismatched,
                        "invalid": invalid,
                        "duration_seconds": time.perf_counter() - started,
                        "solver": backend.description(),
                        "details": details,
                    },
                    indent=2,
                )
            )
            return 0 if mismatched == 0 and invalid == 0 else 1

        layout = _load_layout(args)
        if args.command == "check":
            result = backend.count(layout, cap=args.cap)
            print(
                json.dumps(
                    {
                        "layout": layout,
                        "count": result.count,
                        "exact": result.exact,
                        "multiplicity": result.multiplicity.value,
                        "duration_seconds": result.duration_seconds,
                        "solver": backend.description(),
                    },
                    indent=2,
                )
            )
            return 0

        config = AnnealConfig(
            start_temperature=args.temperature,
            min_temperature=args.min_temperature,
            alpha=args.alpha,
            steps_per_temperature=args.steps_per_temperature,
            reheats=args.reheats,
            initial_cap=args.initial_cap,
            cap_factor=args.cap_factor,
            min_cap=args.min_cap,
            max_cap=args.max_cap,
            diagonal=not args.orthogonal_only,
            use_hastings_correction=not args.no_hastings,
            print_threshold=args.print_threshold,
        )
        if args.output:
            with JsonlRunLogger(args.output) as logger:
                result = anneal(layout, backend, config, seed=args.seed, logger=logger)
        else:
            result = anneal(layout, backend, config, seed=args.seed)
        print(json.dumps(_result_json(result), indent=2))
        return 0 if result.found_unique else 1
    except (GeometryError, NoMutationError, SolverError, ValueError) as error:
        parser.error(str(error))
    return 2


if __name__ == "__main__":
    sys.exit(main())
