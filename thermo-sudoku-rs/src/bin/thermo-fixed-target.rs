// Exact hitting-set master for a fixed-target comparison CEGIS experiment.
//
// This deliberately does not call the thermo solver.  The current library
// requires cell-disjoint thermometer paths, while this pilot studies the
// relaxed universe of arbitrary (possibly overlapping) king-adjacent strict
// comparisons.  It includes an exact Sudoku oracle and can also consume
// externally supplied counterexample grids.

use std::collections::HashSet;
use std::env;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write as IoWrite};
use std::path::PathBuf;
use std::process::ExitCode;

const CELLS: usize = 81;
const MAX_BUDGET: usize = 16;
const MAX_EDGES: usize = 272;
const EDGE_WORDS: usize = MAX_EDGES.div_ceil(64);
const ALL_DIGITS: u16 = 0x01ff;
const ALL_HOUSES: u32 = (1u32 << 27) - 1;
const NO_CELL: u8 = u8::MAX;

const fn make_cell_house_bits() -> [u32; CELLS] {
    let mut bits = [0u32; CELLS];
    let mut cell = 0usize;
    while cell < CELLS {
        let row = cell / 9;
        let column = cell % 9;
        let box_index = (row / 3) * 3 + column / 3;
        bits[cell] = (1u32 << row) | (1u32 << (9 + column)) | (1u32 << (18 + box_index));
        cell += 1;
    }
    bits
}

const fn make_house_cells() -> [[u8; 9]; 27] {
    let mut cells = [[0u8; 9]; 27];
    let mut house = 0usize;
    while house < 27 {
        let mut position = 0usize;
        while position < 9 {
            cells[house][position] = match house {
                0..=8 => (house * 9 + position) as u8,
                9..=17 => (position * 9 + house - 9) as u8,
                _ => {
                    let box_index = house - 18;
                    let box_row = (box_index / 3) * 3;
                    let box_column = (box_index % 3) * 3;
                    ((box_row + position / 3) * 9 + box_column + position % 3) as u8
                }
            };
            position += 1;
        }
        house += 1;
    }
    cells
}

const fn push_peer(
    output: &mut [u8; 20],
    seen: &mut [bool; CELLS],
    count: &mut usize,
    cell: usize,
) {
    if !seen[cell] {
        seen[cell] = true;
        output[*count] = cell as u8;
        *count += 1;
    }
}

const fn make_peers() -> [[u8; 20]; CELLS] {
    let mut result = [[NO_CELL; 20]; CELLS];
    let mut cell = 0usize;
    while cell < CELLS {
        let row = cell / 9;
        let column = cell % 9;
        let mut seen = [false; CELLS];
        seen[cell] = true;
        let mut count = 0usize;
        let mut index = 0usize;
        while index < 9 {
            push_peer(&mut result[cell], &mut seen, &mut count, row * 9 + index);
            push_peer(&mut result[cell], &mut seen, &mut count, index * 9 + column);
            index += 1;
        }
        let box_row = (row / 3) * 3;
        let box_column = (column / 3) * 3;
        let mut dr = 0usize;
        while dr < 3 {
            let mut dc = 0usize;
            while dc < 3 {
                push_peer(
                    &mut result[cell],
                    &mut seen,
                    &mut count,
                    (box_row + dr) * 9 + box_column + dc,
                );
                dc += 1;
            }
            dr += 1;
        }
        assert!(count == 20);
        cell += 1;
    }
    result
}

const CELL_HOUSE_BITS: [u32; CELLS] = make_cell_house_bits();
const HOUSE_CELLS: [[u8; 9]; 27] = make_house_cells();
const PEERS: [[u8; 20]; CELLS] = make_peers();

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct EdgeSet([u64; EDGE_WORDS]);

impl EdgeSet {
    fn insert(&mut self, edge: usize) {
        self.0[edge / 64] |= 1u64 << (edge % 64);
    }

    fn contains(self, edge: usize) -> bool {
        self.0[edge / 64] & (1u64 << (edge % 64)) != 0
    }

    fn intersects(self, other: Self) -> bool {
        self.0
            .iter()
            .zip(other.0)
            .any(|(&left, right)| left & right != 0)
    }

    fn is_subset_of(self, other: Self) -> bool {
        self.0
            .iter()
            .zip(other.0)
            .all(|(&left, right)| left & !right == 0)
    }

    fn without(self, forbidden: Self) -> Self {
        let mut result = self;
        for (word, blocked) in result.0.iter_mut().zip(forbidden.0) {
            *word &= !blocked;
        }
        result
    }

    fn union_with(&mut self, other: Self) {
        for (word, addition) in self.0.iter_mut().zip(other.0) {
            *word |= addition;
        }
    }

    fn count(self) -> usize {
        self.0.iter().map(|word| word.count_ones() as usize).sum()
    }

    fn first(self) -> Option<usize> {
        self.iter().next()
    }

    fn iter(self) -> EdgeSetIter {
        EdgeSetIter {
            words: self.0,
            word: 0,
        }
    }
}

struct EdgeSetIter {
    words: [u64; EDGE_WORDS],
    word: usize,
}

impl Iterator for EdgeSetIter {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        while self.word < EDGE_WORDS {
            let bits = self.words[self.word];
            if bits != 0 {
                let bit = bits.trailing_zeros() as usize;
                self.words[self.word] &= self.words[self.word] - 1;
                return Some(self.word * 64 + bit);
            }
            self.word += 1;
        }
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Comparison {
    lower: u8,
    upper: u8,
}

#[derive(Clone, Debug)]
struct Alternative {
    grid: [u8; CELLS],
    cut: EdgeSet,
}

#[derive(Debug)]
struct Options {
    target: [u8; CELLS],
    direct_alternatives: Vec<String>,
    alternative_files: Vec<PathBuf>,
    budget: usize,
    output: Option<PathBuf>,
    summary_only: bool,
    cegis: bool,
    max_iterations: usize,
    oracle_node_limit: Option<u64>,
    oracle_batch: usize,
    master_node_limit: Option<u64>,
    checkpoint: Option<PathBuf>,
    progress_every: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CegisStatus {
    NotRun,
    RelaxedUnique,
    MasterExceedsBudget,
    Unseparable,
    OracleNodeLimit,
    MasterNodeLimit,
    IterationLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OracleStatus {
    Alternative,
    Unique,
    NodeLimit,
}

#[derive(Debug)]
struct OracleResult {
    status: OracleStatus,
    alternatives: Vec<[u8; CELLS]>,
    nodes: u64,
    exhausted: bool,
    node_limit_hit: bool,
}

#[derive(Debug)]
struct CegisRun {
    iteration: usize,
    cuts: usize,
    selected: Vec<usize>,
    master_nodes: u64,
    oracle_nodes: u64,
    oracle_status: OracleStatus,
    alternatives_added: usize,
    oracle_exhausted: bool,
    oracle_node_limit_hit: bool,
}

#[derive(Debug)]
struct CegisReport {
    status: CegisStatus,
    runs: Vec<CegisRun>,
    total_master_nodes: u64,
    total_oracle_nodes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SearchOutcome {
    Minimum,
    Feasible,
    NoSetWithinBudget,
    Unseparable,
    NodeLimit,
}

#[derive(Debug)]
struct BoundRun {
    bound: usize,
    nodes: u64,
}

#[derive(Debug)]
struct MasterResult {
    outcome: SearchOutcome,
    selected: Vec<usize>,
    unseparable_cut: Option<usize>,
    active_cut_ids: Vec<usize>,
    packing_cut_ids: Vec<usize>,
    packing_lower_bound: usize,
    coverage_lower_bound: usize,
    max_edge_cut_coverage: usize,
    certificate_lower_bound: usize,
    proved_by_search_lower_bound: Option<usize>,
    runs: Vec<BoundRun>,
    nodes: u64,
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
    let comparisons = candidate_comparisons(&options.target);
    let (mut alternative_grids, duplicates_ignored) = load_alternatives(&options)?;
    if options.cegis {
        add_adjacent_digit_swap_seeds(&options.target, &mut alternative_grids);
    }
    let mut alternatives = make_alternatives(&comparisons, alternative_grids);
    let (result, cegis_report) = if options.cegis {
        run_cegis(&options, &comparisons, &mut alternatives)?
    } else {
        let cuts = alternatives
            .iter()
            .map(|alternative| alternative.cut)
            .collect::<Vec<_>>();
        (
            solve_master(&cuts, comparisons.len(), options.budget),
            CegisReport {
                status: CegisStatus::NotRun,
                runs: Vec::new(),
                total_master_nodes: 0,
                total_oracle_nodes: 0,
            },
        )
    };
    let cuts = alternatives
        .iter()
        .map(|alternative| alternative.cut)
        .collect::<Vec<_>>();

    if matches!(
        result.outcome,
        SearchOutcome::Minimum | SearchOutcome::Feasible
    ) && !hits_every_cut(&result.selected, &cuts)
    {
        return Err("internal error: reported selection does not hit every trade cut".into());
    }

    if let Some(path) = &options.output {
        let certificate = format_certificate(
            &options,
            &comparisons,
            &alternatives,
            duplicates_ignored,
            &result,
            &cegis_report,
        );
        fs::write(path, &certificate)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        print_summary(&comparisons, &alternatives, &result, &cegis_report);
        println!("certificate={}", path.display());
    } else if options.summary_only {
        print_summary(&comparisons, &alternatives, &result, &cegis_report);
    } else {
        let certificate = format_certificate(
            &options,
            &comparisons,
            &alternatives,
            duplicates_ignored,
            &result,
            &cegis_report,
        );
        print!("{certificate}");
    }
    Ok(())
}

fn parse_options() -> Result<Options, String> {
    let mut args = env::args().skip(1);
    let mut target = None;
    let mut direct_alternatives = Vec::new();
    let mut alternative_files = Vec::new();
    let mut budget = MAX_BUDGET;
    let mut output = None;
    let mut summary_only = false;
    let mut cegis = false;
    let mut max_iterations = 1_000usize;
    let mut oracle_node_limit = Some(10_000_000u64);
    let mut oracle_batch = 128usize;
    let mut master_node_limit = Some(1_000_000u64);
    let mut checkpoint = None;
    let mut progress_every = 10usize;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--target" => {
                let text = args.next().ok_or("--target requires a solved grid")?;
                let grid = parse_grid(&text).map_err(|error| format!("invalid target: {error}"))?;
                validate_sudoku(&grid)
                    .map_err(|error| format!("invalid target Sudoku: {error}"))?;
                target = Some(grid);
            }
            "--alternative" => {
                direct_alternatives.push(args.next().ok_or("--alternative requires a solved grid")?)
            }
            "--alternatives" => alternative_files.push(PathBuf::from(
                args.next().ok_or("--alternatives requires a file path")?,
            )),
            "--budget" => {
                budget = args
                    .next()
                    .ok_or("--budget requires an integer from 0 through 16")?
                    .parse()
                    .map_err(|_| "invalid --budget")?;
                if budget > MAX_BUDGET {
                    return Err(format!("--budget must be at most {MAX_BUDGET}"));
                }
            }
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().ok_or("--output requires a file path")?,
                ));
            }
            "--summary-only" => summary_only = true,
            "--cegis" => cegis = true,
            "--max-iterations" => {
                max_iterations = args
                    .next()
                    .ok_or("--max-iterations requires a non-negative integer")?
                    .parse()
                    .map_err(|_| "invalid --max-iterations")?;
            }
            "--oracle-node-limit" => {
                let value: u64 = args
                    .next()
                    .ok_or("--oracle-node-limit requires a non-negative integer")?
                    .parse()
                    .map_err(|_| "invalid --oracle-node-limit")?;
                oracle_node_limit = (value != 0).then_some(value);
            }
            "--oracle-batch" => {
                oracle_batch = args
                    .next()
                    .ok_or("--oracle-batch requires a positive integer")?
                    .parse()
                    .map_err(|_| "invalid --oracle-batch")?;
                if oracle_batch == 0 {
                    return Err("--oracle-batch must be positive".into());
                }
            }
            "--master-node-limit" => {
                let value: u64 = args
                    .next()
                    .ok_or("--master-node-limit requires a non-negative integer")?
                    .parse()
                    .map_err(|_| "invalid --master-node-limit")?;
                master_node_limit = (value != 0).then_some(value);
            }
            "--checkpoint" => {
                checkpoint = Some(PathBuf::from(
                    args.next().ok_or("--checkpoint requires a file path")?,
                ));
            }
            "--progress-every" => {
                progress_every = args
                    .next()
                    .ok_or("--progress-every requires a positive integer")?
                    .parse()
                    .map_err(|_| "invalid --progress-every")?;
                if progress_every == 0 {
                    return Err("--progress-every must be positive".into());
                }
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }

