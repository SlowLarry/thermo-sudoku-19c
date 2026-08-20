//! Proof-oriented CNF master for non-overlapping thermometer layouts.
//!
//! It converts a validated `thermo-global-cegis-v1` grid-pair checkpoint into
//! a deterministic DIMACS master, strictly validates complete SAT models, and
//! can run a bounded CaDiCaL-compatible solve/check/learn sidecar loop.  A
//! satisfying assignment is a classic Sudoku together with a vertex-disjoint
//! union of directed king paths covering at most 19 cells and hitting every
//! supplied pair cut.  An UNSAT proof for the emitted CNF is therefore a
//! checkable negative certificate for the 19-cell question.

use std::collections::HashSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use thermo_sudoku::{Multiplicity, Solver};

const CELLS: usize = 81;
const DIGITS: usize = 9;
const DIRECTED_EDGES: usize = 544;
const UNDIRECTED_EDGES: usize = DIRECTED_EDGES / 2;
const COVER_LIMIT: usize = 19;
const CHECKPOINT_HEADER: &str = "# thermo-global-cegis-v1";
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

// Deterministic DIMACS variable ranges (all inclusive):
//   1..729       digit(cell, digit)
//   730..1273    selected directed edge
//   1274..1354   occupied cell
//   1355..2874   Sinz at-most-19 auxiliaries (80 * 19)
//   2875..7226   adjacent-symbol-swap witnesses (8 * 544)
const DIGIT_BASE: i32 = 1;
const EDGE_BASE: i32 = DIGIT_BASE + (CELLS * DIGITS) as i32;
const OCCUPIED_BASE: i32 = EDGE_BASE + DIRECTED_EDGES as i32;
const SEQUENTIAL_BASE: i32 = OCCUPIED_BASE + CELLS as i32;
const SEQUENTIAL_VARIABLES: usize = (CELLS - 1) * COVER_LIMIT;
const SWAP_BASE: i32 = SEQUENTIAL_BASE + SEQUENTIAL_VARIABLES as i32;
const SWAP_VARIABLES: usize = 8 * DIRECTED_EDGES;
const VARIABLE_COUNT: i32 = SWAP_BASE + SWAP_VARIABLES as i32 - 1;

