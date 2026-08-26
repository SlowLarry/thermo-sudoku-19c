//! Exact and pilot search for 17-cell thermo networks with overlap/branching.
//!
//! Once thermometer paths may share cells, only their directed local
//! comparisons matter.  For a fixed catalogue record, Sudoku morph, and digit
//! order, it is sufficient to test the saturated network containing every
//! target-true king-neighbour comparison among the 17 clue cells: adding such
//! comparisons preserves the target and cannot destroy uniqueness.  Requiring
//! every clue cell to be incident makes the saturated network itself a valid
//! 17-covered-cell construction out of overlapping two-cell thermometers.
//!
//! A unique network must compare every consecutive pair in the target digit
//! order.  Otherwise globally swapping those two consecutive digits produces
//! a second solution.  Consequently only Hamiltonian paths of the nine-symbol
//! adjacency graph need solver classification.
//!
//! Coordinate morphs are handled symbolically.  The 1,296 legal row-axis and
//! 1,296 legal column-axis permutations are first deduplicated by their
//! 17-vertex closeness masks; intersections then produce the distinct local
//! adjacency networks.  Transposition is already represented by exchanging
//! the two axis domains.  Reversing a complete digit order is equivalent to
//! complementing every Sudoku digit, so exact/guided modes retain one direction.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use thermo_sudoku::{SolveResult, Solver};

const SIDE: usize = 9;
const CELLS: usize = 81;
const CLUES: usize = 17;
const PAIRS: usize = CLUES * (CLUES - 1) / 2;
const PAIR_WORDS: usize = PAIRS.div_ceil(64);
const RELATIONS: usize = CLUES * CLUES;
const RELATION_WORDS: usize = RELATIONS.div_ceil(64);
const MORPH_COUNT: usize = 1_296;
const CANONICAL_DIGIT_ORDERS: u64 = 362_880 / 2;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;
const SCHEMA: &str = "thermo-17c-overlap-v1";
const CHECKPOINT_SCHEMA: &str = "thermo-17c-overlap-checkpoint-v3";
const ALGORITHM_REVISION: &str = "saturated-axis-poset-antichain-hamiltonian-unified-hasse-mcv-v3";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Identity,
    Exact,
    Guided,
}

impl Mode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "identity" => Ok(Self::Identity),
            "exact" => Ok(Self::Exact),
            "guided" => Ok(Self::Guided),
            _ => Err(format!(
                "invalid --mode {value}; expected identity, exact, or guided"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Exact => "exact",
            Self::Guided => "guided",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
struct PairMask([u64; PAIR_WORDS]);

impl PairMask {
    fn insert(&mut self, pair: usize) {
        self.0[pair / 64] |= 1u64 << (pair % 64);
    }

    fn contains(self, pair: usize) -> bool {
        self.0[pair / 64] & (1u64 << (pair % 64)) != 0
    }

    fn intersection(self, other: Self) -> Self {
        let mut result = Self::default();
        for index in 0..PAIR_WORDS {
            result.0[index] = self.0[index] & other.0[index];
        }
        result
    }

    fn is_subset_of(self, other: Self) -> bool {
        self.0
            .iter()
            .zip(other.0)
            .all(|(&left, right)| left & !right == 0)
    }

    fn count(self) -> u32 {
        self.0.iter().map(|word| word.count_ones()).sum()
    }

    fn is_empty(self) -> bool {
        self.0.iter().all(|&word| word == 0)
    }

    fn hex(self) -> String {
        self.0
            .iter()
            .rev()
            .map(|word| format!("{word:016x}"))
            .collect::<String>()
            .trim_start_matches('0')
            .to_owned()
            .pipe_nonempty("0")
    }
}

trait NonemptyString {
    fn pipe_nonempty(self, fallback: &str) -> String;
}

impl NonemptyString for String {
    fn pipe_nonempty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_owned()
        } else {
            self
        }
    }
}

#[derive(Debug)]
struct PairUniverse {
    pairs: [(u8, u8); PAIRS],
    index: [[u16; CLUES]; CLUES],
}

impl PairUniverse {
    fn new() -> Self {
        let mut pairs = [(0u8, 0u8); PAIRS];
        let mut index = [[u16::MAX; CLUES]; CLUES];
        let mut next = 0usize;
        for left in 0..CLUES {
            for right in left + 1..CLUES {
                pairs[next] = (left as u8, right as u8);
                next += 1;
            }
        }
        assert_eq!(next, PAIRS);
        for (pair, &(left, right)) in pairs.iter().enumerate() {
            index[left as usize][right as usize] = pair as u16;
            index[right as usize][left as usize] = pair as u16;
        }
        Self { pairs, index }
    }

    fn pair_index(&self, left: usize, right: usize) -> usize {
        self.index[left][right] as usize
    }
}

#[derive(Clone, Debug)]
struct Puzzle {
    encoded: String,
    cells: [u8; CLUES],
    digits: [u8; CLUES],
    digit_mask: u16,
}

impl Puzzle {
    fn parse(encoded: &str, line_number: usize) -> Result<Self, String> {
        if encoded.len() != CELLS {
            return Err(format!(
                "line {line_number}: expected 81 ASCII cells, got {}",
                encoded.len()
            ));
        }
        let mut cells = [u8::MAX; CLUES];
        let mut digits = [u8::MAX; CLUES];
        let mut digit_mask = 0u16;
        let mut clue_count = 0usize;
        for (cell, byte) in encoded.bytes().enumerate() {
            match byte {
                b'.' | b'0' => {}
                b'1'..=b'9' => {
                    if clue_count == CLUES {
                        return Err(format!("line {line_number}: more than 17 clues"));
                    }
                    cells[clue_count] = cell as u8;
                    digits[clue_count] = byte - b'1';
                    digit_mask |= 1u16 << (byte - b'1');
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
            digit_mask,
        })
    }

    fn all_digits_present(&self) -> bool {
        self.digit_mask == 0x01ff
    }

    fn unequal_pair_mask(&self, universe: &PairUniverse) -> PairMask {
        let mut mask = PairMask::default();
        for (pair, &(left, right)) in universe.pairs.iter().enumerate() {
            if self.digits[left as usize] != self.digits[right as usize] {
                mask.insert(pair);
            }
        }
        mask
    }
}

#[derive(Clone, Debug)]
struct AxisClass {
    mask: PairMask,
    representative: u16,
    multiplicity: u16,
}

#[derive(Clone, Debug)]
struct NetworkClass {
    mask: PairMask,
    row_morph: u16,
    column_morph: u16,
    morph_pair_multiplicity: u64,
}

#[derive(Clone, Debug)]
struct OrderCase {
    order: [u8; SIDE],
    rank: [u8; SIDE],
    orientation_key: u64,
    strength: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
struct ClosureMask([u64; RELATION_WORDS]);

impl ClosureMask {
    fn insert(&mut self, from: usize, to: usize) {
        let relation = from * CLUES + to;
        self.0[relation / 64] |= 1u64 << (relation % 64);
    }

    fn contains_relation(self, relation: usize) -> bool {
        self.0[relation / 64] & (1u64 << (relation % 64)) != 0
    }

    fn is_subset_of(self, other: Self) -> bool {
        self.0
            .iter()
            .zip(other.0)
            .all(|(&left, right)| left & !right == 0)
    }

    fn count(self) -> u32 {
        self.0.iter().map(|word| word.count_ones()).sum()
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    network_index: usize,
    order_index: usize,
    network: NetworkClass,
    order: OrderCase,
    closure: ClosureMask,
}

#[derive(Clone, Debug)]
struct Options {
    input: PathBuf,
    output: Option<PathBuf>,
    mode: Mode,
    start_line: usize,
    end_line: usize,
    max_units: Option<u64>,
    solution_cap: u64,
    guided_graphs: usize,
    guided_orders: usize,
    emit_cases: bool,
    stop_on_first: bool,
    progress_every: u64,
    checkpoint: Option<PathBuf>,
    resume: bool,
    checkpoint_every: u64,
    expected_records: Option<usize>,
    expected_fnv64: Option<u64>,
    corpus_is_complete: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SearchStats {
    records_in_range: u64,
    records_missing_digits: u64,
    row_axis_classes: u64,
    row_axis_maximal_classes: u64,
    column_axis_classes: u64,
    column_axis_maximal_classes: u64,
    network_intersections_scanned: u64,
    network_classes: u64,
    retained_network_classes: u64,
    represented_morph_pairs: u64,
    retained_mask_morph_pairs: u64,
    guided_nonrepresentative_intersections: u64,
    guided_omitted_orientations: u64,
    coverage_pruned_classes: u64,
    no_hamiltonian_classes: u64,
    canonical_orders: u64,
    hamiltonian_orders: u64,
    structurally_pruned_orders: u64,
    duplicate_orientation_orders: u64,
    raw_candidate_orientations: u64,
    unique_poset_closures: u64,
    duplicate_poset_closures: u64,
    maximal_poset_closures: u64,
    dominated_poset_closures: u64,
    candidate_units: u64,
    classified_units: u64,
    zero: u64,
    unique: u64,
    multiple: u64,
    exact_counts: u64,
    capped_counts: u64,
    observed_solution_sum: u64,
    solver_nodes: u64,
    solver_branches: u64,
    solver_propagation_rounds: u64,
    solver_comparison_revisions: u64,
    best_count: Option<u64>,
    best_capped: bool,
    best_unit: Option<u64>,
    best_line: Option<usize>,
}

impl SearchStats {
    fn add_result(&mut self, unit: u64, line: usize, result: &SolveResult) {
        self.classified_units += 1;
        match result.count {
            0 => self.zero += 1,
            1 if !result.capped => self.unique += 1,
            _ => self.multiple += 1,
        }
        if result.capped {
            self.capped_counts += 1;
        } else {
            self.exact_counts += 1;
        }
        self.observed_solution_sum = self.observed_solution_sum.saturating_add(result.count);
        self.solver_nodes = self.solver_nodes.saturating_add(result.stats.nodes);
        self.solver_branches = self.solver_branches.saturating_add(result.stats.branches);
        self.solver_propagation_rounds = self
            .solver_propagation_rounds
            .saturating_add(result.stats.propagation_rounds);
        self.solver_comparison_revisions = self
            .solver_comparison_revisions
            .saturating_add(result.stats.thermo_revisions);
        let replace = self.best_count.is_none_or(|best| {
            result.count < best || (result.count == best && self.best_capped && !result.capped)
        });
        if replace {
            self.best_count = Some(result.count);
            self.best_capped = result.capped;
            self.best_unit = Some(unit);
            self.best_line = Some(line);
        }
    }
}

#[derive(Clone, Debug)]
struct Checkpoint {
    fingerprint: u64,
    next_unit: u64,
    output_bytes: Option<u64>,
    output_fnv64: Option<u64>,
    classification: SearchStats,
}

#[derive(Debug)]
struct Output {
    writer: BufWriter<File>,
    bytes: u64,
    fnv64: u64,
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
    validate_distinct_paths(&options)?;
    let started = Instant::now();
    let input_bytes = fs::read(&options.input)
        .map_err(|error| format!("cannot read {}: {error}", options.input.display()))?;
    let input_fnv64 = fnv1a64(&input_bytes);
    if let Some(expected) = options.expected_fnv64
        && expected != input_fnv64
    {
        return Err(format!(
            "input FNV-1a mismatch: expected {expected:016x}, got {input_fnv64:016x}"
        ));
    }
    let puzzles = parse_puzzles(&input_bytes)?;
    if let Some(expected) = options.expected_records
        && expected != puzzles.len()
    {
        return Err(format!(
            "input record count mismatch: expected {expected}, got {}",
            puzzles.len()
        ));
    }
    let effective_end = options.end_line.min(puzzles.len());
    let fingerprint = run_fingerprint(&options, input_fnv64, puzzles.len());
    let resumed = if options.resume {
        let path = options
            .checkpoint
            .as_ref()
            .ok_or_else(|| "--resume requires --checkpoint FILE".to_owned())?;
        let checkpoint = read_checkpoint(path)?;
        if checkpoint.fingerprint != fingerprint {
            return Err(format!(
                "checkpoint fingerprint mismatch: expected {fingerprint:016x}, got {:016x}",
                checkpoint.fingerprint
            ));
        }
        Some(checkpoint)
    } else {
        None
    };
    let resume_unit = resumed
        .as_ref()
        .map_or(0, |checkpoint| checkpoint.next_unit);
    let mut stats = resumed
        .as_ref()
        .map_or_else(SearchStats::default, |checkpoint| {
            checkpoint.classification.clone()
        });

    let mut output = open_output(&options, resumed.as_ref())?;
    if options.resume {
        write_jsonl(
            output.as_mut(),
            &format!(
                "{{\"type\":\"resume\",\"schema\":\"{SCHEMA}\",\"fingerprint\":\"{fingerprint:016x}\",\"resume_unit\":{resume_unit}}}"
            ),
        )?;
    } else {
        write_jsonl(
            output.as_mut(),
            &header_json(
                &options,
                input_fnv64,
                input_bytes.len(),
                puzzles.len(),
                effective_end,
                fingerprint,
            ),
        )?;
    }

    let universe = PairUniverse::new();
    let axis_morphs = (options.mode != Mode::Identity).then(generate_axis_morphs);
    let mut next_unit = 0u64;
    let mut processed_this_invocation = 0u64;
    let mut limit_hit = false;
    let mut stopped_on_unique = stop_on_first_reached(&options, &stats);
    let mut last_checkpoint_unit = resume_unit;

