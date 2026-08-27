//! Deterministic constructor for an independent corpus of low-count 18-cell
//! branching thermo-Sudoku states.
//!
//! The input is the project's frozen disjoint 9+8+2 corpus.  Every accepted
//! parent is independently re-enumerated, one cell is deleted, and the
//! remaining footprint is saturated with every target-true unequal
//! king-neighbour comparison.  States are reduced to their unique Hasse DAG,
//! canonicalized under D4 and digit complement, globally deduplicated by the
//! exact Hasse edge vector, and only then counted.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use thermo_sudoku::thermo_graph::{
    CELLS, CanonicalState, Edge, Grid, Transform, canonicalize_state, grid_satisfies_edges,
    incident_cells, network_sha256, saturate_target, sha256_hex, state_sha256, transform_cell,
    transitive_closure_edges, transitive_reduction,
};
#[cfg(test)]
use thermo_sudoku::thermo_graph::{king_adjacent, transform_edges, transform_grid};
use thermo_sudoku::{SolveStats, Solver};

const SCHEMA: &str = "thermo-18c-seed-harvest-v1";
const ALGORITHM_REVISION: &str =
    "verified-982-target-saturation-hasse-d4-complement-global-dedupe-v1";
const PROJECT_V1_INPUT_SHA256: &str =
    "79bec9ad12bf7c3c6cb28948e1c54cd98809929d5fe5a3003a8c6215367046a7";
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

