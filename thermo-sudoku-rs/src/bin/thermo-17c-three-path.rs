//! Exact classic-morph search for the ten three-thermometer partitions of
//! seventeen covered cells.
//!
//! A source digit can occur at most once on each strict path, so its clue
//! occurrences are injected into distinct path roles. The DFS chooses the
//! global digit order and one injection per digit. Consecutive ranks must
//! share a path: otherwise swapping those two values in the source solution
//! gives a second thermo solution. Spatial Sudoku morphs are carried as
//! independent 1,296-bit row and column support domains.

use std::cmp::Reverse;
use std::collections::BTreeSet;
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use thermo_sudoku::Solver;

const SIDE: usize = 9;
const CLUES: usize = 17;
const ROLES: usize = 3;
const NO_VERTEX: u8 = u8::MAX;
const MORPH_COUNT: usize = 1_296;
const MORPH_WORDS: usize = MORPH_COUNT.div_ceil(64);
const LAST_MORPH_WORD_BITS: usize = MORPH_COUNT - 64 * (MORPH_WORDS - 1);
const THREE_PATH_PARTITIONS: [[u8; 3]; 10] = [
    [9, 6, 2],
    [9, 5, 3],
    [9, 4, 4],
    [8, 7, 2],
    [8, 6, 3],
    [8, 5, 4],
    [7, 7, 3],
    [7, 6, 4],
    [7, 5, 5],
    [6, 6, 5],
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MorphSet([u64; MORPH_WORDS]);

impl MorphSet {
    const EMPTY: Self = Self([0; MORPH_WORDS]);

    fn all() -> Self {
        let mut words = [u64::MAX; MORPH_WORDS];
        words[MORPH_WORDS - 1] = (1u64 << LAST_MORPH_WORD_BITS) - 1;
        Self(words)
    }

    fn insert(&mut self, index: usize) {
        self.0[index / 64] |= 1u64 << (index % 64);
    }

    fn intersect_assign(&mut self, other: &Self) -> bool {
        let mut any = false;
        for (left, &right) in self.0.iter_mut().zip(&other.0) {
            *left &= right;
            any |= *left != 0;
        }
        any
    }

    fn first(self) -> Option<usize> {
        self.0.iter().enumerate().find_map(|(word_index, word)| {
            (*word != 0).then(|| word_index * 64 + word.trailing_zeros() as usize)
        })
    }

    #[cfg(test)]
    fn contains(self, index: usize) -> bool {
        self.0[index / 64] & (1u64 << (index % 64)) != 0
    }
}

#[derive(Debug)]
struct AxisMorphs {
    permutations: Vec<[u8; SIDE]>,
    close: [[MorphSet; SIDE]; SIDE],
}

impl AxisMorphs {
    fn new() -> Self {
        let permutations = generate_axis_morphs();
        assert_eq!(permutations.len(), MORPH_COUNT);
        let mut close = [[MorphSet::EMPTY; SIDE]; SIDE];
        for (morph_index, permutation) in permutations.iter().enumerate() {
            for left in 0..SIDE {
                for right in 0..SIDE {
                    if permutation[left].abs_diff(permutation[right]) <= 1 {
                        close[left][right].insert(morph_index);
                    }
                }
            }
        }
        Self {
            permutations,
            close,
        }
    }
}

#[derive(Clone, Debug)]
struct Puzzle {
    encoded: String,
    cells: [u8; CLUES],
    digits: [u8; CLUES],
    occurrences: [[u8; ROLES]; SIDE],
    counts: [u8; SIDE],
}

impl Puzzle {
    fn parse(encoded: &str, line_number: usize) -> Result<Self, String> {
        if encoded.len() != 81 {
            return Err(format!(
                "line {line_number}: expected 81 ASCII cells, got {}",
                encoded.len()
            ));
        }
        let mut cells = [NO_VERTEX; CLUES];
        let mut digits = [NO_VERTEX; CLUES];
        let mut occurrences = [[NO_VERTEX; ROLES]; SIDE];
        let mut counts = [0u8; SIDE];
        let mut clue_count = 0usize;
        for (cell, byte) in encoded.bytes().enumerate() {
            match byte {
                b'.' | b'0' => {}
                b'1'..=b'9' => {
                    if clue_count == CLUES {
                        return Err(format!("line {line_number}: more than 17 clues"));
                    }
                    let digit = (byte - b'1') as usize;
                    let occurrence = counts[digit] as usize;
                    if occurrence < ROLES {
                        occurrences[digit][occurrence] = clue_count as u8;
                    }
                    counts[digit] += 1;
                    cells[clue_count] = cell as u8;
                    digits[clue_count] = digit as u8;
                    clue_count += 1;
                }
                _ => {
                    return Err(format!(
                        "line {line_number}: invalid byte {byte:#04x} at cell {cell}"
                    ));
                }
            }
        }
        if clue_count != CLUES {
            return Err(format!(
                "line {line_number}: expected 17 clues, got {clue_count}"
            ));
        }
        Ok(Self {
            encoded: encoded.to_owned(),
            cells,
            digits,
            occurrences,
            counts,
        })
    }

    fn eligible_for(&self, partition: [u8; 3]) -> bool {
        if self.counts.iter().any(|&count| count == 0 || count > 3) {
            return false;
        }
        assignment_capacity_feasible(self.counts, partition)
    }
}

#[derive(Clone, Copy, Debug)]
struct RoleOption {
    mask: u8,
    vertices: [u8; ROLES],
}

fn role_options(puzzle: &Puzzle, digit: usize) -> Vec<RoleOption> {
    let count = puzzle.counts[digit] as usize;
    let occurrences = &puzzle.occurrences[digit];
    let mut result = Vec::with_capacity(6);
    match count {
        1 => {
            for role in 0..ROLES {
                let mut vertices = [NO_VERTEX; ROLES];
                vertices[role] = occurrences[0];
                result.push(RoleOption {
                    mask: 1u8 << role,
                    vertices,
                });
            }
        }
        2 => {
            for first_role in 0..ROLES {
                for second_role in 0..ROLES {
                    if first_role == second_role {
                        continue;
                    }
                    let mut vertices = [NO_VERTEX; ROLES];
                    vertices[first_role] = occurrences[0];
                    vertices[second_role] = occurrences[1];
                    result.push(RoleOption {
                        mask: (1u8 << first_role) | (1u8 << second_role),
                        vertices,
                    });
                }
            }
        }
        3 => {
            const P3: [[usize; 3]; 6] = [
                [0, 1, 2],
                [0, 2, 1],
                [1, 0, 2],
                [1, 2, 0],
                [2, 0, 1],
                [2, 1, 0],
            ];
            for roles in P3 {
                let mut vertices = [NO_VERTEX; ROLES];
                for occurrence in 0..ROLES {
                    vertices[roles[occurrence]] = occurrences[occurrence];
                }
                result.push(RoleOption {
                    mask: 0b111,
                    vertices,
                });
            }
        }
        _ => {}
    }
    result
}

#[derive(Clone, Debug)]
struct Cover {
    paths: [Vec<u8>; ROLES],
    digit_order: [u8; SIDE],
    row_morph: u16,
    column_morph: u16,
}

#[derive(Default, Debug)]
struct SearchStats {
    nodes: u64,
    adjacency_prunes: u64,
    capacity_prunes: u64,
    spatial_prunes: u64,
    reversal_prunes: u64,
    realized_covers: u64,
}

#[derive(Default, Debug)]
struct PartitionStats {
    records: u64,
    eligible_records: u64,
    nodes: u64,
    adjacency_prunes: u64,
    capacity_prunes: u64,
    spatial_prunes: u64,
    reversal_prunes: u64,
    realized_covers: u64,
    duplicate_layouts: u64,
    classified_layouts: u64,
    multiple_layouts: u64,
    unique_layouts: u64,
}

#[derive(Debug)]
struct Classification {
    count: u64,
    capped: bool,
    first_solution: [u8; 81],
    second_solution: Option<[u8; 81]>,
}

#[derive(Debug)]
struct Options {
    input: PathBuf,
    output: Option<PathBuf>,
    partitions: Vec<[u8; 3]>,
    start_line: usize,
    end_line: usize,
    max_eligible: Option<usize>,
    stop_on_first: bool,
    progress_every: usize,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
    let started = Instant::now();
    let axis = AxisMorphs::new();
    let input = File::open(&options.input)
        .map_err(|error| format!("cannot open {}: {error}", options.input.display()))?;
    let mut puzzles = Vec::new();
    for (index, line) in BufReader::new(input).lines().enumerate() {
        let line_number = index + 1;
        let encoded = line.map_err(|error| format!("cannot read line {line_number}: {error}"))?;
        puzzles.push((line_number, Puzzle::parse(&encoded, line_number)?));
    }
    let mut output = options
        .output
        .as_ref()
        .map(|path| {
            File::create(path)
                .map(BufWriter::new)
                .map_err(|error| format!("cannot create {}: {error}", path.display()))
        })
        .transpose()?;
    if let Some(writer) = output.as_mut() {
        writeln!(writer,
            "{{\"schema\":\"thermo-17c-three-path-v1\",\"partitions\":\"{}\",\"start_line\":{},\"end_line\":{},\"max_eligible\":{}}}",
            options.partitions.iter().map(|&p| compact_partition(p)).collect::<Vec<_>>().join(";"),
            options.start_line, options.end_line,
            options.max_eligible.map_or_else(|| "null".to_owned(), |limit| limit.to_string()),
        ).map_err(|error| format!("cannot write output: {error}"))?;
    }

    let mut found_unique = false;
    for &partition in &options.partitions {
        let partition_started = Instant::now();
        let mut totals = PartitionStats::default();
        let mut seen = BTreeSet::<Vec<u8>>::new();
        for (line_number, puzzle) in &puzzles {
            totals.records += 1;
            if *line_number < options.start_line || *line_number > options.end_line {
                continue;
            }
            if !puzzle.eligible_for(partition) {
                continue;
            }
            if options
                .max_eligible
                .is_some_and(|limit| totals.eligible_records as usize >= limit)
            {
                break;
            }
            totals.eligible_records += 1;
            let mut search_stats = SearchStats::default();
            let mut callback_error = None;
            let exhausted =
                enumerate_covers(puzzle, partition, &axis, &mut search_stats, |cover| {
                    let key = canonical_layout_key(&cover.paths);
                    if !seen.insert(key) {
                        totals.duplicate_layouts += 1;
                        return true;
                    }
                    totals.classified_layouts += 1;
                    match classify_cover(puzzle, &cover, &axis) {
                        Ok(classification) => {
                            if let Err(error) = write_candidate(
                                output.as_mut(),
                                *line_number,
                                puzzle,
                                partition,
                                &cover,
                                &classification,
                            ) {
                                callback_error = Some(error);
                                return false;
                            }
                            if classification.count == 1 {
                                totals.unique_layouts += 1;
                                found_unique = true;
                            } else {
                                totals.multiple_layouts += 1;
                            }
                            !(classification.count == 1 && options.stop_on_first)
                        }
                        Err(error) => {
                            callback_error = Some(format!("line {line_number}: {error}"));
                            false
                        }
                    }
                });
            if let Some(error) = callback_error {
                return Err(error);
            }
            totals.nodes += search_stats.nodes;
            totals.adjacency_prunes += search_stats.adjacency_prunes;
            totals.capacity_prunes += search_stats.capacity_prunes;
            totals.spatial_prunes += search_stats.spatial_prunes;
            totals.reversal_prunes += search_stats.reversal_prunes;
            totals.realized_covers += search_stats.realized_covers;
            if !exhausted && found_unique && options.stop_on_first {
                break;
            }
            if options.progress_every != 0
                && (totals.eligible_records as usize).is_multiple_of(options.progress_every)
            {
                eprintln!(
                    "partition={} eligible={} nodes={} layouts={} unique={} elapsed={:.3}s",
                    compact_partition(partition),
                    totals.eligible_records,
                    totals.nodes,
                    totals.classified_layouts,
                    totals.unique_layouts,
                    partition_started.elapsed().as_secs_f64()
                );
            }
        }
        let complete = options.max_eligible.is_none()
            && options.start_line == 1
            && options.end_line >= puzzles.len()
            && !(found_unique && options.stop_on_first);
        let summary = format!(
            "{{\"type\":\"partition-summary\",\"partition\":\"{}\",\"complete\":{},\"records\":{},\"eligible_records\":{},\"nodes\":{},\"adjacency_prunes\":{},\"capacity_prunes\":{},\"spatial_prunes\":{},\"reversal_prunes\":{},\"realized_covers\":{},\"duplicate_layouts\":{},\"classified_layouts\":{},\"multiple_layouts\":{},\"unique_layouts\":{}}}",
            compact_partition(partition),
            complete,
            totals.records,
            totals.eligible_records,
            totals.nodes,
            totals.adjacency_prunes,
            totals.capacity_prunes,
            totals.spatial_prunes,
            totals.reversal_prunes,
            totals.realized_covers,
            totals.duplicate_layouts,
            totals.classified_layouts,
            totals.multiple_layouts,
            totals.unique_layouts
        );
        println!("{summary}");
        eprintln!(
            "partition {} completed in {:.3}s",
            compact_partition(partition),
            partition_started.elapsed().as_secs_f64()
        );
        if let Some(writer) = output.as_mut() {
            writeln!(writer, "{summary}")
                .map_err(|error| format!("cannot write output: {error}"))?;
        }
        if found_unique && options.stop_on_first {
            break;
        }
    }
    if let Some(writer) = output.as_mut() {
        writer
            .flush()
            .map_err(|error| format!("cannot flush output: {error}"))?;
    }
    eprintln!("total elapsed {:.3}s", started.elapsed().as_secs_f64());
    Ok(())
}

fn enumerate_covers<F>(
    puzzle: &Puzzle,
    partition: [u8; 3],
    axis: &AxisMorphs,
    stats: &mut SearchStats,
    mut callback: F,
) -> bool
where
    F: FnMut(Cover) -> bool,
{
    let options: [Vec<RoleOption>; SIDE] = std::array::from_fn(|digit| role_options(puzzle, digit));
    let mut search = CoverSearch {
        puzzle,
        partition,
        axis,
        role_options: &options,
        stats,
        callback: &mut callback,
    };
    let mut paths = [[NO_VERTEX; 9]; ROLES];
    let mut digit_order = [NO_VERTEX; SIDE];
    search.dfs(
        0,
        0,
        0,
        [0; ROLES],
        [NO_VERTEX; ROLES],
        &mut paths,
        &mut digit_order,
        MorphSet::all(),
        MorphSet::all(),
    )
}

struct CoverSearch<'a, F> {
    puzzle: &'a Puzzle,
    partition: [u8; 3],
    axis: &'a AxisMorphs,
    role_options: &'a [Vec<RoleOption>; SIDE],
    stats: &'a mut SearchStats,
    callback: &'a mut F,
}