    'catalogue: for (index, puzzle) in puzzles.iter().enumerate() {
        let line_number = index + 1;
        if line_number < options.start_line || line_number > effective_end {
            continue;
        }
        stats.records_in_range += 1;
        if !puzzle.all_digits_present() {
            stats.records_missing_digits += 1;
            continue;
        }

        let unequal = puzzle.unequal_pair_mask(&universe);
        let mut networks = match options.mode {
            Mode::Identity => {
                stats.row_axis_classes += 1;
                stats.row_axis_maximal_classes += 1;
                stats.column_axis_classes += 1;
                stats.column_axis_maximal_classes += 1;
                stats.network_intersections_scanned += 1;
                stats.network_classes += 1;
                stats.represented_morph_pairs += 1;
                vec![identity_network(puzzle, &universe, unequal)]
            }
            Mode::Exact | Mode::Guided => network_classes(
                puzzle,
                &universe,
                axis_morphs.as_ref().expect("axis morphs initialized"),
                unequal,
                &mut stats,
                (options.mode == Mode::Guided).then_some(options.guided_graphs),
            ),
        };
        networks.sort_by_key(|network| (Reverse(network.mask.count()), network.mask));
        stats.retained_network_classes = stats
            .retained_network_classes
            .saturating_add(networks.len() as u64);
        stats.retained_mask_morph_pairs = stats.retained_mask_morph_pairs.saturating_add(
            networks
                .iter()
                .map(|network| network.morph_pair_multiplicity)
                .sum::<u64>(),
        );
        let mut raw_candidates = Vec::new();
        for (network_index, network) in networks.iter().enumerate() {
            if !all_vertices_incident(network.mask, &universe) {
                stats.coverage_pruned_classes += 1;
                continue;
            }
            let digit_graph = symbol_graph(network.mask, puzzle, &universe);
            let mut orders = match options.mode {
                Mode::Identity => {
                    stats.canonical_orders += 1;
                    let order = [0, 1, 2, 3, 4, 5, 6, 7, 8];
                    if !is_hamiltonian_order(&order, &digit_graph) {
                        stats.structurally_pruned_orders += 1;
                        Vec::new()
                    } else {
                        stats.hamiltonian_orders += 1;
                        vec![make_order_case(order, network.mask, puzzle, &universe)]
                    }
                }
                Mode::Exact | Mode::Guided => {
                    stats.canonical_orders = stats
                        .canonical_orders
                        .saturating_add(CANONICAL_DIGIT_ORDERS);
                    let (cases, hamiltonian_orders) =
                        hamiltonian_order_cases(&digit_graph, network.mask, puzzle, &universe);
                    stats.hamiltonian_orders =
                        stats.hamiltonian_orders.saturating_add(hamiltonian_orders);
                    stats.structurally_pruned_orders = stats
                        .structurally_pruned_orders
                        .saturating_add(CANONICAL_DIGIT_ORDERS - hamiltonian_orders);
                    stats.duplicate_orientation_orders = stats
                        .duplicate_orientation_orders
                        .saturating_add(hamiltonian_orders - cases.len() as u64);
                    cases
                }
            };
            if orders.is_empty() {
                stats.no_hamiltonian_classes += 1;
                continue;
            }
            orders.sort_by_key(|case| (Reverse(case.strength), case.orientation_key));
            if options.mode == Mode::Guided && orders.len() > options.guided_orders {
                stats.guided_omitted_orientations = stats
                    .guided_omitted_orientations
                    .saturating_add((orders.len() - options.guided_orders) as u64);
                orders.truncate(options.guided_orders);
            }

            for (order_index, order_case) in orders.into_iter().enumerate() {
                let closure =
                    candidate_transitive_closure(network.mask, puzzle, &universe, &order_case.rank);
                raw_candidates.push(Candidate {
                    network_index,
                    order_index,
                    network: network.clone(),
                    order: order_case,
                    closure,
                });
            }
        }
        stats.raw_candidate_orientations = stats
            .raw_candidate_orientations
            .saturating_add(raw_candidates.len() as u64);
        let raw_count = raw_candidates.len() as u64;
        let (candidates, unique_closures) = maximal_closure_candidates(raw_candidates);
        stats.unique_poset_closures = stats
            .unique_poset_closures
            .saturating_add(unique_closures as u64);
        stats.duplicate_poset_closures = stats
            .duplicate_poset_closures
            .saturating_add(raw_count.saturating_sub(unique_closures as u64));
        stats.maximal_poset_closures = stats
            .maximal_poset_closures
            .saturating_add(candidates.len() as u64);
        stats.dominated_poset_closures = stats
            .dominated_poset_closures
            .saturating_add((unique_closures - candidates.len()) as u64);

        for candidate in candidates {
            let unit = next_unit;
            if unit < resume_unit {
                stats.candidate_units += 1;
                next_unit += 1;
                if stopped_on_unique && next_unit == resume_unit {
                    break 'catalogue;
                }
                continue;
            }
            if options
                .max_units
                .is_some_and(|limit| processed_this_invocation >= limit)
            {
                limit_hit = true;
                break 'catalogue;
            }
            stats.candidate_units += 1;

            let comparisons = realize_comparisons(
                candidate.network.mask,
                puzzle,
                &universe,
                &candidate.order,
                axis_morphs.as_deref(),
                candidate.network.row_morph as usize,
                candidate.network.column_morph as usize,
            );
            if comparisons.len() > thermo_sudoku::MAX_COMPARISONS {
                return Err(format!(
                    "line {line_number}, unit {unit}: {} saturated comparisons exceed solver capacity {}",
                    comparisons.len(),
                    thermo_sudoku::MAX_COMPARISONS
                ));
            }
            let solver = Solver::blank_comparisons(&comparisons).map_err(|error| {
                format!("line {line_number}, unit {unit}: cannot build network: {error}")
            })?;
            let result = solver.count_up_to(options.solution_cap);
            stats.add_result(unit, line_number, &result);
            processed_this_invocation += 1;
            next_unit += 1;

            let report_case =
                options.emit_cases || result.count == 0 || (result.count == 1 && !result.capped);
            if report_case {
                let case = case_json(
                    unit,
                    line_number,
                    candidate.network_index,
                    candidate.order_index,
                    puzzle,
                    &candidate.network,
                    &candidate.order,
                    &comparisons,
                    &result,
                )?;
                write_jsonl(output.as_mut(), &case)?;
            }

            if options.progress_every != 0
                && stats
                    .classified_units
                    .is_multiple_of(options.progress_every)
            {
                eprintln!(
                    "classified={} unit={} line={} zero={} unique={} multiple={} best={} elapsed={:.3}s",
                    stats.classified_units,
                    unit,
                    line_number,
                    stats.zero,
                    stats.unique,
                    stats.multiple,
                    format_bound(stats.best_count, stats.best_capped),
                    started.elapsed().as_secs_f64()
                );
            }

            if let Some(path) = options.checkpoint.as_ref()
                && next_unit.saturating_sub(last_checkpoint_unit) >= options.checkpoint_every
            {
                flush_output(output.as_mut())?;
                let (output_bytes, output_fnv64) = output_checkpoint(output.as_ref());
                write_checkpoint(
                    path,
                    fingerprint,
                    next_unit,
                    output_bytes,
                    output_fnv64,
                    &stats,
                )?;
                last_checkpoint_unit = next_unit;
            }
            if result.count == 1 && !result.capped && options.stop_on_first {
                stopped_on_unique = true;
                break 'catalogue;
            }
        }
    }

    if next_unit < resume_unit {
        return Err(format!(
            "checkpoint resumes at unit {resume_unit}, but this deterministic scope contains only {next_unit} units"
        ));
    }
    if let Some(path) = options.checkpoint.as_ref() {
        flush_output(output.as_mut())?;
        let (output_bytes, output_fnv64) = output_checkpoint(output.as_ref());
        write_checkpoint(
            path,
            fingerprint,
            next_unit,
            output_bytes,
            output_fnv64,
            &stats,
        )?;
    }