type ParsedParents = (
    BTreeMap<ParentLayout, ParentSource>,
    Vec<InvalidRecord>,
    Accounting,
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeletionScope {
    LongTerminals,
    AllCells,
}

impl DeletionScope {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "long-terminals" | "endpoints" => Ok(Self::LongTerminals),
            "all-cells" => Ok(Self::AllCells),
            _ => Err(format!(
                "invalid --deletions {value}; expected long-terminals or all-cells"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::LongTerminals => "long-terminals",
            Self::AllCells => "all-cells",
        }
    }
}

#[derive(Clone, Debug)]
struct Options {
    input: PathBuf,
    output: Option<PathBuf>,
    start_line: usize,
    end_line: usize,
    max_parents: Option<usize>,
    solution_cap: u64,
    deletions: DeletionScope,
    progress_every: u64,
    stop_on_first: bool,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ParentLayout {
    path9: Vec<u8>,
    path8: Vec<u8>,
    path2: Vec<u8>,
}

impl ParentLayout {
    fn paths(&self) -> [Vec<u8>; 3] {
        [self.path9.clone(), self.path8.clone(), self.path2.clone()]
    }

    fn path(&self, index: usize) -> &[u8] {
        match index {
            0 => &self.path9,
            1 => &self.path8,
            2 => &self.path2,
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Debug)]
struct ParentSource {
    layout: ParentLayout,
    rows: Vec<SourceRow>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SourceRow {
    line: usize,
    declared_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Deletion {
    path_index: u8,
    position: u8,
    cell: u8,
}

impl Deletion {
    fn path_name(self) -> &'static str {
        match self.path_index {
            0 => "path9",
            1 => "path8",
            2 => "path2",
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Provenance {
    parent: ParentLayout,
    parent_sha256: String,
    source_lines: Vec<usize>,
    declared_parent_solutions: usize,
    target_ordinal: usize,
    deletion: Deletion,
}

#[derive(Clone, Debug)]
struct NetworkRecord {
    state: CanonicalState,
    provenance: Provenance,
    occurrences: u64,
}

impl NetworkRecord {
    fn observe(&mut self, state: CanonicalState, provenance: Provenance) {
        self.occurrences += 1;
        let candidate_rank = (&state.target, &state.full_edges, &state.cells, &provenance);
        let current_rank = (
            &self.state.target,
            &self.state.full_edges,
            &self.state.cells,
            &self.provenance,
        );
        if candidate_rank < current_rank {
            self.state = state;
            self.provenance = provenance;
        }
    }
}

#[derive(Clone, Debug)]
struct InvalidRecord {
    lines: Vec<usize>,
    stage: &'static str,
    message: String,
}

#[derive(Clone, Debug)]
struct SeedRecord {
    ordinal: u64,
    network_sha256: String,
    state_sha256: String,
    network: NetworkRecord,
    count: u64,
    first_solution: Option<Grid>,
    second_solution: Option<Grid>,
    stats: SolveStats,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SolverTotals {
    calls: u64,
    nodes: u64,
    branches: u64,
    propagation_rounds: u64,
    comparison_revisions: u64,
    max_depth: u8,
}

impl SolverTotals {
    fn add(&mut self, stats: SolveStats) {
        self.calls += 1;
        self.nodes = self.nodes.saturating_add(stats.nodes);
        self.branches = self.branches.saturating_add(stats.branches);
        self.propagation_rounds = self
            .propagation_rounds
            .saturating_add(stats.propagation_rounds);
        self.comparison_revisions = self
            .comparison_revisions
            .saturating_add(stats.thermo_revisions);
        self.max_depth = self.max_depth.max(stats.max_depth);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Accounting {
    physical_lines: u64,
    nonempty_lines: u64,
    lines_in_range: u64,
    parsed_rows: u64,
    geometry_valid_rows: u64,
    malformed_rows: u64,
    duplicate_parent_rows: u64,
    unique_parents_in_range: u64,
    parents_with_conflicting_counts: u64,
    eligible_parents: u64,
    selected_parents: u64,
    parents_omitted_by_limit: u64,
    parents_enumerated: u64,
    parents_verified: u64,
    parent_count_mismatches: u64,
    target_solutions: u64,
    deletion_attempts: u64,
    exact_18_cell_footprints: u64,
    coverage_rejections: u64,
    raw_candidate_states: u64,
    canonical_networks: u64,
    duplicate_network_states: u64,
    networks_classified: u64,
    exact_network_counts: u64,
    capped_network_counts: u64,
    zero_solution_errors: u64,
    seed_records: u64,
    unique_seed_records: u64,
    invalid_records: u64,
    stopped_on_first: bool,
    parent_solver: SolverTotals,
    network_solver: SolverTotals,
}

#[derive(Clone, Debug)]
struct HarvestOutcome {
    invalid: Vec<InvalidRecord>,
    networks: BTreeMap<Vec<Edge>, NetworkRecord>,
    seeds: Vec<SeedRecord>,
    count_distribution: BTreeMap<u64, u64>,
    accounting: Accounting,
}

fn should_emit_seed(count: u64, capped: bool, solution_cap: u64) -> bool {
    !capped && count < solution_cap
}

#[derive(Clone, Debug)]
struct BinaryProvenance {
    path: PathBuf,
    bytes: usize,
    sha256: String,
}

struct ReportContext<'a> {
    options: &'a Options,
    input_bytes: &'a [u8],
    input_sha256: &'a str,
    input_fnv64: u64,
    binary: &'a BinaryProvenance,
    outcome: &'a HarvestOutcome,
    elapsed_seconds: f64,
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
    validate_input_output_distinct(&options)?;
    let started = Instant::now();
    let input_bytes = fs::read(&options.input)
        .map_err(|error| format!("cannot read {}: {error}", options.input.display()))?;
    let input_sha256 = sha256_hex(&input_bytes);
    let input_fnv64 = fnv1a64(&input_bytes);
    let binary = binary_provenance()?;

    let mut outcome = harvest(&input_bytes, &options, started)?;
    outcome.invalid.sort_by(|left, right| {
        left.lines
            .first()
            .cmp(&right.lines.first())
            .then_with(|| left.stage.cmp(right.stage))
            .then_with(|| left.message.cmp(&right.message))
    });
    outcome.accounting.invalid_records = outcome.invalid.len() as u64;

    emit_output(
        &options,
        &input_bytes,
        &input_sha256,
        input_fnv64,
        &binary,
        &outcome,
        started.elapsed().as_secs_f64(),
    )?;
    eprintln!(
        "parents_verified={} raw_states={} canonical_networks={} classified={} seeds={} best={} elapsed={:.3}s",
        outcome.accounting.parents_verified,
        outcome.accounting.raw_candidate_states,
        outcome.accounting.canonical_networks,
        outcome.accounting.networks_classified,
        outcome.accounting.seed_records,
        outcome
            .count_distribution
            .keys()
            .next()
            .map_or_else(|| "none".to_owned(), u64::to_string),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn harvest(
    input_bytes: &[u8],
    options: &Options,
    started: Instant,
) -> Result<HarvestOutcome, String> {
    let (parents, mut invalid, mut accounting) = parse_parents(input_bytes, options)?;
    let mut eligible = Vec::new();
    for (_, parent) in parents {
        let counts = parent
            .rows
            .iter()
            .map(|row| row.declared_count)
            .collect::<BTreeSet<_>>();
        if counts.len() != 1 {
            accounting.parents_with_conflicting_counts += 1;
            invalid.push(InvalidRecord {
                lines: parent.rows.iter().map(|row| row.line).collect(),
                stage: "declared-count-conflict",
                message: format!(
                    "symmetry-equivalent rows disagree on declared counts: {counts:?}"
                ),
            });
        } else {
            eligible.push(parent);
        }
    }
    accounting.eligible_parents = eligible.len() as u64;
    let selected_len = options
        .max_parents
        .map_or(eligible.len(), |limit| limit.min(eligible.len()));
    accounting.selected_parents = selected_len as u64;
    accounting.parents_omitted_by_limit = (eligible.len() - selected_len) as u64;

    let mut networks: BTreeMap<Vec<Edge>, NetworkRecord> = BTreeMap::new();
    for (parent_index, parent) in eligible.into_iter().take(selected_len).enumerate() {
        accounting.parents_enumerated += 1;
        let declared = parent.rows[0].declared_count;
        if declared == 0 {
            accounting.parent_count_mismatches += 1;
            invalid.push(InvalidRecord {
                lines: parent.rows.iter().map(|row| row.line).collect(),
                stage: "parent-enumeration",
                message: "declared solution count must be positive".to_owned(),
            });
            continue;
        }
        let solver = Solver::blank(&parent.layout.paths()).map_err(|error| {
            format!("validated parent unexpectedly failed solver construction: {error}")
        })?;
        let batch = solver.enumerate_up_to(declared);
        accounting.parent_solver.add(batch.stats);
        if !batch.exhausted || batch.capped || batch.solutions.len() != declared {
            accounting.parent_count_mismatches += 1;
            let observed = if batch.capped {
                format!("at least {}", declared.saturating_add(1))
            } else {
                batch.solutions.len().to_string()
            };
            invalid.push(InvalidRecord {
                lines: parent.rows.iter().map(|row| row.line).collect(),
                stage: "parent-count-verification",
                message: format!(
                    "declared {declared} solutions, independent enumeration found {observed}"
                ),
            });
            continue;
        }
        accounting.parents_verified += 1;
        let mut targets = batch.solutions;
        targets.sort();
        targets.dedup();
        if targets.len() != declared {
            return Err(format!(
                "parent {:?}: solver enumeration returned duplicate target grids",
                parent.rows.iter().map(|row| row.line).collect::<Vec<_>>()
            ));
        }
        accounting.target_solutions = accounting
            .target_solutions
            .saturating_add(targets.len() as u64);
        let deletions = deletions_for(&parent.layout, options.deletions);
        let parent_sha256 = parent_sha256(&parent.layout);
        let source_lines = parent.rows.iter().map(|row| row.line).collect::<Vec<_>>();

        for (target_index, target) in targets.into_iter().enumerate() {
            if !target_satisfies_parent(&target, &parent.layout) {
                return Err(format!(
                    "parent line {}: enumerated target does not satisfy canonical parent",
                    parent.rows[0].line
                ));
            }
            for deletion in &deletions {
                accounting.deletion_attempts += 1;
                let footprint = footprint_after_deletion(&parent.layout, *deletion);
                if footprint.len() != 18 {
                    continue;
                }
                accounting.exact_18_cell_footprints += 1;
                let full_edges = saturate_target(&footprint, &target);
                if incident_cells(&full_edges) != footprint {
                    accounting.coverage_rejections += 1;
                    continue;
                }
                let hasse_edges = transitive_reduction(&full_edges)?;
                if incident_cells(&hasse_edges) != footprint {
                    return Err("transitive reduction lost an incident vertex".to_owned());
                }
                if transitive_closure_edges(&hasse_edges)? != transitive_closure_edges(&full_edges)?
                {
                    return Err("Hasse reduction changed the comparison closure".to_owned());
                }
                accounting.raw_candidate_states += 1;
                let state = canonicalize_state(&full_edges, &hasse_edges, &target, &footprint);
                let provenance = Provenance {
                    parent: parent.layout.clone(),
                    parent_sha256: parent_sha256.clone(),
                    source_lines: source_lines.clone(),
                    declared_parent_solutions: declared,
                    target_ordinal: target_index + 1,
                    deletion: *deletion,
                };
                match networks.entry(state.hasse_edges.clone()) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(NetworkRecord {
                            state,
                            provenance,
                            occurrences: 1,
                        });
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        accounting.duplicate_network_states += 1;
                        entry.get_mut().observe(state, provenance);
                    }
                }
            }
        }
        if options.progress_every != 0
            && (parent_index as u64 + 1).is_multiple_of(options.progress_every)
        {
            eprintln!(
                "collected_parents={} verified={} raw_states={} canonical_networks={} elapsed={:.3}s",
                parent_index + 1,
                accounting.parents_verified,
                accounting.raw_candidate_states,
                networks.len(),
                started.elapsed().as_secs_f64()
            );
        }
    }
    accounting.canonical_networks = networks.len() as u64;
    debug_assert_eq!(
        accounting.raw_candidate_states,
        accounting.canonical_networks + accounting.duplicate_network_states
    );

    let mut seeds = Vec::new();
    let mut count_distribution = BTreeMap::new();
    let mut stopped = false;
    for (index, (hasse_key, network)) in networks.iter().enumerate() {
        if hasse_key != &network.state.hasse_edges {
            return Err("internal network map key mismatch".to_owned());
        }
        if hasse_key.len() > thermo_sudoku::MAX_COMPARISONS {
            return Err(format!(
                "canonical network {} has {} Hasse comparisons, exceeding solver capacity {}",
                network_sha256(hasse_key),
                hasse_key.len(),
                thermo_sudoku::MAX_COMPARISONS
            ));
        }
        if !grid_satisfies_edges(&network.state.target, hasse_key) {
            return Err("canonical target does not satisfy its Hasse network".to_owned());
        }
        let solver = Solver::blank_comparisons(hasse_key)
            .map_err(|error| format!("cannot build canonical Hasse network: {error}"))?;
        let result = solver.count_up_to(options.solution_cap);
        accounting.network_solver.add(result.stats);
        accounting.networks_classified += 1;
        if result.count == 0 {
            return Err(format!(
                "network {} has zero solutions although its stored target satisfies every edge",
                network_sha256(hasse_key)
            ));
        }
        if result.capped {
            accounting.capped_network_counts += 1;
        } else {
            accounting.exact_network_counts += 1;
            if should_emit_seed(result.count, result.capped, options.solution_cap) {
                accounting.seed_records += 1;
                if result.count == 1 {
                    accounting.unique_seed_records += 1;
                }
                *count_distribution.entry(result.count).or_insert(0) += 1;
                seeds.push(SeedRecord {
                    ordinal: accounting.seed_records,
                    network_sha256: network_sha256(hasse_key),
                    state_sha256: state_sha256(hasse_key, &network.state.target),
                    network: network.clone(),
                    count: result.count,
                    first_solution: result.first_solution,
                    second_solution: result.second_solution,
                    stats: result.stats,
                });
            }
        }
        if options.progress_every != 0
            && accounting
                .networks_classified
                .is_multiple_of(options.progress_every)
        {
            eprintln!(
                "classified={} of {} exact={} capped={} seeds={} best={} elapsed={:.3}s",
                accounting.networks_classified,
                networks.len(),
                accounting.exact_network_counts,
                accounting.capped_network_counts,
                accounting.seed_records,
                count_distribution
                    .keys()
                    .next()
                    .map_or_else(|| "none".to_owned(), u64::to_string),
                started.elapsed().as_secs_f64()
            );
        }
        if result.count == 1 && !result.capped && options.stop_on_first {
            stopped = true;
            eprintln!("stopping after unique network at sorted index {index}");
            break;
        }
    }
    accounting.stopped_on_first = stopped;

    Ok(HarvestOutcome {
        invalid,
        networks,
        seeds,
        count_distribution,
        accounting,
    })
}

fn parse_parents(input_bytes: &[u8], options: &Options) -> Result<ParsedParents, String> {
    let text = std::str::from_utf8(input_bytes)
        .map_err(|error| format!("input is not valid UTF-8 at byte {}", error.valid_up_to()))?;
    let mut parents: BTreeMap<ParentLayout, ParentSource> = BTreeMap::new();
    let mut invalid = Vec::new();
    let mut accounting = Accounting::default();
    for (line_index, raw_line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        accounting.physical_lines += 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        accounting.nonempty_lines += 1;
        if line_number < options.start_line || line_number > options.end_line {
            continue;
        }
        accounting.lines_in_range += 1;
        let parsed = (|| -> Result<(usize, ParentLayout), (&'static str, String)> {
            let (declared, layout_text) = line.split_once(';').ok_or((
                "parse",
                "missing semicolon between count and layout".to_owned(),
            ))?;
            let declared = declared.trim().parse::<usize>().map_err(|_| {
                (
                    "parse",
                    "invalid positive declared solution count".to_owned(),
                )
            })?;
            if declared == 0 {
                return Err((
                    "parse",
                    "declared solution count must be positive".to_owned(),
                ));
            }
            let paths = parse_nested_paths(layout_text).map_err(|error| ("parse", error))?;
            accounting.parsed_rows += 1;
            let layout =
                normalize_and_validate_parent(paths).map_err(|error| ("geometry", error))?;
            accounting.geometry_valid_rows += 1;
            Ok((declared, canonical_parent(&layout)))
        })();
        match parsed {
            Ok((declared_count, layout)) => {
                let row = SourceRow {
                    line: line_number,
                    declared_count,
                };
                match parents.entry(layout.clone()) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(ParentSource {
                            layout,
                            rows: vec![row],
                        });
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        accounting.duplicate_parent_rows += 1;
                        entry.get_mut().rows.push(row);
                    }
                }
            }
            Err((stage, message)) => {
                accounting.malformed_rows += 1;
                invalid.push(InvalidRecord {
                    lines: vec![line_number],
                    stage,
                    message,
                });
            }
        }
    }
    accounting.unique_parents_in_range = parents.len() as u64;
    Ok((parents, invalid, accounting))
}

fn normalize_and_validate_parent(mut paths: Vec<Vec<u8>>) -> Result<ParentLayout, String> {
    if paths.len() != 3 {
        return Err(format!(
            "expected exactly three thermometers, found {}",
            paths.len()
        ));
    }
    paths.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    let lengths = [paths[0].len(), paths[1].len(), paths[2].len()];
    if lengths != [9, 8, 2] {
        return Err(format!(
            "expected thermometer lengths 9+8+2, found {}+{}+{}",
            lengths[0], lengths[1], lengths[2]
        ));
    }
    let mut occupied = [false; CELLS];
    for (path_index, path) in paths.iter().enumerate() {
        let mut local = [false; CELLS];
        for (position, &cell) in path.iter().enumerate() {
            let cell_index = cell as usize;
            if cell_index >= CELLS {
                return Err(format!(
                    "thermometer {path_index}, position {position}: cell {cell} is outside 0..=80"
                ));
            }
            if local[cell_index] {
                return Err(format!("thermometer {path_index} repeats cell {cell}"));
            }
            if occupied[cell_index] {
                return Err(format!("cell {cell} occurs in multiple thermometers"));
            }
            local[cell_index] = true;
            occupied[cell_index] = true;
        }
    }
    if occupied.iter().filter(|&&value| value).count() != 19 {
        return Err("9+8+2 parent does not cover exactly 19 cells".to_owned());
    }
    let layout = ParentLayout {
        path9: paths.remove(0),
        path8: paths.remove(0),
        path2: paths.remove(0),
    };
    let solver = Solver::blank(&layout.paths()).map_err(|error| error.to_string())?;
    if solver.layout().covered_cells() != 19 {
        return Err(format!(
            "solver reports {} covered cells instead of 19",
            solver.layout().covered_cells()
        ));
    }
    Ok(layout)
}

struct NestedPathParser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> NestedPathParser<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            position: 0,
        }
    }

    fn parse(mut self) -> Result<Vec<Vec<u8>>, String> {
        self.skip_space();
        self.expect(b'[', "layout must start with '['")?;
        let mut paths = Vec::new();
        loop {
            self.skip_space();
            if self.consume(b']') {
                break;
            }
            if !paths.is_empty() {
                self.expect(b',', "expected ',' between thermometers")?;
                self.skip_space();
            }
            paths.push(self.parse_path()?);
        }
        self.skip_space();
        if self.position != self.bytes.len() {
            return Err(format!(
                "unexpected trailing input at byte {}",
                self.position
            ));
        }
        Ok(paths)
    }

    fn parse_path(&mut self) -> Result<Vec<u8>, String> {
        let open = self
            .peek()
            .ok_or("unexpected end while reading thermometer")?;
        let close = match open {
            b'(' => b')',
            b'[' => b']',
            _ => return Err(format!("expected '(' or '[' at byte {}", self.position)),
        };
        self.position += 1;
        let mut cells = Vec::new();
        loop {
            self.skip_space();
            if self.consume(close) {
                break;
            }
            if !cells.is_empty() {
                self.expect(b',', "expected ',' between cells")?;
                self.skip_space();
                if self.consume(close) {
                    break;
                }
            }
            let start = self.position;
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.position += 1;
            }
            if start == self.position {
                return Err(format!("expected cell number at byte {}", self.position));
            }
            let value = std::str::from_utf8(&self.bytes[start..self.position])
                .expect("ASCII digits are valid UTF-8")
                .parse::<u16>()
                .map_err(|_| format!("invalid cell number at byte {start}"))?;
            if value >= CELLS as u16 {
                return Err(format!("cell {value} is outside 0..=80"));
            }
            cells.push(value as u8);
        }
        Ok(cells)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: u8, message: &str) -> Result<(), String> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(format!("{message} at byte {}", self.position))
        }
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.position += 1;
        }
    }
}

