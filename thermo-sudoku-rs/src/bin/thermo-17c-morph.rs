//! Exact morph search for 17-clue classics that can be represented by
//! disjoint 9-cell and 8-cell thermometers.
//!
//! A 9-cell thermometer contains every digit once.  In a 17-clue classic
//! that is covered by 9+8 thermometers, eight clue symbols therefore occur
//! twice and one occurs once.  The 9-path orders the symbols (and hence folds
//! the global digit relabeling into the path search); the second occurrence
//! of every non-singleton symbol must form the 8-path in the same order.
//!
//! Sudoku row and column morphs are not enumerated as a Cartesian product.
//! Each possible clue edge carries a 1,296-bit support set for the row morph
//! and another for the column morph.  A path pair is spatially realizable iff
//! both intersections remain non-empty.
//!
//! Transposition need not be enumerated separately for existence: it merely
//! swaps the row- and column-axis support conditions, and both axes have the
//! same 1,296-element morph group.  Transposing the resulting non-transposed
//! realization recovers the corresponding transposed layout.

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
const MORPH_COUNT: usize = 1_296;
const MORPH_WORDS: usize = MORPH_COUNT.div_ceil(64);
const LAST_MORPH_WORD_BITS: usize = MORPH_COUNT - 64 * (MORPH_WORDS - 1);

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
    occurrences: [[u8; 2]; SIDE],
    occurrence_counts: [u8; SIDE],
}

impl Puzzle {
    fn parse(encoded: &str, line_number: usize) -> Result<Self, String> {
        if encoded.len() != 81 {
            return Err(format!(
                "line {line_number}: expected 81 ASCII cells, got {}",
                encoded.len()
            ));
        }
        let mut cells = [u8::MAX; CLUES];
        let mut digits = [u8::MAX; CLUES];
        let mut occurrences = [[u8::MAX; 2]; SIDE];
        let mut occurrence_counts = [0u8; SIDE];
        let mut clue_count = 0usize;

        for (cell, byte) in encoded.bytes().enumerate() {
            match byte {
                b'.' | b'0' => {}
                b'1'..=b'9' => {
                    if clue_count == CLUES {
                        return Err(format!("line {line_number}: more than 17 clues"));
                    }
                    let digit = (byte - b'1') as usize;
                    let count = occurrence_counts[digit] as usize;
                    if count < 2 {
                        occurrences[digit][count] = clue_count as u8;
                    }
                    occurrence_counts[digit] += 1;
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
            occurrence_counts,
        })
    }

    fn is_nine_eight_eligible(&self) -> bool {
        self.occurrence_counts.iter().filter(|&&n| n == 2).count() == 8
            && self.occurrence_counts.iter().filter(|&&n| n == 1).count() == 1
    }

    fn other_occurrence(&self, digit: usize, chosen: u8) -> Option<u8> {
        if self.occurrence_counts[digit] != 2 {
            return None;
        }
        let pair = self.occurrences[digit];
        Some(if pair[0] == chosen { pair[1] } else { pair[0] })
    }
}

#[derive(Clone, Copy, Debug)]
struct RealizedCover {
    path_nine: [u8; 9],
    path_eight: [u8; 8],
    target_missing: u8,
    row_morph: u16,
    column_morph: u16,
}

#[derive(Default, Debug)]
struct SearchStats {
    dfs_nodes: u64,
    spatial_prunes: u64,
    reversed_duplicates: u64,
    realized_covers: u64,
}

#[derive(Default, Debug)]
struct RunStats {
    records: u64,
    eligible_records: u64,
    dfs_nodes: u64,
    spatial_prunes: u64,
    reversed_duplicates: u64,
    realized_covers: u64,
    duplicate_layouts: u64,
    classified_layouts: u64,
    target_checks: u64,
    alternate_template_checks: u64,
    multiple_layouts: u64,
    unique_layouts: u64,
}

#[derive(Debug)]
struct Classification {
    unique: bool,
    target_solution: [u8; 81],
    alternate_missing: Option<u8>,
}

#[derive(Debug)]
struct Options {
    input: PathBuf,
    output: Option<PathBuf>,
    start_line: usize,
    end_line: usize,
    stop_on_first: bool,
    progress_every: usize,
    reference_direct: bool,
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
    if options.reference_direct {
        return run_reference_direct(&options);
    }
    run_fast(options)
}

fn run_fast(options: Options) -> Result<(), String> {
    let started = Instant::now();
    let axis = AxisMorphs::new();
    let input = File::open(&options.input)
        .map_err(|error| format!("cannot open {}: {error}", options.input.display()))?;
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
        writeln!(
            writer,
            "{{\"schema\":\"thermo-17c-morph-v1\",\"mode\":\"exact-9+8\",\"start_line\":{},\"end_line\":{}}}",
            options.start_line, options.end_line
        )
        .map_err(|error| format!("cannot write output: {error}"))?;
    }