    let scope_exhausted = !limit_hit && !stopped_on_unique;
    let catalogue_range_complete = scope_exhausted
        && options.start_line == 1
        && effective_end == puzzles.len()
        && options.end_line >= puzzles.len();
    let identity_slice_complete = options.mode == Mode::Identity && catalogue_range_complete;
    let generalized_complete = options.mode == Mode::Exact
        && catalogue_range_complete
        && options.corpus_is_complete
        && options.expected_records == Some(puzzles.len())
        && options.expected_fnv64 == Some(input_fnv64);
    let summary = summary_json(
        &options,
        &stats,
        input_fnv64,
        fingerprint,
        resume_unit,
        next_unit,
        processed_this_invocation,
        scope_exhausted,
        catalogue_range_complete,
        identity_slice_complete,
        generalized_complete,
        limit_hit,
        stopped_on_unique,
        started.elapsed().as_secs_f64(),
    );
    println!("{summary}");
    write_jsonl(output.as_mut(), &summary)?;
    flush_output(output.as_mut())?;
    Ok(())
}

fn identity_network(puzzle: &Puzzle, universe: &PairUniverse, unequal: PairMask) -> NetworkClass {
    let identity: [u8; SIDE] = std::array::from_fn(|index| index as u8);
    NetworkClass {
        mask: spatial_adjacency_mask(puzzle, universe, &identity, &identity).intersection(unequal),
        row_morph: 0,
        column_morph: 0,
        morph_pair_multiplicity: 1,
    }
}

fn network_classes(
    puzzle: &Puzzle,
    universe: &PairUniverse,
    morphs: &[[u8; SIDE]],
    unequal: PairMask,
    stats: &mut SearchStats,
    guided_limit: Option<usize>,
) -> Vec<NetworkClass> {
    let raw_row_classes = axis_classes(puzzle, universe, morphs, true, unequal);
    let raw_column_classes = axis_classes(puzzle, universe, morphs, false, unequal);
    stats.row_axis_classes = stats
        .row_axis_classes
        .saturating_add(raw_row_classes.len() as u64);
    stats.column_axis_classes = stats
        .column_axis_classes
        .saturating_add(raw_column_classes.len() as u64);
    let row_classes = maximal_axis_classes(raw_row_classes.clone());
    let column_classes = maximal_axis_classes(raw_column_classes.clone());
    stats.row_axis_maximal_classes = stats
        .row_axis_maximal_classes
        .saturating_add(row_classes.len() as u64);
    stats.column_axis_maximal_classes = stats
        .column_axis_maximal_classes
        .saturating_add(column_classes.len() as u64);
    stats.represented_morph_pairs = stats
        .represented_morph_pairs
        .saturating_add((MORPH_COUNT * MORPH_COUNT) as u64);

    let intersections = row_classes.len().saturating_mul(column_classes.len());
    stats.network_intersections_scanned = stats
        .network_intersections_scanned
        .saturating_add(intersections as u64);
    if let Some(limit) = guided_limit {
        let mut selected = guided_network_candidates(&row_classes, &column_classes, limit);
        recompute_exact_network_multiplicities(
            &mut selected,
            &raw_row_classes,
            &raw_column_classes,
        );
        stats.guided_nonrepresentative_intersections = stats
            .guided_nonrepresentative_intersections
            .saturating_add(intersections.saturating_sub(selected.len()) as u64);
        return selected;
    }

    let mut combined = BTreeMap::<PairMask, NetworkClass>::new();
    for rows in &row_classes {
        for columns in &column_classes {
            let mask = rows.mask.intersection(columns.mask);
            let multiplicity = u64::from(rows.multiplicity) * u64::from(columns.multiplicity);
            combined
                .entry(mask)
                .and_modify(|class| {
                    class.morph_pair_multiplicity =
                        class.morph_pair_multiplicity.saturating_add(multiplicity);
                })
                .or_insert(NetworkClass {
                    mask,
                    row_morph: rows.representative,
                    column_morph: columns.representative,
                    morph_pair_multiplicity: multiplicity,
                });
        }
    }
    stats.network_classes = stats.network_classes.saturating_add(combined.len() as u64);
    let mut maximal = maximal_network_antichain(combined.into_values().collect());
    recompute_exact_network_multiplicities(&mut maximal, &raw_row_classes, &raw_column_classes);
    maximal
}

fn guided_network_candidates(
    row_classes: &[AxisClass],
    column_classes: &[AxisClass],
    limit: usize,
) -> Vec<NetworkClass> {
    let mut selected = BTreeMap::<PairMask, NetworkClass>::new();
    for rows in row_classes {
        for columns in column_classes {
            let mask = rows.mask.intersection(columns.mask);
            let multiplicity = u64::from(rows.multiplicity) * u64::from(columns.multiplicity);
            if let Some(class) = selected.get_mut(&mask) {
                class.morph_pair_multiplicity =
                    class.morph_pair_multiplicity.saturating_add(multiplicity);
                continue;
            }
            if selected.len() == limit {
                let worst = selected
                    .keys()
                    .copied()
                    .max_by(|left, right| compare_network_priority(*left, *right))
                    .expect("positive guided limit has a worst retained mask");
                if compare_network_priority(mask, worst).is_gt() {
                    continue;
                }
                selected.remove(&worst);
            }
            selected.insert(
                mask,
                NetworkClass {
                    mask,
                    row_morph: rows.representative,
                    column_morph: columns.representative,
                    morph_pair_multiplicity: multiplicity,
                },
            );
        }
    }
    selected.into_values().collect()
}

fn recompute_exact_network_multiplicities(
    selected: &mut [NetworkClass],
    raw_row_classes: &[AxisClass],
    raw_column_classes: &[AxisClass],
) {
    let indices = selected
        .iter()
        .enumerate()
        .map(|(index, class)| (class.mask, index))
        .collect::<BTreeMap<_, _>>();
    for class in selected.iter_mut() {
        class.morph_pair_multiplicity = 0;
    }
    for rows in raw_row_classes {
        for columns in raw_column_classes {
            let mask = rows.mask.intersection(columns.mask);
            if let Some(&index) = indices.get(&mask) {
                selected[index].morph_pair_multiplicity = selected[index]
                    .morph_pair_multiplicity
                    .saturating_add(u64::from(rows.multiplicity) * u64::from(columns.multiplicity));
            }
        }
    }
}

fn compare_network_priority(left: PairMask, right: PairMask) -> std::cmp::Ordering {
    Reverse(left.count())
        .cmp(&Reverse(right.count()))
        .then_with(|| left.cmp(&right))
}

fn axis_classes(
    puzzle: &Puzzle,
    universe: &PairUniverse,
    morphs: &[[u8; SIDE]],
    rows: bool,
    relevant: PairMask,
) -> Vec<AxisClass> {
    let mut classes = BTreeMap::<PairMask, AxisClass>::new();
    for (morph_index, morph) in morphs.iter().enumerate() {
        let mut mask = PairMask::default();
        for (pair, &(left, right)) in universe.pairs.iter().enumerate() {
            let left_cell = puzzle.cells[left as usize] as usize;
            let right_cell = puzzle.cells[right as usize] as usize;
            let left_axis = if rows {
                left_cell / SIDE
            } else {
                left_cell % SIDE
            };
            let right_axis = if rows {
                right_cell / SIDE
            } else {
                right_cell % SIDE
            };
            if morph[left_axis].abs_diff(morph[right_axis]) <= 1 {
                mask.insert(pair);
            }
        }
        mask = mask.intersection(relevant);
        classes
            .entry(mask)
            .and_modify(|class| class.multiplicity += 1)
            .or_insert(AxisClass {
                mask,
                representative: morph_index as u16,
                multiplicity: 1,
            });
    }
    debug_assert_eq!(
        classes
            .values()
            .map(|class| usize::from(class.multiplicity))
            .sum::<usize>(),
        morphs.len()
    );
    classes.into_values().collect()
}

fn maximal_axis_classes(mut classes: Vec<AxisClass>) -> Vec<AxisClass> {
    classes.sort_by_key(|class| (Reverse(class.mask.count()), class.mask));
    let mut maximal: Vec<AxisClass> = Vec::new();
    let mut masks = Vec::new();
    let mut postings = vec![Vec::<usize>::new(); PAIRS];
    for class in classes {
        if has_indexed_superset(class.mask, &masks, &postings) {
            continue;
        }
        index_maximal_mask(class.mask, masks.len(), &mut postings);
        masks.push(class.mask);
        maximal.push(class);
    }
    maximal
}

fn maximal_network_antichain(mut classes: Vec<NetworkClass>) -> Vec<NetworkClass> {
    classes.sort_by_key(|class| (Reverse(class.mask.count()), class.mask));
    let mut maximal: Vec<NetworkClass> = Vec::new();
    let mut masks = Vec::new();
    let mut postings = vec![Vec::<usize>::new(); PAIRS];
    for class in classes {
        if has_indexed_superset(class.mask, &masks, &postings) {
            continue;
        }
        index_maximal_mask(class.mask, masks.len(), &mut postings);
        masks.push(class.mask);
        maximal.push(class);
    }
    maximal
}

fn has_indexed_superset(mask: PairMask, maximal: &[PairMask], postings: &[Vec<usize>]) -> bool {
    if maximal.is_empty() {
        return false;
    }
    if mask.is_empty() {
        return true;
    }
    let mut shortest: Option<&[usize]> = None;
    for (pair, candidates) in postings.iter().enumerate() {
        if !mask.contains(pair) {
            continue;
        }
        let candidates = candidates.as_slice();
        if shortest.is_none_or(|current| candidates.len() < current.len()) {
            shortest = Some(candidates);
        }
    }
    shortest
        .expect("nonempty mask has an indexed bit")
        .iter()
        .any(|&candidate| mask.is_subset_of(maximal[candidate]))
}

fn index_maximal_mask(mask: PairMask, index: usize, postings: &mut [Vec<usize>]) {
    for (pair, posting) in postings.iter_mut().enumerate() {
        if mask.contains(pair) {
            posting.push(index);
        }
    }
}

fn spatial_adjacency_mask(
    puzzle: &Puzzle,
    universe: &PairUniverse,
    rows: &[u8; SIDE],
    columns: &[u8; SIDE],
) -> PairMask {
    let mut result = PairMask::default();
    for (pair, &(left, right)) in universe.pairs.iter().enumerate() {
        let left_cell = puzzle.cells[left as usize];
        let right_cell = puzzle.cells[right as usize];
        let row_close =
            rows[(left_cell / 9) as usize].abs_diff(rows[(right_cell / 9) as usize]) <= 1;
        let column_close =
            columns[(left_cell % 9) as usize].abs_diff(columns[(right_cell % 9) as usize]) <= 1;
        if row_close && column_close {
            result.insert(pair);
        }
    }
    result
}

fn all_vertices_incident(mask: PairMask, universe: &PairUniverse) -> bool {
    if mask.is_empty() {
        return false;
    }
    let mut incident = 0u32;
    for (pair, &(left, right)) in universe.pairs.iter().enumerate() {
        if mask.contains(pair) {
            incident |= 1u32 << left;
            incident |= 1u32 << right;
        }
    }
    incident == (1u32 << CLUES) - 1
}

fn symbol_graph(mask: PairMask, puzzle: &Puzzle, universe: &PairUniverse) -> [u16; SIDE] {
    let mut graph = [0u16; SIDE];
    for (pair, &(left, right)) in universe.pairs.iter().enumerate() {
        if !mask.contains(pair) {
            continue;
        }
        let left_digit = puzzle.digits[left as usize] as usize;
        let right_digit = puzzle.digits[right as usize] as usize;
        debug_assert_ne!(left_digit, right_digit);
        graph[left_digit] |= 1u16 << right_digit;
        graph[right_digit] |= 1u16 << left_digit;
    }
    graph
}

fn is_hamiltonian_order(order: &[u8; SIDE], graph: &[u16; SIDE]) -> bool {
    order
        .windows(2)
        .all(|pair| graph[pair[0] as usize] & (1u16 << pair[1]) != 0)
}

fn hamiltonian_order_cases(
    graph: &[u16; SIDE],
    mask: PairMask,
    puzzle: &Puzzle,
    universe: &PairUniverse,
) -> (Vec<OrderCase>, u64) {
    if graph.contains(&0) {
        return (Vec::new(), 0);
    }
    let mut order = [u8::MAX; SIDE];
    let mut by_orientation = BTreeMap::<u64, OrderCase>::new();
    let mut hamiltonian_orders = 0u64;
    for start in 0..SIDE as u8 {
        order[0] = start;
        enumerate_hamiltonian_orders(
            graph,
            mask,
            puzzle,
            universe,
            &mut order,
            1,
            1u16 << start,
            &mut hamiltonian_orders,
            &mut by_orientation,
        );
    }
    (by_orientation.into_values().collect(), hamiltonian_orders)
}

#[allow(clippy::too_many_arguments)]
fn enumerate_hamiltonian_orders(
    graph: &[u16; SIDE],
    mask: PairMask,
    puzzle: &Puzzle,
    universe: &PairUniverse,
    order: &mut [u8; SIDE],
    depth: usize,
    visited: u16,
    hamiltonian_orders: &mut u64,
    by_orientation: &mut BTreeMap<u64, OrderCase>,
) {
    if depth == SIDE {
        if order[0] > order[SIDE - 1] {
            return;
        }
        *hamiltonian_orders += 1;
        let case = make_order_case(*order, mask, puzzle, universe);
        by_orientation.entry(case.orientation_key).or_insert(case);
        return;
    }
    let previous = order[depth - 1] as usize;
    let mut choices = graph[previous] & !visited;
    while choices != 0 {
        let next = choices.trailing_zeros() as u8;
        choices &= choices - 1;
        order[depth] = next;
        enumerate_hamiltonian_orders(
            graph,
            mask,
            puzzle,
            universe,
            order,
            depth + 1,
            visited | (1u16 << next),
            hamiltonian_orders,
            by_orientation,
        );
    }
}

fn make_order_case(
    order: [u8; SIDE],
    mask: PairMask,
    puzzle: &Puzzle,
    universe: &PairUniverse,
) -> OrderCase {
    let mut rank = [u8::MAX; SIDE];
    for (position, &digit) in order.iter().enumerate() {
        rank[digit as usize] = position as u8;
    }
    let mut orientation_key = 0u64;
    for (pair, &(left, right)) in universe.pairs.iter().enumerate() {
        if !mask.contains(pair) {
            continue;
        }
        let left_digit = puzzle.digits[left as usize] as usize;
        let right_digit = puzzle.digits[right as usize] as usize;
        let (small, large) = if left_digit < right_digit {
            (left_digit, right_digit)
        } else {
            (right_digit, left_digit)
        };
        if rank[small] < rank[large] {
            orientation_key |= 1u64 << digit_pair_index(small, large);
        }
    }
    let strength = orientation_strength(mask, puzzle, universe, &rank);
    OrderCase {
        order,
        rank,
        orientation_key,
        strength,
    }
}

fn digit_pair_index(left: usize, right: usize) -> usize {
    debug_assert!(left < right && right < SIDE);
    let mut index = 0usize;
    for first in 0..left {
        index += SIDE - first - 1;
    }
    index + right - left - 1
}

fn orientation_strength(
    mask: PairMask,
    puzzle: &Puzzle,
    universe: &PairUniverse,
    rank: &[u8; SIDE],
) -> u32 {
    let mut incoming = [0u8; CLUES];
    let mut outgoing = [0u8; CLUES];
    let mut vertices: [usize; CLUES] = std::array::from_fn(|index| index);
    vertices.sort_by_key(|&vertex| rank[puzzle.digits[vertex] as usize]);
    for &vertex in &vertices {
        for other in 0..CLUES {
            if other == vertex {
                continue;
            }
            let pair = universe.pair_index(vertex, other);
            if mask.contains(pair)
                && rank[puzzle.digits[vertex] as usize] < rank[puzzle.digits[other] as usize]
            {
                incoming[other] = incoming[other].max(incoming[vertex] + 1);
            }
        }
    }
    for &vertex in vertices.iter().rev() {
        for other in 0..CLUES {
            if other == vertex {
                continue;
            }
            let pair = universe.pair_index(vertex, other);
            if mask.contains(pair)
                && rank[puzzle.digits[other] as usize] < rank[puzzle.digits[vertex] as usize]
            {
                outgoing[other] = outgoing[other].max(outgoing[vertex] + 1);
            }
        }
    }
    let bound_strength = (0..CLUES)
        .map(|vertex| {
            let tightened = u32::from(incoming[vertex] + outgoing[vertex]);
            tightened * tightened
        })
        .sum::<u32>();
    let longest = incoming.iter().copied().max().unwrap_or(0);
    1_000 * u32::from(longest) + bound_strength
}

fn candidate_transitive_closure(
    mask: PairMask,
    puzzle: &Puzzle,
    universe: &PairUniverse,
    rank: &[u8; SIDE],
) -> ClosureMask {
    let mut reach = [0u32; CLUES];
    for (pair, &(left, right)) in universe.pairs.iter().enumerate() {
        if !mask.contains(pair) {
            continue;
        }
        let left_rank = rank[puzzle.digits[left as usize] as usize];
        let right_rank = rank[puzzle.digits[right as usize] as usize];
        if left_rank < right_rank {
            reach[left as usize] |= 1u32 << right;
        } else {
            reach[right as usize] |= 1u32 << left;
        }
    }
    for middle in 0..CLUES {
        let suffix = reach[middle];
        for targets in &mut reach {
            if *targets & (1u32 << middle) != 0 {
                *targets |= suffix;
            }
        }
    }
    let mut closure = ClosureMask::default();
    for (source, targets) in reach.iter().copied().enumerate() {
        debug_assert_eq!(targets & (1u32 << source), 0);
        let mut remaining = targets;
        while remaining != 0 {
            let target = remaining.trailing_zeros() as usize;
            remaining &= remaining - 1;
            closure.insert(source, target);
        }
    }
    closure
}

fn maximal_closure_candidates(candidates: Vec<Candidate>) -> (Vec<Candidate>, usize) {
    let mut by_closure = BTreeMap::<ClosureMask, Candidate>::new();
    for candidate in candidates {
        by_closure
            .entry(candidate.closure)
            .and_modify(|existing| {
                let candidate_priority = (
                    candidate.network.mask.count(),
                    candidate.order.strength,
                    Reverse(candidate.network_index),
                    Reverse(candidate.order_index),
                );
                let existing_priority = (
                    existing.network.mask.count(),
                    existing.order.strength,
                    Reverse(existing.network_index),
                    Reverse(existing.order_index),
                );
                if candidate_priority > existing_priority {
                    *existing = candidate.clone();
                }
            })
            .or_insert(candidate);
    }
    let unique = by_closure.len();
    let mut ordered = by_closure.into_values().collect::<Vec<_>>();
    ordered.sort_by_key(|candidate| (Reverse(candidate.closure.count()), candidate.closure));
    let mut maximal = Vec::new();
    let mut masks = Vec::new();
    let mut postings = vec![Vec::<usize>::new(); RELATIONS];
    for candidate in ordered {
        if has_indexed_closure_superset(candidate.closure, &masks, &postings) {
            continue;
        }
        index_maximal_closure(candidate.closure, masks.len(), &mut postings);
        masks.push(candidate.closure);
        maximal.push(candidate);
    }
    (maximal, unique)
}

fn has_indexed_closure_superset(
    closure: ClosureMask,
    maximal: &[ClosureMask],
    postings: &[Vec<usize>],
) -> bool {
    if maximal.is_empty() {
        return false;
    }
    if closure.count() == 0 {
        return true;
    }
    let mut shortest: Option<&[usize]> = None;
    for (relation, candidates) in postings.iter().enumerate() {
        if !closure.contains_relation(relation) {
            continue;
        }
        if shortest.is_none_or(|current| candidates.len() < current.len()) {
            shortest = Some(candidates);
        }
    }
    shortest
        .expect("nonempty closure has an indexed relation")
        .iter()
        .any(|&candidate| closure.is_subset_of(maximal[candidate]))
}

fn index_maximal_closure(closure: ClosureMask, index: usize, postings: &mut [Vec<usize>]) {
    for (relation, posting) in postings.iter_mut().enumerate() {
        if closure.contains_relation(relation) {
            posting.push(index);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn realize_comparisons(
    mask: PairMask,
    puzzle: &Puzzle,
    universe: &PairUniverse,
    order: &OrderCase,
    morphs: Option<&[[u8; SIDE]]>,
    row_morph: usize,
    column_morph: usize,
) -> Vec<(u8, u8)> {
    let identity: [u8; SIDE] = std::array::from_fn(|index| index as u8);
    let rows = morphs.map_or(&identity, |all| &all[row_morph]);
    let columns = morphs.map_or(&identity, |all| &all[column_morph]);
    let mut comparisons = Vec::with_capacity(mask.count() as usize);
    for (pair, &(left, right)) in universe.pairs.iter().enumerate() {
        if !mask.contains(pair) {
            continue;
        }
        let left_digit = puzzle.digits[left as usize] as usize;
        let right_digit = puzzle.digits[right as usize] as usize;
        let left_cell = morph_cell(puzzle.cells[left as usize], rows, columns);
        let right_cell = morph_cell(puzzle.cells[right as usize], rows, columns);
        if order.rank[left_digit] < order.rank[right_digit] {
            comparisons.push((left_cell, right_cell));
        } else {
            comparisons.push((right_cell, left_cell));
        }
    }
    comparisons.sort_unstable();
    comparisons.dedup();
    debug_assert_eq!(comparisons.len(), mask.count() as usize);
    comparisons
}

fn morph_cell(cell: u8, rows: &[u8; SIDE], columns: &[u8; SIDE]) -> u8 {
    9 * rows[(cell / 9) as usize] + columns[(cell % 9) as usize]
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
    assert_eq!(result.len(), MORPH_COUNT);
    result
}

fn parse_puzzles(bytes: &[u8]) -> Result<Vec<Puzzle>, String> {
    let mut puzzles = Vec::new();
    for (index, line) in BufReader::new(Cursor::new(bytes)).lines().enumerate() {
        let line_number = index + 1;
        let encoded = line.map_err(|error| format!("cannot read line {line_number}: {error}"))?;
        puzzles.push(Puzzle::parse(&encoded, line_number)?);
    }
    if puzzles.is_empty() {
        return Err("input contains no puzzle records".to_owned());
    }
    Ok(puzzles)
}

fn validate_distinct_paths(options: &Options) -> Result<(), String> {
    let input = resolved_path(&options.input)?;
    let output = options
        .output
        .as_ref()
        .map(|path| resolved_path(path))
        .transpose()?;
    let checkpoint = options
        .checkpoint
        .as_ref()
        .map(|path| resolved_path(path))
        .transpose()?;
    if output.as_ref().is_some_and(|path| same_path(&input, path)) {
        return Err("--output must not alias --input".to_owned());
    }
    if checkpoint
        .as_ref()
        .is_some_and(|path| same_path(&input, path))
    {
        return Err("--checkpoint must not alias --input".to_owned());
    }
    if let (Some(output), Some(checkpoint)) = (&output, &checkpoint)
        && same_path(output, checkpoint)
    {
        return Err("--output must not alias --checkpoint".to_owned());
    }
    if let Some(checkpoint_path) = options.checkpoint.as_ref() {
        let previous = resolved_path(&checkpoint_sibling(checkpoint_path, "prev"))?;
        if same_path(&input, &previous) {
            return Err("checkpoint backup must not alias --input".to_owned());
        }
        if output
            .as_ref()
            .is_some_and(|output| same_path(output, &previous))
        {
            return Err("checkpoint backup must not alias --output".to_owned());
        }
        if checkpoint
            .as_ref()
            .is_some_and(|checkpoint| same_path(checkpoint, &previous))
        {
            return Err("checkpoint backup must not alias --checkpoint".to_owned());
        }
    }
    Ok(())
}

fn resolved_path(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return fs::canonicalize(path)
            .map_err(|error| format!("cannot resolve {}: {error}", path.display()));
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("{} has no file name", path.display()))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let resolved_parent = fs::canonicalize(parent.unwrap_or_else(|| Path::new(".")))
        .map_err(|error| format!("cannot resolve parent of {}: {error}", path.display()))?;
    Ok(resolved_parent.join(file_name))
}

fn same_path(left: &Path, right: &Path) -> bool {
    let names_match = if cfg!(windows) {
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    } else {
        left == right
    };
    names_match
        || file_identity(left)
            .zip(file_identity(right))
            .is_some_and(|(left, right)| left == right)
}

#[cfg(unix)]
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::metadata(path).ok()?;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;

    let file = File::open(path).ok()?;
    let mut information = MaybeUninit::<WindowsFileInformation>::uninit();
    // SAFETY: `file` keeps the valid handle alive for this call and
    // `information` points to writable storage of the exact Win32 layout.
    let succeeded =
        unsafe { get_file_information_by_handle(file.as_raw_handle(), information.as_mut_ptr()) };
    if succeeded == 0 {
        return None;
    }
    // SAFETY: a nonzero return guarantees that Win32 initialized the struct.
    let information = unsafe { information.assume_init() };
    Some((
        u64::from(information.volume_serial_number),
        (u64::from(information.file_index_high) << 32) | u64::from(information.file_index_low),
    ))
}

#[cfg(windows)]
#[repr(C)]
struct WindowsFileTime {
    low_date_time: u32,
    high_date_time: u32,
}

#[cfg(windows)]
#[repr(C)]
struct WindowsFileInformation {
    file_attributes: u32,
    creation_time: WindowsFileTime,
    last_access_time: WindowsFileTime,
    last_write_time: WindowsFileTime,
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "GetFileInformationByHandle"]
    fn get_file_information_by_handle(
        file: *mut std::ffi::c_void,
        information: *mut WindowsFileInformation,
    ) -> i32;
}

#[cfg(not(any(unix, windows)))]
fn file_identity(_path: &Path) -> Option<(u64, u64)> {
    None
}

fn parse_options() -> Result<Options, String> {
    let mut input = None;
    let mut output = None;
    let mut mode = Mode::Identity;
    let mut start_line = 1usize;
    let mut end_line = usize::MAX;
    let mut max_units = None;
    let mut solution_cap = 2u64;
    let mut guided_graphs = 64usize;
    let mut guided_orders = 256usize;
    let mut emit_cases = false;
    let mut stop_on_first = false;
    let mut progress_every = 1_000u64;
    let mut checkpoint = None;
    let mut resume = false;
    let mut checkpoint_every = 10_000u64;
    let mut expected_records = None;
    let mut expected_fnv64 = None;
    let mut corpus_is_complete = false;

    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--input" => input = Some(PathBuf::from(next_value(&mut arguments, "--input")?)),
            "--output" => output = Some(PathBuf::from(next_value(&mut arguments, "--output")?)),
            "--mode" => mode = Mode::parse(&next_value(&mut arguments, "--mode")?)?,
            "--start-line" => {
                start_line =
                    parse_usize(next_value(&mut arguments, "--start-line")?, "--start-line")?
            }
            "--end-line" => {
                end_line = parse_usize(next_value(&mut arguments, "--end-line")?, "--end-line")?
            }
            "--max-units" => {
                max_units = Some(parse_u64(
                    next_value(&mut arguments, "--max-units")?,
                    "--max-units",
                )?)
            }
            "--solution-cap" => {
                solution_cap = parse_u64(
                    next_value(&mut arguments, "--solution-cap")?,
                    "--solution-cap",
                )?
            }
            "--guided-graphs" => {
                guided_graphs = parse_usize(
                    next_value(&mut arguments, "--guided-graphs")?,
                    "--guided-graphs",
                )?
            }
            "--guided-orders" => {
                guided_orders = parse_usize(
                    next_value(&mut arguments, "--guided-orders")?,
                    "--guided-orders",
                )?
            }
            "--emit-cases" => emit_cases = true,
            "--stop-on-first" => stop_on_first = true,
            "--progress-every" => {
                progress_every = parse_u64(
                    next_value(&mut arguments, "--progress-every")?,
                    "--progress-every",
                )?
            }
            "--checkpoint" => {
                checkpoint = Some(PathBuf::from(next_value(&mut arguments, "--checkpoint")?))
            }
            "--resume" => resume = true,
            "--checkpoint-every" => {
                checkpoint_every = parse_u64(
                    next_value(&mut arguments, "--checkpoint-every")?,
                    "--checkpoint-every",
                )?
            }
            "--expected-records" => {
                expected_records = Some(parse_usize(
                    next_value(&mut arguments, "--expected-records")?,
                    "--expected-records",
                )?)
            }
            "--expected-fnv64" => {
                expected_fnv64 = Some(parse_hex_u64(
                    next_value(&mut arguments, "--expected-fnv64")?,
                    "--expected-fnv64",
                )?)
            }
            "--corpus-is-complete" => corpus_is_complete = true,
            "--help" | "-h" => {
                println!(
                    "Usage: thermo-17c-overlap --input FILE [--output JSONL] [--mode identity|exact|guided] [--start-line N] [--end-line N] [--max-units N] [--solution-cap N] [--guided-graphs N] [--guided-orders N] [--emit-cases] [--stop-on-first] [--progress-every N] [--checkpoint FILE] [--checkpoint-every N] [--resume] [--expected-records N] [--expected-fnv64 HEX] [--corpus-is-complete]"
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
    if max_units == Some(0) {
        return Err("--max-units must be positive".to_owned());
    }
    if solution_cap < 2 {
        return Err("--solution-cap must be at least 2".to_owned());
    }
    if guided_graphs == 0 || guided_orders == 0 {
        return Err("guided graph/order limits must be positive".to_owned());
    }
    if checkpoint_every == 0 {
        return Err("--checkpoint-every must be positive".to_owned());
    }
    if resume && checkpoint.is_none() {
        return Err("--resume requires --checkpoint FILE".to_owned());
    }
    if corpus_is_complete && (expected_records.is_none() || expected_fnv64.is_none()) {
        return Err(
            "--corpus-is-complete requires both --expected-records and --expected-fnv64".to_owned(),
        );
    }
    Ok(Options {
        input,
        output,
        mode,
        start_line,
        end_line,
        max_units,
        solution_cap,
        guided_graphs,
        guided_orders,
        emit_cases,
        stop_on_first,
        progress_every,
        checkpoint,
        resume,
        checkpoint_every,
        expected_records,
        expected_fnv64,
        corpus_is_complete,
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

fn parse_u64(value: String, option: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("invalid integer for {option}: {value}"))
}

fn parse_hex_u64(value: String, option: &str) -> Result<u64, String> {
    let digits = value.strip_prefix("0x").unwrap_or(&value);
    u64::from_str_radix(digits, 16)
        .map_err(|_| format!("invalid hexadecimal for {option}: {value}"))
}

fn open_output(
    options: &Options,
    checkpoint: Option<&Checkpoint>,
) -> Result<Option<Output>, String> {
    if options.resume
        && options.output.is_none()
        && checkpoint.is_some_and(|state| state.output_bytes.is_some())
    {
        return Err("checkpoint is bound to an output file, but --output was omitted".to_owned());
    }
    options
        .output
        .as_ref()
        .map(|path| {
            let mut open = OpenOptions::new();
            open.read(true).write(true);
            if options.resume {
                let expected = checkpoint
                    .and_then(|state| state.output_bytes)
                    .ok_or_else(|| {
                        "checkpoint has no output byte offset for requested --output".to_owned()
                    })?;
                let expected_fnv64 = checkpoint
                    .and_then(|state| state.output_fnv64)
                    .ok_or_else(|| {
                        "checkpoint has no output prefix hash for requested --output".to_owned()
                    })?;
                let mut file = open
                    .open(path)
                    .map_err(|error| format!("cannot open output {}: {error}", path.display()))?;
                let observed = file
                    .metadata()
                    .map_err(|error| format!("cannot inspect output {}: {error}", path.display()))?
                    .len();
                if observed < expected {
                    return Err(format!(
                        "output {} is shorter than checkpoint offset: {observed} < {expected}",
                        path.display()
                    ));
                }
                file.seek(SeekFrom::Start(0)).map_err(|error| {
                    format!("cannot seek output {} for validation: {error}", path.display())
                })?;
                let observed_fnv64 = fnv1a64_reader(&mut file, expected).map_err(|error| {
                    format!("cannot hash output prefix {}: {error}", path.display())
                })?;
                if observed_fnv64 != expected_fnv64 {
                    return Err(format!(
                        "output {} prefix hash mismatch at {expected} bytes: expected {expected_fnv64:016x}, got {observed_fnv64:016x}",
                        path.display()
                    ));
                }
                file.set_len(expected).map_err(|error| {
                    format!(
                        "cannot truncate output {} to checkpoint offset {expected}: {error}",
                        path.display()
                    )
                })?;
                file.seek(SeekFrom::Start(expected)).map_err(|error| {
                    format!(
                        "cannot seek output {} to checkpoint offset {expected}: {error}",
                        path.display()
                    )
                })?;
                return Ok(Output {
                    writer: BufWriter::new(file),
                    bytes: expected,
                    fnv64: expected_fnv64,
                });
            } else {
                open.create(true).truncate(true);
            }
            open.open(path)
                .map(|file| Output {
                    writer: BufWriter::new(file),
                    bytes: 0,
                    fnv64: FNV_OFFSET,
                })
                .map_err(|error| format!("cannot open output {}: {error}", path.display()))
        })
        .transpose()
}

fn write_jsonl(output: Option<&mut Output>, record: &str) -> Result<(), String> {
    if let Some(output) = output {
        output
            .writer
            .write_all(record.as_bytes())
            .and_then(|()| output.writer.write_all(b"\n"))
            .map_err(|error| format!("cannot write output: {error}"))?;
        update_fnv1a64(&mut output.fnv64, record.as_bytes());
        update_fnv1a64(&mut output.fnv64, b"\n");
        output.bytes = output
            .bytes
            .saturating_add(record.len() as u64)
            .saturating_add(1);
    }
    Ok(())
}

fn flush_output(output: Option<&mut Output>) -> Result<(), String> {
    if let Some(output) = output {
        output
            .writer
            .flush()
            .map_err(|error| format!("cannot flush output: {error}"))?;
        let observed = output
            .writer
            .get_ref()
            .metadata()
            .map_err(|error| format!("cannot inspect flushed output: {error}"))?
            .len();
        if observed != output.bytes {
            return Err(format!(
                "flushed output length mismatch: tracked {}, observed {observed}",
                output.bytes
            ));
        }
    }
    Ok(())
}

fn output_checkpoint(output: Option<&Output>) -> (Option<u64>, Option<u64>) {
    output.map_or((None, None), |output| {
        (Some(output.bytes), Some(output.fnv64))
    })
}

fn header_json(
    options: &Options,
    input_fnv64: u64,
    input_bytes: usize,
    records: usize,
    effective_end: usize,
    fingerprint: u64,
) -> String {
    let search_scope = match options.mode {
        Mode::Identity => "identity row/column morph and natural digit order only",
        Mode::Exact => {
            "all distinct row/column morph adjacency masks and all Hamiltonian digit-order orientations"
        }
        Mode::Guided => {
            "dense-first bounded morph adjacency masks and strong-first Hamiltonian orientations; never a completeness proof"
        }
    };
    let mask_reduction = match options.mode {
        Mode::Identity => "not applicable to the fixed identity slice",
        Mode::Exact => {
            "only inclusion-maximal unequal-pair axis masks, physical networks, and final 17-vertex transitive-closure posets are retained; every omitted case is dominated by a realizable stronger case"
        }
        Mode::Guided => {
            "axis masks use the exact inclusion-maximal reduction; the network cross-product retains only the bounded densest masks, followed by poset dominance, and is intentionally incomplete"
        }
    };
    format!(
        "{{\"type\":\"header\",\"schema\":\"{SCHEMA}\",\"algorithm_revision\":\"{ALGORITHM_REVISION}\",\"fingerprint\":\"{fingerprint:016x}\",\"input_name\":\"{}\",\"input_bytes\":{input_bytes},\"input_fnv1a64\":\"{input_fnv64:016x}\",\"records\":{records},\"expected_records\":{},\"expected_fnv1a64\":{},\"corpus_is_complete_assertion\":{},\"mode\":\"{}\",\"search_scope\":\"{}\",\"start_line\":{},\"end_line\":{},\"max_units\":{},\"solution_cap\":{},\"guided_graphs_per_record\":{},\"guided_orientations_per_graph\":{},\"emit_cases\":{},\"checkpoint_every\":{},\"axis_morphs_per_dimension\":{MORPH_COUNT},\"transposition\":\"covered by exchanging row and column axis domains\",\"digit_order_reversal\":\"one direction retained; the other is equivalent by global digit complement\",\"saturation_reduction\":\"all target-true local comparisons are included because adding them preserves a target and cannot destroy uniqueness\",\"maximal_mask_reduction\":\"{}\",\"coverage_rule\":\"all 17 clue cells must be incident\",\"order_pruning\":\"orders without every consecutive symbol pair adjacent are multiple by a consecutive-digit swap\",\"thermo_scope\":\"arbitrary overlapping and branching two-cell king-neighbour inequalities on exactly the 17 catalogue clue cells\",\"input_assumption\":\"records are solvable 17-clue classic Sudoku representatives; global conclusions additionally require a complete catalogue\",\"concurrency_rule\":\"one writer per output/checkpoint pair; use distinct paths for parallel shards\",\"checkpoint_durability\":\"process-crash recovery uses flushed byte length plus prefix hash and same-directory checkpoint replacement with a validated backup fallback; sudden power-loss durability depends on the filesystem\"}}",
        json_escape(
            options
                .input
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<non-UTF8-input-name>"),
        ),
        option_usize_json(options.expected_records),
        option_hex_json(options.expected_fnv64),
        options.corpus_is_complete,
        options.mode.as_str(),
        json_escape(search_scope),
        options.start_line,
        effective_end,
        option_u64_json(options.max_units),
        options.solution_cap,
        options.guided_graphs,
        options.guided_orders,
        options.emit_cases,
        options.checkpoint_every,
        json_escape(mask_reduction),
    )
}

#[allow(clippy::too_many_arguments)]
fn case_json(
    unit: u64,
    line_number: usize,
    network_index: usize,
    order_index: usize,
    puzzle: &Puzzle,
    network: &NetworkClass,
    order: &OrderCase,
    comparisons: &[(u8, u8)],
    result: &SolveResult,
) -> Result<String, String> {
    let multiplicity = match result.count {
        0 => "zero",
        1 if !result.capped => "unique",
        _ => "multiple",
    };
    let solution = if result.count == 1 && !result.capped {
        let grid = result
            .first_solution
            .as_ref()
            .ok_or_else(|| format!("unit {unit}: unique result has no solution witness"))?;
        format!("\"{}\"", solution_string(grid))
    } else {
        "null".to_owned()
    };
    format!(
        "{{\"type\":\"case\",\"schema\":\"{SCHEMA}\",\"unit\":{unit},\"line\":{line_number},\"source\":\"{}\",\"network_index\":{network_index},\"order_index\":{order_index},\"usable_adjacency_mask\":\"{}\",\"row_morph\":{},\"column_morph\":{},\"represented_morph_pairs\":{},\"digit_order_low_to_high\":{},\"orientation_key\":\"{:09x}\",\"guidance_strength\":{},\"comparison_count\":{},\"comparisons\":{},\"solution_count\":{},\"count_capped\":{},\"multiplicity\":\"{multiplicity}\",\"solution_if_unique\":{solution},\"solver_nodes\":{},\"solver_branches\":{},\"solver_propagation_rounds\":{},\"solver_comparison_revisions\":{},\"solver_max_depth\":{}}}",
        puzzle.encoded,
        network.mask.hex(),
        network.row_morph,
        network.column_morph,
        network.morph_pair_multiplicity,
        order_json(&order.order),
        order.orientation_key,
        order.strength,
        comparisons.len(),
        comparisons_json(comparisons),
        result.count,
        result.capped,
        result.stats.nodes,
        result.stats.branches,
        result.stats.propagation_rounds,
        result.stats.thermo_revisions,
        result.stats.max_depth,
    )
    .pipe(Ok)
}

trait Pipe: Sized {
    fn pipe<T>(self, function: impl FnOnce(Self) -> T) -> T {
        function(self)
    }
}

impl<T> Pipe for T {}

#[allow(clippy::too_many_arguments)]
fn summary_json(
    options: &Options,
    stats: &SearchStats,
    input_fnv64: u64,
    fingerprint: u64,
    resume_unit: u64,
    next_unit: u64,
    processed_this_invocation: u64,
    scope_exhausted: bool,
    catalogue_range_complete: bool,
    identity_slice_complete: bool,
    generalized_complete: bool,
    limit_hit: bool,
    stopped_on_unique: bool,
    elapsed_seconds: f64,
) -> String {
    let exact_requested_range_complete = options.mode == Mode::Exact && scope_exhausted;
    let guided = options.mode == Mode::Guided;
    format!(
        "{{\"type\":\"summary\",\"schema\":\"{SCHEMA}\",\"algorithm_revision\":\"{ALGORITHM_REVISION}\",\"fingerprint\":\"{fingerprint:016x}\",\"input_fnv1a64\":\"{input_fnv64:016x}\",\"mode\":\"{}\",\"resume_unit\":{resume_unit},\"next_unit\":{next_unit},\"processed_this_invocation\":{processed_this_invocation},\"scope_exhausted\":{scope_exhausted},\"input_range_exhausted\":{catalogue_range_complete},\"exact_requested_range_complete\":{exact_requested_range_complete},\"identity_slice_complete_for_supplied_input\":{identity_slice_complete},\"guided_incomplete_by_definition\":{guided},\"generalized_complete\":{generalized_complete},\"corpus_is_complete_assertion\":{},\"limit_hit\":{limit_hit},\"stopped_on_unique\":{stopped_on_unique},\"records_in_range\":{},\"records_missing_digits\":{},\"row_axis_classes_raw\":{},\"row_axis_classes_maximal\":{},\"column_axis_classes_raw\":{},\"column_axis_classes_maximal\":{},\"network_intersections_scanned\":{},\"network_classes_raw\":{},\"network_classes_raw_available\":{},\"network_classes_retained\":{},\"coordinate_morph_pairs_in_scope\":{},\"retained_exact_mask_morph_pairs\":{},\"guided_nonrepresentative_intersections\":{},\"guided_omitted_orientations\":{},\"coverage_pruned_classes\":{},\"no_hamiltonian_classes\":{},\"canonical_digit_orders\":{},\"hamiltonian_orders\":{},\"structurally_pruned_orders\":{},\"duplicate_orientation_orders\":{},\"raw_candidate_orientations\":{},\"unique_poset_closures\":{},\"duplicate_poset_closures\":{},\"maximal_poset_closures\":{},\"dominated_poset_closures\":{},\"candidate_units\":{},\"classified_units\":{},\"zero\":{},\"unique\":{},\"multiple\":{},\"exact_counts\":{},\"capped_counts\":{},\"observed_solution_count_sum\":{},\"best_count\":{},\"best_count_capped\":{},\"best_unit\":{},\"best_line\":{},\"solver_nodes\":{},\"solver_branches\":{},\"solver_propagation_rounds\":{},\"solver_comparison_revisions\":{},\"elapsed_seconds\":{elapsed_seconds:.6}}}",
        options.mode.as_str(),
        options.corpus_is_complete,
        stats.records_in_range,
        stats.records_missing_digits,
        stats.row_axis_classes,
        stats.row_axis_maximal_classes,
        stats.column_axis_classes,
        stats.column_axis_maximal_classes,
        stats.network_intersections_scanned,
        stats.network_classes,
        options.mode != Mode::Guided,
        stats.retained_network_classes,
        stats.represented_morph_pairs,
        stats.retained_mask_morph_pairs,
        stats.guided_nonrepresentative_intersections,
        stats.guided_omitted_orientations,
        stats.coverage_pruned_classes,
        stats.no_hamiltonian_classes,
        stats.canonical_orders,
        stats.hamiltonian_orders,
        stats.structurally_pruned_orders,
        stats.duplicate_orientation_orders,
        stats.raw_candidate_orientations,
        stats.unique_poset_closures,
        stats.duplicate_poset_closures,
        stats.maximal_poset_closures,
        stats.dominated_poset_closures,
        stats.candidate_units,
        stats.classified_units,
        stats.zero,
        stats.unique,
        stats.multiple,
        stats.exact_counts,
        stats.capped_counts,
        stats.observed_solution_sum,
        option_u64_json(stats.best_count),
        stats.best_capped,
        option_u64_json(stats.best_unit),
        option_usize_json(stats.best_line),
        stats.solver_nodes,
        stats.solver_branches,
        stats.solver_propagation_rounds,
        stats.solver_comparison_revisions,
    )
}

fn run_fingerprint(options: &Options, input_fnv64: u64, records: usize) -> u64 {
    let material = format!(
        "{SCHEMA}|{ALGORITHM_REVISION}|{input_fnv64:016x}|{records}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        options.mode.as_str(),
        options.start_line,
        options.end_line,
        options.solution_cap,
        options.guided_graphs,
        options.guided_orders,
        options.corpus_is_complete,
        options.emit_cases,
        options.stop_on_first,
        options
            .output
            .as_ref()
            .map_or_else(|| "<none>".to_owned(), |path| path.display().to_string()),
    );
    fnv1a64(material.as_bytes())
}

fn stop_on_first_reached(options: &Options, stats: &SearchStats) -> bool {
    options.stop_on_first && stats.unique != 0
}

fn write_checkpoint(
    path: &Path,
    fingerprint: u64,
    next_unit: u64,
    output_bytes: Option<u64>,
    output_fnv64: Option<u64>,
    stats: &SearchStats,
) -> Result<(), String> {
    if output_bytes.is_some() != output_fnv64.is_some() {
        return Err("checkpoint output bytes/hash presence mismatch".to_owned());
    }
    let record = format!(
        "{{\"schema\":\"{CHECKPOINT_SCHEMA}\",\"algorithm_revision\":\"{ALGORITHM_REVISION}\",\"fingerprint\":\"{fingerprint:016x}\",\"next_unit\":{next_unit},\"output_bytes\":{},\"output_fnv1a64\":{},\"classified_units\":{},\"zero\":{},\"unique\":{},\"multiple\":{},\"exact_counts\":{},\"capped_counts\":{},\"observed_solution_count_sum\":{},\"solver_nodes\":{},\"solver_branches\":{},\"solver_propagation_rounds\":{},\"solver_comparison_revisions\":{},\"best_count\":{},\"best_count_capped\":{},\"best_unit\":{},\"best_line\":{}}}",
        option_u64_json(output_bytes),
        option_hex_json(output_fnv64),
        stats.classified_units,
        stats.zero,
        stats.unique,
        stats.multiple,
        stats.exact_counts,
        stats.capped_counts,
        stats.observed_solution_sum,
        stats.solver_nodes,
        stats.solver_branches,
        stats.solver_propagation_rounds,
        stats.solver_comparison_revisions,
        option_u64_json(stats.best_count),
        stats.best_capped,
        option_u64_json(stats.best_unit),
        option_usize_json(stats.best_line),
    );
    let (temporary, mut file) = create_checkpoint_temporary(path)?;
    let previous = checkpoint_sibling(path, "prev");
    let prepared = writeln!(file, "{record}")
        .map_err(|error| format!("cannot write checkpoint {}: {error}", temporary.display()))
        .and_then(|()| {
            file.flush().map_err(|error| {
                format!("cannot flush checkpoint {}: {error}", temporary.display())
            })
        })
        .and_then(|()| {
            file.sync_all()
                .map_err(|error| format!("cannot sync checkpoint {}: {error}", temporary.display()))
        });
    drop(file);
    if let Err(error) = prepared {
        return match fs::remove_file(&temporary) {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(format!(
                "{error}; cannot remove owned temporary {}: {cleanup_error}",
                temporary.display()
            )),
        };
    }

    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(replace_error) if path.exists() => {
            let current_is_owned = read_checkpoint_exact(path)
                .is_ok_and(|checkpoint| checkpoint.fingerprint == fingerprint);
            if current_is_owned {
                remove_owned_checkpoint_backup(&previous, fingerprint)?;
                fs::rename(path, &previous).map_err(|error| {
                    format!(
                        "cannot preserve checkpoint {} as {} after replace failure {replace_error}: {error}",
                        path.display(),
                        previous.display()
                    )
                })?;
                if let Err(error) = fs::rename(&temporary, path) {
                    let restore = fs::rename(&previous, path);
                    return Err(format!(
                        "cannot install checkpoint {}: {error}; restore result: {restore:?}",
                        path.display()
                    ));
                }
            } else {
                validate_owned_checkpoint_backup(&previous, fingerprint)?;
                fs::remove_file(path).map_err(|error| {
                    format!(
                        "cannot remove invalid or stale checkpoint {} after replace failure {replace_error}: {error}",
                        path.display()
                    )
                })?;
                fs::rename(&temporary, path).map_err(|error| {
                    format!(
                        "cannot install checkpoint {} after removing its invalid or stale predecessor: {error}; any valid backup remains at {}",
                        path.display(),
                        previous.display()
                    )
                })?;
            }
            Ok(())
        }
        Err(error) => Err(format!(
            "cannot install checkpoint {} from {}: {error}",
            path.display(),
            temporary.display()
        )),
    }
}

fn create_checkpoint_temporary(path: &Path) -> Result<(PathBuf, File), String> {
    const ATTEMPTS: u32 = 256;

    for attempt in 0..ATTEMPTS {
        let temporary = checkpoint_sibling(path, &format!("tmp-{}-{attempt}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create checkpoint temporary {}: {error}",
                    temporary.display()
                ));
            }
        }
    }
    Err(format!(
        "cannot create a unique checkpoint temporary beside {} after {ATTEMPTS} attempts",
        path.display()
    ))
}

fn validate_owned_checkpoint_backup(path: &Path, fingerprint: u64) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "cannot inspect checkpoint backup {}: {error}",
                path.display()
            ));
        }
        Ok(_) => {}
    }
    let checkpoint = read_checkpoint_exact(path).map_err(|error| {
        format!(
            "refusing to replace checkpoint backup {} because it is not an owned checkpoint: {error}",
            path.display()
        )
    })?;
    if checkpoint.fingerprint != fingerprint {
        return Err(format!(
            "refusing to replace checkpoint backup {}: fingerprint {:016x} does not match current run {fingerprint:016x}",
            path.display(),
            checkpoint.fingerprint
        ));
    }
    Ok(true)
}