type Grid = [u8; CELLS];
type Clause = Vec<i32>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DirectedEdge {
    lower: u8,
    upper: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GridPair {
    first: Grid,
    second: Grid,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PairCut([u64; DIRECTED_EDGES.div_ceil(64)]);

#[derive(Debug)]
struct Checkpoint {
    budget: usize,
    checksum: u64,
    pairs: Vec<GridPair>,
    cuts: Vec<PairCut>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SatStatus {
    Satisfiable,
    Unsatisfiable,
    Unknown,
}

#[derive(Debug)]
struct SatResult {
    status: SatStatus,
    assignment: Option<Vec<bool>>,
}

#[derive(Debug)]
struct DecodedCandidate {
    target: Grid,
    selected: Vec<usize>,
    paths: Vec<Vec<u8>>,
    covered_cells: usize,
}

impl GridPair {
    fn new(left: Grid, right: Grid) -> Result<Self, String> {
        if left == right {
            return Err("a learned pair must contain two different grids".into());
        }
        Ok(if left < right {
            Self {
                first: left,
                second: right,
            }
        } else {
            Self {
                first: right,
                second: left,
            }
        })
    }
}

fn digit_var(cell: usize, digit_index: usize) -> i32 {
    debug_assert!(cell < CELLS && digit_index < DIGITS);
    DIGIT_BASE + (cell * DIGITS + digit_index) as i32
}

fn edge_var(edge: usize) -> i32 {
    debug_assert!(edge < DIRECTED_EDGES);
    EDGE_BASE + edge as i32
}

fn occupied_var(cell: usize) -> i32 {
    debug_assert!(cell < CELLS);
    OCCUPIED_BASE + cell as i32
}

/// `prefix` is 0..80 for prefixes ending at cell 0..79, and `count` is
/// 0..19 for the proposition "this prefix contains at least count + 1 cells".
fn sequential_var(prefix: usize, count: usize) -> i32 {
    debug_assert!(prefix < CELLS - 1 && count < COVER_LIMIT);
    SEQUENTIAL_BASE + (prefix * COVER_LIMIT + count) as i32
}

fn swap_var(digit_index: usize, edge: usize) -> i32 {
    debug_assert!(digit_index < 8 && edge < DIRECTED_EDGES);
    SWAP_BASE + (digit_index * DIRECTED_EDGES + edge) as i32
}

fn directed_edges() -> Vec<DirectedEdge> {
    let mut result = Vec::with_capacity(DIRECTED_EDGES);
    for left in 0..CELLS {
        for right in left + 1..CELLS {
            let row_distance = (left / 9).abs_diff(right / 9);
            let column_distance = (left % 9).abs_diff(right % 9);
            if row_distance <= 1 && column_distance <= 1 {
                result.push(DirectedEdge {
                    lower: left as u8,
                    upper: right as u8,
                });
                result.push(DirectedEdge {
                    lower: right as u8,
                    upper: left as u8,
                });
            }
        }
    }
    assert_eq!(result.len(), DIRECTED_EDGES);
    result
}

fn push_at_most_one(clauses: &mut Vec<Clause>, variables: &[i32]) {
    for left in 0..variables.len() {
        for right in left + 1..variables.len() {
            clauses.push(vec![-variables[left], -variables[right]]);
        }
    }
}

fn push_exactly_one(clauses: &mut Vec<Clause>, variables: &[i32]) {
    clauses.push(variables.to_vec());
    push_at_most_one(clauses, variables);
}

fn classic_sudoku_clauses(clauses: &mut Vec<Clause>) {
    for cell in 0..CELLS {
        push_exactly_one(
            clauses,
            &(0..DIGITS)
                .map(|digit| digit_var(cell, digit))
                .collect::<Vec<_>>(),
        );
    }

    for digit in 0..DIGITS {
        for row in 0..9 {
            push_exactly_one(
                clauses,
                &(0..9)
                    .map(|column| digit_var(row * 9 + column, digit))
                    .collect::<Vec<_>>(),
            );
        }
        for column in 0..9 {
            push_exactly_one(
                clauses,
                &(0..9)
                    .map(|row| digit_var(row * 9 + column, digit))
                    .collect::<Vec<_>>(),
            );
        }
        for box_index in 0..9 {
            let box_row = (box_index / 3) * 3;
            let box_column = (box_index % 3) * 3;
            push_exactly_one(
                clauses,
                &(0..9)
                    .map(|position| {
                        digit_var(
                            (box_row + position / 3) * 9 + box_column + position % 3,
                            digit,
                        )
                    })
                    .collect::<Vec<_>>(),
            );
        }
    }
}

fn comparison_clauses(clauses: &mut Vec<Clause>, edges: &[DirectedEdge]) {
    for (edge_id, edge) in edges.iter().enumerate() {
        let selected = edge_var(edge_id);
        for lower_digit in 0..DIGITS {
            for upper_digit in 0..=lower_digit {
                clauses.push(vec![
                    -selected,
                    -digit_var(edge.lower as usize, lower_digit),
                    -digit_var(edge.upper as usize, upper_digit),
                ]);
            }
        }
    }
}

fn topology_clauses(clauses: &mut Vec<Clause>, edges: &[DirectedEdge]) {
    let mut incoming = vec![Vec::<i32>::new(); CELLS];
    let mut outgoing = vec![Vec::<i32>::new(); CELLS];
    let mut incident = vec![Vec::<i32>::new(); CELLS];

    for (edge_id, edge) in edges.iter().enumerate() {
        let selected = edge_var(edge_id);
        let lower = edge.lower as usize;
        let upper = edge.upper as usize;
        outgoing[lower].push(selected);
        incoming[upper].push(selected);
        incident[lower].push(selected);
        incident[upper].push(selected);
        clauses.push(vec![-selected, occupied_var(lower)]);
        clauses.push(vec![-selected, occupied_var(upper)]);
    }

    for cell in 0..CELLS {
        let mut occupied_implies_incident = vec![-occupied_var(cell)];
        occupied_implies_incident.extend(incident[cell].iter().copied());
        clauses.push(occupied_implies_incident);
        push_at_most_one(clauses, &incoming[cell]);
        push_at_most_one(clauses, &outgoing[cell]);
    }

    // A pair of opposite arcs is a directed 2-cycle.  Longer directed cycles
    // are impossible because selected arcs also impose strict digit increase.
    for pair in 0..UNDIRECTED_EDGES {
        clauses.push(vec![-edge_var(2 * pair), -edge_var(2 * pair + 1)]);
    }
}

fn coverage_clauses(clauses: &mut Vec<Clause>) {
    // Sinz sequential-counter encoding of sum(occupied[0..81]) <= 19.
    clauses.push(vec![-occupied_var(0), sequential_var(0, 0)]);
    for cell in 1..CELLS - 1 {
        clauses.push(vec![-occupied_var(cell), sequential_var(cell, 0)]);
        clauses.push(vec![-sequential_var(cell - 1, 0), sequential_var(cell, 0)]);
        for count in 1..COVER_LIMIT {
            clauses.push(vec![
                -occupied_var(cell),
                -sequential_var(cell - 1, count - 1),
                sequential_var(cell, count),
            ]);
            clauses.push(vec![
                -sequential_var(cell - 1, count),
                sequential_var(cell, count),
            ]);
        }
    }
    for cell in 1..CELLS {
        clauses.push(vec![
            -occupied_var(cell),
            -sequential_var(cell - 1, COVER_LIMIT - 1),
        ]);
    }
}

fn adjacent_swap_necessity_clauses(clauses: &mut Vec<Clause>, edges: &[DirectedEdge]) {
    // If swapping symbols d and d+1 in the existential target is not to leave
    // every selected comparison unchanged, at least one selected arc must
    // join exactly d to d+1.  The auxiliaries are one-way witnesses; the long
    // clause forces one real witness for each d.
    for digit_index in 0..8 {
        let mut witnesses = Vec::with_capacity(DIRECTED_EDGES);
        for (edge_id, edge) in edges.iter().enumerate() {
            let witness = swap_var(digit_index, edge_id);
            witnesses.push(witness);
            clauses.push(vec![-witness, edge_var(edge_id)]);
            clauses.push(vec![-witness, digit_var(edge.lower as usize, digit_index)]);
            clauses.push(vec![
                -witness,
                digit_var(edge.upper as usize, digit_index + 1),
            ]);
        }
        clauses.push(witnesses);
    }
}

fn base_clauses(edges: &[DirectedEdge]) -> Vec<Clause> {
    let mut clauses = Vec::new();
    classic_sudoku_clauses(&mut clauses);
    comparison_clauses(&mut clauses, edges);
    topology_clauses(&mut clauses, edges);
    coverage_clauses(&mut clauses);
    adjacent_swap_necessity_clauses(&mut clauses, edges);
    clauses
}

fn pair_cut(pair: &GridPair, edges: &[DirectedEdge]) -> PairCut {
    let mut cut = PairCut([0; DIRECTED_EDGES.div_ceil(64)]);
    for (edge_id, edge) in edges.iter().enumerate() {
        let lower = edge.lower as usize;
        let upper = edge.upper as usize;
        if !(pair.first[lower] < pair.first[upper] && pair.second[lower] < pair.second[upper]) {
            cut.0[edge_id / 64] |= 1u64 << (edge_id % 64);
        }
    }
    cut
}

fn pair_clause(cut: PairCut) -> Clause {
    let mut clause = Vec::new();
    for edge_id in 0..DIRECTED_EDGES {
        if cut.0[edge_id / 64] & (1u64 << (edge_id % 64)) != 0 {
            clause.push(edge_var(edge_id));
        }
    }
    clause
}

fn validate_unit(grid: &Grid, cells: impl Iterator<Item = usize>) -> bool {
    let mut seen = 0u16;
    for cell in cells {
        let digit = grid[cell];
        if !(1..=9).contains(&digit) {
            return false;
        }
        let bit = 1u16 << (digit - 1);
        if seen & bit != 0 {
            return false;
        }
        seen |= bit;
    }
    seen == 0x01ff
}

fn validate_sudoku(grid: &Grid) -> bool {
    for row in 0..9 {
        if !validate_unit(grid, (0..9).map(|column| row * 9 + column)) {
            return false;
        }
    }
    for column in 0..9 {
        if !validate_unit(grid, (0..9).map(|row| row * 9 + column)) {
            return false;
        }
    }
    for box_index in 0..9 {
        let box_row = (box_index / 3) * 3;
        let box_column = (box_index % 3) * 3;
        if !validate_unit(
            grid,
            (0..9).map(|position| (box_row + position / 3) * 9 + box_column + position % 3),
        ) {
            return false;
        }
    }
    true
}

fn parse_grid(text: &str) -> Result<Grid, String> {
    if text.len() != CELLS || !text.bytes().all(|byte| (b'1'..=b'9').contains(&byte)) {
        return Err("expected exactly 81 ASCII digits 1-9".into());
    }
    let mut grid = [0u8; CELLS];
    for (cell, byte) in text.bytes().enumerate() {
        grid[cell] = byte - b'0';
    }
    if !validate_sudoku(&grid) {
        return Err("grid is not a solved classic Sudoku".into());
    }
    Ok(grid)
}

fn fnv_byte(hash: &mut u64, byte: u8) {
    *hash ^= u64::from(byte);
    *hash = hash.wrapping_mul(FNV_PRIME);
}

fn edges_checksum(edges: &[DirectedEdge]) -> u64 {
    let mut checksum = FNV_OFFSET;
    for edge in edges {
        fnv_byte(&mut checksum, edge.lower);
        fnv_byte(&mut checksum, edge.upper);
    }
    checksum
}

fn load_checkpoint(path: &Path) -> Result<Checkpoint, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut lines = contents.lines().enumerate();
    if lines.next().map(|(_, line)| line) != Some(CHECKPOINT_HEADER) {
        return Err("wrong or missing checkpoint schema header".into());
    }

    let mut budget = None;
    let mut declared_edges = None;
    let mut declared_pairs = None;
    let mut declared_checksum = None;
    let mut footer = None;
    let mut data_started = false;
    let mut checksum = FNV_OFFSET;
    let mut pairs = Vec::new();

    for (zero_line, line) in lines {
        let line_number = zero_line + 1;
        if line.is_empty() {
            return Err(format!("line {line_number}: blank lines are not allowed"));
        }
        if let Some(value) = line.strip_prefix("# end pairs=") {
            if footer.is_some() {
                return Err(format!("line {line_number}: duplicate footer"));
            }
            let (count, hash) = value
                .split_once(" fnv1a64=")
                .ok_or_else(|| format!("line {line_number}: malformed footer"))?;
            footer = Some((
                count
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_number}: invalid footer count"))?,
                u64::from_str_radix(hash, 16)
                    .map_err(|_| format!("line {line_number}: invalid footer checksum"))?,
            ));
            continue;
        }
        if footer.is_some() {
            return Err(format!("line {line_number}: data after footer"));
        }
        if let Some(value) = line.strip_prefix("# budget=") {
            if data_started || budget.is_some() {
                return Err(format!("line {line_number}: misplaced or duplicate budget"));
            }
            budget = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_number}: invalid budget"))?,
            );
            continue;
        }
        if let Some(value) = line.strip_prefix("# directed_edges=") {
            if data_started || declared_edges.is_some() {
                return Err(format!(
                    "line {line_number}: misplaced or duplicate directed_edges"
                ));
            }
            declared_edges = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_number}: invalid directed_edges"))?,
            );
            continue;
        }
        if let Some(value) = line.strip_prefix("# pairs=") {
            if data_started || declared_pairs.is_some() {
                return Err(format!("line {line_number}: misplaced or duplicate pairs"));
            }
            declared_pairs = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_number}: invalid pair count"))?,
            );
            continue;
        }
        if let Some(value) = line.strip_prefix("# fnv1a64=") {
            if data_started || declared_checksum.is_some() {
                return Err(format!(
                    "line {line_number}: misplaced or duplicate checksum"
                ));
            }
            declared_checksum = Some(
                u64::from_str_radix(value, 16)
                    .map_err(|_| format!("line {line_number}: invalid checksum"))?,
            );
            continue;
        }
        if line.starts_with('#') {
            return Err(format!("line {line_number}: unexpected metadata {line:?}"));
        }

        data_started = true;
        let (first_text, second_text) = line
            .split_once('|')
            .ok_or_else(|| format!("line {line_number}: missing pair separator"))?;
        if second_text.contains('|') {
            return Err(format!("line {line_number}: too many pair separators"));
        }
        if first_text >= second_text {
            return Err(format!(
                "line {line_number}: pair grids are not distinct and canonically ordered"
            ));
        }
        let first = parse_grid(first_text)
            .map_err(|error| format!("line {line_number}, first grid: {error}"))?;
        let second = parse_grid(second_text)
            .map_err(|error| format!("line {line_number}, second grid: {error}"))?;
        for byte in first {
            fnv_byte(&mut checksum, byte);
        }
        fnv_byte(&mut checksum, 0xfe);
        for byte in second {
            fnv_byte(&mut checksum, byte);
        }
        fnv_byte(&mut checksum, 0xff);
        pairs.push(GridPair { first, second });
    }

    let expected = (pairs.len(), checksum);
    if declared_edges != Some(DIRECTED_EDGES) {
        return Err(format!(
            "checkpoint declares {:?} directed edges, expected {DIRECTED_EDGES}",
            declared_edges
        ));
    }
    if declared_pairs != Some(pairs.len())
        || declared_checksum != Some(checksum)
        || footer != Some(expected)
        || budget.is_none()
    {
        return Err(format!(
            "checkpoint metadata/checksum mismatch (computed pairs={}, fnv1a64={checksum:016x})",
            pairs.len()
        ));
    }
    let edges = directed_edges();
    let mut seen_cuts = HashSet::new();
    let cuts = pairs
        .iter()
        .map(|pair| pair_cut(pair, &edges))
        .filter(|cut| seen_cuts.insert(*cut))
        .collect();
    Ok(Checkpoint {
        budget: budget.expect("checked above"),
        checksum,
        pairs,
        cuts,
    })
}