    let mut totals = RunStats::default();
    let mut seen_layouts = BTreeSet::<[u8; CLUES]>::new();
    let mut found_unique = false;
    for (index, line) in BufReader::new(input).lines().enumerate() {
        let line_number = index + 1;
        let encoded = line.map_err(|error| format!("cannot read line {line_number}: {error}"))?;
        totals.records += 1;
        let puzzle = Puzzle::parse(&encoded, line_number)?;
        if line_number < options.start_line || line_number > options.end_line {
            continue;
        }
        if !puzzle.is_nine_eight_eligible() {
            continue;
        }
        totals.eligible_records += 1;
        eprintln!("eligible source line {line_number}: {}", puzzle.encoded);

        let mut search_stats = SearchStats::default();
        let mut callback_error = None;
        let keep_searching =
            enumerate_nine_eight_covers(&puzzle, &axis, &mut search_stats, |cover| {
                let key = canonical_layout_key(&cover.path_nine, &cover.path_eight);
                if !seen_layouts.insert(key) {
                    totals.duplicate_layouts += 1;
                    return true;
                }
                totals.classified_layouts += 1;
                match classify_cover(&cover, &mut totals) {
                    Ok(classification) => {
                        if classification.unique {
                            totals.unique_layouts += 1;
                            found_unique = true;
                        } else {
                            totals.multiple_layouts += 1;
                        }
                        if let Err(error) = print_candidate(
                            output.as_mut(),
                            line_number,
                            &puzzle,
                            &cover,
                            &classification,
                        ) {
                            callback_error = Some(error);
                            return false;
                        }
                        !(classification.unique && options.stop_on_first)
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
        totals.dfs_nodes += search_stats.dfs_nodes;
        totals.spatial_prunes += search_stats.spatial_prunes;
        totals.reversed_duplicates += search_stats.reversed_duplicates;
        totals.realized_covers += search_stats.realized_covers;
        if let Some(writer) = output.as_mut() {
            writeln!(
                writer,
                "{{\"type\":\"eligible-record\",\"source_line\":{line_number},\"source_puzzle\":\"{}\",\"dfs_nodes\":{},\"spatial_prunes\":{},\"reversed_duplicates\":{},\"realized_covers\":{}}}",
                puzzle.encoded,
                search_stats.dfs_nodes,
                search_stats.spatial_prunes,
                search_stats.reversed_duplicates,
                search_stats.realized_covers,
            )
            .map_err(|error| format!("cannot write output: {error}"))?;
        }
        if !keep_searching && found_unique && options.stop_on_first {
            break;
        }
        if options.progress_every != 0
            && (totals.eligible_records as usize).is_multiple_of(options.progress_every)
        {
            eprintln!(
                "progress: records={} eligible={} layouts={} unique={} elapsed={:.3}s",
                totals.records,
                totals.eligible_records,
                totals.classified_layouts,
                totals.unique_layouts,
                started.elapsed().as_secs_f64()
            );
        }
    }

    let elapsed = started.elapsed().as_secs_f64();
    let summary = format!(
        "{{\"type\":\"summary\",\"records\":{},\"eligible_records\":{},\"dfs_nodes\":{},\"spatial_prunes\":{},\"reversed_duplicates\":{},\"realized_covers\":{},\"duplicate_layouts\":{},\"classified_layouts\":{},\"target_checks\":{},\"alternate_template_checks\":{},\"multiple_layouts\":{},\"unique_layouts\":{}}}",
        totals.records,
        totals.eligible_records,
        totals.dfs_nodes,
        totals.spatial_prunes,
        totals.reversed_duplicates,
        totals.realized_covers,
        totals.duplicate_layouts,
        totals.classified_layouts,
        totals.target_checks,
        totals.alternate_template_checks,
        totals.multiple_layouts,
        totals.unique_layouts,
    );
    println!("{summary}");
    eprintln!("completed in {elapsed:.6}s");
    if let Some(writer) = output.as_mut() {
        writeln!(writer, "{summary}").map_err(|error| format!("cannot write output: {error}"))?;
        writer
            .flush()
            .map_err(|error| format!("cannot flush output: {error}"))?;
    }
    Ok(())
}

/// Deliberately simple cross-check for the optimized support-set search.
///
/// This mode independently generates the 1,296 legal permutations of one
/// Sudoku axis by visiting all 9! permutations and retaining the ones that
/// preserve bands.  It then examines every row/column Cartesian-product
/// geometry directly.  There are no shared support bitsets or symbolic morph
/// intersections: each fixed geometry gets an explicit 17-vertex adjacency
/// graph and a fresh occurrence/path DFS.
fn run_reference_direct(options: &Options) -> Result<(), String> {
    let started = Instant::now();
    let permutations = generate_axis_morphs_reference();
    if permutations.len() != MORPH_COUNT {
        return Err(format!(
            "reference axis generator produced {} morphs, expected {MORPH_COUNT}",
            permutations.len()
        ));
    }

    let input = File::open(&options.input)
        .map_err(|error| format!("cannot open {}: {error}", options.input.display()))?;
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
        writeln!(
            writer,
            "{{\"schema\":\"thermo-17c-morph-reference-v1\",\"mode\":\"direct-9+8\",\"axis_morphs\":{MORPH_COUNT},\"start_line\":{},\"end_line\":{}}}",
            options.start_line, options.end_line
        )
        .map_err(|error| format!("cannot write output: {error}"))?;
    }

    let mut totals = ReferenceRunStats::default();
    let mut stop = false;
    for (index, line) in BufReader::new(input).lines().enumerate() {
        let line_number = index + 1;
        let encoded = line.map_err(|error| format!("cannot read line {line_number}: {error}"))?;
        totals.records += 1;
        let puzzle = Puzzle::parse(&encoded, line_number)?;
        if line_number < options.start_line || line_number > options.end_line {
            continue;
        }
        if !reference_nine_eight_eligible(&puzzle) {
            continue;
        }
        totals.eligible_records += 1;
        let geometries_before = totals.direct_geometries;
        let dfs_nodes_before = totals.direct_dfs_nodes;
        let realized_before = totals.realized_geometries;

        let row_masks = reference_axis_close_masks(&puzzle, &permutations, true);
        let column_masks = reference_axis_close_masks(&puzzle, &permutations, false);
        let mut first_witness = None;
        'rows: for (row_morph, row_close) in row_masks.iter().enumerate() {
            for (column_morph, column_close) in column_masks.iter().enumerate() {
                totals.direct_geometries += 1;
                let adjacency = std::array::from_fn(|vertex| {
                    row_close[vertex] & column_close[vertex] & !(1u32 << vertex)
                });
                let mut path_nine = [u8::MAX; 9];
                if let Some(witness) = reference_geometry_cover(
                    &puzzle,
                    &adjacency,
                    0,
                    0,
                    None,
                    None,
                    &mut path_nine,
                    &mut totals.direct_dfs_nodes,
                ) {
                    totals.realized_geometries += 1;
                    first_witness.get_or_insert(ReferenceRealizedCover {
                        witness,
                        row_morph: row_morph as u16,
                        column_morph: column_morph as u16,
                    });
                    if options.stop_on_first {
                        stop = true;
                        break 'rows;
                    }
                }
            }
        }

        if let Some(realized) = first_witness {
            totals.records_with_cover += 1;
            print_reference_candidate(
                output.as_mut(),
                line_number,
                &puzzle,
                &realized,
                &permutations,
            )?;
        }
        if let Some(writer) = output.as_mut() {
            writeln!(
                writer,
                "{{\"type\":\"reference-record\",\"source_line\":{line_number},\"source_puzzle\":\"{}\",\"direct_geometries\":{},\"direct_dfs_nodes\":{},\"realized_geometries\":{}}}",
                puzzle.encoded,
                totals.direct_geometries - geometries_before,
                totals.direct_dfs_nodes - dfs_nodes_before,
                totals.realized_geometries - realized_before,
            )
            .map_err(|error| format!("cannot write reference record: {error}"))?;
        }
        if options.progress_every != 0
            && (totals.eligible_records as usize).is_multiple_of(options.progress_every)
        {
            eprintln!(
                "reference progress: records={} eligible={} geometries={} records_with_cover={} elapsed={:.3}s",
                totals.records,
                totals.eligible_records,
                totals.direct_geometries,
                totals.records_with_cover,
                started.elapsed().as_secs_f64()
            );
        }
        if stop {
            break;
        }
    }

    let elapsed = started.elapsed().as_secs_f64();
    let summary = format!(
        "{{\"type\":\"reference_summary\",\"records\":{},\"eligible_records\":{},\"axis_morphs\":{MORPH_COUNT},\"direct_geometries\":{},\"direct_dfs_nodes\":{},\"realized_geometries\":{},\"records_with_cover\":{},\"elapsed_seconds\":{elapsed:.6}}}",
        totals.records,
        totals.eligible_records,
        totals.direct_geometries,
        totals.direct_dfs_nodes,
        totals.realized_geometries,
        totals.records_with_cover,
    );
    println!("{summary}");
    if let Some(writer) = output.as_mut() {
        writeln!(writer, "{summary}")
            .map_err(|error| format!("cannot write reference summary: {error}"))?;
        writer
            .flush()
            .map_err(|error| format!("cannot flush output: {error}"))?;
    }
    Ok(())
}

#[derive(Default, Debug)]
struct ReferenceRunStats {
    records: u64,
    eligible_records: u64,
    direct_geometries: u64,
    direct_dfs_nodes: u64,
    realized_geometries: u64,
    records_with_cover: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReferenceWitness {
    path_nine_vertices: [u8; 9],
    path_eight_vertices: [u8; 8],
    target_missing: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReferenceRealizedCover {
    witness: ReferenceWitness,
    row_morph: u16,
    column_morph: u16,
}

#[allow(clippy::too_many_arguments)]
fn reference_geometry_cover(
    puzzle: &Puzzle,
    adjacency: &[u32; CLUES],
    depth: usize,
    used_digits: u16,
    last_nine: Option<u8>,
    last_eight: Option<u8>,
    path_nine_vertices: &mut [u8; 9],
    dfs_nodes: &mut u64,
) -> Option<ReferenceWitness> {
    *dfs_nodes += 1;
    if depth == 9 {
        let mut path_eight_vertices = [u8::MAX; 8];
        let mut eight_length = 0usize;
        let mut singleton_position = None;
        for (position, &vertex) in path_nine_vertices.iter().enumerate() {
            let digit = puzzle.digits[vertex as usize] as usize;
            if puzzle.occurrence_counts[digit] == 2 {
                let pair = puzzle.occurrences[digit];
                path_eight_vertices[eight_length] =
                    if pair[0] == vertex { pair[1] } else { pair[0] };
                eight_length += 1;
            } else {
                singleton_position = Some(position as u8 + 1);
            }
        }
        debug_assert_eq!(eight_length, 8);
        return Some(ReferenceWitness {
            path_nine_vertices: *path_nine_vertices,
            path_eight_vertices,
            target_missing: singleton_position.expect("one singleton symbol"),
        });
    }

    for digit in 0..SIDE {
        if used_digits & (1u16 << digit) != 0 {
            continue;
        }
        for occurrence in 0..puzzle.occurrence_counts[digit] as usize {
            let nine = puzzle.occurrences[digit][occurrence];
            if let Some(previous) = last_nine
                && adjacency[previous as usize] & (1u32 << nine) == 0
            {
                continue;
            }
            let eight = if puzzle.occurrence_counts[digit] == 2 {
                let pair = puzzle.occurrences[digit];
                Some(if pair[0] == nine { pair[1] } else { pair[0] })
            } else {
                None
            };
            if let (Some(previous), Some(next)) = (last_eight, eight)
                && adjacency[previous as usize] & (1u32 << next) == 0
            {
                continue;
            }
            path_nine_vertices[depth] = nine;
            if let Some(witness) = reference_geometry_cover(
                puzzle,
                adjacency,
                depth + 1,
                used_digits | (1u16 << digit),
                Some(nine),
                eight.or(last_eight),
                path_nine_vertices,
                dfs_nodes,
            ) {
                return Some(witness);
            }
        }
    }
    None
}

fn reference_nine_eight_eligible(puzzle: &Puzzle) -> bool {
    puzzle
        .occurrence_counts
        .iter()
        .copied()
        .filter(|&count| count == 2)
        .count()
        == 8
        && puzzle
            .occurrence_counts
            .iter()
            .copied()
            .filter(|&count| count == 1)
            .count()
            == 1
}

fn reference_axis_close_masks(
    puzzle: &Puzzle,
    permutations: &[[u8; SIDE]],
    rows: bool,
) -> Vec<[u32; CLUES]> {
    permutations
        .iter()
        .map(|permutation| {
            std::array::from_fn(|left_vertex| {
                let left_cell = puzzle.cells[left_vertex] as usize;
                let left_axis = if rows {
                    left_cell / SIDE
                } else {
                    left_cell % SIDE
                };
                let mut mask = 0u32;
                for right_vertex in 0..CLUES {
                    let right_cell = puzzle.cells[right_vertex] as usize;
                    let right_axis = if rows {
                        right_cell / SIDE
                    } else {
                        right_cell % SIDE
                    };
                    if permutation[left_axis].abs_diff(permutation[right_axis]) <= 1 {
                        mask |= 1u32 << right_vertex;
                    }
                }
                mask
            })
        })
        .collect()
}

/// Reference-only generator: enumerate all 9! mappings, then recognize the
/// legal Sudoku-axis subgroup from its defining band-preservation property.
fn generate_axis_morphs_reference() -> Vec<[u8; SIDE]> {
    fn visit(position: usize, permutation: &mut [u8; SIDE], result: &mut Vec<[u8; SIDE]>) {
        if position == SIDE {
            let preserves_bands = (0..3).all(|band| {
                let first = permutation[3 * band] / 3;
                permutation[3 * band + 1] / 3 == first && permutation[3 * band + 2] / 3 == first
            });
            if preserves_bands {
                result.push(*permutation);
            }
            return;
        }
        for swap in position..SIDE {
            permutation.swap(position, swap);
            visit(position + 1, permutation, result);
            permutation.swap(position, swap);
        }
    }

    let mut permutation = std::array::from_fn(|index| index as u8);
    let mut result = Vec::with_capacity(MORPH_COUNT);
    visit(0, &mut permutation, &mut result);
    result.sort_unstable();
    result.dedup();
    result
}

fn print_reference_candidate(
    mut output: Option<&mut BufWriter<File>>,
    line_number: usize,
    puzzle: &Puzzle,
    realized: &ReferenceRealizedCover,
    permutations: &[[u8; SIDE]],
) -> Result<(), String> {
    let rows = &permutations[realized.row_morph as usize];
    let columns = &permutations[realized.column_morph as usize];
    let path_nine = realized
        .witness
        .path_nine_vertices
        .map(|vertex| reference_morph_cell(puzzle.cells[vertex as usize], rows, columns));
    let path_eight = realized
        .witness
        .path_eight_vertices
        .map(|vertex| reference_morph_cell(puzzle.cells[vertex as usize], rows, columns));
    let record = format!(
        "{{\"type\":\"reference_candidate\",\"source_line\":{line_number},\"source_puzzle\":\"{}\",\"target_missing\":{},\"row_morph\":{},\"column_morph\":{},\"path9\":\"{}\",\"path8\":\"{}\"}}",
        puzzle.encoded,
        realized.witness.target_missing,
        realized.row_morph,
        realized.column_morph,
        compact_cells(&path_nine),
        compact_cells(&path_eight),
    );
    println!("{record}");
    if let Some(writer) = output.as_mut() {
        writeln!(writer, "{record}")
            .map_err(|error| format!("cannot write reference candidate: {error}"))?;
    }
    Ok(())
}

fn reference_morph_cell(cell: u8, rows: &[u8; SIDE], columns: &[u8; SIDE]) -> u8 {
    SIDE as u8 * rows[(cell / SIDE as u8) as usize] + columns[(cell % SIDE as u8) as usize]
}

fn enumerate_nine_eight_covers<F>(
    puzzle: &Puzzle,
    axis: &AxisMorphs,
    stats: &mut SearchStats,
    mut callback: F,
) -> bool
where
    F: FnMut(RealizedCover) -> bool,
{
    debug_assert!(puzzle.is_nine_eight_eligible());
    let mut path_nine_vertices = [u8::MAX; 9];
    let mut search = CoverSearch {
        puzzle,
        axis,
        stats,
        callback: &mut callback,
    };
    search.dfs(
        0,
        0,
        &mut path_nine_vertices,
        None,
        None,
        MorphSet::all(),
        MorphSet::all(),
    )
}

struct CoverSearch<'a, F> {
    puzzle: &'a Puzzle,
    axis: &'a AxisMorphs,
    stats: &'a mut SearchStats,
    callback: &'a mut F,
}

impl<F> CoverSearch<'_, F>
where
    F: FnMut(RealizedCover) -> bool,
{
    #[allow(clippy::too_many_arguments)]
    fn dfs(
        &mut self,
        depth: usize,
        used_digits: u16,
        path_nine_vertices: &mut [u8; 9],
        last_nine: Option<u8>,
        last_eight: Option<u8>,
        row_support: MorphSet,
        column_support: MorphSet,
    ) -> bool {
        self.stats.dfs_nodes += 1;
        if depth == 9 {
            if path_nine_vertices[0] > path_nine_vertices[8] {
                self.stats.reversed_duplicates += 1;
                return true;
            }
            let mut path_eight_vertices = [u8::MAX; 8];
            let mut eight_length = 0usize;
            let mut singleton_position = None;
            for (position, &vertex) in path_nine_vertices.iter().enumerate() {
                let digit = self.puzzle.digits[vertex as usize] as usize;
                if let Some(other) = self.puzzle.other_occurrence(digit, vertex) {
                    path_eight_vertices[eight_length] = other;
                    eight_length += 1;
                } else {
                    singleton_position = Some(position as u8 + 1);
                }
            }
            debug_assert_eq!(eight_length, 8);
            let row_morph = row_support.first().expect("non-empty row support");
            let column_morph = column_support.first().expect("non-empty column support");
            let row_permutation = self.axis.permutations[row_morph];
            let column_permutation = self.axis.permutations[column_morph];
            let path_nine = path_nine_vertices.map(|vertex| {
                morph_cell(
                    self.puzzle.cells[vertex as usize],
                    &row_permutation,
                    &column_permutation,
                )
            });
            let path_eight = path_eight_vertices.map(|vertex| {
                morph_cell(
                    self.puzzle.cells[vertex as usize],
                    &row_permutation,
                    &column_permutation,
                )
            });
            debug_assert!(is_king_path(&path_nine));
            debug_assert!(is_king_path(&path_eight));
            self.stats.realized_covers += 1;
            return (self.callback)(RealizedCover {
                path_nine,
                path_eight,
                target_missing: singleton_position.expect("one singleton symbol"),
                row_morph: row_morph as u16,
                column_morph: column_morph as u16,
            });
        }

        for digit in 0..SIDE {
            if used_digits & (1u16 << digit) != 0 {
                continue;
            }
            let occurrence_count = self.puzzle.occurrence_counts[digit] as usize;
            for occurrence in 0..occurrence_count {
                let vertex = self.puzzle.occurrences[digit][occurrence];
                let mut next_rows = row_support;
                let mut next_columns = column_support;
                if let Some(previous) = last_nine
                    && !self.intersect_edge(&mut next_rows, &mut next_columns, previous, vertex)
                {
                    self.stats.spatial_prunes += 1;
                    continue;
                }
                let next_eight = self.puzzle.other_occurrence(digit, vertex);
                if let (Some(previous), Some(next)) = (last_eight, next_eight)
                    && !self.intersect_edge(&mut next_rows, &mut next_columns, previous, next)
                {
                    self.stats.spatial_prunes += 1;
                    continue;
                }
                path_nine_vertices[depth] = vertex;
                if !self.dfs(
                    depth + 1,
                    used_digits | (1u16 << digit),
                    path_nine_vertices,
                    Some(vertex),
                    next_eight.or(last_eight),
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

fn classify_cover(cover: &RealizedCover, totals: &mut RunStats) -> Result<Classification, String> {
    let paths = vec![cover.path_nine.to_vec(), cover.path_eight.to_vec()];
    let target_givens = template_givens(cover, cover.target_missing);
    totals.target_checks += 1;
    let target = Solver::new(target_givens, &[])
        .map_err(|error| format!("cannot construct target classic: {error}"))?
        .count_up_to(2);
    if target.count != 1 || target.capped {
        return Err(format!(
            "source target template is not unique (count={}, capped={})",
            target.count, target.capped
        ));
    }
    let target_solution = target.first_solution.expect("unique target witness");

    for missing in 1..=9u8 {
        if missing == cover.target_missing {
            continue;
        }
        totals.alternate_template_checks += 1;
        let result = Solver::new(template_givens(cover, missing), &[])
            .map_err(|error| format!("cannot construct alternate classic: {error}"))?
            .count_up_to(2);
        if result.count != 0 {
            return Ok(Classification {
                unique: false,
                target_solution,
                alternate_missing: Some(missing),
            });
        }
    }

    let generic = Solver::blank(&paths)
        .map_err(|error| format!("cannot construct thermo candidate: {error}"))?
        .count_up_to(2);
    if generic.count != 1 || generic.capped || generic.first_solution != Some(target_solution) {
        return Err(format!(
            "template proof and generic thermo count disagree (count={}, capped={})",
            generic.count, generic.capped
        ));
    }
    Ok(Classification {
        unique: true,
        target_solution,
        alternate_missing: None,
    })
}

fn template_givens(cover: &RealizedCover, missing: u8) -> [u8; 81] {
    let mut givens = [0u8; 81];
    for (position, &cell) in cover.path_nine.iter().enumerate() {
        givens[cell as usize] = position as u8 + 1;
    }
    let mut next_digit = 1u8;
    for &cell in &cover.path_eight {
        if next_digit == missing {
            next_digit += 1;
        }
        givens[cell as usize] = next_digit;
        next_digit += 1;
    }
    givens
}

fn print_candidate(
    mut output: Option<&mut BufWriter<File>>,
    line_number: usize,
    puzzle: &Puzzle,
    cover: &RealizedCover,
    classification: &Classification,
) -> Result<(), String> {
    let path_nine = compact_cells(&cover.path_nine);
    let path_eight = compact_cells(&cover.path_eight);
    let solution = compact_solution(&classification.target_solution);
    let multiplicity = if classification.unique {
        "unique"
    } else {
        "multiple"
    };
    let alternate = classification
        .alternate_missing
        .map_or_else(|| "null".to_owned(), |digit| digit.to_string());
    let record = format!(
        "{{\"type\":\"candidate\",\"source_line\":{line_number},\"source_puzzle\":\"{}\",\"multiplicity\":\"{multiplicity}\",\"target_missing\":{},\"alternate_missing\":{alternate},\"row_morph\":{},\"column_morph\":{},\"path9\":\"{path_nine}\",\"path8\":\"{path_eight}\",\"solution\":\"{solution}\"}}",
        puzzle.encoded, cover.target_missing, cover.row_morph, cover.column_morph
    );
    if classification.unique {
        println!("{record}");
    }
    if let Some(writer) = output.as_mut() {
        // The corpus and generated fields contain only ASCII digits,
        // punctuation and dots, so no JSON string escaping is required.
        writeln!(writer, "{record}").map_err(|error| format!("cannot write output: {error}"))?;
    }
    Ok(())
}

fn compact_cells<const N: usize>(cells: &[u8; N]) -> String {
    cells
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn compact_solution(solution: &[u8; 81]) -> String {
    solution
        .iter()
        .map(|digit| char::from(b'0' + *digit))
        .collect()
}

fn canonical_layout_key(path_nine: &[u8; 9], path_eight: &[u8; 8]) -> [u8; CLUES] {
    let mut best = [u8::MAX; CLUES];
    for spatial in 0..8u8 {
        for reverse in [false, true] {
            let mut candidate = [0u8; CLUES];
            for (index, target) in candidate[..9].iter_mut().enumerate() {
                let source = if reverse { 8 - index } else { index };
                *target = transform_cell(path_nine[source], spatial);
            }
            for (index, target) in candidate[9..].iter_mut().enumerate() {
                let source = if reverse { 7 - index } else { index };
                *target = transform_cell(path_eight[source], spatial);
            }
            best = best.min(candidate);
        }
    }
    best
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

fn morph_cell(cell: u8, rows: &[u8; SIDE], columns: &[u8; SIDE]) -> u8 {
    9 * rows[(cell / 9) as usize] + columns[(cell % 9) as usize]
}

fn is_king_path<const N: usize>(path: &[u8; N]) -> bool {
    path.windows(2).all(|edge| {
        let left_row = edge[0] / 9;
        let left_column = edge[0] % 9;
        let right_row = edge[1] / 9;
        let right_column = edge[1] % 9;
        edge[0] != edge[1]
            && left_row.abs_diff(right_row) <= 1
            && left_column.abs_diff(right_column) <= 1
    })
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
    let mut start_line = 1usize;
    let mut end_line = usize::MAX;
    let mut stop_on_first = false;
    let mut progress_every = 1usize;
    let mut reference_direct = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--input" => input = Some(PathBuf::from(next_value(&mut arguments, "--input")?)),
            "--output" => output = Some(PathBuf::from(next_value(&mut arguments, "--output")?)),
            "--start-line" => {
                start_line =
                    parse_usize(next_value(&mut arguments, "--start-line")?, "--start-line")?
            }
            "--end-line" => {
                end_line = parse_usize(next_value(&mut arguments, "--end-line")?, "--end-line")?
            }
            "--progress-every" => {
                progress_every = parse_usize(
                    next_value(&mut arguments, "--progress-every")?,
                    "--progress-every",
                )?
            }
            "--stop-on-first" => stop_on_first = true,
            "--reference-direct" => reference_direct = true,
            "--help" | "-h" => {
                println!(
                    "Usage: thermo-17c-morph --input FILE [--output JSONL] [--start-line N] [--end-line N] [--stop-on-first] [--progress-every N] [--reference-direct]"
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
    Ok(Options {
        input,
        output,
        start_line,
        end_line,
        stop_on_first,
        progress_every,
        reference_direct,
    })
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
    fn axis_morphs_are_complete_unique_sudoku_permutations() {
        let morphs = generate_axis_morphs();
        assert_eq!(morphs.len(), MORPH_COUNT);
        let unique = morphs.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(unique.len(), MORPH_COUNT);
        assert_eq!(morphs[0], [0, 1, 2, 3, 4, 5, 6, 7, 8]);
        for morph in morphs {
            assert_eq!(
                morph.into_iter().collect::<BTreeSet<_>>(),
                (0..9u8).collect()
            );
            for row in 0..9 {
                assert_eq!(morph[row] / 3, morph[3 * (row / 3)] / 3);
            }
        }
    }

    #[test]
    fn close_sets_match_every_axis_morph() {
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
    fn reference_axis_generator_matches_the_sudoku_axis_group() {
        let optimized = generate_axis_morphs().into_iter().collect::<BTreeSet<_>>();
        let reference = generate_axis_morphs_reference()
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert_eq!(reference.len(), MORPH_COUNT);
        assert_eq!(reference, optimized);
    }

    #[test]
    fn nine_eight_requires_eight_doubles_and_one_singleton() {
        let encoded = synthetic_parallel_paths();
        let puzzle = Puzzle::parse(&encoded, 1).unwrap();
        assert_eq!(puzzle.occurrence_counts, [2, 2, 2, 2, 2, 2, 2, 2, 1]);
        assert!(puzzle.is_nine_eight_eligible());

        let mut changed = encoded.into_bytes();
        changed[17] = b'.';
        changed[18] = b'2';
        let changed = String::from_utf8(changed).unwrap();
        let puzzle = Puzzle::parse(&changed, 1).unwrap();
        assert!(!puzzle.is_nine_eight_eligible());
    }

    #[test]
    fn symbolic_search_finds_parallel_nine_eight_cover() {
        let encoded = synthetic_parallel_paths();
        let puzzle = Puzzle::parse(&encoded, 1).unwrap();
        let axis = AxisMorphs::new();
        let mut stats = SearchStats::default();
        let mut first = None;
        let exhausted = enumerate_nine_eight_covers(&puzzle, &axis, &mut stats, |cover| {
            first = Some(cover);
            false
        });
        assert!(!exhausted);
        let cover = first.unwrap();
        assert_eq!(cover.path_nine, [0, 1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(cover.path_eight, [17, 16, 15, 14, 13, 12, 11, 10]);
        assert_eq!(cover.target_missing, 9);
        assert_eq!(cover.row_morph, 0);
        assert_eq!(cover.column_morph, 0);
        assert!(is_king_path(&cover.path_nine));
        assert!(is_king_path(&cover.path_eight));
    }

    #[test]
    fn direct_reference_and_symbolic_search_find_scrambled_cover() {
        let rows = [6, 8, 7, 3, 5, 4, 2, 0, 1];
        let columns = [2, 0, 1, 5, 3, 4, 8, 6, 7];
        let scrambled = morph_encoded(&synthetic_parallel_paths(), &rows, &columns);
        let puzzle = Puzzle::parse(&scrambled, 1).unwrap();

        let inverse_rows = invert_axis_morph(&rows);
        let inverse_columns = invert_axis_morph(&columns);
        let row_masks = reference_axis_close_masks(&puzzle, &[inverse_rows], true);
        let column_masks = reference_axis_close_masks(&puzzle, &[inverse_columns], false);
        let adjacency = std::array::from_fn(|vertex| {
            row_masks[0][vertex] & column_masks[0][vertex] & !(1u32 << vertex)
        });
        let mut path_nine = [u8::MAX; 9];
        let mut nodes = 0;
        let reference = reference_geometry_cover(
            &puzzle,
            &adjacency,
            0,
            0,
            None,
            None,
            &mut path_nine,
            &mut nodes,
        )
        .expect("inverse morph must restore the two paths");
        assert!(nodes > 0);
        let realized_nine = reference.path_nine_vertices.map(|vertex| {
            reference_morph_cell(
                puzzle.cells[vertex as usize],
                &inverse_rows,
                &inverse_columns,
            )
        });
        let realized_eight = reference.path_eight_vertices.map(|vertex| {
            reference_morph_cell(
                puzzle.cells[vertex as usize],
                &inverse_rows,
                &inverse_columns,
            )
        });
        assert!(is_king_path(&realized_nine));
        assert!(is_king_path(&realized_eight));

        let axis = AxisMorphs::new();
        let mut stats = SearchStats::default();
        let mut symbolic = None;
        let exhausted = enumerate_nine_eight_covers(&puzzle, &axis, &mut stats, |cover| {
            symbolic = Some(cover);
            false
        });
        assert!(!exhausted);
        assert!(symbolic.is_some());
    }

    #[test]
    fn template_givens_match_requested_omission() {
        let cover = RealizedCover {
            path_nine: [0, 1, 2, 3, 4, 5, 6, 7, 8],
            path_eight: [17, 16, 15, 14, 13, 12, 11, 10],
            target_missing: 9,
            row_morph: 0,
            column_morph: 0,
        };
        let givens = template_givens(&cover, 4);
        assert_eq!(&givens[..9], &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(
            cover
                .path_eight
                .iter()
                .map(|&cell| givens[cell as usize])
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 5, 6, 7, 8, 9]
        );
    }

    #[test]
    fn canonical_key_identifies_d4_and_global_reversal() {
        let p9 = [0, 1, 2, 3, 4, 5, 6, 7, 8];
        let p8 = [17, 16, 15, 14, 13, 12, 11, 10];
        let transformed9 = p9.map(|cell| transform_cell(cell, 3));
        let transformed8 = p8.map(|cell| transform_cell(cell, 3));
        assert_eq!(
            canonical_layout_key(&p9, &p8),
            canonical_layout_key(&transformed9, &transformed8)
        );
        let reversed9 = std::array::from_fn(|index| p9[8 - index]);
        let reversed8 = std::array::from_fn(|index| p8[7 - index]);
        assert_eq!(
            canonical_layout_key(&p9, &p8),
            canonical_layout_key(&reversed9, &reversed8)
        );
    }

    fn synthetic_parallel_paths() -> String {
        let mut cells = [b'.'; 81];
        for (index, cell) in (0..9).enumerate() {
            cells[cell] = b'1' + index as u8;
        }
        for (index, cell) in (10..=17).rev().enumerate() {
            cells[cell] = b'1' + index as u8;
        }
        String::from_utf8(cells.to_vec()).unwrap()
    }

    fn morph_encoded(encoded: &str, rows: &[u8; SIDE], columns: &[u8; SIDE]) -> String {
        let mut cells = [b'.'; 81];
        for (cell, byte) in encoded.bytes().enumerate() {
            if byte != b'.' {
                cells[morph_cell(cell as u8, rows, columns) as usize] = byte;
            }
        }
        String::from_utf8(cells.to_vec()).unwrap()
    }

    fn invert_axis_morph(morph: &[u8; SIDE]) -> [u8; SIDE] {
        let mut inverse = [u8::MAX; SIDE];
        for (old, &new) in morph.iter().enumerate() {
            inverse[new as usize] = old as u8;
        }
        inverse
    }
}