fn remove_owned_checkpoint_backup(path: &Path, fingerprint: u64) -> Result<(), String> {
    if !validate_owned_checkpoint_backup(path, fingerprint)? {
        return Ok(());
    }
    fs::remove_file(path).map_err(|error| {
        format!(
            "cannot remove owned checkpoint backup {}: {error}",
            path.display()
        )
    })
}

fn read_checkpoint(path: &Path) -> Result<Checkpoint, String> {
    match read_checkpoint_exact(path) {
        Ok(checkpoint) => Ok(checkpoint),
        Err(primary_error) => {
            let previous = checkpoint_sibling(path, "prev");
            read_checkpoint_exact(&previous).map_err(|backup_error| {
                format!(
                    "cannot load checkpoint {} ({primary_error}); backup {} also failed ({backup_error})",
                    path.display(),
                    previous.display()
                )
            })
        }
    }
}

fn read_checkpoint_exact(path: &Path) -> Result<Checkpoint, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read checkpoint {}: {error}", path.display()))?;
    if json_string_field(&text, "schema")? != CHECKPOINT_SCHEMA {
        return Err(format!(
            "{} is not a {CHECKPOINT_SCHEMA} checkpoint",
            path.display()
        ));
    }
    if json_string_field(&text, "algorithm_revision")? != ALGORITHM_REVISION {
        return Err(format!(
            "checkpoint algorithm revision does not match {ALGORITHM_REVISION}"
        ));
    }
    let classification = SearchStats {
        classified_units: json_u64_field(&text, "classified_units")?,
        zero: json_u64_field(&text, "zero")?,
        unique: json_u64_field(&text, "unique")?,
        multiple: json_u64_field(&text, "multiple")?,
        exact_counts: json_u64_field(&text, "exact_counts")?,
        capped_counts: json_u64_field(&text, "capped_counts")?,
        observed_solution_sum: json_u64_field(&text, "observed_solution_count_sum")?,
        solver_nodes: json_u64_field(&text, "solver_nodes")?,
        solver_branches: json_u64_field(&text, "solver_branches")?,
        solver_propagation_rounds: json_u64_field(&text, "solver_propagation_rounds")?,
        solver_comparison_revisions: json_u64_field(&text, "solver_comparison_revisions")?,
        best_count: json_optional_u64_field(&text, "best_count")?,
        best_capped: json_bool_field(&text, "best_count_capped")?,
        best_unit: json_optional_u64_field(&text, "best_unit")?,
        best_line: json_optional_u64_field(&text, "best_line")?
            .map(|line| usize::try_from(line).map_err(|_| "best_line exceeds usize".to_owned()))
            .transpose()?,
        ..SearchStats::default()
    };
    let next_unit = json_u64_field(&text, "next_unit")?;
    let output_bytes = json_optional_u64_field(&text, "output_bytes")?;
    let output_fnv64 = match json_raw_field(&text, "output_fnv1a64")? {
        "null" => None,
        _ => Some(parse_hex_u64(
            json_string_field(&text, "output_fnv1a64")?.to_owned(),
            "checkpoint output_fnv1a64",
        )?),
    };
    if output_bytes.is_some() != output_fnv64.is_some() {
        return Err("checkpoint output_bytes/output_fnv1a64 presence mismatch".to_owned());
    }
    if classification.classified_units != next_unit {
        return Err(format!(
            "checkpoint invariant failed: classified_units {} != next_unit {next_unit}",
            classification.classified_units
        ));
    }
    if classification
        .zero
        .saturating_add(classification.unique)
        .saturating_add(classification.multiple)
        != classification.classified_units
    {
        return Err(
            "checkpoint invariant failed: multiplicities do not sum to classified_units".to_owned(),
        );
    }
    if classification
        .exact_counts
        .saturating_add(classification.capped_counts)
        != classification.classified_units
    {
        return Err(
            "checkpoint invariant failed: exact/capped counts do not sum to classified_units"
                .to_owned(),
        );
    }
    if classification.classified_units == 0 {
        if classification.best_count.is_some()
            || classification.best_unit.is_some()
            || classification.best_line.is_some()
        {
            return Err("checkpoint invariant failed: empty prefix has a best case".to_owned());
        }
    } else if classification.best_count.is_none()
        || classification.best_line.is_none()
        || classification
            .best_unit
            .is_none_or(|unit| unit >= next_unit)
    {
        return Err(
            "checkpoint invariant failed: best case is missing or outside prefix".to_owned(),
        );
    }
    Ok(Checkpoint {
        fingerprint: parse_hex_u64(
            json_string_field(&text, "fingerprint")?.to_owned(),
            "checkpoint fingerprint",
        )?,
        next_unit,
        output_bytes,
        output_fnv64,
        classification,
    })
}