fn parse_nested_paths(text: &str) -> Result<Vec<Vec<u8>>, String> {
    NestedPathParser::new(text).parse()
}

fn transform_path(path: &[u8], transform: Transform) -> Vec<u8> {
    let mut result = path
        .iter()
        .map(|&cell| transform_cell(cell, transform.spatial))
        .collect::<Vec<_>>();
    if transform.complement {
        result.reverse();
    }
    result
}

fn transform_parent(layout: &ParentLayout, transform: Transform) -> ParentLayout {
    ParentLayout {
        path9: transform_path(&layout.path9, transform),
        path8: transform_path(&layout.path8, transform),
        path2: transform_path(&layout.path2, transform),
    }
}

fn canonical_parent(layout: &ParentLayout) -> ParentLayout {
    let mut best: Option<ParentLayout> = None;
    for complement in [false, true] {
        for spatial in 0..8 {
            let candidate = transform_parent(
                layout,
                Transform {
                    spatial,
                    complement,
                },
            );
            if best.as_ref().is_none_or(|current| candidate < *current) {
                best = Some(candidate);
            }
        }
    }
    best.expect("the D4/complement orbit is nonempty")
}

fn deletions_for(layout: &ParentLayout, scope: DeletionScope) -> Vec<Deletion> {
    match scope {
        DeletionScope::LongTerminals => [0usize, 1]
            .into_iter()
            .flat_map(|path_index| {
                let path = layout.path(path_index);
                [0usize, path.len() - 1]
                    .into_iter()
                    .map(move |position| Deletion {
                        path_index: path_index as u8,
                        position: position as u8,
                        cell: path[position],
                    })
            })
            .collect(),
        DeletionScope::AllCells => {
            (0usize..3)
                .flat_map(|path_index| {
                    layout.path(path_index).iter().copied().enumerate().map(
                        move |(position, cell)| Deletion {
                            path_index: path_index as u8,
                            position: position as u8,
                            cell,
                        },
                    )
                })
                .collect()
        }
    }
}