fn pairs_checksum(pairs: &[GridPair]) -> u64 {
    let mut checksum = FNV_OFFSET;
    for pair in pairs {
        for byte in pair.first {
            fnv_byte(&mut checksum, byte);
        }
        fnv_byte(&mut checksum, 0xfe);
        for byte in pair.second {
            fnv_byte(&mut checksum, byte);
        }
        fnv_byte(&mut checksum, 0xff);
    }
    checksum
}

fn format_grid(grid: &Grid) -> String {
    grid.iter().map(|digit| char::from(b'0' + digit)).collect()
}

fn write_checkpoint(checkpoint: &Checkpoint, output: &Path) -> Result<(), String> {
    let checksum = pairs_checksum(&checkpoint.pairs);
    if checksum != checkpoint.checksum {
        return Err("internal checkpoint checksum is stale".into());
    }
    let file = fs::File::create(output)
        .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "{CHECKPOINT_HEADER}")
        .and_then(|_| writeln!(writer, "# budget={}", checkpoint.budget))
        .and_then(|_| writeln!(writer, "# directed_edges={DIRECTED_EDGES}"))
        .and_then(|_| writeln!(writer, "# pairs={}", checkpoint.pairs.len()))
        .and_then(|_| writeln!(writer, "# fnv1a64={checksum:016x}"))
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    for pair in &checkpoint.pairs {
        writeln!(
            writer,
            "{}|{}",
            format_grid(&pair.first),
            format_grid(&pair.second)
        )
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    }
    writeln!(
        writer,
        "# end pairs={} fnv1a64={checksum:016x}",
        checkpoint.pairs.len()
    )
    .and_then(|_| writer.flush())
    .map_err(|error| format!("cannot finish {}: {error}", output.display()))?;
    Ok(())
}

fn parse_sat_result(text: &str) -> Result<SatResult, String> {
    let mut status = None;
    let mut values = vec![None::<bool>; VARIABLE_COUNT as usize + 1];
    let mut saw_literal = false;
    for (line_index, raw_line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('c') {
            continue;
        }
        if let Some(value) = line.strip_prefix("s ") {
            let parsed = match value.trim() {
                "SATISFIABLE" => SatStatus::Satisfiable,
                "UNSATISFIABLE" => SatStatus::Unsatisfiable,
                "UNKNOWN" => SatStatus::Unknown,
                other => {
                    return Err(format!(
                        "model line {line_number}: unknown status {other:?}"
                    ));
                }
            };
            if status.replace(parsed).is_some_and(|old| old != parsed) {
                return Err("model contains conflicting status lines".into());
            }
            continue;
        }
        let Some(literals) = line.strip_prefix('v') else {
            return Err(format!(
                "model line {line_number}: expected a comment, status, or witness line"
            ));
        };
        if !literals.is_empty() && !literals.starts_with(char::is_whitespace) {
            return Err(format!(
                "model line {line_number}: malformed witness prefix"
            ));
        }
        for token in literals.split_whitespace() {
            let literal = token.parse::<i32>().map_err(|_| {
                format!("model line {line_number}: invalid literal token {token:?}")
            })?;
            if literal == 0 {
                continue;
            }
            let variable = literal.unsigned_abs() as usize;
            if variable > VARIABLE_COUNT as usize {
                return Err(format!(
                    "model line {line_number}: variable {variable} exceeds {VARIABLE_COUNT}"
                ));
            }
            saw_literal = true;
            let value = literal > 0;
            if values[variable]
                .replace(value)
                .is_some_and(|old| old != value)
            {
                return Err(format!(
                    "model line {line_number}: variable {variable} has conflicting values"
                ));
            }
        }
    }

    let status = status.ok_or_else(|| "model has no status line".to_string())?;
    let assignment = if status == SatStatus::Satisfiable {
        if !saw_literal {
            return Err("SAT model contains no witness literals".into());
        }
        let missing = (1..=VARIABLE_COUNT as usize)
            .filter(|&variable| values[variable].is_none())
            .take(8)
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(format!(
                "SAT model is partial; first missing variables are {missing:?}"
            ));
        }
        Some(
            values
                .into_iter()
                .map(|value| value.unwrap_or(false))
                .collect(),
        )
    } else {
        if saw_literal {
            return Err("non-SAT result unexpectedly contains witness literals".into());
        }
        None
    };
    Ok(SatResult { status, assignment })
}