    Ok(Options {
        target: target.ok_or("--target is required")?,
        direct_alternatives,
        alternative_files,
        budget,
        output,
        summary_only,
        cegis,
        max_iterations,
        oracle_node_limit,
        oracle_batch,
        master_node_limit,
        checkpoint,
        progress_every,
    })
}

fn print_help() {
    println!(
        "thermo-fixed-target --target GRID [OPTIONS]\n\
         \n\
         Exact hitting-set master for a fixed-target, relaxed comparison CEGIS\n\
         experiment. GRID is an 81-digit solved classic Sudoku. Candidate clues\n\
         are all strict king-adjacent comparisons true in the target; equal\n\
         target pairs are omitted. Comparisons may overlap.\n\
         \n\
         Options:\n\
           --alternative GRID   add one explicit alternative Sudoku (repeatable)\n\
           --alternatives FILE  add alternatives, one solved grid per line\n\
           --budget N           maximum comparisons to use (default 16, maximum 16)\n\
           --output FILE        write the checkable line certificate to FILE\n\
           --summary-only       suppress the potentially large line certificate\n\
           --cegis              alternate exact master and comparison-Sudoku oracle\n\
           --max-iterations N   cap oracle calls (default 1000)\n\
           --oracle-node-limit N cap nodes per oracle call (default 10000000; 0 unlimited)\n\
           --oracle-batch N      alternatives per master candidate (default 128)\n\
           --master-node-limit N cap fallback master nodes (default 1000000; 0 unlimited)\n\
           --checkpoint FILE    load/save counterexample grids for deterministic restart\n\
           --progress-every N   report every Nth CEGIS iteration (default 10)\n\
           -h, --help           show this help\n\
         \n\
         Without --cegis this is a standalone master over only the supplied\n\
         alternatives. With --cegis it uses its self-contained exact classic\n\
         Sudoku plus arbitrary-comparison oracle. Neither mode enforces\n\
         disjoint thermometer geometry."
    );
}