fn footprint_after_deletion(layout: &ParentLayout, deletion: Deletion) -> Vec<u8> {
    let mut cells = layout
        .path9
        .iter()
        .chain(&layout.path8)
        .chain(&layout.path2)
        .copied()
        .filter(|&cell| cell != deletion.cell)
        .collect::<Vec<_>>();
    cells.sort_unstable();
    cells.dedup();
    cells
}

fn target_satisfies_parent(target: &Grid, layout: &ParentLayout) -> bool {
    [&layout.path9, &layout.path8, &layout.path2]
        .into_iter()
        .all(|path| {
            path.windows(2)
                .all(|pair| target[pair[0] as usize] < target[pair[1] as usize])
        })
}

fn parse_options() -> Result<Options, String> {
    let mut input = None;
    let mut output = None;
    let mut start_line = 1usize;
    let mut end_line = usize::MAX;
    let mut max_parents = None;
    let mut solution_cap = 184u64;
    let mut deletions = DeletionScope::LongTerminals;
    let mut progress_every = 1_000u64;
    let mut stop_on_first = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        match argument.as_str() {
            "--input" => input = Some(PathBuf::from(value()?)),
            "--output" => {
                let path = value()?;
                output = (path != "-").then(|| PathBuf::from(path));
            }
            "--start-line" => start_line = parse_usize(&argument, &value()?)?,
            "--end-line" => end_line = parse_usize(&argument, &value()?)?,
            "--max-parents" => max_parents = Some(parse_usize(&argument, &value()?)?),
            "--solution-cap" | "--count-cap" => solution_cap = parse_u64(&argument, &value()?)?,
            "--deletions" => deletions = DeletionScope::parse(&value()?)?,
            "--progress-every" => progress_every = parse_u64(&argument, &value()?)?,
            "--stop-on-first" => stop_on_first = true,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {argument}; use --help")),
        }
    }
    let input = input.ok_or("--input FILE is required")?;
    if start_line == 0 {
        return Err("--start-line must be at least 1".to_owned());
    }
    if end_line < start_line {
        return Err("--end-line must be greater than or equal to --start-line".to_owned());
    }
    if solution_cap < 2 {
        return Err("--solution-cap must be at least 2".to_owned());
    }
    Ok(Options {
        input,
        output,
        start_line,
        end_line,
        max_parents,
        solution_cap,
        deletions,
        progress_every,
        stop_on_first,
    })
}

fn parse_usize(option: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("invalid value for {option}: {value}"))
}

fn parse_u64(option: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid value for {option}: {value}"))
}

fn print_usage() {
    println!(
        "thermo-18c-seed-harvest --input FILE [options]\n\
         \n\
         Build a corpus-relative set of saturated 18-cell branching states from\n\
         independently verified disjoint 9+8+2 parents. This is a constructive\n\
         search and is not an exhaustive search of all 18-cell thermo Sudokus.\n\
         \n\
         Options:\n\
           --output FILE             atomically write JSONL (default: stdout; '-' means stdout)\n\
           --start-line N            first physical input line, inclusive (default: 1)\n\
           --end-line N              last physical input line, inclusive (default: all)\n\
           --max-parents N           canonical parents selected after global deduplication\n\
           --solution-cap N          exact-count threshold; emit only counts below N (default: 184)\n\
           --deletions MODE          long-terminals (default) or all-cells\n\
           --progress-every N        stderr progress interval; 0 disables (default: 1000)\n\
           --stop-on-first           stop classification after the first exact unique state\n\
           -h, --help                show this help"
    );
}

fn validate_input_output_distinct(options: &Options) -> Result<(), String> {
    let Some(output) = options.output.as_ref() else {
        return Ok(());
    };
    let input = fs::canonicalize(&options.input)
        .map_err(|error| format!("cannot resolve input {}: {error}", options.input.display()))?;
    let output = comparable_output_path(output)?;
    if paths_equal(&input, &output) {
        return Err(format!(
            "refusing input/output alias: {} and {} resolve to the same path",
            options.input.display(),
            options.output.as_ref().expect("present").display()
        ));
    }
    if options.output.as_ref().is_some_and(|path| path.exists()) {
        return Err(format!(
            "refusing to overwrite existing output {}; choose a new path",
            options.output.as_ref().expect("present").display()
        ));
    }
    Ok(())
}

fn binary_provenance() -> Result<BinaryProvenance, String> {
    let path = env::current_exe()
        .map_err(|error| format!("cannot resolve the running executable: {error}"))?;
    let bytes = fs::read(&path)
        .map_err(|error| format!("cannot read running executable {}: {error}", path.display()))?;
    Ok(BinaryProvenance {
        path,
        bytes: bytes.len(),
        sha256: sha256_hex(&bytes),
    })
}

fn comparable_output_path(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return fs::canonicalize(path)
            .map_err(|error| format!("cannot resolve output {}: {error}", path.display()));
    }
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir()
            .map_err(|error| format!("cannot determine current directory: {error}"))?
            .join(path)
    };
    let parent = absolute
        .parent()
        .ok_or_else(|| format!("output {} has no parent directory", path.display()))?;
    let file_name = absolute
        .file_name()
        .ok_or_else(|| format!("output {} has no file name", path.display()))?;
    let parent = fs::canonicalize(parent).map_err(|error| {
        format!(
            "cannot resolve output directory {}: {error}",
            parent.display()
        )
    })?;
    Ok(parent.join(file_name))
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn parent_sha256(parent: &ParentLayout) -> String {
    let mut bytes = b"thermo-18c-parent-v1\0".to_vec();
    for path in [&parent.path9, &parent.path8, &parent.path2] {
        bytes.push(path.len() as u8);
        bytes.extend_from_slice(path);
    }
    sha256_hex(&bytes)
}