fn clause_satisfied(clause: &[i32], assignment: &[bool]) -> bool {
    clause.iter().any(|&literal| {
        let value = assignment[literal.unsigned_abs() as usize];
        if literal > 0 { value } else { !value }
    })
}

fn decode_candidate(
    checkpoint: &Checkpoint,
    assignment: &[bool],
) -> Result<DecodedCandidate, String> {
    if assignment.len() != VARIABLE_COUNT as usize + 1 {
        return Err(format!(
            "assignment has {} entries, expected {}",
            assignment.len(),
            VARIABLE_COUNT + 1
        ));
    }
    let edges = directed_edges();
    let base = base_clauses(&edges);
    if let Some((index, _)) = base
        .iter()
        .enumerate()
        .find(|(_, clause)| !clause_satisfied(clause, assignment))
    {
        return Err(format!("model violates base CNF clause {}", index + 1));
    }
    for (cut_index, &cut) in checkpoint.cuts.iter().enumerate() {
        if !clause_satisfied(&pair_clause(cut), assignment) {
            return Err(format!(
                "model violates checkpoint pair-cut clause {}",
                cut_index + 1
            ));
        }
    }

    let mut target = [0u8; CELLS];
    for (cell, target_digit) in target.iter_mut().enumerate() {
        let selected_digits = (0..DIGITS)
            .filter(|&digit| assignment[digit_var(cell, digit) as usize])
            .collect::<Vec<_>>();
        if selected_digits.len() != 1 {
            return Err(format!(
                "model selects {} digits for cell {cell}",
                selected_digits.len()
            ));
        }
        *target_digit = selected_digits[0] as u8 + 1;
    }
    if !validate_sudoku(&target) {
        return Err("decoded target is not a solved classic Sudoku".into());
    }

    let selected = (0..DIRECTED_EDGES)
        .filter(|&edge| assignment[edge_var(edge) as usize])
        .collect::<Vec<_>>();
    let mut incoming = [None::<usize>; CELLS];
    let mut outgoing = [None::<usize>; CELLS];
    let mut occupied = [false; CELLS];
    for &edge_id in &selected {
        let edge = edges[edge_id];
        let lower = edge.lower as usize;
        let upper = edge.upper as usize;
        if target[lower] >= target[upper] {
            return Err(format!(
                "selected edge {edge_id} ({lower}>{upper}) is false in the target"
            ));
        }
        if outgoing[lower].replace(edge_id).is_some() {
            return Err(format!("cell {lower} has two outgoing selected edges"));
        }
        if incoming[upper].replace(edge_id).is_some() {
            return Err(format!("cell {upper} has two incoming selected edges"));
        }
        occupied[lower] = true;
        occupied[upper] = true;
    }

    let covered_cells = occupied.iter().filter(|&&value| value).count();
    if covered_cells > COVER_LIMIT {
        return Err(format!(
            "decoded layout covers {covered_cells} cells, exceeding {COVER_LIMIT}"
        ));
    }
    let mut visited_edges = vec![false; DIRECTED_EDGES];
    let mut paths = Vec::new();
    for cell in 0..CELLS {
        if incoming[cell].is_some() || outgoing[cell].is_none() {
            continue;
        }
        let mut path = vec![cell as u8];
        let mut current = cell;
        while let Some(edge_id) = outgoing[current] {
            if visited_edges[edge_id] {
                return Err("selected graph contains a directed cycle".into());
            }
            visited_edges[edge_id] = true;
            let next = edges[edge_id].upper;
            path.push(next);
            current = next as usize;
            if path.len() > 9 {
                return Err("decoded thermometer has more than nine cells".into());
            }
        }
        paths.push(path);
    }
    if selected.iter().any(|&edge| !visited_edges[edge]) {
        return Err("selected graph contains an unrooted directed cycle".into());
    }
    paths.sort_unstable();
    let solver = Solver::blank(&paths)
        .map_err(|error| format!("decoded paths fail the thermo layout validator: {error}"))?;
    if solver.layout().covered_cells() != covered_cells {
        return Err("decoded paths and occupied-cell count disagree".into());
    }
    Ok(DecodedCandidate {
        target,
        selected,
        paths,
        covered_cells,
    })
}

fn format_paths(paths: &[Vec<u8>]) -> String {
    paths
        .iter()
        .map(|path| path.iter().map(u8::to_string).collect::<Vec<_>>().join(","))
        .collect::<Vec<_>>()
        .join("|")
}

fn format_candidate_body(candidate: &DecodedCandidate, edges: &[DirectedEdge]) -> String {
    let selected_edges = candidate
        .selected
        .iter()
        .map(|&edge_id| {
            let edge = edges[edge_id];
            format!("{}>{}", edge.lower, edge.upper)
        })
        .collect::<Vec<_>>()
        .join(";");
    let selected_edge_ids = candidate
        .selected
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(";");
    format!(
        "target={}\nthermos={}\ncovered_cells={}\nthermometer_count={}\ncomparison_count={}\nselected_edge_ids={}\nselected_edges={}\n",
        format_grid(&candidate.target),
        format_paths(&candidate.paths),
        candidate.covered_cells,
        candidate.paths.len(),
        candidate.selected.len(),
        selected_edge_ids,
        selected_edges
    )
}

fn format_candidate(candidate: &DecodedCandidate, edges: &[DirectedEdge]) -> String {
    format!("status=sat\n{}", format_candidate_body(candidate, edges))
}