fn load_alternatives(options: &Options) -> Result<(Vec<[u8; CELLS]>, usize), String> {
    let mut inputs = options.direct_alternatives.clone();
    let mut paths = options.alternative_files.clone();
    if let Some(checkpoint) = &options.checkpoint
        && checkpoint.exists()
        && !paths.contains(checkpoint)
    {
        paths.push(checkpoint.clone());
    }
    for path in &paths {
        let contents = fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        for (line_index, line) in contents.lines().enumerate() {
            let line = line.trim();
            if let Some(checkpoint_target) = line.strip_prefix("# target=") {
                let checkpoint_target = parse_grid(checkpoint_target).map_err(|error| {
                    format!(
                        "{}:{} has an invalid checkpoint target: {error}",
                        path.display(),
                        line_index + 1
                    )
                })?;
                if checkpoint_target != options.target {
                    return Err(format!(
                        "{}:{} checkpoint target does not match --target",
                        path.display(),
                        line_index + 1
                    ));
                }
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let grid = parse_grid(line)
                .map_err(|error| format!("{}:{}: {error}", path.display(), line_index + 1))?;
            validate_sudoku(&grid).map_err(|error| {
                format!(
                    "{}:{} is not a solved classic Sudoku: {error}",
                    path.display(),
                    line_index + 1
                )
            })?;
            inputs.push(format_grid(&grid));
        }
    }

    let mut seen = HashSet::new();
    let mut grids = Vec::new();
    let mut duplicates = 0usize;
    for (index, text) in inputs.into_iter().enumerate() {
        let grid = parse_grid(&text)
            .map_err(|error| format!("invalid alternative {}: {error}", index + 1))?;
        validate_sudoku(&grid).map_err(|error| {
            format!("alternative {} is not a solved Sudoku: {error}", index + 1)
        })?;
        if grid == options.target {
            return Err(format!(
                "alternative {} is the target itself, not a counterexample",
                index + 1
            ));
        }
        if seen.insert(grid) {
            grids.push(grid);
        } else {
            duplicates += 1;
        }
    }
    Ok((grids, duplicates))
}

fn make_alternatives(comparisons: &[Comparison], grids: Vec<[u8; CELLS]>) -> Vec<Alternative> {
    grids
        .into_iter()
        .map(|grid| Alternative {
            cut: trade_cut(comparisons, &grid),
            grid,
        })
        .collect()
}

fn add_adjacent_digit_swap_seeds(target: &[u8; CELLS], alternatives: &mut Vec<[u8; CELLS]>) {
    let existing = std::mem::take(alternatives);
    let mut seeded = Vec::with_capacity(existing.len() + 8);
    let mut seen = HashSet::with_capacity(existing.len() + 8);
    for lower in 1..9 {
        let upper = lower + 1;
        let mut swapped = *target;
        for digit in &mut swapped {
            if *digit == lower {
                *digit = upper;
            } else if *digit == upper {
                *digit = lower;
            }
        }
        if seen.insert(swapped) {
            seeded.push(swapped);
        }
    }
    for grid in existing {
        if seen.insert(grid) {
            seeded.push(grid);
        }
    }
    *alternatives = seeded;
}

fn parse_grid(text: &str) -> Result<[u8; CELLS], String> {
    let mut digits = Vec::with_capacity(CELLS);
    for character in text.chars() {
        match character {
            '1'..='9' => digits.push(character as u8 - b'0'),
            '/' | '|' | ',' | ';' | ':' | '-' | '_' | '[' | ']' | '(' | ')' => {}
            character if character.is_whitespace() => {}
            '0' | '.' => return Err("solved grids cannot contain blanks".into()),
            _ => return Err(format!("unexpected character {character:?}")),
        }
    }
    if digits.len() != CELLS {
        return Err(format!("expected 81 digits, found {}", digits.len()));
    }
    let mut grid = [0u8; CELLS];
    grid.copy_from_slice(&digits);
    Ok(grid)
}

fn validate_sudoku(grid: &[u8; CELLS]) -> Result<(), String> {
    for row in 0..9 {
        validate_house((0..9).map(|column| row * 9 + column), grid)
            .map_err(|error| format!("row {}: {error}", row + 1))?;
    }
    for column in 0..9 {
        validate_house((0..9).map(|row| row * 9 + column), grid)
            .map_err(|error| format!("column {}: {error}", column + 1))?;
    }
    for box_row in 0..3 {
        for box_column in 0..3 {
            validate_house(
                (0..9).map(|offset| (box_row * 3 + offset / 3) * 9 + box_column * 3 + offset % 3),
                grid,
            )
            .map_err(|error| format!("box ({}, {}): {error}", box_row + 1, box_column + 1))?;
        }
    }
    Ok(())
}

fn validate_house(cells: impl Iterator<Item = usize>, grid: &[u8; CELLS]) -> Result<(), String> {
    let mut seen = 0u16;
    for cell in cells {
        let digit = grid[cell];
        if !(1..=9).contains(&digit) {
            return Err(format!("cell {} has digit {digit}", cell + 1));
        }
        let bit = 1u16 << (digit - 1);
        if seen & bit != 0 {
            return Err(format!("digit {digit} is repeated"));
        }
        seen |= bit;
    }
    Ok(())
}

fn candidate_comparisons(target: &[u8; CELLS]) -> Vec<Comparison> {
    let mut comparisons = Vec::with_capacity(MAX_EDGES);
    for left in 0..CELLS {
        for right in left + 1..CELLS {
            let row_distance = (left / 9).abs_diff(right / 9);
            let column_distance = (left % 9).abs_diff(right % 9);
            if row_distance > 1 || column_distance > 1 {
                continue;
            }
            match target[left].cmp(&target[right]) {
                std::cmp::Ordering::Less => comparisons.push(Comparison {
                    lower: left as u8,
                    upper: right as u8,
                }),
                std::cmp::Ordering::Greater => comparisons.push(Comparison {
                    lower: right as u8,
                    upper: left as u8,
                }),
                std::cmp::Ordering::Equal => {}
            }
        }
    }
    comparisons
}

fn trade_cut(comparisons: &[Comparison], alternative: &[u8; CELLS]) -> EdgeSet {
    let mut cut = EdgeSet::default();
    for (edge, comparison) in comparisons.iter().enumerate() {
        if alternative[comparison.lower as usize] >= alternative[comparison.upper as usize] {
            cut.insert(edge);
        }
    }
    cut
}

fn run_cegis(
    options: &Options,
    comparisons: &[Comparison],
    alternatives: &mut Vec<Alternative>,
) -> Result<(MasterResult, CegisReport), String> {
    let mut report = CegisReport {
        status: CegisStatus::IterationLimit,
        runs: Vec::new(),
        total_master_nodes: 0,
        total_oracle_nodes: 0,
    };
    let mut seen = alternatives
        .iter()
        .map(|alternative| alternative.grid)
        .collect::<HashSet<_>>();
    prepare_checkpoint(options, alternatives)?;

    for iteration in 0..options.max_iterations {
        let cuts = alternatives
            .iter()
            .map(|alternative| alternative.cut)
            .collect::<Vec<_>>();
        let master = solve_cegis_master(
            &cuts,
            comparisons.len(),
            options.budget,
            options.master_node_limit,
        );
        report.total_master_nodes += master.nodes;
        match master.outcome {
            SearchOutcome::NoSetWithinBudget => {
                report.status = CegisStatus::MasterExceedsBudget;
                return Ok((master, report));
            }
            SearchOutcome::Unseparable => {
                report.status = CegisStatus::Unseparable;
                return Ok((master, report));
            }
            SearchOutcome::NodeLimit => {
                report.status = CegisStatus::MasterNodeLimit;
                return Ok((master, report));
            }
            SearchOutcome::Minimum | SearchOutcome::Feasible => {}
        }

        let selected_comparisons = master
            .selected
            .iter()
            .map(|&edge| comparisons[edge])
            .collect::<Vec<_>>();
        let oracle = ComparisonOracle::new(&options.target, &selected_comparisons)
            .find_alternatives(options.oracle_node_limit, options.oracle_batch);
        let alternatives_added = oracle.alternatives.len();
        report.total_oracle_nodes += oracle.nodes;
        report.runs.push(CegisRun {
            iteration,
            cuts: alternatives.len(),
            selected: master.selected.clone(),
            master_nodes: master.nodes,
            oracle_nodes: oracle.nodes,
            oracle_status: oracle.status,
            alternatives_added,
            oracle_exhausted: oracle.exhausted,
            oracle_node_limit_hit: oracle.node_limit_hit,
        });
        if iteration % options.progress_every == 0 {
            eprintln!(
                "cegis iteration={iteration} cuts={} selected={} master_nodes={} oracle_nodes={} oracle={} added={alternatives_added}",
                alternatives.len(),
                master.selected.len(),
                master.nodes,
                oracle.nodes,
                oracle_status_label(oracle.status)
            );
        }

        match oracle.status {
            OracleStatus::Unique => {
                report.status = CegisStatus::RelaxedUnique;
                return Ok((master, report));
            }
            OracleStatus::NodeLimit => {
                report.status = CegisStatus::OracleNodeLimit;
                return Ok((master, report));
            }
            OracleStatus::Alternative => {
                let first_new_alternative = alternatives.len();
                let selected_set = set_from_edges(&master.selected);
                for grid in oracle.alternatives {
                    validate_sudoku(&grid).map_err(|error| {
                        format!("internal error: oracle returned invalid grid: {error}")
                    })?;
                    if grid == options.target
                        || selected_comparisons.iter().any(|comparison| {
                            grid[comparison.lower as usize] >= grid[comparison.upper as usize]
                        })
                    {
                        return Err(
                            "internal error: oracle returned a target or constraint violation"
                                .into(),
                        );
                    }
                    if !seen.insert(grid) {
                        return Err(
                            "internal error: oracle repeated an already cut alternative".into()
                        );
                    }
                    let cut = trade_cut(comparisons, &grid);
                    if cut.intersects(selected_set) {
                        return Err(
                            "internal error: oracle alternative violates a selected comparison"
                                .into(),
                        );
                    }
                    alternatives.push(Alternative { grid, cut });
                }
                append_checkpoint(options, &alternatives[first_new_alternative..])?;
            }
        }
    }

    // The last oracle call may have added a cut. Re-solve once so the final
    // certificate's selection and lower bounds refer to every saved witness.
    let cuts = alternatives
        .iter()
        .map(|alternative| alternative.cut)
        .collect::<Vec<_>>();
    let master = solve_cegis_master(
        &cuts,
        comparisons.len(),
        options.budget,
        options.master_node_limit,
    );
    report.total_master_nodes += master.nodes;
    report.status = match master.outcome {
        SearchOutcome::Minimum | SearchOutcome::Feasible => CegisStatus::IterationLimit,
        SearchOutcome::NoSetWithinBudget => CegisStatus::MasterExceedsBudget,
        SearchOutcome::Unseparable => CegisStatus::Unseparable,
        SearchOutcome::NodeLimit => CegisStatus::MasterNodeLimit,
    };
    Ok((master, report))
}

fn prepare_checkpoint(options: &Options, alternatives: &[Alternative]) -> Result<(), String> {
    let Some(path) = &options.checkpoint else {
        return Ok(());
    };
    // A normal restart loaded and validated this exact file already.  Leave it
    // untouched and append only newly found alternatives, so an interruption
    // cannot destroy a large working checkpoint during startup.
    if path.exists() {
        if options.direct_alternatives.is_empty() && options.alternative_files.is_empty() {
            return Ok(());
        }
        return Err(format!(
            "checkpoint {} already exists; do not combine an existing checkpoint with --alternative or --alternatives",
            path.display()
        ));
    }
    let file = fs::File::create(path)
        .map_err(|error| format!("cannot write checkpoint {}: {error}", path.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "# thermo-fixed-target-cegis-v1")
        .map_err(|error| format!("cannot write checkpoint {}: {error}", path.display()))?;
    writeln!(writer, "# target={}", format_grid(&options.target))
        .map_err(|error| format!("cannot write checkpoint {}: {error}", path.display()))?;
    write_checkpoint_grids(path, &mut writer, alternatives)?;
    writer
        .flush()
        .map_err(|error| format!("cannot flush checkpoint {}: {error}", path.display()))
}

fn append_checkpoint(options: &Options, alternatives: &[Alternative]) -> Result<(), String> {
    let Some(path) = &options.checkpoint else {
        return Ok(());
    };
    if alternatives.is_empty() {
        return Ok(());
    }
    let file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|error| format!("cannot append checkpoint {}: {error}", path.display()))?;
    let mut writer = BufWriter::new(file);
    write_checkpoint_grids(path, &mut writer, alternatives)?;
    writer
        .flush()
        .map_err(|error| format!("cannot flush checkpoint {}: {error}", path.display()))
}

fn write_checkpoint_grids(
    path: &std::path::Path,
    writer: &mut impl IoWrite,
    alternatives: &[Alternative],
) -> Result<(), String> {
    for alternative in alternatives {
        writeln!(writer, "{}", format_grid(&alternative.grid))
            .map_err(|error| format!("cannot write checkpoint {}: {error}", path.display()))?;
    }
    Ok(())
}

fn set_from_edges(edges: &[usize]) -> EdgeSet {
    let mut result = EdgeSet::default();
    for &edge in edges {
        result.insert(edge);
    }
    result
}

fn oracle_status_label(status: OracleStatus) -> &'static str {
    match status {
        OracleStatus::Alternative => "alternative",
        OracleStatus::Unique => "unique",
        OracleStatus::NodeLimit => "node-limit",
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct OracleWork {
    singles_low: u64,
    singles_high: u32,
    dirty_houses: u32,
    dirty_comparisons: u32,
}

impl OracleWork {
    fn add_single(&mut self, cell: usize) {
        if cell < 64 {
            self.singles_low |= 1u64 << cell;
        } else {
            self.singles_high |= 1u32 << (cell - 64);
        }
    }

    fn pop_single(&mut self) -> Option<usize> {
        if self.singles_low != 0 {
            let cell = self.singles_low.trailing_zeros() as usize;
            self.singles_low &= self.singles_low - 1;
            Some(cell)
        } else if self.singles_high != 0 {
            let cell = self.singles_high.trailing_zeros() as usize;
            self.singles_high &= self.singles_high - 1;
            Some(cell + 64)
        } else {
            None
        }
    }
}

struct ComparisonOracle<'a> {
    target: &'a [u8; CELLS],
    comparisons: &'a [Comparison],
    incident: [u32; CELLS],
    degree: [u8; CELLS],
    node_limit: Option<u64>,
    nodes: u64,
}

enum OracleSearch {
    Exhausted,
    BatchFull,
    NodeLimit,
}

impl<'a> ComparisonOracle<'a> {
    fn new(target: &'a [u8; CELLS], comparisons: &'a [Comparison]) -> Self {
        assert!(comparisons.len() <= MAX_BUDGET);
        let mut incident = [0u32; CELLS];
        let mut degree = [0u8; CELLS];
        for (index, comparison) in comparisons.iter().enumerate() {
            let bit = 1u32 << index;
            incident[comparison.lower as usize] |= bit;
            incident[comparison.upper as usize] |= bit;
            degree[comparison.lower as usize] += 1;
            degree[comparison.upper as usize] += 1;
        }
        Self {
            target,
            comparisons,
            incident,
            degree,
            node_limit: None,
            nodes: 0,
        }
    }

    fn find_alternatives(mut self, node_limit: Option<u64>, batch: usize) -> OracleResult {
        assert!(batch > 0);
        self.node_limit = node_limit;
        let mut work = OracleWork {
            dirty_houses: ALL_HOUSES,
            dirty_comparisons: if self.comparisons.is_empty() {
                0
            } else {
                (1u32 << self.comparisons.len()) - 1
            },
            ..OracleWork::default()
        };
        let mut state = [ALL_DIGITS; CELLS];
        // Static endpoint bounds make the first comparison revisions cheaper
        // and are exact consequences of a strict digit inequality.
        for comparison in self.comparisons {
            if !oracle_restrict(
                &self,
                &mut state,
                &mut work,
                comparison.lower as usize,
                ALL_DIGITS & !(1 << 8),
            ) || !oracle_restrict(
                &self,
                &mut state,
                &mut work,
                comparison.upper as usize,
                ALL_DIGITS & !1,
            ) {
                return OracleResult {
                    status: OracleStatus::Unique,
                    alternatives: Vec::new(),
                    nodes: self.nodes,
                    exhausted: true,
                    node_limit_hit: false,
                };
            }
        }
        let mut cell_order = std::array::from_fn(|cell| cell as u8);
        let mut alternatives = Vec::with_capacity(batch);
        let termination = self.search(state, work, &mut cell_order, &mut alternatives, batch);
        let status = if alternatives.is_empty() {
            match termination {
                OracleSearch::Exhausted => OracleStatus::Unique,
                OracleSearch::NodeLimit => OracleStatus::NodeLimit,
                OracleSearch::BatchFull => unreachable!("a full batch cannot be empty"),
            }
        } else {
            OracleStatus::Alternative
        };
        OracleResult {
            status,
            alternatives,
            nodes: self.nodes,
            exhausted: matches!(termination, OracleSearch::Exhausted),
            node_limit_hit: matches!(termination, OracleSearch::NodeLimit),
        }
    }

    fn search(
        &mut self,
        mut state: [u16; CELLS],
        mut work: OracleWork,
        cell_order: &mut [u8; CELLS],
        alternatives: &mut Vec<[u8; CELLS]>,
        batch: usize,
    ) -> OracleSearch {
        if self.node_limit.is_some_and(|limit| self.nodes >= limit) {
            return OracleSearch::NodeLimit;
        }
        self.nodes += 1;
        if !oracle_propagate(self, &mut state, &mut work) {
            return OracleSearch::Exhausted;
        }
        let Some(cell) = choose_oracle_branch_cell(&state, &self.degree, cell_order) else {
            let solution = domains_to_grid(&state);
            if &solution != self.target {
                alternatives.push(solution);
                if alternatives.len() == batch {
                    return OracleSearch::BatchFull;
                }
            }
            return OracleSearch::Exhausted;
        };

        let target_bit = bit_for_digit(self.target[cell]);
        let candidates = state[cell];
        let mut non_target = candidates & !target_bit;
        while non_target != 0 {
            let value = low_bit(non_target);
            non_target &= non_target - 1;
            let mut child = state;
            let mut child_work = OracleWork::default();
            if oracle_restrict(self, &mut child, &mut child_work, cell, value) {
                match self.search(child, child_work, cell_order, alternatives, batch) {
                    OracleSearch::BatchFull => return OracleSearch::BatchFull,
                    OracleSearch::NodeLimit => return OracleSearch::NodeLimit,
                    OracleSearch::Exhausted => {}
                }
            }
        }
        if candidates & target_bit != 0 {
            let mut child = state;
            let mut child_work = OracleWork::default();
            if oracle_restrict(self, &mut child, &mut child_work, cell, target_bit) {
                return self.search(child, child_work, cell_order, alternatives, batch);
            }
        }
        OracleSearch::Exhausted
    }
}

fn oracle_restrict(
    oracle: &ComparisonOracle<'_>,
    state: &mut [u16; CELLS],
    work: &mut OracleWork,
    cell: usize,
    allowed: u16,
) -> bool {
    let old = state[cell];
    let next = old & allowed;
    if next == 0 {
        return false;
    }
    if next == old {
        return true;
    }
    state[cell] = next;
    work.dirty_houses |= CELL_HOUSE_BITS[cell];
    work.dirty_comparisons |= oracle.incident[cell];
    if !old.is_power_of_two() && next.is_power_of_two() {
        work.add_single(cell);
    }
    true
}

fn oracle_propagate(
    oracle: &ComparisonOracle<'_>,
    state: &mut [u16; CELLS],
    work: &mut OracleWork,
) -> bool {
    loop {
        if let Some(cell) = work.pop_single() {
            let value = state[cell];
            for &peer in &PEERS[cell] {
                if !oracle_restrict(oracle, state, work, peer as usize, ALL_DIGITS & !value) {
                    return false;
                }
            }
            continue;
        }

        if work.dirty_comparisons != 0 {
            let index = work.dirty_comparisons.trailing_zeros() as usize;
            let bit = 1u32 << index;
            work.dirty_comparisons &= !bit;
            if !revise_comparison(oracle, state, work, oracle.comparisons[index]) {
                return false;
            }
            // A binary less-than revision reaches its own fixed point.
            work.dirty_comparisons &= !bit;
            continue;
        }

        if work.dirty_houses != 0 {
            let dirty_boxes = work.dirty_houses & (0x01ff << 18);
            let house = if dirty_boxes != 0 {
                dirty_boxes.trailing_zeros() as usize
            } else {
                work.dirty_houses.trailing_zeros() as usize
            };
            work.dirty_houses &= !(1u32 << house);
            if !revise_oracle_house(oracle, state, work, house) {
                return false;
            }
            continue;
        }
        return true;
    }
}

fn revise_comparison(
    oracle: &ComparisonOracle<'_>,
    state: &mut [u16; CELLS],
    work: &mut OracleWork,
    comparison: Comparison,
) -> bool {
    let lower = comparison.lower as usize;
    let upper = comparison.upper as usize;
    let lower_min = low_bit(state[lower]);
    if lower_min == 0 {
        return false;
    }
    let greater_than_lower = ALL_DIGITS & !(lower_min.wrapping_shl(1).wrapping_sub(1));
    if !oracle_restrict(oracle, state, work, upper, greater_than_lower) {
        return false;
    }
    let upper_max = high_bit(state[upper]);
    upper_max != 0 && oracle_restrict(oracle, state, work, lower, upper_max.wrapping_sub(1))
}

fn revise_oracle_house(
    oracle: &ComparisonOracle<'_>,
    state: &mut [u16; CELLS],
    work: &mut OracleWork,
    house: usize,
) -> bool {
    let mut once = 0u16;
    let mut twice = 0u16;
    for position in 0..9 {
        let domain = state[house_cell(house, position)];
        twice |= once & domain;
        once |= domain;
    }
    if once != ALL_DIGITS {
        return false;
    }
    let unique = once & !twice;
    if unique != 0 {
        for position in 0..9 {
            let cell = house_cell(house, position);
            let forced = state[cell] & unique;
            if forced != 0
                && (!forced.is_power_of_two()
                    || !oracle_restrict(oracle, state, work, cell, forced))
            {
                return false;
            }
        }
    }

    match house {
        0..=8 => {
            let row = house;
            let mut segments = [0u16; 3];
            for (stack, segment) in segments.iter_mut().enumerate() {
                for offset in 0..3 {
                    *segment |= state[row * 9 + stack * 3 + offset];
                }
            }
            for stack in 0..3 {
                let confined =
                    segments[stack] & !(segments[(stack + 1) % 3] | segments[(stack + 2) % 3]);
                if confined == 0 {
                    continue;
                }
                let box_row = (row / 3) * 3;
                for other_row in box_row..box_row + 3 {
                    if other_row == row {
                        continue;
                    }
                    for column in stack * 3..stack * 3 + 3 {
                        if !oracle_restrict(
                            oracle,
                            state,
                            work,
                            other_row * 9 + column,
                            ALL_DIGITS & !confined,
                        ) {
                            return false;
                        }
                    }
                }
            }
        }
        9..=17 => {
            let column = house - 9;
            let mut segments = [0u16; 3];
            for (band, segment) in segments.iter_mut().enumerate() {
                for offset in 0..3 {
                    *segment |= state[(band * 3 + offset) * 9 + column];
                }
            }
            for band in 0..3 {
                let confined =
                    segments[band] & !(segments[(band + 1) % 3] | segments[(band + 2) % 3]);
                if confined == 0 {
                    continue;
                }
                let box_column = (column / 3) * 3;
                for other_column in box_column..box_column + 3 {
                    if other_column == column {
                        continue;
                    }
                    for row in band * 3..band * 3 + 3 {
                        if !oracle_restrict(
                            oracle,
                            state,
                            work,
                            row * 9 + other_column,
                            ALL_DIGITS & !confined,
                        ) {
                            return false;
                        }
                    }
                }
            }
        }
        _ => {
            let box_index = house - 18;
            let box_row = (box_index / 3) * 3;
            let box_column = (box_index % 3) * 3;
            let mut mini_rows = [0u16; 3];
            let mut mini_columns = [0u16; 3];
            for dr in 0..3 {
                for dc in 0..3 {
                    let domain = state[(box_row + dr) * 9 + box_column + dc];
                    mini_rows[dr] |= domain;
                    mini_columns[dc] |= domain;
                }
            }
            for dr in 0..3 {
                let confined = mini_rows[dr] & !(mini_rows[(dr + 1) % 3] | mini_rows[(dr + 2) % 3]);
                if confined != 0 {
                    let row = box_row + dr;
                    for column in 0..9 {
                        if column / 3 == box_column / 3 {
                            continue;
                        }
                        if !oracle_restrict(
                            oracle,
                            state,
                            work,
                            row * 9 + column,
                            ALL_DIGITS & !confined,
                        ) {
                            return false;
                        }
                    }
                }
            }
            for dc in 0..3 {
                let confined =
                    mini_columns[dc] & !(mini_columns[(dc + 1) % 3] | mini_columns[(dc + 2) % 3]);
                if confined != 0 {
                    let column = box_column + dc;
                    for row in 0..9 {
                        if row / 3 == box_row / 3 {
                            continue;
                        }
                        if !oracle_restrict(
                            oracle,
                            state,
                            work,
                            row * 9 + column,
                            ALL_DIGITS & !confined,
                        ) {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

fn choose_oracle_branch_cell(
    state: &[u16; CELLS],
    degree: &[u8; CELLS],
    cell_order: &mut [u8; CELLS],
) -> Option<usize> {
    let mut resolved = 0usize;
    for scan in 0..CELLS {
        let cell = cell_order[scan] as usize;
        if state[cell].is_power_of_two() {
            cell_order.swap(resolved, scan);
            resolved += 1;
        }
    }
    if resolved == CELLS {
        return None;
    }
    let mut best_index = resolved;
    let mut best_size = u32::MAX;
    let mut best_degree = 0u8;
    for (index, &ordered_cell) in cell_order.iter().enumerate().skip(resolved) {
        let cell = ordered_cell as usize;
        let size = state[cell].count_ones();
        if size < best_size || (size == best_size && degree[cell] > best_degree) {
            best_index = index;
            best_size = size;
            best_degree = degree[cell];
        }
    }
    cell_order.swap(resolved, best_index);
    Some(cell_order[resolved] as usize)
}

fn domains_to_grid(state: &[u16; CELLS]) -> [u8; CELLS] {
    std::array::from_fn(|cell| state[cell].trailing_zeros() as u8 + 1)
}

fn bit_for_digit(digit: u8) -> u16 {
    1u16 << (digit - 1)
}

fn low_bit(mask: u16) -> u16 {
    mask & mask.wrapping_neg()
}

fn high_bit(mask: u16) -> u16 {
    if mask == 0 {
        0
    } else {
        1u16 << (u16::BITS - 1 - mask.leading_zeros())
    }
}

fn house_cell(house: usize, position: usize) -> usize {
    HOUSE_CELLS[house][position] as usize
}

/// Fast feasibility master used inside CEGIS.
///
/// Proving the minimum after every new cut is wasted work: CEGIS only needs a
/// set within the budget.  A deterministic maximum-coverage greedy pass is
/// normally sufficient.  If it exceeds the budget, one exact depth-bounded
/// decision search is attempted, subject to an explicit node cap.
fn solve_cegis_master(
    cuts: &[EdgeSet],
    edge_count: usize,
    budget: usize,
    node_limit: Option<u64>,
) -> MasterResult {
    if let Some(unseparable_cut) = cuts.iter().position(|cut| cut.count() == 0) {
        return MasterResult {
            outcome: SearchOutcome::Unseparable,
            selected: Vec::new(),
            unseparable_cut: Some(unseparable_cut),
            active_cut_ids: Vec::new(),
            packing_cut_ids: Vec::new(),
            packing_lower_bound: 0,
            coverage_lower_bound: 0,
            max_edge_cut_coverage: 0,
            certificate_lower_bound: 0,
            proved_by_search_lower_bound: None,
            runs: Vec::new(),
            nodes: 0,
        };
    }

    // Most CEGIS iterations only need a feasible set, not a reduced cut
    // family or a minimum proof. Try the full family first: this avoids the
    // quadratic subset-reduction cost once hundreds of thousands of explicit
    // counterexamples have accumulated.
    let (mut selected, maximum_coverage) =
        greedy_hitting_set_with_coverage(cuts, edge_count, budget + 1);
    if selected.len() <= budget {
        selected.sort_unstable();
        let coverage_lower_bound = if cuts.is_empty() {
            0
        } else {
            cuts.len().div_ceil(maximum_coverage)
        };
        let packing_cut_ids = greedy_disjoint_packing(cuts, EdgeSet::default());
        let packing_lower_bound = packing_cut_ids.len();
        return MasterResult {
            outcome: SearchOutcome::Feasible,
            selected,
            unseparable_cut: None,
            active_cut_ids: (0..cuts.len()).collect(),
            packing_cut_ids,
            packing_lower_bound,
            coverage_lower_bound,
            max_edge_cut_coverage: maximum_coverage,
            certificate_lower_bound: coverage_lower_bound.max(packing_lower_bound),
            proved_by_search_lower_bound: None,
            runs: Vec::new(),
            nodes: 0,
        };
    }

    let active_cut_ids = inclusion_minimal_cut_ids(cuts);
    let active_cuts = active_cut_ids
        .iter()
        .map(|&index| cuts[index])
        .collect::<Vec<_>>();
    let packing_active_ids = greedy_disjoint_packing(&active_cuts, EdgeSet::default());
    let packing_cut_ids = packing_active_ids
        .iter()
        .map(|&index| active_cut_ids[index])
        .collect::<Vec<_>>();
    let packing_lower_bound = packing_cut_ids.len();
    let (coverage_lower_bound, max_edge_cut_coverage) =
        coverage_lower_bound(&active_cuts, EdgeSet::default(), edge_count);
    let certificate_lower_bound = packing_lower_bound.max(coverage_lower_bound);

    let mut selected = greedy_hitting_set(&active_cuts, edge_count, budget + 1);
    if selected.len() <= budget {
        selected.sort_unstable();
        return MasterResult {
            outcome: SearchOutcome::Feasible,
            selected,
            unseparable_cut: None,
            active_cut_ids,
            packing_cut_ids,
            packing_lower_bound,
            coverage_lower_bound,
            max_edge_cut_coverage,
            certificate_lower_bound,
            proved_by_search_lower_bound: None,
            runs: Vec::new(),
            nodes: 0,
        };
    }

    if certificate_lower_bound > budget {
        return MasterResult {
            outcome: SearchOutcome::NoSetWithinBudget,
            selected: Vec::new(),
            unseparable_cut: None,
            active_cut_ids,
            packing_cut_ids,
            packing_lower_bound,
            coverage_lower_bound,
            max_edge_cut_coverage,
            certificate_lower_bound,
            proved_by_search_lower_bound: Some(budget + 1),
            runs: Vec::new(),
            nodes: 0,
        };
    }

    let mut search = ExactHittingSet::new(&active_cuts, edge_count, node_limit);
    let selected = search.find(budget);
    let nodes = search.nodes;
    let node_limit_hit = search.node_limit_hit;
    let (outcome, mut selected, proved_by_search_lower_bound) = if let Some(selected) = selected {
        (SearchOutcome::Feasible, selected, None)
    } else if node_limit_hit {
        (SearchOutcome::NodeLimit, Vec::new(), None)
    } else {
        (
            SearchOutcome::NoSetWithinBudget,
            Vec::new(),
            Some(budget + 1),
        )
    };
    selected.sort_unstable();
    MasterResult {
        outcome,
        selected,
        unseparable_cut: None,
        active_cut_ids,
        packing_cut_ids,
        packing_lower_bound,
        coverage_lower_bound,
        max_edge_cut_coverage,
        certificate_lower_bound,
        proved_by_search_lower_bound,
        runs: vec![BoundRun {
            bound: budget,
            nodes,
        }],
        nodes,
    }
}

fn greedy_hitting_set(cuts: &[EdgeSet], edge_count: usize, stop_after: usize) -> Vec<usize> {
    greedy_hitting_set_with_coverage(cuts, edge_count, stop_after).0
}

fn greedy_hitting_set_with_coverage(
    cuts: &[EdgeSet],
    edge_count: usize,
    stop_after: usize,
) -> (Vec<usize>, usize) {
    let mut selected = Vec::new();
    let mut selected_set = EdgeSet::default();
    let mut uncovered = vec![true; cuts.len()];
    let mut uncovered_count = cuts.len();
    let mut coverage = vec![0usize; edge_count];
    for cut in cuts {
        for edge in cut.iter().filter(|&edge| edge < edge_count) {
            coverage[edge] += 1;
        }
    }
    let maximum_initial_coverage = coverage.iter().copied().max().unwrap_or(0);

    while uncovered_count != 0 {
        let Some((edge, &best_coverage)) = coverage
            .iter()
            .enumerate()
            .filter(|(edge, _)| !selected_set.contains(*edge))
            .max_by_key(|(edge, count)| (**count, std::cmp::Reverse(*edge)))
        else {
            break;
        };
        if best_coverage == 0 {
            break;
        }
        selected_set.insert(edge);
        selected.push(edge);

        for (index, cut) in cuts.iter().enumerate() {
            if uncovered[index] && cut.contains(edge) {
                uncovered[index] = false;
                uncovered_count -= 1;
                for covered_edge in cut.iter().filter(|&covered_edge| covered_edge < edge_count) {
                    coverage[covered_edge] -= 1;
                }
            }
        }
        if selected.len() >= stop_after {
            break;
        }
    }
    (selected, maximum_initial_coverage)
}

fn solve_master(cuts: &[EdgeSet], edge_count: usize, budget: usize) -> MasterResult {
    if let Some(unseparable_cut) = cuts.iter().position(|cut| cut.count() == 0) {
        return MasterResult {
            outcome: SearchOutcome::Unseparable,
            selected: Vec::new(),
            unseparable_cut: Some(unseparable_cut),
            active_cut_ids: Vec::new(),
            packing_cut_ids: Vec::new(),
            packing_lower_bound: 0,
            coverage_lower_bound: 0,
            max_edge_cut_coverage: 0,
            certificate_lower_bound: 0,
            proved_by_search_lower_bound: None,
            runs: Vec::new(),
            nodes: 0,
        };
    }

    let active_cut_ids = inclusion_minimal_cut_ids(cuts);
    let active_cuts = active_cut_ids
        .iter()
        .map(|&index| cuts[index])
        .collect::<Vec<_>>();
    let packing_active_ids = greedy_disjoint_packing(&active_cuts, EdgeSet::default());
    let packing_cut_ids = packing_active_ids
        .iter()
        .map(|&index| active_cut_ids[index])
        .collect::<Vec<_>>();
    let packing_lower_bound = packing_cut_ids.len();
    let (coverage_lower_bound, max_edge_cut_coverage) =
        coverage_lower_bound(&active_cuts, EdgeSet::default(), edge_count);
    let certificate_lower_bound = packing_lower_bound.max(coverage_lower_bound);

    if active_cuts.is_empty() {
        return MasterResult {
            outcome: SearchOutcome::Minimum,
            selected: Vec::new(),
            unseparable_cut: None,
            active_cut_ids,
            packing_cut_ids,
            packing_lower_bound,
            coverage_lower_bound,
            max_edge_cut_coverage,
            certificate_lower_bound,
            proved_by_search_lower_bound: Some(0),
            runs: Vec::new(),
            nodes: 0,
        };
    }

    let mut runs = Vec::new();
    let mut total_nodes = 0u64;
    for bound in certificate_lower_bound..=budget {
        let mut search = ExactHittingSet::new(&active_cuts, edge_count, None);
        let selected = search.find(bound);
        total_nodes += search.nodes;
        runs.push(BoundRun {
            bound,
            nodes: search.nodes,
        });
        if let Some(mut selected) = selected {
            selected.sort_unstable();
            debug_assert!(hits_every_cut(&selected, cuts));
            return MasterResult {
                outcome: SearchOutcome::Minimum,
                proved_by_search_lower_bound: Some(selected.len()),
                selected,
                unseparable_cut: None,
                active_cut_ids,
                packing_cut_ids,
                packing_lower_bound,
                coverage_lower_bound,
                max_edge_cut_coverage,
                certificate_lower_bound,
                runs,
                nodes: total_nodes,
            };
        }
    }

    MasterResult {
        outcome: SearchOutcome::NoSetWithinBudget,
        selected: Vec::new(),
        unseparable_cut: None,
        active_cut_ids,
        packing_cut_ids,
        packing_lower_bound,
        coverage_lower_bound,
        max_edge_cut_coverage,
        certificate_lower_bound,
        proved_by_search_lower_bound: Some(budget + 1),
        runs,
        nodes: total_nodes,
    }
}

fn inclusion_minimal_cut_ids(cuts: &[EdgeSet]) -> Vec<usize> {
    let mut ordered = cuts
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, cut)| cut.count() != 0)
        .collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|(index, cut)| (cut.count(), *cut, *index));

    let mut kept: Vec<(usize, EdgeSet)> = Vec::new();
    for (index, cut) in ordered {
        if kept.iter().any(|(_, smaller)| smaller.is_subset_of(cut)) {
            continue;
        }
        kept.push((index, cut));
    }
    kept.into_iter().map(|(index, _)| index).collect()
}

fn greedy_disjoint_packing(cuts: &[EdgeSet], forbidden: EdgeSet) -> Vec<usize> {
    let mut ordered = cuts
        .iter()
        .copied()
        .enumerate()
        .map(|(index, cut)| {
            (
                cut.without(forbidden).count(),
                index,
                cut.without(forbidden),
            )
        })
        .collect::<Vec<_>>();
    ordered.sort_unstable();

    let mut used = EdgeSet::default();
    let mut packing = Vec::new();
    for (_, index, available) in ordered {
        if !available.intersects(used) {
            used.union_with(available);
            packing.push(index);
        }
    }
    packing
}

fn coverage_lower_bound(cuts: &[EdgeSet], forbidden: EdgeSet, edge_count: usize) -> (usize, usize) {
    if cuts.is_empty() {
        return (0, 0);
    }
    let mut coverage = vec![0usize; edge_count];
    for cut in cuts {
        for edge in cut
            .without(forbidden)
            .iter()
            .filter(|&edge| edge < edge_count)
        {
            coverage[edge] += 1;
        }
    }
    let maximum = coverage.into_iter().max().unwrap_or(0);
    if maximum == 0 {
        (usize::MAX, 0)
    } else {
        (cuts.len().div_ceil(maximum), maximum)
    }
}

struct ExactHittingSet<'a> {
    cuts: &'a [EdgeSet],
    edge_count: usize,
    nodes: u64,
    node_limit: Option<u64>,
    node_limit_hit: bool,
}

impl<'a> ExactHittingSet<'a> {
    fn new(cuts: &'a [EdgeSet], edge_count: usize, node_limit: Option<u64>) -> Self {
        Self {
            cuts,
            edge_count,
            nodes: 0,
            node_limit,
            node_limit_hit: false,
        }
    }

    fn find(&mut self, bound: usize) -> Option<Vec<usize>> {
        self.dfs(
            EdgeSet::default(),
            EdgeSet::default(),
            Vec::with_capacity(bound),
            bound,
        )
    }

    fn dfs(
        &mut self,
        mut selected_set: EdgeSet,
        forbidden: EdgeSet,
        mut selected: Vec<usize>,
        mut remaining: usize,
    ) -> Option<Vec<usize>> {
        if self.node_limit.is_some_and(|limit| self.nodes >= limit) {
            self.node_limit_hit = true;
            return None;
        }
        self.nodes += 1;

        loop {
            let mut all_hit = true;
            let mut forced = None;
            for &cut in self.cuts {
                if cut.intersects(selected_set) {
                    continue;
                }
                all_hit = false;
                let available = cut.without(forbidden);
                match available.count() {
                    0 => return None,
                    1 => {
                        forced = available.first();
                        break;
                    }
                    _ => {}
                }
            }
            if all_hit {
                return Some(selected);
            }
            let Some(edge) = forced else {
                break;
            };
            if remaining == 0 {
                return None;
            }
            debug_assert!(!selected_set.contains(edge));
            selected_set.insert(edge);
            selected.push(edge);
            remaining -= 1;
        }

        if remaining == 0 {
            return None;
        }

        let unhit = self
            .cuts
            .iter()
            .copied()
            .filter(|cut| !cut.intersects(selected_set))
            .collect::<Vec<_>>();
        let packing_bound = greedy_disjoint_packing(&unhit, forbidden).len();
        let (coverage_bound, _) = coverage_lower_bound(&unhit, forbidden, self.edge_count);
        if packing_bound.max(coverage_bound) > remaining {
            return None;
        }

        let pivot = unhit
            .iter()
            .copied()
            .min_by_key(|cut| (cut.without(forbidden).count(), cut.without(forbidden)))?;
        let mut choices = pivot.without(forbidden).iter().collect::<Vec<_>>();
        choices.sort_unstable_by_key(|&edge| {
            let coverage = unhit.iter().filter(|cut| cut.contains(edge)).count();
            (std::cmp::Reverse(coverage), edge)
        });

        // These branches partition all ways to hit the pivot: in the branch
        // for choice i, all earlier choices are forbidden and choice i is in.
        let mut prefix_forbidden = forbidden;
        for edge in choices {
            let mut next_selected_set = selected_set;
            next_selected_set.insert(edge);
            let mut next_selected = selected.clone();
            next_selected.push(edge);
            if let Some(solution) = self.dfs(
                next_selected_set,
                prefix_forbidden,
                next_selected,
                remaining - 1,
            ) {
                return Some(solution);
            }
            if self.node_limit_hit {
                return None;
            }
            prefix_forbidden.insert(edge);
        }
        None
    }
}

fn hits_every_cut(selected: &[usize], cuts: &[EdgeSet]) -> bool {
    let mut set = EdgeSet::default();
    for &edge in selected {
        set.insert(edge);
    }
    cuts.iter().all(|cut| cut.intersects(set))
}

fn format_certificate(
    options: &Options,
    comparisons: &[Comparison],
    alternatives: &[Alternative],
    duplicates_ignored: usize,
    result: &MasterResult,
    cegis: &CegisReport,
) -> String {
    let mut output = String::new();
    writeln!(output, "certificate_version=thermo-fixed-target-master-v1").unwrap();
    writeln!(output, "model=relaxed-overlapping-king-comparisons").unwrap();
    let fixed_target_complete = matches!(
        cegis.status,
        CegisStatus::RelaxedUnique | CegisStatus::MasterExceedsBudget | CegisStatus::Unseparable
    );
    writeln!(output, "target_scope=single-fixed-target").unwrap();
    writeln!(
        output,
        "scope={}",
        if fixed_target_complete {
            "all-classic-sudoku-alternatives-for-target"
        } else {
            "provided-alternatives-only"
        }
    )
    .unwrap();
    writeln!(output, "geometry_enforced=false").unwrap();
    writeln!(output, "cegis_status={}", cegis_status_label(cegis.status)).unwrap();
    writeln!(
        output,
        "cegis_fixed_target_conclusion={}",
        if fixed_target_complete {
            "true"
        } else {
            "false"
        }
    )
    .unwrap();
    writeln!(output, "global_19c_conclusion=false").unwrap();
    writeln!(
        output,
        "cegis_total_master_nodes={}",
        cegis.total_master_nodes
    )
    .unwrap();
    writeln!(
        output,
        "cegis_total_oracle_nodes={}",
        cegis.total_oracle_nodes
    )
    .unwrap();
    writeln!(
        output,
        "cegis_alternatives_added={}",
        cegis
            .runs
            .iter()
            .map(|run| run.alternatives_added)
            .sum::<usize>()
    )
    .unwrap();
    writeln!(output, "target={}", format_grid(&options.target)).unwrap();
    writeln!(output, "budget={}", options.budget).unwrap();
    writeln!(output, "oracle_batch={}", options.oracle_batch).unwrap();
    writeln!(output, "candidate_edges={}", comparisons.len()).unwrap();
    writeln!(output, "trade_cuts={}", alternatives.len()).unwrap();
    writeln!(
        output,
        "duplicate_alternatives_ignored={duplicates_ignored}"
    )
    .unwrap();
    writeln!(
        output,
        "active_cut_ids={}",
        join_usize(&result.active_cut_ids, ";")
    )
    .unwrap();
    writeln!(
        output,
        "packing_cut_ids={}",
        join_usize(&result.packing_cut_ids, ";")
    )
    .unwrap();
    writeln!(output, "packing_lower_bound={}", result.packing_lower_bound).unwrap();
    writeln!(
        output,
        "max_edge_cut_coverage={}",
        result.max_edge_cut_coverage
    )
    .unwrap();
    writeln!(
        output,
        "coverage_lower_bound={}",
        result.coverage_lower_bound
    )
    .unwrap();
    writeln!(
        output,
        "certificate_lower_bound={}",
        result.certificate_lower_bound
    )
    .unwrap();
    writeln!(output, "search_nodes={}", result.nodes).unwrap();
    match result.outcome {
        SearchOutcome::Minimum | SearchOutcome::Feasible => {
            writeln!(
                output,
                "result={}",
                if result.outcome == SearchOutcome::Minimum {
                    "minimum-over-provided-cuts"
                } else {
                    "feasible-over-provided-cuts"
                }
            )
            .unwrap();
            writeln!(
                output,
                "{}={}",
                if result.outcome == SearchOutcome::Minimum {
                    "minimum_size"
                } else {
                    "selected_size"
                },
                result.selected.len()
            )
            .unwrap();
            if let Some(lower_bound) = result.proved_by_search_lower_bound {
                writeln!(output, "proved_by_search_lower_bound={lower_bound}").unwrap();
            }
            writeln!(
                output,
                "selected_edge_ids={}",
                join_usize(&result.selected, ";")
            )
            .unwrap();
        }
        SearchOutcome::NoSetWithinBudget => {
            writeln!(output, "result=no-set-within-budget").unwrap();
            writeln!(
                output,
                "proved_by_search_lower_bound={}",
                result.proved_by_search_lower_bound.unwrap()
            )
            .unwrap();
            writeln!(output, "selected_edge_ids=").unwrap();
        }
        SearchOutcome::Unseparable => {
            writeln!(output, "result=unseparable-alternative").unwrap();
            writeln!(
                output,
                "unseparable_cut_id={}",
                result.unseparable_cut.unwrap()
            )
            .unwrap();
            writeln!(output, "selected_edge_ids=").unwrap();
        }
        SearchOutcome::NodeLimit => {
            writeln!(output, "result=master-node-limit").unwrap();
            writeln!(output, "selected_edge_ids=").unwrap();
        }
    }
    for run in &result.runs {
        writeln!(output, "search_run={},{}", run.bound, run.nodes).unwrap();
    }
    for run in &cegis.runs {
        writeln!(
            output,
            "cegis_run={},{},{},{},{},{},{},{},{}",
            run.iteration,
            run.cuts,
            join_usize(&run.selected, ";"),
            run.master_nodes,
            run.oracle_nodes,
            oracle_status_label(run.oracle_status),
            run.alternatives_added,
            run.oracle_exhausted,
            run.oracle_node_limit_hit
        )
        .unwrap();
    }
    for &edge in &result.selected {
        let comparison = comparisons[edge];
        writeln!(
            output,
            "selected_edge={edge},{},{},{},{}",
            comparison.lower,
            comparison.upper,
            options.target[comparison.lower as usize],
            options.target[comparison.upper as usize]
        )
        .unwrap();
    }
    for (edge, comparison) in comparisons.iter().enumerate() {
        writeln!(
            output,
            "edge={edge},{},{},{},{}",
            comparison.lower,
            comparison.upper,
            options.target[comparison.lower as usize],
            options.target[comparison.upper as usize]
        )
        .unwrap();
    }
    for (cut_id, alternative) in alternatives.iter().enumerate() {
        writeln!(
            output,
            "cut={cut_id},{},{}",
            format_grid(&alternative.grid),
            join_usize(&alternative.cut.iter().collect::<Vec<_>>(), ";")
        )
        .unwrap();
    }
    output
}

fn print_summary(
    comparisons: &[Comparison],
    alternatives: &[Alternative],
    result: &MasterResult,
    cegis: &CegisReport,
) {
    println!("mode=fixed-target-hitting-set-master");
    println!("candidate_edges={}", comparisons.len());
    println!("trade_cuts={}", alternatives.len());
    println!("certificate_lower_bound={}", result.certificate_lower_bound);
    println!("search_nodes={}", result.nodes);
    println!("cegis_status={}", cegis_status_label(cegis.status));
    println!("cegis_total_master_nodes={}", cegis.total_master_nodes);
    println!("cegis_total_oracle_nodes={}", cegis.total_oracle_nodes);
    println!(
        "cegis_alternatives_added={}",
        cegis
            .runs
            .iter()
            .map(|run| run.alternatives_added)
            .sum::<usize>()
    );
    match result.outcome {
        SearchOutcome::Minimum | SearchOutcome::Feasible => {
            println!(
                "result={}",
                if result.outcome == SearchOutcome::Minimum {
                    "minimum-over-provided-cuts"
                } else {
                    "feasible-over-provided-cuts"
                }
            );
            println!("selected_size={}", result.selected.len());
            println!("selected_edge_ids={}", join_usize(&result.selected, ";"));
        }
        SearchOutcome::NoSetWithinBudget => {
            println!("result=no-set-within-budget");
            println!(
                "proved_by_search_lower_bound={}",
                result.proved_by_search_lower_bound.unwrap()
            );
        }
        SearchOutcome::Unseparable => {
            println!("result=unseparable-alternative");
            println!("unseparable_cut_id={}", result.unseparable_cut.unwrap());
        }
        SearchOutcome::NodeLimit => println!("result=master-node-limit"),
    }
}

fn cegis_status_label(status: CegisStatus) -> &'static str {
    match status {
        CegisStatus::NotRun => "not-run",
        CegisStatus::RelaxedUnique => "relaxed-unique",
        CegisStatus::MasterExceedsBudget => "master-exceeds-budget",
        CegisStatus::Unseparable => "unseparable-alternative",
        CegisStatus::OracleNodeLimit => "oracle-node-limit",
        CegisStatus::MasterNodeLimit => "master-node-limit",
        CegisStatus::IterationLimit => "iteration-limit",
    }
}

fn format_grid(grid: &[u8; CELLS]) -> String {
    grid.iter().map(|digit| char::from(b'0' + *digit)).collect()
}

fn join_usize(values: &[usize], separator: &str) -> String {
    values
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(separator)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str =
        "326891745985674123714523869832769514697415238451238697243157986178946352569382471";

    fn set(edges: &[usize]) -> EdgeSet {
        let mut result = EdgeSet::default();
        for &edge in edges {
            result.insert(edge);
        }
        result
    }

    fn brute_force_minimum(cuts: &[EdgeSet], edge_count: usize) -> Option<usize> {
        if cuts.iter().any(|cut| cut.count() == 0) {
            return None;
        }
        for size in 0..=edge_count {
            for mask in 0u64..1u64 << edge_count {
                if mask.count_ones() as usize != size {
                    continue;
                }
                let selected = (0..edge_count)
                    .filter(|&edge| mask & (1u64 << edge) != 0)
                    .collect::<Vec<_>>();
                if hits_every_cut(&selected, cuts) {
                    return Some(size);
                }
            }
        }
        None
    }

    #[test]
    fn target_is_valid_and_has_expected_comparison_universe() {
        let target = parse_grid(TARGET).unwrap();
        validate_sudoku(&target).unwrap();
        assert_eq!(candidate_comparisons(&target).len(), 263);
    }

    #[test]
    fn digit_complement_violates_every_candidate_comparison() {
        let target = parse_grid(TARGET).unwrap();
        let alternative = target.map(|digit| 10 - digit);
        validate_sudoku(&alternative).unwrap();
        let comparisons = candidate_comparisons(&target);
        assert_eq!(
            trade_cut(&comparisons, &alternative).count(),
            comparisons.len()
        );
    }

    #[test]
    fn adjacent_digit_swaps_give_eight_disjoint_structural_cuts() {
        let target = parse_grid(TARGET).unwrap();
        let comparisons = candidate_comparisons(&target);
        let mut swaps = Vec::new();
        add_adjacent_digit_swap_seeds(&target, &mut swaps);
        assert_eq!(swaps.len(), 8);

        let cuts = swaps
            .iter()
            .map(|grid| {
                validate_sudoku(grid).unwrap();
                trade_cut(&comparisons, grid)
            })
            .collect::<Vec<_>>();
        assert!(cuts.iter().all(|cut| cut.count() > 0));
        for left in 0..cuts.len() {
            for right in left + 1..cuts.len() {
                assert!(!cuts[left].intersects(cuts[right]));
            }
        }

        let master = solve_cegis_master(&cuts, comparisons.len(), MAX_BUDGET, None);
        assert!(matches!(
            master.outcome,
            SearchOutcome::Minimum | SearchOutcome::Feasible
        ));
        assert_eq!(master.certificate_lower_bound, 8);
        assert_eq!(master.selected.len(), 8);
    }

    #[test]
    fn solves_small_masters_exactly() {
        let cuts = vec![set(&[0, 1]), set(&[1, 2])];
        let result = solve_master(&cuts, 3, 2);
        assert_eq!(result.outcome, SearchOutcome::Minimum);
        assert_eq!(result.selected, vec![1]);

        let triangle = vec![set(&[0, 1]), set(&[1, 2]), set(&[0, 2])];
        let result = solve_master(&triangle, 3, 3);
        assert_eq!(result.outcome, SearchOutcome::Minimum);
        assert_eq!(result.selected.len(), 2);
        assert!(hits_every_cut(&result.selected, &triangle));
        assert_eq!(result.coverage_lower_bound, 2);
    }

    #[test]
    fn recognizes_unseparable_alternative_and_budget_failure() {
        let impossible = solve_master(&[EdgeSet::default()], 3, 3);
        assert_eq!(impossible.outcome, SearchOutcome::Unseparable);

        let cuts = vec![set(&[0]), set(&[1])];
        let result = solve_master(&cuts, 2, 1);
        assert_eq!(result.outcome, SearchOutcome::NoSetWithinBudget);
        assert_eq!(result.proved_by_search_lower_bound, Some(2));
    }

    #[test]
    fn exact_search_matches_brute_force_on_deterministic_instances() {
        let mut state = 0x4d59_5df4_d0f3_3173u64;
        for _ in 0..128 {
            let edge_count = 6usize;
            let cut_count = 1 + (next_random(&mut state) as usize % 8);
            let mut cuts = Vec::new();
            for _ in 0..cut_count {
                let mask = 1 + (next_random(&mut state) as usize % ((1 << edge_count) - 1));
                cuts.push(set(&(0..edge_count)
                    .filter(|&edge| mask & (1 << edge) != 0)
                    .collect::<Vec<_>>()));
            }
            let expected = brute_force_minimum(&cuts, edge_count).unwrap();
            let result = solve_master(&cuts, edge_count, MAX_BUDGET);
            assert_eq!(result.outcome, SearchOutcome::Minimum);
            assert_eq!(result.selected.len(), expected);
            assert!(hits_every_cut(&result.selected, &cuts));
        }
    }

    fn next_random(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state
    }

    #[test]
    fn inclusion_reduction_keeps_an_equivalent_minimal_family() {
        let cuts = vec![set(&[0]), set(&[0, 1]), set(&[2, 3]), set(&[2, 3])];
        let active = inclusion_minimal_cut_ids(&cuts);
        assert_eq!(active, vec![0, 2]);
        let reduced = active.iter().map(|&index| cuts[index]).collect::<Vec<_>>();
        assert_eq!(
            brute_force_minimum(&cuts, 4),
            brute_force_minimum(&reduced, 4)
        );
    }

    #[test]
    fn cegis_master_uses_greedy_then_bounded_exact_fallback() {
        // Edge 2 is the unique greedy first choice (coverage four), but then
        // needs both 0 and 1. The optimum is {0, 1}.
        let cuts = vec![
            set(&[0, 2, 3]),
            set(&[0, 2, 4]),
            set(&[0, 5]),
            set(&[1, 2, 6]),
            set(&[1, 2, 7]),
            set(&[1, 8]),
        ];
        assert_eq!(greedy_hitting_set(&cuts, 9, 3).len(), 3);

        let capped = solve_cegis_master(&cuts, 9, 2, Some(0));
        assert_eq!(capped.outcome, SearchOutcome::NodeLimit);

        let solved = solve_cegis_master(&cuts, 9, 2, Some(10_000));
        assert_eq!(solved.outcome, SearchOutcome::Feasible);
        assert_eq!(solved.selected.len(), 2);
        assert!(hits_every_cut(&solved.selected, &cuts));

        let excluded = solve_cegis_master(&cuts, 9, 1, Some(10_000));
        assert_eq!(excluded.outcome, SearchOutcome::NoSetWithinBudget);
    }

    #[test]
    fn comparison_oracle_returns_a_valid_distinct_solution() {
        let target = parse_grid(TARGET).unwrap();
        let universe = candidate_comparisons(&target);
        let selected = [universe[0], universe[17], universe[101], universe[200]];
        let result = ComparisonOracle::new(&target, &selected).find_alternatives(Some(100_000), 1);
        assert_eq!(result.status, OracleStatus::Alternative);
        let alternative = result.alternatives[0];
        validate_sudoku(&alternative).unwrap();
        assert_ne!(alternative, target);
        for comparison in selected {
            assert!(
                alternative[comparison.lower as usize] < alternative[comparison.upper as usize]
            );
        }
    }

    #[test]
    fn comparison_oracle_honors_a_deterministic_node_limit() {
        let target = parse_grid(TARGET).unwrap();
        let result = ComparisonOracle::new(&target, &[]).find_alternatives(Some(1), 8);
        assert_eq!(result.status, OracleStatus::NodeLimit);
        assert_eq!(result.nodes, 1);
        assert!(result.alternatives.is_empty());
        assert!(result.node_limit_hit);
    }

    #[test]
    fn comparison_oracle_batches_distinct_valid_alternatives() {
        let target = parse_grid(TARGET).unwrap();
        let universe = candidate_comparisons(&target);
        let selected_ids = [3usize, 41, 97];
        let selected = selected_ids.map(|edge| universe[edge]);
        let selected_set = set_from_edges(&selected_ids);
        let result =
            ComparisonOracle::new(&target, &selected).find_alternatives(Some(100_000), 128);
        assert_eq!(result.status, OracleStatus::Alternative);
        assert_eq!(result.alternatives.len(), 128);
        assert!(!result.exhausted);
        assert!(!result.node_limit_hit);

        let distinct = result.alternatives.iter().copied().collect::<HashSet<_>>();
        assert_eq!(distinct.len(), result.alternatives.len());
        for alternative in result.alternatives {
            validate_sudoku(&alternative).unwrap();
            assert_ne!(alternative, target);
            for comparison in selected {
                assert!(
                    alternative[comparison.lower as usize] < alternative[comparison.upper as usize]
                );
            }
            assert!(!trade_cut(&universe, &alternative).intersects(selected_set));
        }
    }
}