fn is_project_v1_input(input_sha256: &str) -> bool {
    input_sha256 == PROJECT_V1_INPUT_SHA256
}

fn emit_output(
    options: &Options,
    input_bytes: &[u8],
    input_sha256: &str,
    input_fnv64: u64,
    binary: &BinaryProvenance,
    outcome: &HarvestOutcome,
    elapsed_seconds: f64,
) -> Result<(), String> {
    let context = ReportContext {
        options,
        input_bytes,
        input_sha256,
        input_fnv64,
        binary,
        outcome,
        elapsed_seconds,
    };
    if let Some(path) = options.output.as_ref() {
        atomic_write(path, |writer| write_jsonl_records(writer, &context))
    } else {
        let stdout = std::io::stdout();
        let mut writer = BufWriter::new(stdout.lock());
        write_jsonl_records(&mut writer, &context)?;
        writer
            .flush()
            .map_err(|error| format!("cannot flush stdout: {error}"))
    }
}

fn atomic_write<F>(destination: &Path, write: F) -> Result<(), String>
where
    F: FnOnce(&mut BufWriter<File>) -> Result<(), String>,
{
    if destination.exists() {
        return Err(format!(
            "refusing to overwrite existing output {}; concurrent writers are unsupported",
            destination.display()
        ));
    }
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| format!("output {} has no file name", destination.display()))?
        .to_string_lossy();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut temporary = None;
    let mut file = None;
    for attempt in 0..100u32 {
        let path = parent.join(format!(
            ".{file_name}.tmp-{}-{stamp}-{attempt}",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(created) => {
                temporary = Some(path);
                file = Some(created);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!(
                    "cannot create temporary output in {}: {error}",
                    parent.display()
                ));
            }
        }
    }
    let temporary = temporary.ok_or_else(|| {
        format!(
            "cannot allocate a unique temporary output in {}",
            parent.display()
        )
    })?;
    let file = file.expect("temporary path and file are created together");
    let mut writer = BufWriter::new(file);
    if let Err(error) = write(&mut writer) {
        drop(writer);
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = writer.flush() {
        drop(writer);
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "cannot flush temporary output {}: {error}",
            temporary.display()
        ));
    }
    if let Err(error) = writer.get_ref().sync_all() {
        drop(writer);
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "cannot sync temporary output {}: {error}",
            temporary.display()
        ));
    }
    drop(writer);
    // Publishing a same-directory hard link is atomic and cannot replace an
    // existing destination, even if another writer wins the final race.
    if let Err(error) = fs::hard_link(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "cannot atomically publish {} as {} without overwriting: {error}",
            temporary.display(),
            destination.display()
        ));
    }
    fs::remove_file(&temporary).map_err(|error| {
        format!(
            "published {} but cannot remove temporary link {}: {error}",
            destination.display(),
            temporary.display()
        )
    })?;
    Ok(())
}

fn write_jsonl_records<W: Write>(
    writer: &mut W,
    context: &ReportContext<'_>,
) -> Result<(), String> {
    write_json_line(
        writer,
        &header_json(
            context.options,
            context.input_bytes,
            context.input_sha256,
            context.input_fnv64,
            context.binary,
            context.outcome,
        ),
    )?;
    for invalid in &context.outcome.invalid {
        write_json_line(writer, &invalid_json(invalid))?;
    }
    for seed in &context.outcome.seeds {
        write_json_line(writer, &seed_json(seed, context.options.solution_cap))?;
    }
    write_json_line(
        writer,
        &summary_json(
            context.options,
            context.input_sha256,
            context.input_fnv64,
            context.outcome,
            context.elapsed_seconds,
        ),
    )
}

fn write_json_line<W: Write>(writer: &mut W, json: &str) -> Result<(), String> {
    writer
        .write_all(json.as_bytes())
        .and_then(|_| writer.write_all(b"\n"))
        .map_err(|error| format!("cannot write JSONL output: {error}"))
}

fn header_json(
    options: &Options,
    input_bytes: &[u8],
    input_sha256: &str,
    input_fnv64: u64,
    binary: &BinaryProvenance,
    outcome: &HarvestOutcome,
) -> String {
    let end_line = if options.end_line == usize::MAX {
        "null".to_owned()
    } else {
        options.end_line.to_string()
    };
    let max_parents = options
        .max_parents
        .map_or_else(|| "null".to_owned(), |value| value.to_string());
    let project_v1_input = is_project_v1_input(input_sha256);
    let held_out = if project_v1_input {
        "[\"Philip Newman corpora\",\"Denis Berthier 18c collection\",\"pictured 9+9 11-solution puzzle\"]"
    } else {
        "[]"
    };
    format!(
        concat!(
            "{{\"type\":\"header\",\"schema\":{schema},",
            "\"algorithm_revision\":{revision},",
            "\"scope\":{{\"kind\":\"constructive-corpus-relative\",",
            "\"global_18c_exhaustive\":false,",
            "\"claim\":\"low-count seed construction only\"}},",
            "\"model\":{{\"covered_cells\":18,",
            "\"constraints\":\"directed unequal king-neighbour comparisons\",",
            "\"overlap_and_branching\":true,",
            "\"coverage_rule\":\"all 18 cells incident in saturated graph\"}},",
            "\"input\":{{\"path\":{path},\"bytes\":{bytes},",
            "\"sha256\":{sha},\"fnv1a64\":\"{fnv:016x}\",",
            "\"physical_lines\":{physical},\"nonempty_lines\":{nonempty},",
            "\"matches_project_v1_independent_source\":{project_v1_input}}},",
            "\"binary\":{{\"path\":{binary_path},\"bytes\":{binary_bytes},",
            "\"sha256\":{binary_sha}}},",
            "\"selection\":{{\"start_line\":{start},\"end_line\":{end},",
            "\"max_parents\":{max_parents},\"deletions\":{deletions},",
            "\"solution_cap\":{cap},\"stop_on_first\":{stop}}},",
            "\"method\":{{",
            "\"parent_validation\":\"exact disjoint 9+8+2 geometry plus unified Solver construction\",",
            "\"parent_solution_enumeration\":\"declared count as prefix limit; accept only exhausted matching enumeration; targets lexicographically sorted\",",
            "\"saturation\":\"all target-true unequal king-neighbour comparisons on the post-deletion footprint\",",
            "\"normal_form\":\"unique transitive reduction (Hasse DAG)\",",
            "\"canonicalization\":\"D4 spatial orbit times digit complement/global edge reversal\",",
            "\"network_equality\":\"exact canonical Hasse edge vector; hashes are identifiers only\",",
            "\"classification\":\"global collection and exact-vector deduplication before sorted capped counting\",",
            "\"emission\":\"only uncapped exact counts strictly below solution_cap\",",
            "\"output_policy\":\"same-directory temporary file, sync, atomic hard-link publication without overwrite; concurrent writers unsupported\"}},",
            "\"cell_encoding\":\"zero-based 9*r+c\",",
            "\"edge_encoding\":\"[lower,upper] means digit(lower)<digit(upper)\",",
            "\"independence\":{{\"asserted_for_this_input\":{project_v1_input},",
            "\"held_out_from_seed_generation\":{held_out}}},",
            "\"accounting_snapshot\":{{\"unique_parents_in_range\":{parents},",
            "\"invalid_records\":{invalid},\"canonical_networks\":{networks}}}}}"
        ),
        schema = json_quote(SCHEMA),
        revision = json_quote(ALGORITHM_REVISION),
        path = json_quote(&options.input.to_string_lossy()),
        bytes = input_bytes.len(),
        sha = json_quote(input_sha256),
        fnv = input_fnv64,
        physical = outcome.accounting.physical_lines,
        nonempty = outcome.accounting.nonempty_lines,
        binary_path = json_quote(&binary.path.to_string_lossy()),
        binary_bytes = binary.bytes,
        binary_sha = json_quote(&binary.sha256),
        project_v1_input = project_v1_input,
        held_out = held_out,
        start = options.start_line,
        end = end_line,
        max_parents = max_parents,
        deletions = json_quote(options.deletions.as_str()),
        cap = options.solution_cap,
        stop = options.stop_on_first,
        parents = outcome.accounting.unique_parents_in_range,
        invalid = outcome.invalid.len(),
        networks = outcome.networks.len(),
    )
}