fn invoke_sat(
    executable: &Path,
    cnf: &Path,
    model: &Path,
    proof: Option<&Path>,
    conflicts: Option<u64>,
) -> Result<SatResult, String> {
    if model.exists() {
        fs::remove_file(model)
            .map_err(|error| format!("cannot remove stale model {}: {error}", model.display()))?;
    }
    if let Some(proof) = proof
        && proof.exists()
    {
        fs::remove_file(proof)
            .map_err(|error| format!("cannot remove stale proof {}: {error}", proof.display()))?;
    }
    let mut command = Command::new(executable);
    command.arg("-q").arg("-w").arg(model);
    if let Some(conflicts) = conflicts {
        command.arg("-c").arg(conflicts.to_string());
    }
    command.arg(cnf);
    if let Some(proof) = proof {
        command.arg(proof);
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot run {}: {error}", executable.display()))?;
    let code = output.status.code();
    if !matches!(code, Some(0 | 10 | 20)) {
        return Err(format!(
            "SAT process exited with {:?}: {}",
            code,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let model_text = if model.exists() {
        fs::read_to_string(model)
            .map_err(|error| format!("cannot read model {}: {error}", model.display()))?
    } else {
        String::from_utf8(output.stdout)
            .map_err(|_| "SAT solver stdout is not valid UTF-8".to_string())?
    };
    let result = parse_sat_result(&model_text)?;
    let expected_code = match result.status {
        SatStatus::Satisfiable => 10,
        SatStatus::Unsatisfiable => 20,
        SatStatus::Unknown => 0,
    };
    if code != Some(expected_code) {
        return Err(format!(
            "SAT result {:?} disagrees with process exit code {:?}",
            result.status, code
        ));
    }
    Ok(result)
}

fn write_clause(writer: &mut impl Write, clause: &[i32]) -> std::io::Result<()> {
    for literal in clause {
        write!(writer, "{literal} ")?;
    }
    writeln!(writer, "0")
}

fn write_cnf(checkpoint: &Checkpoint, output: &Path) -> Result<(usize, usize), String> {
    let edges = directed_edges();
    let edge_checksum = edges_checksum(&edges);
    let base = base_clauses(&edges);
    let clause_count = base.len() + checkpoint.cuts.len();
    let file = fs::File::create(output)
        .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "c thermo-topology-cnf-v1")
        .and_then(|_| {
            writeln!(
                writer,
                "c model classic-sudoku plus disjoint-directed-king-paths"
            )
        })
        .and_then(|_| writeln!(writer, "c covered_cells_at_most {COVER_LIMIT}"))
        .and_then(|_| writeln!(writer, "c diagonal_crossings_without_shared_cells allowed"))
        .and_then(|_| writeln!(writer, "c checkpoint_budget {}", checkpoint.budget))
        .and_then(|_| writeln!(writer, "c checkpoint_pairs {}", checkpoint.pairs.len()))
        .and_then(|_| writeln!(writer, "c unique_pair_cuts {}", checkpoint.cuts.len()))
        .and_then(|_| writeln!(writer, "c checkpoint_fnv1a64 {:016x}", checkpoint.checksum))
        .and_then(|_| writeln!(writer, "c digit_variables 1 729"))
        .and_then(|_| writeln!(writer, "c edge_variables 730 1273"))
        .and_then(|_| writeln!(writer, "c occupied_variables 1274 1354"))
        .and_then(|_| writeln!(writer, "c sequential_variables 1355 2874"))
        .and_then(|_| writeln!(writer, "c swap_witness_variables 2875 7226"))
        .and_then(|_| {
            writeln!(
                writer,
                "c edge_order lexicographic unordered cell pair then forward and reverse"
            )
        })
        .and_then(|_| writeln!(writer, "c edge_order_fnv1a64 {edge_checksum:016x}"))
        .and_then(|_| writeln!(writer, "c digit_var 1+9*cell+(digit-1)"))
        .and_then(|_| writeln!(writer, "c edge_var 730+edge_id"))
        .and_then(|_| writeln!(writer, "c occupied_var 1274+cell"))
        .and_then(|_| writeln!(writer, "c sequential_var 1355+19*prefix+count"))
        .and_then(|_| writeln!(writer, "c swap_var 2875+544*(digit-1)+edge_id"))
        .and_then(|_| writeln!(writer, "p cnf {VARIABLE_COUNT} {clause_count}"))
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;

    for clause in &base {
        write_clause(&mut writer, clause)
            .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    }
    for &cut in &checkpoint.cuts {
        write_clause(&mut writer, &pair_clause(cut))
            .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    }
    writer
        .flush()
        .map_err(|error| format!("cannot finish {}: {error}", output.display()))?;
    Ok((VARIABLE_COUNT as usize, clause_count))
}

fn find_once(haystack: &[u8], needle: &[u8], label: &str) -> Result<usize, String> {
    let matches = haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, window)| (window == needle).then_some(index))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "CNF prefix contains {} occurrences of the expected {label}",
            matches.len()
        ));
    }
    Ok(matches[0])
}

/// Append one pair clause and patch the fixed prefix in place.  When a decimal
/// field crosses a width boundary, fall back to a deterministic full rewrite.
/// In either case the result is byte-for-byte identical to `write_cnf(after)`.
fn append_pair_to_cnf(
    path: &Path,
    before_pairs: usize,
    before_cuts: usize,
    before_checksum: u64,
    after: &Checkpoint,
    pair: &GridPair,
) -> Result<(), String> {
    if after.pairs.len() != before_pairs + 1
        || after.cuts.len() != before_cuts + 1
        || after.pairs.last() != Some(pair)
        || after.checksum != pairs_checksum(&after.pairs)
    {
        return Err("invalid before/after state for incremental CNF append".into());
    }
    let base_count = base_clauses(&directed_edges()).len();
    let old_pairs = format!("c checkpoint_pairs {before_pairs}\n");
    let new_pairs = format!("c checkpoint_pairs {}\n", after.pairs.len());
    let old_cuts = format!("c unique_pair_cuts {before_cuts}\n");
    let new_cuts = format!("c unique_pair_cuts {}\n", after.cuts.len());
    let old_checksum = format!("c checkpoint_fnv1a64 {before_checksum:016x}\n");
    let new_checksum = format!("c checkpoint_fnv1a64 {:016x}\n", after.checksum);
    let old_header = format!("p cnf {VARIABLE_COUNT} {}\n", base_count + before_cuts);
    let new_header = format!("p cnf {VARIABLE_COUNT} {}\n", base_count + after.cuts.len());
    if old_pairs.len() != new_pairs.len()
        || old_cuts.len() != new_cuts.len()
        || old_header.len() != new_header.len()
    {
        write_cnf(after, path)?;
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("cannot open {} for append: {error}", path.display()))?;
    let mut prefix = vec![0u8; 4096];
    let read = file
        .read(&mut prefix)
        .map_err(|error| format!("cannot read CNF prefix {}: {error}", path.display()))?;
    prefix.truncate(read);
    let pair_offset = find_once(&prefix, old_pairs.as_bytes(), "pair count")?;
    let cut_offset = find_once(&prefix, old_cuts.as_bytes(), "unique pair-cut count")?;
    let checksum_offset = find_once(&prefix, old_checksum.as_bytes(), "checkpoint checksum")?;
    let header_offset = find_once(&prefix, old_header.as_bytes(), "DIMACS header")?;

    file.seek(SeekFrom::End(0))
        .and_then(|_| {
            write_clause(
                &mut file,
                &pair_clause(*after.cuts.last().expect("checked above")),
            )
        })
        .map_err(|error| format!("cannot append pair clause to {}: {error}", path.display()))?;
    for (offset, replacement) in [
        (pair_offset, new_pairs.as_bytes()),
        (cut_offset, new_cuts.as_bytes()),
        (checksum_offset, new_checksum.as_bytes()),
        (header_offset, new_header.as_bytes()),
    ] {
        file.seek(SeekFrom::Start(offset as u64))
            .and_then(|_| file.write_all(replacement))
            .map_err(|error| format!("cannot patch CNF prefix {}: {error}", path.display()))?;
    }
    file.flush()
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("cannot finish CNF append {}: {error}", path.display()))?;
    Ok(())
}

#[derive(Debug)]
enum Mode {
    Stats {
        checkpoint: PathBuf,
    },
    Emit {
        checkpoint: PathBuf,
        output: PathBuf,
    },
    Decode {
        checkpoint: PathBuf,
        model: PathBuf,
        output: Option<PathBuf>,
    },
    Loop {
        checkpoint: PathBuf,
        next_checkpoint: PathBuf,
        sat_exe: PathBuf,
        cnf: PathBuf,
        model: PathBuf,
        proof: Option<PathBuf>,
        max_iterations: usize,
        conflicts: Option<u64>,
    },
}