fn checkpoint_sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("checkpoint"))
        .to_os_string();
    name.push(format!(".{suffix}"));
    path.with_file_name(name)
}

fn json_raw_field<'a>(text: &'a str, key: &str) -> Result<&'a str, String> {
    let needle = format!("\"{key}\":");
    let start = text
        .find(&needle)
        .ok_or_else(|| format!("checkpoint is missing {key}"))?
        + needle.len();
    let rest = &text[start..];
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped
            .find('"')
            .ok_or_else(|| format!("checkpoint string {key} is unterminated"))?;
        Ok(&rest[..end + 2])
    } else {
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        Ok(rest[..end].trim())
    }
}

fn json_string_field<'a>(text: &'a str, key: &str) -> Result<&'a str, String> {
    let raw = json_raw_field(text, key)?;
    raw.strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(|| format!("checkpoint field {key} is not a string"))
}

fn json_u64_field(text: &str, key: &str) -> Result<u64, String> {
    let raw = json_raw_field(text, key)?;
    raw.parse()
        .map_err(|_| format!("checkpoint field {key} is not a u64: {raw}"))
}

fn json_optional_u64_field(text: &str, key: &str) -> Result<Option<u64>, String> {
    let raw = json_raw_field(text, key)?;
    if raw == "null" {
        Ok(None)
    } else {
        raw.parse()
            .map(Some)
            .map_err(|_| format!("checkpoint field {key} is not a u64 or null: {raw}"))
    }
}