fn invalid_json(record: &InvalidRecord) -> String {
    format!(
        "{{\"type\":\"invalid\",\"schema\":{schema},\"lines\":{lines},\"stage\":{stage},\"message\":{message}}}",
        schema = json_quote(SCHEMA),
        lines = usize_list_json(&record.lines),
        stage = json_quote(record.stage),
        message = json_quote(&record.message),
    )
}

fn seed_json(seed: &SeedRecord, solution_cap: u64) -> String {
    let provenance = &seed.network.provenance;
    let state = &seed.network.state;
    format!(
        "{{\"type\":\"seed\",\"schema\":{schema},\"ordinal\":{ordinal},\"network_sha256\":{network_sha},\"state_sha256\":{state_sha},\"solution_count\":{count},\"exact\":true,\"capped\":false,\"solution_cap\":{cap},\"occurrences_before_network_dedup\":{occurrences},\"canonical_cells\":{cells},\"full_saturated_edges\":{full},\"hasse_edges\":{hasse},\"canonical_target\":{target},\"first_witness\":{first},\"second_witness\":{second},\"canonical_transform\":{{\"spatial\":{spatial},\"digit_complement_and_edge_reversal\":{complement}}},\"provenance\":{{\"parent_sha256\":{parent_sha},\"source_lines\":{source_lines},\"declared_parent_solutions\":{declared},\"target_ordinal_after_lexicographic_sort\":{target_ordinal},\"deletion\":{{\"scope_path\":{path_name},\"path_index\":{path_index},\"position\":{position},\"cell\":{deleted_cell}}},\"canonical_parent_before_state_transform\":{parent}}},\"solver_stats\":{stats}}}",
        schema = json_quote(SCHEMA),
        ordinal = seed.ordinal,
        network_sha = json_quote(&seed.network_sha256),
        state_sha = json_quote(&seed.state_sha256),
        count = seed.count,
        cap = solution_cap,
        occurrences = seed.network.occurrences,
        cells = u8_list_json(&state.cells),
        full = edges_json(&state.full_edges),
        hasse = edges_json(&state.hasse_edges),
        target = json_quote(&grid_string(&state.target)),
        first = optional_grid_json(seed.first_solution.as_ref()),
        second = optional_grid_json(seed.second_solution.as_ref()),
        spatial = state.transform.spatial,
        complement = state.transform.complement,
        parent_sha = json_quote(&provenance.parent_sha256),
        source_lines = usize_list_json(&provenance.source_lines),
        declared = provenance.declared_parent_solutions,
        target_ordinal = provenance.target_ordinal,
        path_name = json_quote(provenance.deletion.path_name()),
        path_index = provenance.deletion.path_index,
        position = provenance.deletion.position,
        deleted_cell = provenance.deletion.cell,
        parent = parent_json(&provenance.parent),
        stats = solve_stats_json(seed.stats),
    )
}

fn summary_json(
    options: &Options,
    input_sha256: &str,
    input_fnv64: u64,
    outcome: &HarvestOutcome,
    elapsed_seconds: f64,
) -> String {
    let accounting = &outcome.accounting;
    let classification_complete = accounting.networks_classified == accounting.canonical_networks;
    let selected_parent_scope_complete = accounting.parents_enumerated
        == accounting.selected_parents
        && accounting.parents_verified == accounting.selected_parents
        && accounting.parent_count_mismatches == 0;
    let full_input_parent_scope =
        options.start_line == 1 && options.end_line == usize::MAX && options.max_parents.is_none();
    let status = if accounting.stopped_on_first {
        "stopped-on-first-unique"
    } else if selected_parent_scope_complete && classification_complete {
        "selected-scope-complete"
    } else {
        "selected-scope-incomplete"
    };
    let best = outcome
        .count_distribution
        .keys()
        .next()
        .map_or_else(|| "null".to_owned(), |value| value.to_string());
    format!(
        "{{\"type\":\"summary\",\"schema\":{schema},\"status\":{status},\"input_sha256\":{sha},\"input_fnv1a64\":\"{fnv:016x}\",\"scope\":{{\"constructive_corpus_relative\":true,\"global_18c_exhaustive\":false,\"full_input_parent_scope\":{full_scope},\"selected_parent_scope_complete\":{parent_complete},\"classification_complete\":{classification_complete}}},\"accounting\":{accounting},\"count_distribution_exact_below_cap\":{distribution},\"best_exact_count\":{best},\"solution_cap\":{cap},\"elapsed_seconds\":{elapsed:.6}}}",
        schema = json_quote(SCHEMA),
        status = json_quote(status),
        sha = json_quote(input_sha256),
        fnv = input_fnv64,
        full_scope = full_input_parent_scope,
        parent_complete = selected_parent_scope_complete,
        classification_complete = classification_complete,
        accounting = accounting_json(accounting),
        distribution = count_distribution_json(&outcome.count_distribution),
        best = best,
        cap = options.solution_cap,
        elapsed = elapsed_seconds,
    )
}

fn accounting_json(value: &Accounting) -> String {
    format!(
        "{{\"physical_lines\":{},\"nonempty_lines\":{},\"lines_in_range\":{},\"parsed_rows\":{},\"geometry_valid_rows\":{},\"malformed_rows\":{},\"duplicate_parent_rows\":{},\"unique_parents_in_range\":{},\"parents_with_conflicting_counts\":{},\"eligible_parents\":{},\"selected_parents\":{},\"parents_omitted_by_limit\":{},\"parents_enumerated\":{},\"parents_verified\":{},\"parent_count_mismatches\":{},\"target_solutions\":{},\"deletion_attempts\":{},\"exact_18_cell_footprints\":{},\"coverage_rejections\":{},\"raw_candidate_states\":{},\"canonical_networks\":{},\"duplicate_network_states\":{},\"networks_classified\":{},\"exact_network_counts\":{},\"capped_network_counts\":{},\"zero_solution_errors\":{},\"seed_records\":{},\"unique_seed_records\":{},\"invalid_records\":{},\"stopped_on_first\":{},\"parent_solver\":{},\"network_solver\":{}}}",
        value.physical_lines,
        value.nonempty_lines,
        value.lines_in_range,
        value.parsed_rows,
        value.geometry_valid_rows,
        value.malformed_rows,
        value.duplicate_parent_rows,
        value.unique_parents_in_range,
        value.parents_with_conflicting_counts,
        value.eligible_parents,
        value.selected_parents,
        value.parents_omitted_by_limit,
        value.parents_enumerated,
        value.parents_verified,
        value.parent_count_mismatches,
        value.target_solutions,
        value.deletion_attempts,
        value.exact_18_cell_footprints,
        value.coverage_rejections,
        value.raw_candidate_states,
        value.canonical_networks,
        value.duplicate_network_states,
        value.networks_classified,
        value.exact_network_counts,
        value.capped_network_counts,
        value.zero_solution_errors,
        value.seed_records,
        value.unique_seed_records,
        value.invalid_records,
        value.stopped_on_first,
        solver_totals_json(&value.parent_solver),
        solver_totals_json(&value.network_solver),
    )
}