fn print_help() {
    println!(
        "thermo-topology-cnf [emit] --checkpoint PATH --output CNF\n\
         thermo-topology-cnf stats --checkpoint PATH\n\
         thermo-topology-cnf decode --checkpoint PATH --model MODEL [--output FILE]\n\
         thermo-topology-cnf loop --checkpoint PATH --next-checkpoint PATH\n\
             --sat-exe PATH --cnf PATH [--model PATH] [--proof PATH]\n\
             [--max-iterations N] [--conflicts N]\n\
         \n\
         `stats` reports exact pair-clause deduplication without writing a CNF.\n\
         `emit` writes the deterministic topology master. `decode` validates a\n\
         complete SAT competition-format model against that exact master and\n\
         emits its target and directed paths. `loop` runs a CaDiCaL-compatible\n\
         executable, validates every model, asks the exact thermo solver for\n\
         0/1/2+ solutions, and persists one new pair cut per non-unique\n\
         iteration. Resource-limit/UNKNOWN results are inconclusive."
    );
    println!(
        "\nSAT sidecar contract:\n\
         - the executable must accept CaDiCaL's `-q -w MODEL [-c N] CNF [PROOF]`;\n\
         - exit 10 means SAT, exit 20 UNSAT, and exit 0 UNKNOWN;\n\
         - SAT output must assign every variable 1..=7226 exactly once in\n\
           competition `s`/`v` format; partial, conflicting, out-of-range, or\n\
           clause-violating models are rejected;\n\
         - `--proof` is useful only when the final status is UNSAT. On SAT or\n\
           UNKNOWN it is not a negative certificate;\n\
         - each iteration starts a fresh SAT process. The CNF itself is kept\n\
           current by an exact in-place pair-clause append, but learned SAT\n\
           state is not retained across iterations."
    );
}

fn require_value(option: &str, index: &mut usize, arguments: &[String]) -> Result<String, String> {
    *index += 1;
    arguments
        .get(*index)
        .cloned()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_usize(option: &str, value: String) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("invalid value for {option}: {value:?}"))
}

fn parse_u64(option: &str, value: String) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid value for {option}: {value:?}"))
}

fn resolved_destination(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return fs::canonicalize(path)
            .map_err(|error| format!("cannot resolve {}: {error}", path.display()));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent)
        .map_err(|error| format!("cannot resolve parent of {}: {error}", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("path {} has no file name", path.display()))?;
    Ok(parent.join(name))
}

fn paths_equal(left: &Path, right: &Path) -> Result<bool, String> {
    let left = resolved_destination(left)?;
    let right = resolved_destination(right)?;
    #[cfg(windows)]
    {
        Ok(left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy()))
    }
    #[cfg(not(windows))]
    {
        Ok(left == right)
    }
}

fn reject_collisions(paths: &[(&str, &Path)]) -> Result<(), String> {
    for left in 0..paths.len() {
        for right in left + 1..paths.len() {
            if paths_equal(paths[left].1, paths[right].1)? {
                return Err(format!(
                    "{} and {} must name different files",
                    paths[left].0, paths[right].0
                ));
            }
        }
    }
    Ok(())
}

fn parse_options() -> Result<Mode, String> {
    let mut arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
    {
        print_help();
        std::process::exit(0);
    }
    let command = if arguments
        .first()
        .is_some_and(|value| !value.starts_with('-'))
    {
        arguments.remove(0)
    } else {
        "emit".to_string()
    };
    let mut checkpoint = None;
    let mut output = None;
    let mut model = None;
    let mut next_checkpoint = None;
    let mut sat_exe = None;
    let mut cnf = None;
    let mut proof = None;
    let mut max_iterations = 1usize;
    let mut conflicts = None;
    let mut index = 0usize;
    while index < arguments.len() {
        let argument = &arguments[index];
        match argument.as_str() {
            "--checkpoint" => {
                checkpoint = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--output" => {
                output = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--model" => {
                model = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--next-checkpoint" => {
                next_checkpoint = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--sat-exe" => {
                sat_exe = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--cnf" => {
                cnf = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--proof" => {
                proof = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--max-iterations" => {
                max_iterations =
                    parse_usize(argument, require_value(argument, &mut index, &arguments)?)?;
                if max_iterations == 0 {
                    return Err("--max-iterations must be positive".into());
                }
            }
            "--conflicts" => {
                conflicts = Some(parse_u64(
                    argument,
                    require_value(argument, &mut index, &arguments)?,
                )?);
            }
            _ => return Err(format!("unknown option {argument:?}; use --help")),
        }
        index += 1;
    }

    let checkpoint = checkpoint.ok_or_else(|| "--checkpoint is required".to_string())?;
    match command.as_str() {
        "stats" => {
            if output.is_some()
                || model.is_some()
                || next_checkpoint.is_some()
                || sat_exe.is_some()
                || cnf.is_some()
                || proof.is_some()
                || conflicts.is_some()
                || max_iterations != 1
            {
                return Err("stats accepts only --checkpoint".into());
            }
            Ok(Mode::Stats { checkpoint })
        }
        "emit" => {
            if model.is_some()
                || next_checkpoint.is_some()
                || sat_exe.is_some()
                || cnf.is_some()
                || proof.is_some()
                || conflicts.is_some()
                || max_iterations != 1
            {
                return Err("emit accepts only --checkpoint and --output".into());
            }
            let output = output.ok_or_else(|| "--output is required for emit".to_string())?;
            reject_collisions(&[("checkpoint", &checkpoint), ("output", &output)])?;
            Ok(Mode::Emit { checkpoint, output })
        }
        "decode" => {
            if next_checkpoint.is_some()
                || sat_exe.is_some()
                || cnf.is_some()
                || proof.is_some()
                || conflicts.is_some()
                || max_iterations != 1
            {
                return Err("decode accepts only --checkpoint, --model, and --output".into());
            }
            let model = model.ok_or_else(|| "--model is required for decode".to_string())?;
            let mut paths = vec![
                ("checkpoint", checkpoint.as_path()),
                ("model", model.as_path()),
            ];
            if let Some(output) = &output {
                paths.push(("output", output.as_path()));
            }
            reject_collisions(&paths)?;
            Ok(Mode::Decode {
                checkpoint,
                model,
                output,
            })
        }
        "loop" => {
            if output.is_some() {
                return Err("--output is not used by loop; use --cnf".into());
            }
            let next_checkpoint = next_checkpoint
                .ok_or_else(|| "--next-checkpoint is required for loop".to_string())?;
            let sat_exe = sat_exe.ok_or_else(|| "--sat-exe is required for loop".to_string())?;
            let cnf = cnf.ok_or_else(|| "--cnf is required for loop".to_string())?;
            let model = model.unwrap_or_else(|| cnf.with_extension("model"));
            let mut paths = vec![
                ("checkpoint", checkpoint.as_path()),
                ("sat-exe", sat_exe.as_path()),
                ("next-checkpoint", next_checkpoint.as_path()),
                ("cnf", cnf.as_path()),
                ("model", model.as_path()),
            ];
            if let Some(proof) = &proof {
                paths.push(("proof", proof.as_path()));
            }
            reject_collisions(&paths)?;
            Ok(Mode::Loop {
                checkpoint,
                next_checkpoint,
                sat_exe,
                cnf,
                model,
                proof,
                max_iterations,
                conflicts,
            })
        }
        other => Err(format!(
            "unknown command {other:?}; expected emit, decode, or loop"
        )),
    }
}

fn run_emit(checkpoint_path: &Path, output: &Path) -> Result<(), String> {
    let checkpoint = load_checkpoint(checkpoint_path)?;
    let (variables, clauses) = write_cnf(&checkpoint, output)?;
    println!(
        "wrote {}: variables={variables} clauses={clauses} checkpoint_pairs={} unique_pair_cuts={} checkpoint_fnv1a64={:016x}",
        output.display(),
        checkpoint.pairs.len(),
        checkpoint.cuts.len(),
        checkpoint.checksum
    );
    Ok(())
}