impl<F> CoverSearch<'_, F>
where
    F: FnMut(Cover) -> bool,
{
    #[allow(clippy::too_many_arguments)]
    fn dfs(
        &mut self,
        depth: usize,
        used_digits: u16,
        previous_mask: u8,
        lengths: [u8; ROLES],
        last_vertices: [u8; ROLES],
        paths: &mut [[u8; 9]; ROLES],
        digit_order: &mut [u8; SIDE],
        row_support: MorphSet,
        column_support: MorphSet,
    ) -> bool {
        self.stats.nodes += 1;
        if depth == SIDE {
            debug_assert_eq!(lengths, self.partition);
            if digit_order[0] > digit_order[8] {
                self.stats.reversal_prunes += 1;
                return true;
            }
            let row_morph = row_support.first().expect("non-empty row support");
            let column_morph = column_support.first().expect("non-empty column support");
            let rows = self.axis.permutations[row_morph];
            let columns = self.axis.permutations[column_morph];
            let realized_paths: [Vec<u8>; ROLES] = std::array::from_fn(|role| {
                paths[role][..lengths[role] as usize]
                    .iter()
                    .map(|&vertex| morph_cell(self.puzzle.cells[vertex as usize], &rows, &columns))
                    .collect()
            });
            debug_assert!(realized_paths.iter().all(|path| is_king_path(path)));
            self.stats.realized_covers += 1;
            return (self.callback)(Cover {
                paths: realized_paths,
                digit_order: *digit_order,
                row_morph: row_morph as u16,
                column_morph: column_morph as u16,
            });
        }
        for digit in 0..SIDE {
            if used_digits & (1u16 << digit) != 0 {
                continue;
            }
            for option in &self.role_options[digit] {
                if depth != 0 && option.mask & previous_mask == 0 {
                    self.stats.adjacency_prunes += 1;
                    continue;
                }
                let mut next_lengths = lengths;
                let mut capacity_ok = true;
                for (role, length) in next_lengths.iter_mut().enumerate() {
                    if option.mask & (1u8 << role) != 0 {
                        *length += 1;
                    }
                    let remaining_digits = (SIDE - depth - 1) as u8;
                    if *length > self.partition[role]
                        || self.partition[role] - *length > remaining_digits
                    {
                        capacity_ok = false;
                    }
                }
                if !capacity_ok {
                    self.stats.capacity_prunes += 1;
                    continue;
                }
                let mut next_rows = row_support;
                let mut next_columns = column_support;
                let mut spatial_ok = true;
                for (role, &previous) in last_vertices.iter().enumerate() {
                    let vertex = option.vertices[role];
                    if vertex != NO_VERTEX
                        && previous != NO_VERTEX
                        && !self.intersect_edge(&mut next_rows, &mut next_columns, previous, vertex)
                    {
                        spatial_ok = false;
                        break;
                    }
                }
                if !spatial_ok {
                    self.stats.spatial_prunes += 1;
                    continue;
                }
                let mut next_last = last_vertices;
                for (role, &vertex) in option.vertices.iter().enumerate() {
                    if vertex != NO_VERTEX {
                        paths[role][lengths[role] as usize] = vertex;
                        next_last[role] = vertex;
                    }
                }
                digit_order[depth] = digit as u8;
                if !self.dfs(
                    depth + 1,
                    used_digits | (1u16 << digit),
                    option.mask,
                    next_lengths,
                    next_last,
                    paths,
                    digit_order,
                    next_rows,
                    next_columns,
                ) {
                    return false;
                }
            }
        }
        true
    }

    fn intersect_edge(
        &self,
        rows: &mut MorphSet,
        columns: &mut MorphSet,
        left_vertex: u8,
        right_vertex: u8,
    ) -> bool {
        let left = self.puzzle.cells[left_vertex as usize] as usize;
        let right = self.puzzle.cells[right_vertex as usize] as usize;
        rows.intersect_assign(&self.axis.close[left / SIDE][right / SIDE])
            && columns.intersect_assign(&self.axis.close[left % SIDE][right % SIDE])
    }
}

