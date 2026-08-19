import math
import random
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path

from thermo_search.thermo_anneal import (
    AnnealConfig,
    ConsoleSolver,
    CountResult,
    GeometryError,
    Multiplicity,
    RustSolver,
    anneal,
    legal_mutations,
    main,
    mutate,
    neighbors,
    parse_layout_text,
    read_legacy_layout,
    validate_layout,
)


BLUE_20 = (
    (18, 27, 28, 19, 20, 11, 12, 13, 4),
    (57, 48, 49),
    (59, 68, 69, 60, 61, 52, 53, 44),
)
KNOWN_THREE = (
    (19, 29, 28, 20, 11, 12, 13, 3, 4),
    (77, 69, 78, 70, 62, 53, 44, 52),
    (41, 51),
)
UNSAT_TWO_ROWS = (tuple(range(9)), tuple(range(9, 18)))


class GeometryTests(unittest.TestCase):
    def test_diagonal_switch(self):
        self.assertIn(10, neighbors(0))
        self.assertNotIn(10, neighbors(0, diagonal=False))

    def test_rejects_overlap_and_non_neighbor(self):
        with self.assertRaises(GeometryError):
            validate_layout(((0, 1), (1, 2)))
        with self.assertRaises(GeometryError):
            validate_layout(((0, 2),))

    def test_layout_parser_does_not_coerce_or_unorder_cells(self):
        for raw in (
            ((0, 1.5),),
            ((0, True),),
            ((0, "1"),),
            ({0, 1},),
        ):
            with self.subTest(raw=raw), self.assertRaises(GeometryError):
                parse_layout_text(repr(raw))

    def test_seeded_mutation_is_reproducible_and_valid(self):
        left = random.Random(12345)
        right = random.Random(12345)
        current_left = KNOWN_THREE
        current_right = KNOWN_THREE
        for _ in range(100):
            current_left = mutate(current_left, left)
            current_right = mutate(current_right, right)
            self.assertEqual(current_left, current_right)
            validate_layout(current_left)
            self.assertEqual(tuple(map(len, current_left)), (9, 8, 2))

    def test_mutations_have_no_self_transition(self):
        self.assertNotIn(KNOWN_THREE, legal_mutations(KNOWN_THREE))

    def test_console_description_records_extra_constraints(self):
        backend = ConsoleSolver(sys.executable, extra_constraints=("knight",))
        self.assertIn('extra_constraints=["knight"]', backend.description())


class RustIntegrationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        try:
            cls.solver = RustSolver()
        except Exception as error:  # pragma: no cover - build-dependent skip
            raise unittest.SkipTest(str(error)) from error

    def test_known_counts(self):
        unique = self.solver.count(BLUE_20, cap=2)
        self.assertEqual(unique.multiplicity, Multiplicity.UNIQUE)
        three = self.solver.count(KNOWN_THREE, cap=4)
        self.assertEqual((three.count, three.exact), (3, True))

    def test_bad_source_record_is_rejected(self):
        source = Path(__file__).resolve().parents[1] / "sources" / "min_thermos_9_8_2.txt"
        with self.assertRaises(GeometryError):
            read_legacy_layout(source, 1192)

    def test_corpus_validator_handles_zero_and_fails_on_invalid_geometry(self):
        with tempfile.TemporaryDirectory() as directory:
            corpus = Path(directory) / "corpus.txt"
            corpus.write_text(f"0;{UNSAT_TWO_ROWS!r}\n", encoding="utf-8")
            with redirect_stdout(StringIO()):
                self.assertEqual(
                    main(["validate-corpus", "--input", str(corpus)]),
                    0,
                )

            corpus.write_text("0;[(0, 1), (1, 2)]\n", encoding="utf-8")
            with redirect_stdout(StringIO()):
                self.assertEqual(
                    main(["validate-corpus", "--input", str(corpus)]),
                    1,
                )


class FakeSolver:
    def __init__(self, initial_layout):
        self.initial_layout = initial_layout

    def description(self):
        return "fake"

    def count(self, layout, *, cap):
        if layout != self.initial_layout:
            return CountResult(1, True, 0.0)
        return CountResult(min(10, cap), 10 < cap, 0.0)


class CensoredSolver:
    """Makes a lower-cap current score look deceptively small."""

    def __init__(self, initial_layout):
        self.initial_layout = initial_layout

    def description(self):
        return "censored-fake"

    def count(self, layout, *, cap):
        actual = 100_000 if layout == self.initial_layout else 2_000
        return CountResult(min(actual, cap), actual < cap, 0.0)


class AnnealTests(unittest.TestCase):
    def test_non_finite_settings_are_rejected(self):
        for changes in (
            {"start_temperature": math.inf},
            {"min_temperature": math.nan},
            {"alpha": math.inf},
            {"cap_factor": math.inf},
        ):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                AnnealConfig(**changes).validate()

    def test_unique_neighbor_halts(self):
        config = AnnealConfig(
            start_temperature=1.0,
            min_temperature=0.5,
            alpha=0.1,
            steps_per_temperature=200,
            reheats=1,
            initial_cap=10,
            min_cap=2,
            max_cap=100,
            print_threshold=None,
        )
        result = anneal(KNOWN_THREE, FakeSolver(KNOWN_THREE), config, seed=7)
        self.assertTrue(result.found_unique)
        self.assertEqual(result.proposals, 1)
        self.assertEqual(result.best.count, 1)

    def test_capped_scores_are_compared_at_one_cap(self):
        config = AnnealConfig(
            start_temperature=0.001,
            min_temperature=0.0005,
            alpha=0.1,
            steps_per_temperature=1,
            reheats=1,
            initial_cap=1_000,
            cap_factor=4,
            min_cap=2,
            max_cap=5_000,
            print_threshold=None,
        )
        result = anneal(KNOWN_THREE, CensoredSolver(KNOWN_THREE), config, seed=7)
        self.assertEqual((result.best.count, result.best.exact), (2_000, True))
        self.assertEqual(result.accepted, 1)

    def test_best_includes_an_improving_but_rejected_proposal(self):
        config = AnnealConfig(
            start_temperature=0.001,
            min_temperature=0.0005,
            alpha=0.1,
            steps_per_temperature=1,
            reheats=1,
            initial_cap=1_000,
            cap_factor=4,
            min_cap=2,
            max_cap=5_000,
            print_threshold=None,
        )
        result = anneal(KNOWN_THREE, CensoredSolver(KNOWN_THREE), config, seed=111)
        self.assertEqual((result.best.count, result.best.exact), (2_000, True))
        self.assertEqual(result.accepted, 0)


if __name__ == "__main__":
    unittest.main()