fn run_stats(checkpoint_path: &Path) -> Result<(), String> {
    let checkpoint = load_checkpoint(checkpoint_path)?;
    println!(
        "checkpoint_pairs={}\nunique_pair_cuts={}\nduplicate_pair_clauses={}\ncheckpoint_fnv1a64={:016x}",
        checkpoint.pairs.len(),
        checkpoint.cuts.len(),
        checkpoint.pairs.len() - checkpoint.cuts.len(),
        checkpoint.checksum
    );
    Ok(())
}

fn run_decode(checkpoint_path: &Path, model: &Path, output: Option<&Path>) -> Result<(), String> {
    let checkpoint = load_checkpoint(checkpoint_path)?;
    let text = fs::read_to_string(model)
        .map_err(|error| format!("cannot read model {}: {error}", model.display()))?;
    let result = parse_sat_result(&text)?;
    let rendered = match result.status {
        SatStatus::Satisfiable => {
            let candidate = decode_candidate(
                &checkpoint,
                result.assignment.as_deref().expect("SAT has assignment"),
            )?;
            format_candidate(&candidate, &directed_edges())
        }
        SatStatus::Unsatisfiable => "status=unsat\n".to_string(),
        SatStatus::Unknown => "status=unknown\n".to_string(),
    };
    if let Some(output) = output {
        fs::write(output, rendered)
            .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    } else {
        print!("{rendered}");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_loop(
    checkpoint_path: &Path,
    next_checkpoint: &Path,
    sat_exe: &Path,
    cnf: &Path,
    model: &Path,
    proof: Option<&Path>,
    max_iterations: usize,
    conflicts: Option<u64>,
) -> Result<(), String> {
    let mut checkpoint = load_checkpoint(checkpoint_path)?;
    let edges = directed_edges();
    let mut seen_cuts = checkpoint.cuts.iter().copied().collect::<HashSet<_>>();
    let (_, mut clauses) = write_cnf(&checkpoint, cnf)?;
    for iteration in 0..max_iterations {
        eprintln!(
            "topology-loop iteration={iteration} pairs={} unique_cuts={} clauses={clauses}",
            checkpoint.pairs.len(),
            checkpoint.cuts.len()
        );
        let sat = invoke_sat(sat_exe, cnf, model, proof, conflicts)?;
        if sat.status != SatStatus::Unsatisfiable
            && let Some(proof) = proof
            && proof.exists()
        {
            fs::remove_file(proof).map_err(|error| {
                format!(
                    "cannot remove non-certificate proof {}: {error}",
                    proof.display()
                )
            })?;
        }
        match sat.status {
            SatStatus::Unknown => {
                write_checkpoint(&checkpoint, next_checkpoint)?;
                println!(
                    "status=inconclusive-sat-limit\niterations={}\npairs={}\ncheckpoint={}\n",
                    iteration + 1,
                    checkpoint.pairs.len(),
                    next_checkpoint.display()
                );
                return Ok(());
            }
            SatStatus::Unsatisfiable => {
                write_checkpoint(&checkpoint, next_checkpoint)?;
                println!(
                    "status=topology-excluded\niterations={}\npairs={}\ncheckpoint={}\nproof={}\n",
                    iteration + 1,
                    checkpoint.pairs.len(),
                    next_checkpoint.display(),
                    proof.map_or_else(|| "none".to_string(), |path| path.display().to_string())
                );
                return Ok(());
            }
            SatStatus::Satisfiable => {}
        }

        let candidate = decode_candidate(
            &checkpoint,
            sat.assignment.as_deref().expect("SAT has assignment"),
        )?;
        eprintln!(
            "topology-loop iteration={iteration} selected={} covered={} thermometers={}",
            candidate.selected.len(),
            candidate.covered_cells,
            candidate.paths.len()
        );
        let solve = Solver::blank(&candidate.paths)
            .map_err(|error| format!("cannot build thermo oracle: {error}"))?
            .count_up_to(2);
        match solve.multiplicity() {
            Multiplicity::Zero => {
                return Err("SAT model target is not accepted by the thermo oracle".into());
            }
            Multiplicity::Unique => {
                if solve.first_solution != Some(candidate.target) {
                    return Err("unique thermo solution differs from the SAT target".into());
                }
                write_checkpoint(&checkpoint, next_checkpoint)?;
                println!("status=unique");
                print!("{}", format_candidate_body(&candidate, &edges));
                println!(
                    "iterations={}\npairs={}\ncheckpoint={}\noracle_nodes={}\n",
                    iteration + 1,
                    checkpoint.pairs.len(),
                    next_checkpoint.display(),
                    solve.stats.nodes
                );
                return Ok(());
            }
            Multiplicity::Multiple => {
                let first = solve
                    .first_solution
                    .ok_or_else(|| "multiple result has no first solution".to_string())?;
                let second = solve
                    .second_solution
                    .ok_or_else(|| "multiple result has no second solution".to_string())?;
                let pair = GridPair::new(first, second)?;
                let cut = pair_cut(&pair, &edges);
                if pair_clause(cut).iter().any(|&variable| {
                    let edge_id = (variable - EDGE_BASE) as usize;
                    candidate.selected.binary_search(&edge_id).is_ok()
                }) {
                    return Err("thermo alternatives generated a cut hit by the candidate".into());
                }
                if checkpoint.pairs.contains(&pair) {
                    return Err("thermo alternatives generated a duplicate learned pair".into());
                }
                if !seen_cuts.insert(cut) {
                    return Err("thermo alternatives generated a duplicate learned pair cut".into());
                }
                let before_pairs = checkpoint.pairs.len();
                let before_cuts = checkpoint.cuts.len();
                let before_checksum = checkpoint.checksum;
                checkpoint.pairs.push(pair);
                checkpoint.cuts.push(cut);
                checkpoint.checksum = pairs_checksum(&checkpoint.pairs);
                append_pair_to_cnf(
                    cnf,
                    before_pairs,
                    before_cuts,
                    before_checksum,
                    &checkpoint,
                    &pair,
                )?;
                clauses += 1;
                write_checkpoint(&checkpoint, next_checkpoint)?;
                eprintln!(
                    "topology-loop iteration={iteration} learned_pair={}|{} oracle_nodes={}",
                    format_grid(&pair.first),
                    format_grid(&pair.second),
                    solve.stats.nodes
                );
            }
        }
    }
    println!(
        "status=iteration-limit\niterations={max_iterations}\npairs={}\ncheckpoint={}\n",
        checkpoint.pairs.len(),
        next_checkpoint.display()
    );
    Ok(())
}

fn run() -> Result<(), String> {
    match parse_options()? {
        Mode::Stats { checkpoint } => run_stats(&checkpoint),
        Mode::Emit { checkpoint, output } => run_emit(&checkpoint, &output),
        Mode::Decode {
            checkpoint,
            model,
            output,
        } => run_decode(&checkpoint, &model, output.as_deref()),
        Mode::Loop {
            checkpoint,
            next_checkpoint,
            sat_exe,
            cnf,
            model,
            proof,
            max_iterations,
            conflicts,
        } => run_loop(
            &checkpoint,
            &next_checkpoint,
            &sat_exe,
            &cnf,
            &model,
            proof.as_deref(),
            max_iterations,
            conflicts,
        ),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANONICAL: &str =
        "123456789456789123789123456214365897365897214897214365531642978642978531978531642";

    fn edge_id(edges: &[DirectedEdge], lower: usize, upper: usize) -> usize {
        edges
            .iter()
            .position(|edge| edge.lower as usize == lower && edge.upper as usize == upper)
            .expect("directed king edge")
    }

    fn assignment_for_row_thermo(edges: &[DirectedEdge]) -> Vec<bool> {
        let grid = parse_grid(CANONICAL).unwrap();
        let mut assignment = vec![false; VARIABLE_COUNT as usize + 1];
        for (cell, &digit) in grid.iter().enumerate() {
            assignment[digit_var(cell, digit as usize - 1) as usize] = true;
        }
        for cell in 0..9 {
            assignment[occupied_var(cell) as usize] = true;
        }
        for cell in 0..8 {
            let id = edge_id(edges, cell, cell + 1);
            assignment[edge_var(id) as usize] = true;
            assignment[swap_var(cell, id) as usize] = true;
        }
        let mut occupied = 0usize;
        for prefix in 0..CELLS - 1 {
            if prefix < 9 {
                occupied += 1;
            }
            for count in 0..COVER_LIMIT {
                assignment[sequential_var(prefix, count) as usize] = occupied > count;
            }
        }
        assignment
    }

    fn clause_true(clause: &[i32], assignment: &[bool]) -> bool {
        clause.iter().any(|&literal| {
            let value = assignment[literal.unsigned_abs() as usize];
            if literal > 0 { value } else { !value }
        })
    }

    fn empty_checkpoint() -> Checkpoint {
        Checkpoint {
            budget: 16,
            checksum: FNV_OFFSET,
            pairs: Vec::new(),
            cuts: Vec::new(),
        }
    }

    fn format_complete_model(assignment: &[bool]) -> String {
        let literals = (1..=VARIABLE_COUNT as usize)
            .map(|variable| {
                if assignment[variable] {
                    variable.to_string()
                } else {
                    format!("-{variable}")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("s SATISFIABLE\nv {literals} 0\n")
    }

    fn temporary_path(suffix: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!(
            "thermo-topology-cnf-test-{}-{nonce}.{suffix}",
            std::process::id()
        ))
    }

    #[test]
    fn variable_ranges_are_contiguous_and_stable() {
        assert_eq!(digit_var(0, 0), 1);
        assert_eq!(digit_var(80, 8), 729);
        assert_eq!(edge_var(0), 730);
        assert_eq!(edge_var(543), 1273);
        assert_eq!(occupied_var(0), 1274);
        assert_eq!(occupied_var(80), 1354);
        assert_eq!(sequential_var(0, 0), 1355);
        assert_eq!(sequential_var(79, 18), 2874);
        assert_eq!(swap_var(0, 0), 2875);
        assert_eq!(swap_var(7, 543), VARIABLE_COUNT);
        assert_eq!(VARIABLE_COUNT, 7226);
        assert_eq!(edges_checksum(&directed_edges()), 0xf12501e5f1df08d5);
    }

    #[test]
    fn valid_nine_cell_thermometer_satisfies_the_base_master() {
        let edges = directed_edges();
        let clauses = base_clauses(&edges);
        let assignment = assignment_for_row_thermo(&edges);
        assert!(
            clauses
                .iter()
                .all(|clause| clause_true(clause, &assignment))
        );
    }

    #[test]
    fn complete_model_decodes_to_stable_target_and_path() {
        let edges = directed_edges();
        let assignment = assignment_for_row_thermo(&edges);
        let parsed = parse_sat_result(&format_complete_model(&assignment)).unwrap();
        assert_eq!(parsed.status, SatStatus::Satisfiable);
        let decoded =
            decode_candidate(&empty_checkpoint(), parsed.assignment.as_deref().unwrap()).unwrap();
        assert_eq!(format_grid(&decoded.target), CANONICAL);
        assert_eq!(decoded.paths, vec![(0u8..=8).collect::<Vec<_>>()]);
        assert_eq!(decoded.covered_cells, 9);
        assert_eq!(decoded.selected.len(), 8);
    }

    #[test]
    fn model_parser_rejects_partial_and_conflicting_assignments() {
        assert!(parse_sat_result("s SATISFIABLE\nv 1 -2 0\n").is_err());
        assert!(parse_sat_result("s SATISFIABLE\nv 1 -1 0\n").is_err());
        assert!(parse_sat_result("s UNSATISFIABLE\nv 1 0\n").is_err());
    }

    #[test]
    fn incremental_pair_append_matches_a_fresh_cnf_byte_for_byte() {
        let first = parse_grid(CANONICAL).unwrap();
        let mut second = first;
        for digit in &mut second {
            *digit = match *digit {
                1 => 2,
                2 => 1,
                value => value,
            };
        }
        let pair = GridPair::new(first, second).unwrap();
        let before = empty_checkpoint();
        let mut after = empty_checkpoint();
        after.pairs.push(pair);
        after.cuts.push(pair_cut(&pair, &directed_edges()));
        after.checksum = pairs_checksum(&after.pairs);
        let appended = temporary_path("appended.cnf");
        let fresh = temporary_path("fresh.cnf");
        write_cnf(&before, &appended).unwrap();
        append_pair_to_cnf(
            &appended,
            before.pairs.len(),
            before.cuts.len(),
            before.checksum,
            &after,
            &pair,
        )
        .unwrap();
        write_cnf(&after, &fresh).unwrap();
        assert_eq!(fs::read(&appended).unwrap(), fs::read(&fresh).unwrap());
        fs::remove_file(appended).unwrap();
        fs::remove_file(fresh).unwrap();
    }

    #[test]
    fn topology_rejects_a_branch() {
        let edges = directed_edges();
        let mut clauses = Vec::new();
        topology_clauses(&mut clauses, &edges);
        let mut assignment = assignment_for_row_thermo(&edges);
        let branch = edge_id(&edges, 0, 9);
        assignment[edge_var(branch) as usize] = true;
        assignment[occupied_var(9) as usize] = true;
        assert!(
            clauses
                .iter()
                .any(|clause| !clause_true(clause, &assignment))
        );
    }

    #[test]
    fn coverage_counter_rejects_twenty_occupied_cells() {
        let mut clauses = Vec::new();
        coverage_clauses(&mut clauses);
        let mut assignment = vec![false; VARIABLE_COUNT as usize + 1];
        for cell in 0..20 {
            assignment[occupied_var(cell) as usize] = true;
        }
        for prefix in 0..CELLS - 1 {
            let occupied = (prefix + 1).min(20);
            for count in 0..COVER_LIMIT {
                assignment[sequential_var(prefix, count) as usize] = occupied > count;
            }
        }
        assert!(
            clauses
                .iter()
                .any(|clause| !clause_true(clause, &assignment))
        );
    }

    #[test]
    fn swap_pair_cut_requires_the_corresponding_consecutive_symbols() {
        let edges = directed_edges();
        let first = parse_grid(CANONICAL).unwrap();
        let mut second = first;
        for digit in &mut second {
            *digit = match *digit {
                1 => 2,
                2 => 1,
                value => value,
            };
        }
        let pair = GridPair { first, second };
        let cut = pair_clause(pair_cut(&pair, &edges));
        let under_first = cut
            .into_iter()
            .map(|variable| (variable - EDGE_BASE) as usize)
            .filter(|&edge_id| {
                let edge = edges[edge_id];
                first[edge.lower as usize] < first[edge.upper as usize]
            })
            .collect::<Vec<_>>();
        assert!(!under_first.is_empty());
        assert!(under_first.iter().all(|&edge_id| {
            let edge = edges[edge_id];
            first[edge.lower as usize] == 1 && first[edge.upper as usize] == 2
        }));
    }
}