fn classify_cover(
    puzzle: &Puzzle,
    cover: &Cover,
    axis: &AxisMorphs,
) -> Result<Classification, String> {
    let result = Solver::blank(&cover.paths)
        .map_err(|error| format!("cannot construct thermo candidate: {error}"))?
        .count_up_to(2);
    if result.count == 0 {
        return Err("source-derived candidate unexpectedly has no solution".to_owned());
    }
    let first_solution = result.first_solution.expect("positive thermo witness");
    if result.count != 1 || result.capped {
        return Ok(Classification {
            count: result.count,
            capped: result.capped,
            first_solution,
            second_solution: result.second_solution,
        });
    }
    let target = Solver::new(target_givens(puzzle, cover, axis), &[])
        .map_err(|error| format!("cannot construct target classic: {error}"))?
        .count_up_to(2);
    if target.count != 1 || target.capped {
        return Err(format!(
            "source target template is not unique (count={}, capped={})",
            target.count, target.capped
        ));
    }
    if result.first_solution != target.first_solution {
        return Err("unique thermo and source target solutions differ".to_owned());
    }
    Ok(Classification {
        count: 1,
        capped: false,
        first_solution,
        second_solution: None,
    })
}

/// Apply the selected positional morph and global digit order to all seventeen
/// source clues. A unique thermo hit is checked against this exact classic
/// target rather than merely trusting the symbolic construction.
fn target_givens(puzzle: &Puzzle, cover: &Cover, axis: &AxisMorphs) -> [u8; 81] {
    let rows = axis.permutations[cover.row_morph as usize];
    let columns = axis.permutations[cover.column_morph as usize];
    let mut rank = [0u8; SIDE];
    for (position, &digit) in cover.digit_order.iter().enumerate() {
        rank[digit as usize] = position as u8 + 1;
    }
    let mut givens = [0u8; 81];
    for vertex in 0..CLUES {
        let cell = morph_cell(puzzle.cells[vertex], &rows, &columns);
        givens[cell as usize] = rank[puzzle.digits[vertex] as usize];
    }
    givens
}