fn solver_totals_json(value: &SolverTotals) -> String {
    format!(
        "{{\"calls\":{},\"nodes\":{},\"branches\":{},\"propagation_rounds\":{},\"comparison_revisions\":{},\"max_depth\":{}}}",
        value.calls,
        value.nodes,
        value.branches,
        value.propagation_rounds,
        value.comparison_revisions,
        value.max_depth,
    )
}

fn solve_stats_json(value: SolveStats) -> String {
    format!(
        "{{\"nodes\":{},\"branches\":{},\"propagation_rounds\":{},\"comparison_revisions\":{},\"max_depth\":{}}}",
        value.nodes,
        value.branches,
        value.propagation_rounds,
        value.thermo_revisions,
        value.max_depth,
    )
}

fn count_distribution_json(distribution: &BTreeMap<u64, u64>) -> String {
    let entries = distribution
        .iter()
        .map(|(count, frequency)| format!("{}:{}", json_quote(&count.to_string()), frequency))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{entries}}}")
}

fn parent_json(parent: &ParentLayout) -> String {
    format!(
        "[{}, {}, {}]",
        u8_list_json(&parent.path9),
        u8_list_json(&parent.path8),
        u8_list_json(&parent.path2)
    )
}

fn edges_json(edges: &[Edge]) -> String {
    format!(
        "[{}]",
        edges
            .iter()
            .map(|&(lower, upper)| format!("[{lower},{upper}]"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn u8_list_json(values: &[u8]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn usize_list_json(values: &[usize]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn grid_string(grid: &Grid) -> String {
    grid.iter().map(|digit| char::from(b'0' + *digit)).collect()
}

fn optional_grid_json(grid: Option<&Grid>) -> String {
    grid.map_or_else(|| "null".to_owned(), |grid| json_quote(&grid_string(grid)))
}

fn json_quote(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 2);
    result.push('"');
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{08}' => result.push_str("\\b"),
            '\u{0c}' => result.push_str("\\f"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character <= '\u{1f}' => {
                result.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => result.push(character),
        }
    }
    result.push('"');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST_PARENT: &str =
        "03;[(19, 29, 28, 20, 11, 12, 13, 3, 4), (77, 69, 78, 70, 62, 53, 44, 52), (41, 51)]";

    fn sample_parent() -> ParentLayout {
        let (_, text) = FIRST_PARENT.split_once(';').unwrap();
        normalize_and_validate_parent(parse_nested_paths(text).unwrap()).unwrap()
    }

    fn solved_grid() -> Grid {
        std::array::from_fn(|cell| {
            let row = cell / 9;
            let column = cell % 9;
            ((row * 3 + row / 3 + column) % 9 + 1) as u8
        })
    }

    fn test_options() -> Options {
        Options {
            input: PathBuf::from("fixture.txt"),
            output: None,
            start_line: 1,
            end_line: usize::MAX,
            max_parents: None,
            solution_cap: 4,
            deletions: DeletionScope::LongTerminals,
            progress_every: 0,
            stop_on_first: false,
        }
    }

    #[test]
    fn parser_accepts_parent_syntax_and_rejects_trailing_junk() {
        let (_, text) = FIRST_PARENT.split_once(';').unwrap();
        let paths = parse_nested_paths(text).unwrap();
        assert_eq!(paths.iter().map(Vec::len).collect::<Vec<_>>(), [9, 8, 2]);
        assert!(parse_nested_paths(&format!("{text} junk")).is_err());
        assert!(parse_nested_paths("[(0,1), nope]").is_err());
    }

    #[test]
    fn source_validation_requires_exact_disjoint_982_solver_geometry() {
        let parent = sample_parent();
        assert_eq!(parent.path9.len(), 9);
        assert_eq!(parent.path8.len(), 8);
        assert_eq!(parent.path2.len(), 2);
        assert_eq!(
            Solver::blank(&parent.paths())
                .unwrap()
                .layout()
                .covered_cells(),
            19
        );

        let mut overlap = parent.paths().to_vec();
        overlap[2][0] = overlap[0][0];
        assert!(normalize_and_validate_parent(overlap).is_err());
        let mut non_adjacent = parent.paths().to_vec();
        non_adjacent[2] = vec![0, 80];
        assert!(normalize_and_validate_parent(non_adjacent).is_err());
    }

    #[test]
    fn deletion_scopes_are_exact_and_deterministic() {
        let parent = sample_parent();
        let terminals = deletions_for(&parent, DeletionScope::LongTerminals);
        assert_eq!(terminals.len(), 4);
        assert_eq!(terminals[0].path_index, 0);
        assert_eq!(terminals[0].position, 0);
        assert_eq!(terminals[1].position as usize, parent.path9.len() - 1);
        assert_eq!(terminals[2].path_index, 1);
        assert_eq!(terminals[3].position as usize, parent.path8.len() - 1);
        assert!(
            terminals
                .iter()
                .all(|&deletion| footprint_after_deletion(&parent, deletion).len() == 18)
        );

        let all = deletions_for(&parent, DeletionScope::AllCells);
        assert_eq!(all.len(), 19);
        assert_eq!(
            all.iter()
                .map(|deletion| deletion.cell)
                .collect::<BTreeSet<_>>()
                .len(),
            19
        );
        assert!(
            all.iter()
                .all(|&deletion| footprint_after_deletion(&parent, deletion).len() == 18)
        );
    }

    #[test]
    fn saturation_is_target_true_king_local_and_covers_the_fixture() {
        let target = solved_grid();
        let footprint = (0u8..18).collect::<Vec<_>>();
        let edges = saturate_target(&footprint, &target);
        assert!(!edges.is_empty());
        assert_eq!(incident_cells(&edges), footprint);
        assert!(grid_satisfies_edges(&target, &edges));
        assert!(
            edges
                .iter()
                .all(|&(lower, upper)| king_adjacent(lower, upper))
        );
        assert!(
            edges
                .iter()
                .all(|&(lower, upper)| { target[lower as usize] != target[upper as usize] })
        );
    }

    #[test]
    fn transitive_reduction_preserves_closure_and_is_unique_chain() {
        let full = vec![(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
        let reduced = transitive_reduction(&full).unwrap();
        assert_eq!(reduced, vec![(0, 1), (1, 2), (2, 3)]);
        assert_eq!(
            transitive_closure_edges(&full).unwrap(),
            transitive_closure_edges(&reduced).unwrap()
        );
        assert!(transitive_reduction(&[(0, 1), (1, 0)]).is_err());
    }

    #[test]
    fn canonical_state_is_invariant_under_all_16_transforms() {
        let target = solved_grid();
        let cells = (0u8..18).collect::<Vec<_>>();
        let full = saturate_target(&cells, &target);
        let hasse = transitive_reduction(&full).unwrap();
        let baseline = canonicalize_state(&full, &hasse, &target, &cells);
        for complement in [false, true] {
            for spatial in 0..8 {
                let transform = Transform {
                    spatial,
                    complement,
                };
                let transformed_full = transform_edges(&full, transform);
                let transformed_hasse = transform_edges(&hasse, transform);
                let transformed_target = transform_grid(&target, transform);
                let mut transformed_cells = cells
                    .iter()
                    .map(|&cell| transform_cell(cell, spatial))
                    .collect::<Vec<_>>();
                transformed_cells.sort_unstable();
                let candidate = canonicalize_state(
                    &transformed_full,
                    &transformed_hasse,
                    &transformed_target,
                    &transformed_cells,
                );
                assert_eq!(candidate.hasse_edges, baseline.hasse_edges);
                assert_eq!(candidate.target, baseline.target);
                assert_eq!(candidate.full_edges, baseline.full_edges);
                assert_eq!(candidate.cells, baseline.cells);
            }
        }
    }

    #[test]
    fn complement_reverses_edges_and_digits() {
        let target = solved_grid();
        let transform = Transform {
            spatial: 0,
            complement: true,
        };
        assert_eq!(transform_edges(&[(0, 1)], transform), vec![(1, 0)]);
        let complement = transform_grid(&target, transform);
        assert_eq!(complement[0], 10 - target[0]);
        assert_eq!(complement[80], 10 - target[80]);
    }

    #[test]
    fn sha256_matches_published_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(fnv1a64(b"hello"), 0xa430d84680aabd0b);
    }

    #[test]
    fn exact_hasse_dedupe_is_order_independent() {
        let target = solved_grid();
        let cells = (0u8..18).collect::<Vec<_>>();
        let full = saturate_target(&cells, &target);
        let hasse = transitive_reduction(&full).unwrap();
        let state = canonicalize_state(&full, &hasse, &target, &cells);
        let parent = sample_parent();
        let make_provenance = |target_ordinal| Provenance {
            parent: parent.clone(),
            parent_sha256: parent_sha256(&parent),
            source_lines: vec![1],
            declared_parent_solutions: 3,
            target_ordinal,
            deletion: deletions_for(&parent, DeletionScope::LongTerminals)[0],
        };
        let build = |order: [usize; 2]| {
            let mut map: BTreeMap<Vec<Edge>, NetworkRecord> = BTreeMap::new();
            for ordinal in order {
                let provenance = make_provenance(ordinal);
                match map.entry(state.hasse_edges.clone()) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(NetworkRecord {
                            state: state.clone(),
                            provenance,
                            occurrences: 1,
                        });
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        entry.get_mut().observe(state.clone(), provenance);
                    }
                }
            }
            map
        };
        let forward = build([2, 1]);
        let reverse = build([1, 2]);
        assert_eq!(
            forward.keys().collect::<Vec<_>>(),
            reverse.keys().collect::<Vec<_>>()
        );
        assert_eq!(forward.values().next().unwrap().occurrences, 2);
        assert_eq!(
            forward.values().next().unwrap().provenance.target_ordinal,
            reverse.values().next().unwrap().provenance.target_ordinal
        );
        assert_eq!(
            forward.values().next().unwrap().provenance.target_ordinal,
            1
        );
    }

    #[test]
    fn small_end_to_end_run_has_balanced_accounting() {
        let input = format!("{FIRST_PARENT}\nnot-a-record\n");
        let outcome = harvest(input.as_bytes(), &test_options(), Instant::now()).unwrap();
        assert_eq!(outcome.accounting.parents_verified, 1);
        assert_eq!(outcome.accounting.target_solutions, 3);
        assert_eq!(outcome.accounting.deletion_attempts, 12);
        assert_eq!(outcome.accounting.malformed_rows, 1);
        assert_eq!(outcome.invalid.len(), 1);
        assert_eq!(
            outcome.accounting.raw_candidate_states,
            outcome.accounting.canonical_networks + outcome.accounting.duplicate_network_states
        );
        assert_eq!(
            outcome.accounting.networks_classified,
            outcome.accounting.canonical_networks
        );
        assert_eq!(
            outcome.accounting.exact_network_counts + outcome.accounting.capped_network_counts,
            outcome.accounting.networks_classified
        );
        assert!(
            outcome
                .seeds
                .iter()
                .all(|seed| seed.count < test_options().solution_cap)
        );
    }

    #[test]
    fn cap_boundary_and_parent_completion_are_reported_conservatively() {
        assert!(should_emit_seed(183, false, 184));
        assert!(!should_emit_seed(184, false, 184));
        assert!(!should_emit_seed(183, true, 184));

        let mut outcome = harvest(
            format!("{FIRST_PARENT}\n").as_bytes(),
            &test_options(),
            Instant::now(),
        )
        .unwrap();
        outcome.accounting.selected_parents = 2;
        outcome.accounting.parents_enumerated = 2;
        outcome.accounting.parents_verified = 1;
        outcome.accounting.parent_count_mismatches = 1;
        let summary = summary_json(&test_options(), PROJECT_V1_INPUT_SHA256, 0, &outcome, 0.0);
        assert!(summary.contains("\"selected_parent_scope_complete\":false"));
        assert!(summary.contains("\"status\":\"selected-scope-incomplete\""));
    }

    #[test]
    fn independence_declaration_is_bound_to_the_frozen_input_hash() {
        let outcome = harvest(
            format!("{FIRST_PARENT}\n").as_bytes(),
            &test_options(),
            Instant::now(),
        )
        .unwrap();
        let binary = BinaryProvenance {
            path: PathBuf::from("harvester"),
            bytes: 1,
            sha256: "00".repeat(32),
        };
        let bound = header_json(
            &test_options(),
            b"fixture",
            PROJECT_V1_INPUT_SHA256,
            0,
            &binary,
            &outcome,
        );
        assert!(bound.contains("\"asserted_for_this_input\":true"));
        assert!(bound.contains("Philip Newman corpora"));

        let unbound = header_json(
            &test_options(),
            b"fixture",
            &"11".repeat(32),
            0,
            &binary,
            &outcome,
        );
        assert!(unbound.contains("\"asserted_for_this_input\":false"));
        assert!(unbound.contains("\"held_out_from_seed_generation\":[]"));
        assert!(!unbound.contains("Philip Newman corpora"));
    }

    #[test]
    fn atomic_output_never_replaces_an_existing_file() {
        let directory = env::temp_dir().join(format!(
            "thermo-18c-seed-harvest-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let output = directory.join("result.jsonl");
        fs::write(&output, b"owned\n").unwrap();
        let error = atomic_write(&output, |writer| {
            writer.write_all(b"replacement\n").unwrap();
            Ok(())
        })
        .unwrap_err();
        assert!(error.contains("refusing to overwrite"));
        assert_eq!(fs::read(&output).unwrap(), b"owned\n");
        fs::remove_file(output).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn frozen_source_identity_and_parser_audit_are_stable() {
        let bytes = include_bytes!("../../../sources/min_thermos_9_8_2.txt");
        assert_eq!(bytes.len(), 108_967);
        assert_eq!(sha256_hex(bytes), PROJECT_V1_INPUT_SHA256);
        let (parents, invalid, accounting) = parse_parents(bytes, &test_options()).unwrap();
        assert_eq!(accounting.physical_lines, 1_280);
        assert_eq!(accounting.geometry_valid_rows, 1_279);
        assert_eq!(accounting.malformed_rows, 1);
        assert_eq!(accounting.duplicate_parent_rows, 165);
        assert_eq!(parents.len(), 1_114);
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0].lines, vec![1_192]);
        let unique_declared_sum = parents
            .values()
            .map(|parent| parent.rows[0].declared_count)
            .sum::<usize>();
        assert_eq!(unique_declared_sum, 84_531);
    }
}