fn json_bool_field(text: &str, key: &str) -> Result<bool, String> {
    match json_raw_field(text, key)? {
        "true" => Ok(true),
        "false" => Ok(false),
        raw => Err(format!("checkpoint field {key} is not Boolean: {raw}")),
    }
}

fn option_u64_json(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |number| number.to_string())
}

fn option_usize_json(value: Option<usize>) -> String {
    value.map_or_else(|| "null".to_owned(), |number| number.to_string())
}

fn option_hex_json(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |number| format!("\"{number:016x}\""))
}

fn comparisons_json(comparisons: &[(u8, u8)]) -> String {
    let body = comparisons
        .iter()
        .map(|&(lower, upper)| format!("[{lower},{upper}]"))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{body}]")
}

fn order_json(order: &[u8; SIDE]) -> String {
    let body = order
        .iter()
        .map(|digit| (digit + 1).to_string())
        .collect::<Vec<_>>()
        .join(",");
    format!("[{body}]")
}

fn solution_string(solution: &[u8; CELLS]) -> String {
    solution
        .iter()
        .map(|digit| char::from(b'0' + *digit))
        .collect()
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\u{:04x}", control as u32);
            }
            other => escaped.push(other),
        }
    }
    escaped
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    update_fnv1a64(&mut hash, bytes);
    hash
}

fn update_fnv1a64(hash: &mut u64, bytes: &[u8]) {
    for &byte in bytes {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn fnv1a64_reader(reader: &mut impl Read, bytes: u64) -> std::io::Result<u64> {
    let mut hash = FNV_OFFSET;
    let mut remaining = bytes;
    let mut buffer = [0u8; 64 * 1024];
    while remaining != 0 {
        let requested = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
        let read = reader.read(&mut buffer[..requested])?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "output ended before checkpoint byte offset",
            ));
        }
        update_fnv1a64(&mut hash, &buffer[..read]);
        remaining -= read as u64;
    }
    Ok(hash)
}