fn assignment_capacity_feasible(counts: [u8; SIDE], partition: [u8; 3]) -> bool {
    let mut current = [[[false; 10]; 10]; 10];
    current[0][0][0] = true;
    for count in counts {
        let masks: &[u8] = match count {
            1 => &[1, 2, 4],
            2 => &[3, 5, 6],
            3 => &[7],
            _ => return false,
        };
        let mut next = [[[false; 10]; 10]; 10];
        for (a, plane) in current.iter().enumerate().take(partition[0] as usize + 1) {
            for (b, row) in plane.iter().enumerate().take(partition[1] as usize + 1) {
                for (c, &reachable) in row.iter().enumerate().take(partition[2] as usize + 1) {
                    if !reachable {
                        continue;
                    }
                    for &mask in masks {
                        let na = a + usize::from(mask & 1 != 0);
                        let nb = b + usize::from(mask & 2 != 0);
                        let nc = c + usize::from(mask & 4 != 0);
                        if na <= partition[0] as usize
                            && nb <= partition[1] as usize
                            && nc <= partition[2] as usize
                        {
                            next[na][nb][nc] = true;
                        }
                    }
                }
            }
        }
        current = next;
    }
    current[partition[0] as usize][partition[1] as usize][partition[2] as usize]
}

fn write_candidate(
    mut output: Option<&mut BufWriter<File>>,
    line_number: usize,
    puzzle: &Puzzle,
    partition: [u8; 3],
    cover: &Cover,
    classification: &Classification,
) -> Result<(), String> {
    let multiplicity = if classification.count == 1 {
        "unique"
    } else {
        "multiple"
    };
    let second_solution = classification.second_solution.map_or_else(
        || "null".to_owned(),
        |solution| format!("\"{}\"", compact_solution(&solution)),
    );
    let record = format!(
        "{{\"type\":\"candidate\",\"multiplicity\":\"{multiplicity}\",\"count\":{},\"capped\":{},\"source_line\":{line_number},\"source_puzzle\":\"{}\",\"partition\":\"{}\",\"row_morph\":{},\"column_morph\":{},\"digit_order\":\"{}\",\"paths\":\"{}\",\"first_solution\":\"{}\",\"second_solution\":{second_solution}}}",
        classification.count,
        classification.capped,
        puzzle.encoded,
        compact_partition(partition),
        cover.row_morph,
        cover.column_morph,
        cover
            .digit_order
            .iter()
            .map(|digit| (digit + 1).to_string())
            .collect::<Vec<_>>()
            .join(","),
        compact_paths(&cover.paths),
        compact_solution(&classification.first_solution)
    );
    if classification.count == 1 {
        println!("{record}");
    }
    if let Some(writer) = output.as_mut() {
        writeln!(writer, "{record}").map_err(|error| format!("cannot write output: {error}"))?;
    }
    Ok(())
}

