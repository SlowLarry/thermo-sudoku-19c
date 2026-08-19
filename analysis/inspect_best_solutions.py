import ast
import json
import os
import subprocess
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RESULTS = ROOT / "sources" / "min_thermos_9_8_2.txt"
solver_value = os.environ.get("SUDOKU_SOLVER")
if not solver_value:
    raise SystemExit("Set SUDOKU_SOLVER to SudokuSolverConsole.exe")
SOLVER = Path(solver_value).expanduser().resolve()
if not SOLVER.is_file():
    raise SystemExit(f"Sudoku solver does not exist: {SOLVER}")


def encode(path):
    return "".join(f"R{cell // 9 + 1}C{cell % 9 + 1}" for cell in path)


def solve(paths, include_solutions=False):
    args = [
        str(SOLVER), "--json", "-b=9", "-n", "-x=100000", "--hide-banner",
        *[f"-c=thermo:{encode(path)}" for path in paths],
    ]
    if include_solutions:
        args.append("-o=json")
    return json.loads(subprocess.check_output(args))


rows = []
for line_number, line in enumerate(RESULTS.read_text().splitlines(), 1):
    count_text, paths_text = line.split(";", 1)
    if int(count_text) != 3:
        continue
    rows.append((line_number, tuple(tuple(p) for p in ast.literal_eval(paths_text))))

solution_sets = defaultdict(list)
records = []
for line_number, paths in rows:
    base = solve(paths[:2])
    full = solve(paths, include_solutions=True)
    solutions = tuple(sorted(full["solutions"]))
    solution_sets[solutions].append(line_number)
    edge = paths[2]
    records.append((line_number, paths, solutions, base["count"]))
    print(
        line_number,
        "base", base["count"],
        "edge", edge,
        "forward", full["count"],
        "solution_set", len(solution_sets),
    )

print("distinct_solution_sets", len(solution_sets))
print("solution_set_line_groups", list(solution_sets.values()))
for i, (solutions, line_numbers) in enumerate(solution_sets.items(), 1):
    print("SET", i, "LINES", line_numbers)
    for solution in solutions:
        print(solution)

first_paths = records[0][1]
first_solutions = records[0][2]
for line_number, paths, solutions, _ in records:
    # A length-9 thermo assigns digit i+1 to path cell i.  The shared footprint
    # therefore determines the digit relabeling induced by a new path order.
    new_position = {cell: i + 1 for i, cell in enumerate(paths[0])}
    relabel = {str(i + 1): str(new_position[cell]) for i, cell in enumerate(first_paths[0])}
    transformed = tuple(sorted("".join(relabel[d] for d in s) for s in first_solutions))
    print("digit_relabel", line_number, relabel, "maps_first_set", transformed == solutions)