fn format_bound(count: Option<u64>, capped: bool) -> String {
    count.map_or_else(
        || "none".to_owned(),
        |value| {
            if capped {
                format!(">={value}")
            } else {
                value.to_string()
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use thermo_sudoku::SolveStats;

    fn synthetic_puzzle() -> Puzzle {
        Puzzle {
            encoded: format!("{}{}", "12345678912345678", ".".repeat(64)),
            cells: [
                0, 1, 2, 9, 10, 11, 18, 19, 20, 30, 31, 32, 39, 40, 41, 48, 49,
            ],
            digits: [0, 1, 2, 3, 4, 5, 6, 7, 8, 0, 1, 2, 3, 4, 5, 6, 7],
            digit_mask: 0x01ff,
        }
    }

    fn path_mask(universe: &PairUniverse) -> PairMask {
        let mut mask = PairMask::default();
        for vertex in 0..CLUES - 1 {
            mask.insert(universe.pair_index(vertex, vertex + 1));
        }
        mask
    }

    fn test_options(input: PathBuf) -> Options {
        Options {
            input,
            output: None,
            mode: Mode::Exact,
            start_line: 1,
            end_line: 1,
            max_units: None,
            solution_cap: 2,
            guided_graphs: 64,
            guided_orders: 256,
            emit_cases: false,
            stop_on_first: false,
            progress_every: 0,
            checkpoint: None,
            resume: false,
            checkpoint_every: 1,
            expected_records: None,
            expected_fnv64: None,
            corpus_is_complete: false,
        }
    }

    #[test]
    fn saturation_contains_every_target_true_local_comparison() {
        let universe = PairUniverse::new();
        let puzzle = synthetic_puzzle();
        let mut adjacency = PairMask::default();
        for &(left, right) in &[(0usize, 1usize), (1, 2), (0, 2), (8, 9)] {
            adjacency.insert(universe.pair_index(left, right));
        }
        let order = make_order_case([8, 7, 6, 5, 4, 3, 2, 1, 0], adjacency, &puzzle, &universe);
        let comparisons = realize_comparisons(adjacency, &puzzle, &universe, &order, None, 0, 0);
        assert_eq!(comparisons.len(), adjacency.count() as usize);
        assert!(comparisons.contains(&(1, 0)));
        assert!(comparisons.contains(&(2, 1)));
        assert!(comparisons.contains(&(2, 0)));
        // Vertex 8 has digit 9 and vertex 9 has digit 1, so reverse order
        // directs the comparison from vertex 8's cell to vertex 9's cell.
        assert!(comparisons.contains(&(20, 30)));

        let mut proposed_subset = PairMask::default();
        proposed_subset.insert(universe.pair_index(0, 1));
        proposed_subset.insert(universe.pair_index(8, 9));
        assert!(proposed_subset.is_subset_of(adjacency));
    }

    #[test]
    fn coverage_requires_every_one_of_the_seventeen_vertices() {
        let universe = PairUniverse::new();
        let full = path_mask(&universe);
        assert!(all_vertices_incident(full, &universe));
        let mut missing_last = PairMask::default();
        for vertex in 0..CLUES - 2 {
            missing_last.insert(universe.pair_index(vertex, vertex + 1));
        }
        assert!(!all_vertices_incident(missing_last, &universe));
    }

    #[test]
    fn morph_masks_and_realized_orientations_agree() {
        let universe = PairUniverse::new();
        let puzzle = synthetic_puzzle();
        let morphs = generate_axis_morphs();
        let unequal = puzzle.unequal_pair_mask(&universe);
        let mut stats = SearchStats::default();
        let networks = network_classes(&puzzle, &universe, &morphs, unequal, &mut stats, None);
        assert!(!networks.is_empty());
        for network in networks.iter().take(8) {
            let rows = &morphs[network.row_morph as usize];
            let columns = &morphs[network.column_morph as usize];
            let direct =
                spatial_adjacency_mask(&puzzle, &universe, rows, columns).intersection(unequal);
            assert_eq!(network.mask, direct);
            let order = make_order_case(
                [0, 1, 2, 3, 4, 5, 6, 7, 8],
                network.mask,
                &puzzle,
                &universe,
            );
            for (lower, upper) in realize_comparisons(
                network.mask,
                &puzzle,
                &universe,
                &order,
                Some(&morphs),
                network.row_morph as usize,
                network.column_morph as usize,
            ) {
                assert!(is_king_neighbour(lower, upper));
            }
        }
        assert!(stats.row_axis_classes >= stats.row_axis_maximal_classes);
        assert!(stats.column_axis_classes >= stats.column_axis_maximal_classes);
        assert!(stats.network_classes >= networks.len() as u64);
    }

    #[test]
    fn hamiltonian_pruning_keeps_one_global_reversal() {
        let universe = PairUniverse::new();
        let puzzle = synthetic_puzzle();
        let mut mask = PairMask::default();
        // Use the first occurrence of each symbol to form exactly one symbol path.
        for digit in 0..SIDE - 1 {
            mask.insert(universe.pair_index(digit, digit + 1));
        }
        let graph = symbol_graph(mask, &puzzle, &universe);
        let (cases, orders) = hamiltonian_order_cases(&graph, mask, &puzzle, &universe);
        assert_eq!(orders, 1);
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].order, [0, 1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn maximal_antichain_dominates_every_removed_mask() {
        let universe = PairUniverse::new();
        let mut small = PairMask::default();
        small.insert(universe.pair_index(0, 1));
        let mut medium = small;
        medium.insert(universe.pair_index(1, 2));
        let mut other = PairMask::default();
        other.insert(universe.pair_index(3, 4));
        let classes = vec![
            AxisClass {
                mask: small,
                representative: 0,
                multiplicity: 1,
            },
            AxisClass {
                mask: medium,
                representative: 1,
                multiplicity: 1,
            },
            AxisClass {
                mask: other,
                representative: 2,
                multiplicity: 1,
            },
        ];
        let maximal = maximal_axis_classes(classes);
        assert_eq!(maximal.len(), 2);
        assert!(maximal.iter().any(|class| class.mask == medium));
        assert!(maximal.iter().any(|class| class.mask == other));
        assert!(maximal.iter().any(|class| small.is_subset_of(class.mask)));
    }

    #[test]
    fn two_stage_antichain_dominates_every_axis_class_and_morph_pair_class() {
        let universe = PairUniverse::new();
        let puzzle = Puzzle::parse(
            ".................1.....2.3......3.2...1.4......5....6..3......4.7..8...962...7...",
            1,
        )
        .unwrap();
        let morphs = generate_axis_morphs();
        let unequal = puzzle.unequal_pair_mask(&universe);
        let raw_rows = axis_classes(&puzzle, &universe, &morphs, true, unequal);
        let raw_columns = axis_classes(&puzzle, &universe, &morphs, false, unequal);
        assert_eq!(
            raw_rows
                .iter()
                .map(|class| usize::from(class.multiplicity))
                .sum::<usize>(),
            MORPH_COUNT
        );
        assert_eq!(
            raw_columns
                .iter()
                .map(|class| usize::from(class.multiplicity))
                .sum::<usize>(),
            MORPH_COUNT
        );
        let maximal_rows = maximal_axis_classes(raw_rows.clone());
        let maximal_columns = maximal_axis_classes(raw_columns.clone());
        assert!(raw_rows.iter().all(|raw| {
            maximal_rows
                .iter()
                .any(|maximal| raw.mask.is_subset_of(maximal.mask))
        }));
        assert!(raw_columns.iter().all(|raw| {
            maximal_columns
                .iter()
                .any(|maximal| raw.mask.is_subset_of(maximal.mask))
        }));

        let mut stats = SearchStats::default();
        let maximal_networks =
            network_classes(&puzzle, &universe, &morphs, unequal, &mut stats, None);
        let retained_masks = maximal_networks
            .iter()
            .map(|network| network.mask)
            .collect::<Vec<_>>();
        let mut postings = vec![Vec::<usize>::new(); PAIRS];
        for (index, &mask) in retained_masks.iter().enumerate() {
            index_maximal_mask(mask, index, &mut postings);
        }
        let represented_pairs = raw_rows
            .iter()
            .flat_map(|rows| {
                raw_columns.iter().map(move |columns| {
                    u64::from(rows.multiplicity) * u64::from(columns.multiplicity)
                })
            })
            .sum::<u64>();
        assert_eq!(represented_pairs, (MORPH_COUNT * MORPH_COUNT) as u64);
        let mut exact_multiplicities = BTreeMap::<PairMask, u64>::new();
        for rows in &raw_rows {
            for columns in &raw_columns {
                let raw_network = rows.mask.intersection(columns.mask);
                *exact_multiplicities.entry(raw_network).or_default() +=
                    u64::from(rows.multiplicity) * u64::from(columns.multiplicity);
                assert!(
                    has_indexed_superset(raw_network, &retained_masks, &postings),
                    "undominated raw network {}",
                    raw_network.hex()
                );
            }
        }
        for retained in &maximal_networks {
            assert_eq!(
                retained.morph_pair_multiplicity,
                exact_multiplicities[&retained.mask]
            );
        }
    }

    #[test]
    fn hamiltonian_enumerator_matches_brute_permutations_and_orientation_dedup() {
        let universe = PairUniverse::new();
        let puzzle = synthetic_puzzle();
        let mut mask = PairMask::default();
        for &(left, right) in &[
            (0usize, 1usize),
            (1, 2),
            (2, 3),
            (3, 4),
            (4, 5),
            (5, 6),
            (6, 7),
            (7, 8),
            (0, 2),
            (2, 4),
            (4, 6),
            (6, 8),
        ] {
            mask.insert(universe.pair_index(left, right));
        }
        let graph = symbol_graph(mask, &puzzle, &universe);
        let (observed, observed_orders) = hamiltonian_order_cases(&graph, mask, &puzzle, &universe);

        let mut permutation = [0, 1, 2, 3, 4, 5, 6, 7, 8];
        let mut brute_orders = 0u64;
        let mut brute_orientations = BTreeSet::new();
        loop {
            if permutation[0] < permutation[8] && is_hamiltonian_order(&permutation, &graph) {
                brute_orders += 1;
                brute_orientations
                    .insert(make_order_case(permutation, mask, &puzzle, &universe).orientation_key);
            }
            if !next_permutation(&mut permutation) {
                break;
            }
        }
        assert_eq!(observed_orders, brute_orders);
        assert_eq!(
            observed
                .iter()
                .map(|case| case.orientation_key)
                .collect::<BTreeSet<_>>(),
            brute_orientations
        );
    }

    #[test]
    fn transitive_closure_and_indexed_poset_antichain_match_brute_force() {
        let universe = PairUniverse::new();
        let puzzle = synthetic_puzzle();
        let order = make_order_case(
            [0, 1, 2, 3, 4, 5, 6, 7, 8],
            PairMask::default(),
            &puzzle,
            &universe,
        );
        let mut first_mask = PairMask::default();
        first_mask.insert(universe.pair_index(0, 1));
        first_mask.insert(universe.pair_index(1, 2));
        let first_closure =
            candidate_transitive_closure(first_mask, &puzzle, &universe, &order.rank);
        assert!(first_closure.contains_relation(1));
        assert!(first_closure.contains_relation(CLUES + 2));
        assert!(first_closure.contains_relation(2));

        let mut masks = Vec::new();
        for edges in [
            &[(0usize, 1usize), (1, 2)][..],
            &[(0, 1), (1, 2), (2, 3)][..],
            &[(0, 1), (1, 3)][..],
            &[(0, 1), (1, 2)][..],
            &[(4, 5), (5, 6)][..],
        ] {
            let mut mask = PairMask::default();
            for &(left, right) in edges {
                mask.insert(universe.pair_index(left, right));
            }
            masks.push(mask);
        }
        let raw = masks
            .iter()
            .enumerate()
            .map(|(index, &mask)| Candidate {
                network_index: index,
                order_index: 0,
                network: NetworkClass {
                    mask,
                    row_morph: 0,
                    column_morph: 0,
                    morph_pair_multiplicity: 1,
                },
                order: order.clone(),
                closure: candidate_transitive_closure(mask, &puzzle, &universe, &order.rank),
            })
            .collect::<Vec<_>>();
        let unique_closures = raw
            .iter()
            .map(|candidate| candidate.closure)
            .collect::<BTreeSet<_>>();
        let brute_maximal = unique_closures
            .iter()
            .copied()
            .filter(|&candidate| {
                !unique_closures
                    .iter()
                    .copied()
                    .any(|other| candidate != other && candidate.is_subset_of(other))
            })
            .collect::<BTreeSet<_>>();
        let (indexed, unique) = maximal_closure_candidates(raw.clone());
        assert_eq!(unique, unique_closures.len());
        assert_eq!(
            indexed
                .iter()
                .map(|candidate| candidate.closure)
                .collect::<BTreeSet<_>>(),
            brute_maximal
        );
        assert!(raw.iter().all(|candidate| {
            indexed
                .iter()
                .any(|maximal| candidate.closure.is_subset_of(maximal.closure))
        }));
    }

    #[test]
    fn realized_full_coverage_case_is_mask_exact_adjacent_and_target_true() {
        let universe = PairUniverse::new();
        let puzzle = Puzzle::parse(
            ".................1.....2.3......3.2...1.4......5....6..3......4.7..8...962...7...",
            1,
        )
        .unwrap();
        let morphs = generate_axis_morphs();
        let unequal = puzzle.unequal_pair_mask(&universe);
        let mut stats = SearchStats::default();
        let networks = network_classes(&puzzle, &universe, &morphs, unequal, &mut stats, None);
        let (network, order) = networks
            .iter()
            .find_map(|network| {
                if !all_vertices_incident(network.mask, &universe) {
                    return None;
                }
                let graph = symbol_graph(network.mask, &puzzle, &universe);
                hamiltonian_order_cases(&graph, network.mask, &puzzle, &universe)
                    .0
                    .into_iter()
                    .next()
                    .map(|order| (network, order))
            })
            .expect("sample has a fully incident Hamiltonian network");
        let comparisons = realize_comparisons(
            network.mask,
            &puzzle,
            &universe,
            &order,
            Some(&morphs),
            network.row_morph as usize,
            network.column_morph as usize,
        );
        assert_eq!(comparisons.len(), network.mask.count() as usize);
        let rows = &morphs[network.row_morph as usize];
        let columns = &morphs[network.column_morph as usize];
        let mut vertex_at = [usize::MAX; CELLS];
        for vertex in 0..CLUES {
            let cell = morph_cell(puzzle.cells[vertex], rows, columns) as usize;
            vertex_at[cell] = vertex;
        }
        let mut incident = 0u32;
        for &(lower, upper) in &comparisons {
            assert!(is_king_neighbour(lower, upper));
            let lower_vertex = vertex_at[lower as usize];
            let upper_vertex = vertex_at[upper as usize];
            assert_ne!(lower_vertex, usize::MAX);
            assert_ne!(upper_vertex, usize::MAX);
            assert!(
                network
                    .mask
                    .contains(universe.pair_index(lower_vertex, upper_vertex))
            );
            assert!(
                order.rank[puzzle.digits[lower_vertex] as usize]
                    < order.rank[puzzle.digits[upper_vertex] as usize]
            );
            incident |= 1u32 << lower_vertex;
            incident |= 1u32 << upper_vertex;
        }
        assert_eq!(incident, (1u32 << CLUES) - 1);
    }

    #[test]
    fn checkpoint_resume_reconstructs_uninterrupted_exact_state() {
        let path = env::temp_dir().join(format!(
            "thermo-17c-overlap-checkpoint-{}-{}.json",
            std::process::id(),
            fnv1a64(b"resume-equivalence")
        ));
        let first = SolveResult {
            count: 2,
            capped: true,
            first_solution: None,
            second_solution: None,
            stats: SolveStats {
                nodes: 10,
                branches: 3,
                propagation_rounds: 7,
                thermo_revisions: 11,
                max_depth: 2,
            },
        };
        let second = SolveResult {
            count: 1,
            capped: false,
            first_solution: None,
            second_solution: None,
            stats: SolveStats {
                nodes: 20,
                branches: 5,
                propagation_rounds: 13,
                thermo_revisions: 17,
                max_depth: 3,
            },
        };

        let mut uninterrupted = SearchStats {
            records_in_range: 1,
            row_axis_classes: 12,
            row_axis_maximal_classes: 4,
            column_axis_classes: 10,
            column_axis_maximal_classes: 3,
            network_classes: 7,
            retained_network_classes: 2,
            candidate_units: 2,
            ..SearchStats::default()
        };
        uninterrupted.add_result(0, 1, &first);
        uninterrupted.add_result(1, 1, &second);

        let mut prefix = SearchStats::default();
        prefix.add_result(0, 1, &first);
        write_checkpoint(&path, 0x1234, 1, None, None, &prefix).unwrap();
        let loaded = read_checkpoint(&path).unwrap();
        assert_eq!(loaded.next_unit, 1);
        assert_eq!(loaded.classification.records_in_range, 0);
        assert_eq!(loaded.classification.row_axis_classes, 0);

        let mut resumed = SearchStats {
            records_in_range: 1,
            row_axis_classes: 12,
            row_axis_maximal_classes: 4,
            column_axis_classes: 10,
            column_axis_maximal_classes: 3,
            network_classes: 7,
            retained_network_classes: 2,
            candidate_units: 2,
            ..loaded.classification
        };
        resumed.add_result(1, 1, &second);
        assert_eq!(resumed, uninterrupted);
        fs::remove_file(&path).unwrap();
        let previous = checkpoint_sibling(&path, "prev");
        if previous.exists() {
            fs::remove_file(previous).unwrap();
        }
    }

    #[test]
    fn stop_on_first_resume_preserves_an_already_reached_stop() {
        let mut options = test_options(PathBuf::from("catalogue.txt"));
        let stats = SearchStats {
            classified_units: 4,
            unique: 1,
            multiple: 3,
            ..SearchStats::default()
        };
        assert!(!stop_on_first_reached(&options, &stats));
        let without_stop = run_fingerprint(&options, 0x1234, 49_158);
        options.stop_on_first = true;
        assert!(stop_on_first_reached(&options, &stats));
        assert_ne!(without_stop, run_fingerprint(&options, 0x1234, 49_158));
    }

    #[test]
    fn checkpoint_atomic_replace_retains_a_readable_backup() {
        let path = env::temp_dir().join(format!(
            "thermo-17c-overlap-atomic-{}-{}.json",
            std::process::id(),
            fnv1a64(b"atomic-checkpoint")
        ));
        let first = SearchStats {
            classified_units: 1,
            multiple: 1,
            capped_counts: 1,
            best_count: Some(2),
            best_capped: true,
            best_unit: Some(0),
            best_line: Some(1),
            ..SearchStats::default()
        };
        let second = SearchStats {
            classified_units: 2,
            multiple: 2,
            capped_counts: 2,
            best_count: Some(2),
            best_capped: true,
            best_unit: Some(0),
            best_line: Some(1),
            ..SearchStats::default()
        };
        write_checkpoint(&path, 7, 1, None, None, &first).unwrap();
        write_checkpoint(&path, 7, 2, None, None, &second).unwrap();
        assert_eq!(read_checkpoint(&path).unwrap().next_unit, 2);

        let previous = checkpoint_sibling(&path, "prev");
        if !previous.exists() {
            // Unix rename replaces atomically, so manufacture the same valid
            // backup state solely to exercise corruption fallback.
            fs::copy(&path, &previous).unwrap();
        }
        fs::write(&path, b"truncated").unwrap();
        assert!(read_checkpoint(&path).is_ok());
        fs::remove_file(&path).unwrap();
        fs::remove_file(&previous).unwrap();
    }

    #[test]
    fn checkpoint_temporary_creation_never_truncates_an_existing_sibling() {
        let directory = env::temp_dir();
        let path = directory.join(format!(
            "thermo-17c-overlap-exclusive-temp-{}.json",
            std::process::id()
        ));
        let occupied = checkpoint_sibling(&path, &format!("tmp-{}-0", std::process::id()));
        let sentinel = b"unrelated temporary sibling";
        fs::write(&occupied, sentinel).unwrap();

        write_checkpoint(&path, 0x1234, 0, None, None, &SearchStats::default()).unwrap();
        assert_eq!(fs::read(&occupied).unwrap(), sentinel);
        assert_eq!(read_checkpoint(&path).unwrap().fingerprint, 0x1234);

        fs::remove_file(path).unwrap();
        fs::remove_file(occupied).unwrap();
    }

    #[test]
    fn checkpoint_backup_replacement_requires_owned_matching_state() {
        let directory = env::temp_dir();
        let unrelated = directory.join(format!(
            "thermo-17c-overlap-unrelated-prev-{}.json",
            std::process::id()
        ));
        let sentinel = b"not a checkpoint";
        fs::write(&unrelated, sentinel).unwrap();
        assert!(
            remove_owned_checkpoint_backup(&unrelated, 7)
                .unwrap_err()
                .contains("refusing to replace checkpoint backup")
        );
        assert_eq!(fs::read(&unrelated).unwrap(), sentinel);
        fs::remove_file(&unrelated).unwrap();

        let owned = directory.join(format!(
            "thermo-17c-overlap-owned-prev-{}.json",
            std::process::id()
        ));
        write_checkpoint(&owned, 7, 0, None, None, &SearchStats::default()).unwrap();
        assert!(
            remove_owned_checkpoint_backup(&owned, 8)
                .unwrap_err()
                .contains("does not match current run")
        );
        assert!(owned.exists());
        remove_owned_checkpoint_backup(&owned, 7).unwrap();
        assert!(!owned.exists());
    }

    #[test]
    fn replacing_an_invalid_primary_preserves_its_valid_backup() {
        let directory = env::temp_dir();
        let primary = directory.join(format!(
            "thermo-17c-overlap-invalid-primary-{}.json",
            std::process::id()
        ));
        let previous = checkpoint_sibling(&primary, "prev");
        let prefix = SearchStats {
            classified_units: 1,
            multiple: 1,
            capped_counts: 1,
            best_count: Some(2),
            best_capped: true,
            best_unit: Some(0),
            best_line: Some(1),
            ..SearchStats::default()
        };
        write_checkpoint(&previous, 7, 1, None, None, &prefix).unwrap();
        fs::write(&primary, b"truncated checkpoint").unwrap();

        write_checkpoint(&primary, 7, 1, None, None, &prefix).unwrap();
        assert_eq!(read_checkpoint_exact(&primary).unwrap().fingerprint, 7);
        assert_eq!(read_checkpoint_exact(&previous).unwrap().fingerprint, 7);

        fs::remove_file(primary).unwrap();
        fs::remove_file(previous).unwrap();
    }

    #[test]
    fn resume_validates_hash_and_truncates_output_to_checkpoint_boundary() {
        let directory = env::temp_dir();
        let output_path = directory.join(format!(
            "thermo-17c-overlap-output-resume-{}.jsonl",
            std::process::id()
        ));
        let prefix = b"{\"type\":\"header\"}\n{\"type\":\"case\",\"unit\":0}\n";
        let mut with_tail = prefix.to_vec();
        with_tail.extend_from_slice(b"partial-or-duplicated-tail");
        fs::write(&output_path, &with_tail).unwrap();

        let mut options = test_options(directory.join("unread-input.txt"));
        options.output = Some(output_path.clone());
        options.resume = true;
        let checkpoint = Checkpoint {
            fingerprint: 1,
            next_unit: 1,
            output_bytes: Some(prefix.len() as u64),
            output_fnv64: Some(fnv1a64(prefix)),
            classification: SearchStats::default(),
        };
        let mut output = open_output(&options, Some(&checkpoint)).unwrap();
        flush_output(output.as_mut()).unwrap();
        drop(output);
        assert_eq!(fs::read(&output_path).unwrap(), prefix);

        let mut corrupted = prefix.to_vec();
        corrupted[2] ^= 1;
        fs::write(&output_path, &corrupted).unwrap();
        assert!(
            open_output(&options, Some(&checkpoint))
                .unwrap_err()
                .contains("prefix hash mismatch")
        );
        fs::remove_file(output_path).unwrap();
    }

    #[test]
    fn json_provenance_never_calls_guided_mode_complete() {
        let mut options = test_options(PathBuf::from("catalogue.txt"));
        options.mode = Mode::Guided;
        let header = header_json(&options, 0xabc, 123, 49_158, 49_158, 0xdef);
        assert!(header.contains("\"guided\""));
        assert!(header.contains("never a completeness proof"));
        assert!(header.contains("maximal_mask_reduction"));
        let summary = summary_json(
            &options,
            &SearchStats::default(),
            0xabc,
            0xdef,
            0,
            0,
            0,
            true,
            true,
            false,
            false,
            false,
            false,
            0.0,
        );
        assert!(summary.contains("\"guided_incomplete_by_definition\":true"));
        assert!(summary.contains("\"generalized_complete\":false"));
    }

    #[test]
    fn write_paths_cannot_alias_input_or_each_other() {
        let directory = env::temp_dir();
        let input = directory.join(format!(
            "thermo-17c-overlap-alias-input-{}.txt",
            std::process::id()
        ));
        fs::write(&input, b"test").unwrap();

        let mut options = test_options(input.clone());
        options.output = Some(input.clone());
        assert_eq!(
            validate_distinct_paths(&options).unwrap_err(),
            "--output must not alias --input"
        );

        options.output = None;
        options.checkpoint = Some(input.clone());
        assert_eq!(
            validate_distinct_paths(&options).unwrap_err(),
            "--checkpoint must not alias --input"
        );

        let shared = directory.join(format!(
            "thermo-17c-overlap-alias-shared-{}.json",
            std::process::id()
        ));
        options.checkpoint = Some(shared.clone());
        options.output = Some(shared);
        assert_eq!(
            validate_distinct_paths(&options).unwrap_err(),
            "--output must not alias --checkpoint"
        );

        let hard_link = directory.join(format!(
            "thermo-17c-overlap-alias-hardlink-{}.txt",
            std::process::id()
        ));
        fs::hard_link(&input, &hard_link).unwrap();
        options.checkpoint = None;
        options.output = Some(hard_link.clone());
        assert_eq!(
            validate_distinct_paths(&options).unwrap_err(),
            "--output must not alias --input"
        );

        let checkpoint = directory.join(format!(
            "thermo-17c-overlap-alias-aux-{}.json",
            std::process::id()
        ));
        let previous = checkpoint_sibling(&checkpoint, "prev");
        fs::hard_link(&input, &previous).unwrap();
        options.output = None;
        options.checkpoint = Some(checkpoint);
        assert_eq!(
            validate_distinct_paths(&options).unwrap_err(),
            "checkpoint backup must not alias --input"
        );

        fs::remove_file(previous).unwrap();
        fs::remove_file(hard_link).unwrap();
        fs::remove_file(input).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_alias_is_rejected() {
        use std::os::unix::fs::symlink;

        let directory = env::temp_dir();
        let input = directory.join(format!(
            "thermo-17c-overlap-symlink-input-{}.txt",
            std::process::id()
        ));
        let link = directory.join(format!(
            "thermo-17c-overlap-symlink-output-{}.txt",
            std::process::id()
        ));
        fs::write(&input, b"test").unwrap();
        symlink(&input, &link).unwrap();
        let mut options = test_options(input.clone());
        options.output = Some(link.clone());
        assert_eq!(
            validate_distinct_paths(&options).unwrap_err(),
            "--output must not alias --input"
        );
        fs::remove_file(link).unwrap();
        fs::remove_file(input).unwrap();
    }

    fn is_king_neighbour(left: u8, right: u8) -> bool {
        left != right && (left / 9).abs_diff(right / 9) <= 1 && (left % 9).abs_diff(right % 9) <= 1
    }

    fn next_permutation(values: &mut [u8]) -> bool {
        let Some(pivot) = (0..values.len() - 1)
            .rev()
            .find(|&index| values[index] < values[index + 1])
        else {
            return false;
        };
        let successor = (pivot + 1..values.len())
            .rev()
            .find(|&index| values[pivot] < values[index])
            .unwrap();
        values.swap(pivot, successor);
        values[pivot + 1..].reverse();
        true
    }
}