fn canonical_layout_key(paths: &[Vec<u8>; ROLES]) -> Vec<u8> {
    let mut best = None;
    for spatial in 0..8u8 {
        for reverse in [false, true] {
            let mut transformed = paths
                .iter()
                .map(|path| {
                    let cells = if reverse {
                        path.iter().rev().copied().collect::<Vec<_>>()
                    } else {
                        path.clone()
                    };
                    cells
                        .into_iter()
                        .map(|cell| transform_cell(cell, spatial))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            transformed.sort_by_key(|path| (Reverse(path.len()), path.clone()));
            let mut key = Vec::with_capacity(CLUES + ROLES);
            for path in transformed {
                key.push(path.len() as u8);
                key.extend(path);
            }
            if best.as_ref().is_none_or(|current| key < *current) {
                best = Some(key);
            }
        }
    }
    best.expect("one canonical transform")
}

fn compact_paths(paths: &[Vec<u8>; ROLES]) -> String {
    paths
        .iter()
        .map(|path| path.iter().map(u8::to_string).collect::<Vec<_>>().join(","))
        .collect::<Vec<_>>()
        .join("|")
}

fn compact_solution(solution: &[u8; 81]) -> String {
    solution
        .iter()
        .map(|digit| char::from(b'0' + *digit))
        .collect()
}

fn compact_partition(partition: [u8; 3]) -> String {
    format!("{}+{}+{}", partition[0], partition[1], partition[2])
}

fn is_king_path(path: &[u8]) -> bool {
    path.windows(2).all(|edge| {
        edge[0] != edge[1]
            && (edge[0] / 9).abs_diff(edge[1] / 9) <= 1
            && (edge[0] % 9).abs_diff(edge[1] % 9) <= 1
    })
}

fn morph_cell(cell: u8, rows: &[u8; SIDE], columns: &[u8; SIDE]) -> u8 {
    9 * rows[(cell / 9) as usize] + columns[(cell % 9) as usize]
}

fn transform_cell(cell: u8, spatial: u8) -> u8 {
    let row = cell / 9;
    let column = cell % 9;
    let (new_row, new_column) = match spatial {
        0 => (row, column),
        1 => (column, 8 - row),
        2 => (8 - row, 8 - column),
        3 => (8 - column, row),
        4 => (row, 8 - column),
        5 => (8 - row, column),
        6 => (column, row),
        7 => (8 - column, 8 - row),
        _ => unreachable!(),
    };
    9 * new_row + new_column
}

fn generate_axis_morphs() -> Vec<[u8; SIDE]> {
    const P3: [[u8; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let mut result = Vec::with_capacity(MORPH_COUNT);
    for band_map in P3 {
        for first_rows in P3 {
            for second_rows in P3 {
                for third_rows in P3 {
                    let within = [first_rows, second_rows, third_rows];
                    let mut permutation = [0u8; SIDE];
                    for old_band in 0..3 {
                        for old_offset in 0..3 {
                            permutation[3 * old_band + old_offset] =
                                3 * band_map[old_band] + within[old_band][old_offset];
                        }
                    }
                    result.push(permutation);
                }
            }
        }
    }
    result
}

fn parse_options() -> Result<Options, String> {
    let mut input = None;
    let mut output = None;
    let mut partitions = vec![[9, 6, 2]];
    let mut start_line = 1usize;
    let mut end_line = usize::MAX;
    let mut max_eligible = None;
    let mut stop_on_first = false;
    let mut progress_every = 100usize;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--input" => input = Some(PathBuf::from(next_value(&mut arguments, "--input")?)),
            "--output" => output = Some(PathBuf::from(next_value(&mut arguments, "--output")?)),
            "--partition" => {
                let value = next_value(&mut arguments, "--partition")?;
                partitions = if value == "all" {
                    THREE_PATH_PARTITIONS.to_vec()
                } else {
                    vec![parse_partition(&value)?]
                };
            }
            "--start-line" => {
                start_line =
                    parse_usize(next_value(&mut arguments, "--start-line")?, "--start-line")?
            }
            "--end-line" => {
                end_line = parse_usize(next_value(&mut arguments, "--end-line")?, "--end-line")?
            }
            "--max-eligible" => {
                max_eligible = Some(parse_usize(
                    next_value(&mut arguments, "--max-eligible")?,
                    "--max-eligible",
                )?)
            }
            "--progress-every" => {
                progress_every = parse_usize(
                    next_value(&mut arguments, "--progress-every")?,
                    "--progress-every",
                )?
            }
            "--stop-on-first" => stop_on_first = true,
            "--help" | "-h" => {
                println!(
                    "Usage: thermo-17c-three-path --input FILE [--partition 9+6+2|all] [--output JSONL] [--start-line N] [--end-line N] [--max-eligible N] [--stop-on-first] [--progress-every N]"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    let input = input.ok_or_else(|| "--input FILE is required".to_owned())?;
    if start_line == 0 || start_line > end_line {
        return Err("line range must be non-empty and one-based".to_owned());
    }
    if max_eligible == Some(0) {
        return Err("--max-eligible must be positive".to_owned());
    }
    Ok(Options {
        input,
        output,
        partitions,
        start_line,
        end_line,
        max_eligible,
        stop_on_first,
        progress_every,
    })
}

fn parse_partition(value: &str) -> Result<[u8; 3], String> {
    let parts = value
        .split(['+', ','])
        .map(|part| {
            part.parse::<u8>()
                .map_err(|_| format!("invalid partition: {value}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let partition: [u8; 3] = parts
        .try_into()
        .map_err(|_| format!("expected three lengths in partition: {value}"))?;
    if !THREE_PATH_PARTITIONS.contains(&partition) {
        return Err(format!(
            "unsupported three-path partition: {}",
            compact_partition(partition)
        ));
    }
    Ok(partition)
}

fn next_value(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_usize(value: String, option: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("invalid integer for {option}: {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_options_are_exact_injections() {
        let puzzle = Puzzle::parse(&synthetic_three_paths(), 1).unwrap();
        assert_eq!(role_options(&puzzle, 8).len(), 3);
        assert_eq!(role_options(&puzzle, 0).len(), 6);
        for digit in 0..8 {
            for option in role_options(&puzzle, digit) {
                assert_eq!(option.mask.count_ones(), 2);
                assert_eq!(
                    option
                        .vertices
                        .into_iter()
                        .filter(|&v| v != NO_VERTEX)
                        .collect::<BTreeSet<_>>()
                        .len(),
                    2
                );
            }
        }

        let mut threefold = puzzle.clone();
        threefold.counts[0] = 3;
        threefold.occurrences[0] = [0, 1, 2];
        let options = role_options(&threefold, 0);
        assert_eq!(options.len(), 6);
        assert!(options.iter().all(|option| option.mask == 0b111));
        assert_eq!(
            options
                .iter()
                .map(|option| option.vertices)
                .collect::<BTreeSet<_>>()
                .len(),
            6
        );
    }

    #[test]
    fn capacity_filter_matches_known_profiles() {
        assert!(assignment_capacity_feasible(
            [2, 2, 2, 2, 2, 2, 2, 2, 1],
            [9, 6, 2]
        ));
        assert!(!assignment_capacity_feasible(
            [3, 3, 3, 3, 1, 1, 1, 1, 1],
            [9, 6, 2]
        ));
        assert!(assignment_capacity_feasible(
            [3, 3, 2, 2, 2, 2, 1, 1, 1],
            [9, 6, 2]
        ));
    }

    #[test]
    fn capacity_dp_matches_independent_count_formula() {
        for singles in 0..=9usize {
            for doubles in 0..=9 - singles {
                let triples = 9 - singles - doubles;
                if singles + 2 * doubles + 3 * triples != CLUES {
                    continue;
                }
                let mut counts = [0u8; SIDE];
                for count in counts.iter_mut().take(singles) {
                    *count = 1;
                }
                for count in counts.iter_mut().skip(singles).take(doubles) {
                    *count = 2;
                }
                for count in counts.iter_mut().skip(singles + doubles) {
                    *count = 3;
                }
                for partition in THREE_PATH_PARTITIONS {
                    assert_eq!(
                        assignment_capacity_feasible(counts, partition),
                        capacity_formula(singles, doubles, triples, partition),
                        "profile {singles}/{doubles}/{triples}, partition {}",
                        compact_partition(partition)
                    );
                }
            }
        }
    }

    #[test]
    fn symbolic_search_finds_synthetic_nine_six_two() {
        let puzzle = Puzzle::parse(&synthetic_three_paths(), 1).unwrap();
        let axis = AxisMorphs::new();
        let mut stats = SearchStats::default();
        let mut found = None;
        let exhausted = enumerate_covers(&puzzle, [9, 6, 2], &axis, &mut stats, |cover| {
            found = Some(cover);
            false
        });
        assert!(!exhausted);
        let cover = found.unwrap();
        assert_eq!(
            cover.paths.iter().map(Vec::len).collect::<Vec<_>>(),
            [9, 6, 2]
        );
        assert!(cover.paths.iter().all(|path| is_king_path(path)));
    }

    #[test]
    fn missing_or_fourfold_digit_is_ineligible() {
        let mut puzzle = Puzzle::parse(&synthetic_three_paths(), 1).unwrap();
        puzzle.counts = [3, 3, 3, 3, 1, 1, 1, 1, 0];
        assert!(!puzzle.eligible_for([9, 6, 2]));
        puzzle.counts = [4, 2, 2, 2, 2, 2, 1, 1, 1];
        assert!(!puzzle.eligible_for([9, 6, 2]));
    }

    #[test]
    fn parse_all_supported_partitions() {
        for partition in THREE_PATH_PARTITIONS {
            assert_eq!(
                parse_partition(&compact_partition(partition)).unwrap(),
                partition
            );
        }
        assert!(parse_partition("9+7+1").is_err());
    }

    #[test]
    fn axis_supports_are_exact() {
        let axis = AxisMorphs::new();
        for left in 0..9 {
            for right in 0..9 {
                for (index, morph) in axis.permutations.iter().enumerate() {
                    assert_eq!(
                        axis.close[left][right].contains(index),
                        morph[left].abs_diff(morph[right]) <= 1
                    );
                }
            }
        }
    }

    #[test]
    fn intersected_axis_support_is_the_exact_conjunction() {
        let axis = AxisMorphs::new();
        let constraints = [(0usize, 1usize), (1, 2), (3, 4), (4, 5)];
        let mut support = MorphSet::all();
        for &(left, right) in &constraints {
            support.intersect_assign(&axis.close[left][right]);
        }
        assert!(support.first().is_some());
        for (index, permutation) in axis.permutations.iter().enumerate() {
            let direct = constraints
                .iter()
                .all(|&(left, right)| permutation[left].abs_diff(permutation[right]) <= 1);
            assert_eq!(support.contains(index), direct);
        }
    }

    #[test]
    fn canonical_key_handles_global_reversal_and_equal_role_swap() {
        let paths = [
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8],
            vec![18, 19, 28, 29],
            vec![40, 31, 32, 23],
        ];
        let swapped = [paths[0].clone(), paths[2].clone(), paths[1].clone()];
        let reversed = paths.clone().map(|mut path| {
            path.reverse();
            path
        });
        assert_eq!(canonical_layout_key(&paths), canonical_layout_key(&swapped));
        assert_eq!(
            canonical_layout_key(&paths),
            canonical_layout_key(&reversed)
        );
    }

    #[test]
    fn disjoint_adjacent_rank_masks_really_give_a_second_solution() {
        let paths = [vec![0, 1, 2], vec![9, 10, 11], vec![18, 19, 20]];
        let first = parse_solution(
            "123456789456789123789123456214365897365897214897214365531642978642978531978531642",
        );
        assert_solution(&first, &paths);
        let mut swapped = first;
        for digit in &mut swapped {
            *digit = match *digit {
                3 => 4,
                4 => 3,
                other => other,
            };
        }
        assert_ne!(first, swapped);
        assert_solution(&swapped, &paths);
    }

    #[test]
    fn retained_nine_six_two_candidate_has_unique_source_but_two_thermo_witnesses() {
        let puzzle = Puzzle::parse(
            "................12..3.45........63...1.......27..........271..5...8.......9...4..",
            14_560,
        )
        .unwrap();
        let axis = AxisMorphs::new();
        let cover = Cover {
            paths: [
                vec![77, 69, 61, 51, 59, 58, 48, 57, 65],
                vec![22, 21, 20, 28, 27, 36],
                vec![16, 17],
            ],
            digit_order: [7, 3, 4, 2, 1, 6, 5, 0, 8],
            row_morph: 31,
            column_morph: 103,
        };
        let target = Solver::new(target_givens(&puzzle, &cover, &axis), &[])
            .unwrap()
            .count_up_to(2);
        assert_eq!(target.count, 1);
        assert!(!target.capped);
        assert_solution(target.first_solution.as_ref().unwrap(), &cover.paths);

        let thermo = Solver::blank(&cover.paths).unwrap().count_up_to(2);
        assert_eq!(thermo.count, 2);
        assert!(thermo.capped);
        assert_solution(thermo.first_solution.as_ref().unwrap(), &cover.paths);
        assert_solution(thermo.second_solution.as_ref().unwrap(), &cover.paths);
        assert_ne!(thermo.first_solution, thermo.second_solution);
    }

    fn capacity_formula(
        singles: usize,
        doubles: usize,
        triples: usize,
        partition: [u8; 3],
    ) -> bool {
        let residual = partition.map(|length| i16::from(length) - triples as i16);
        if residual.iter().any(|&value| value < 0) {
            return false;
        }
        for absent_zero in 0..=doubles {
            for absent_one in 0..=doubles - absent_zero {
                let absent = [absent_zero, absent_one, doubles - absent_zero - absent_one];
                let singleton_uses = std::array::from_fn::<_, ROLES, _>(|role| {
                    residual[role] - (doubles - absent[role]) as i16
                });
                if singleton_uses.iter().all(|&value| value >= 0)
                    && singleton_uses.iter().sum::<i16>() == singles as i16
                {
                    return true;
                }
            }
        }
        false
    }

    fn parse_solution(encoded: &str) -> [u8; 81] {
        assert_eq!(encoded.len(), 81);
        std::array::from_fn(|cell| encoded.as_bytes()[cell] - b'0')
    }

    fn assert_solution(solution: &[u8; 81], paths: &[Vec<u8>; ROLES]) {
        const ALL: u16 = (1 << 9) - 1;
        let mask = |cells: &[usize]| {
            cells
                .iter()
                .fold(0u16, |bits, &cell| bits | 1u16 << (solution[cell] - 1))
        };
        for row in 0..9 {
            assert_eq!(
                mask(&(0..9).map(|column| 9 * row + column).collect::<Vec<_>>()),
                ALL
            );
        }
        for column in 0..9 {
            assert_eq!(
                mask(&(0..9).map(|row| 9 * row + column).collect::<Vec<_>>()),
                ALL
            );
        }
        for box_row in 0..3 {
            for box_column in 0..3 {
                let cells = (0..3)
                    .flat_map(|row| {
                        (0..3).map(move |column| 9 * (3 * box_row + row) + 3 * box_column + column)
                    })
                    .collect::<Vec<_>>();
                assert_eq!(mask(&cells), ALL);
            }
        }
        for path in paths {
            assert!(
                path.windows(2)
                    .all(|edge| { solution[edge[0] as usize] < solution[edge[1] as usize] })
            );
        }
    }

    fn synthetic_three_paths() -> String {
        let mut cells = [b'.'; 81];
        for (position, cell) in (0..9).enumerate() {
            cells[cell] = b'1' + position as u8;
        }
        for (position, cell) in (12..=17).rev().enumerate() {
            cells[cell] = b'1' + position as u8;
        }
        cells[18] = b'7';
        cells[19] = b'8';
        String::from_utf8(cells.to_vec()).unwrap()
    }
}
