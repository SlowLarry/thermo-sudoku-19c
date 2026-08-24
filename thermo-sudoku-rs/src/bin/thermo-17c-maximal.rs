//! Exact catalogue search for merge-maximal thermo layouts covering 17 cells.
//!
//! A unique target induces a unique order of the nine source symbols.  Hence
//! every consecutive pair in that order must occur as a physical thermometer
//! edge; otherwise the pair can be swapped to obtain another solution.  The
//! eight distinguished edges form a nine-component forest on the 17 clue
//! occurrences.  All remaining 4--8-path layouts are obtained by adding only
//! one through five endpoint-to-endpoint component merges.
//!
//! We classify only merge-maximal layouts.  Adding a target-true bridge to a
//! unique layout preserves uniqueness, so any omitted non-maximal witness has
//! a unique maximal extension.  The already-excluded lower path strata are the
//! terminal boundary for this reduction.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use thermo_sudoku::Solver;

const SIDE: usize = 9;
const CELLS: usize = 81;
const CLUES: usize = 17;
const MAX_EDGES: usize = 14;
const DIRECTED_EDGES: usize = 544;
const EDGE_WORDS: usize = DIRECTED_EDGES.div_ceil(64);
const NO_VERTEX: u8 = u8::MAX;
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

    fn intersect(self, other: Self) -> Option<Self> {
        let mut result = self;
        let mut any = false;
        for (left, right) in result.0.iter_mut().zip(other.0) {
            *left &= right;
            any |= *left != 0;
        }
        any.then_some(result)
    }

    fn first(self) -> Option<usize> {
        self.0.iter().enumerate().find_map(|(word_index, word)| {
            (*word != 0).then(|| word_index * 64 + word.trailing_zeros() as usize)
        })
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
    occurrences: [Vec<u8>; SIDE],
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
        let mut occurrences: [Vec<u8>; SIDE] = std::array::from_fn(|_| Vec::new());
        let mut clue_count = 0usize;
        for (cell, byte) in encoded.bytes().enumerate() {
            match byte {
                b'.' | b'0' => {}
                b'1'..=b'9' => {
                    if clue_count == CLUES {
                        return Err(format!("line {line_number}: more than 17 clues"));
                    }
                    let digit = (byte - b'1') as usize;
                    cells[clue_count] = cell as u8;
                    digits[clue_count] = digit as u8;
                    occurrences[digit].push(clue_count as u8);
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
        })
    }

    fn eligible(&self, max_paths: usize) -> bool {
        self.occurrences
            .iter()
            .all(|vertices| !vertices.is_empty() && vertices.len() <= max_paths)
    }
}

#[derive(Clone, Debug)]
struct EdgeSupports {
    rows: [[MorphSet; CLUES]; CLUES],
    columns: [[MorphSet; CLUES]; CLUES],
}

impl EdgeSupports {
    fn new(puzzle: &Puzzle, axis: &AxisMorphs) -> Self {
        let mut rows = [[MorphSet::EMPTY; CLUES]; CLUES];
        let mut columns = [[MorphSet::EMPTY; CLUES]; CLUES];
        for left in 0..CLUES {
            for right in 0..CLUES {
                let left_cell = puzzle.cells[left] as usize;
                let right_cell = puzzle.cells[right] as usize;
                rows[left][right] = axis.close[left_cell / SIDE][right_cell / SIDE];
                columns[left][right] = axis.close[left_cell % SIDE][right_cell % SIDE];
            }
        }
        Self { rows, columns }
    }

    fn add_edge(
        &self,
        row_support: MorphSet,
        column_support: MorphSet,
        from: u8,
        to: u8,
    ) -> Option<(MorphSet, MorphSet)> {
        let rows = row_support.intersect(self.rows[from as usize][to as usize])?;
        let columns = column_support.intersect(self.columns[from as usize][to as usize])?;
        Some((rows, columns))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Forest {
    next: [u8; CLUES],
    previous: [u8; CLUES],
    edge_from: [u8; MAX_EDGES],
    edge_to: [u8; MAX_EDGES],
    edge_count: u8,
}

impl Forest {
    fn empty() -> Self {
        Self {
            next: [NO_VERTEX; CLUES],
            previous: [NO_VERTEX; CLUES],
            edge_from: [NO_VERTEX; MAX_EDGES],
            edge_to: [NO_VERTEX; MAX_EDGES],
            edge_count: 0,
        }
    }

    fn add_edge(mut self, from: u8, to: u8) -> Option<Self> {
        if from == to
            || self.next[from as usize] != NO_VERTEX
            || self.previous[to as usize] != NO_VERTEX
            || self.edge_count as usize == MAX_EDGES
        {
            return None;
        }
        let mut cursor = to;
        while cursor != NO_VERTEX {
            if cursor == from {
                return None;
            }
            cursor = self.next[cursor as usize];
        }
        self.next[from as usize] = to;
        self.previous[to as usize] = from;
        let edge = self.edge_count as usize;
        self.edge_from[edge] = from;
        self.edge_to[edge] = to;
        self.edge_count += 1;
        Some(self)
    }

    fn isolated_count(&self) -> usize {
        (0..CLUES)
            .filter(|&vertex| self.next[vertex] == NO_VERTEX && self.previous[vertex] == NO_VERTEX)
            .count()
    }

    fn components(&self, digits: &[u8; CLUES], rank: &[u8; SIDE]) -> Option<Vec<Component>> {
        let mut components = Vec::new();
        let mut visited = 0u32;
        for head in 0..CLUES {
            if self.previous[head] != NO_VERTEX {
                continue;
            }
            let mut cursor = head as u8;
            let mut tail = cursor;
            let mut size = 0u8;
            let mut digit_mask = 0u16;
            let mut minimum_rank = u8::MAX;
            let mut maximum_rank = 0u8;
            let mut previous_rank = None;
            while cursor != NO_VERTEX {
                let bit = 1u32 << cursor;
                if visited & bit != 0 {
                    return None;
                }
                visited |= bit;
                size += 1;
                if size > 9 {
                    return None;
                }
                let digit = digits[cursor as usize] as usize;
                let digit_bit = 1u16 << digit;
                if digit_mask & digit_bit != 0 {
                    return None;
                }
                digit_mask |= digit_bit;
                let current_rank = rank[digit];
                if previous_rank.is_some_and(|value| value >= current_rank) {
                    return None;
                }
                previous_rank = Some(current_rank);
                minimum_rank = minimum_rank.min(current_rank);
                maximum_rank = maximum_rank.max(current_rank);
                tail = cursor;
                cursor = self.next[cursor as usize];
            }
            components.push(Component {
                head: head as u8,
                tail,
                size,
                digit_mask,
                minimum_rank,
                maximum_rank,
            });
        }
        (visited.count_ones() as usize == CLUES).then_some(components)
    }

    fn vertex_paths(&self) -> Vec<Vec<u8>> {
        let mut paths = Vec::new();
        for head in 0..CLUES {
            if self.previous[head] != NO_VERTEX {
                continue;
            }
            let mut path = Vec::new();
            let mut cursor = head as u8;
            while cursor != NO_VERTEX {
                path.push(cursor);
                cursor = self.next[cursor as usize];
            }
            paths.push(path);
        }
        paths
    }

    fn edges(&self) -> impl Iterator<Item = (u8, u8)> + '_ {
        (0..self.edge_count as usize).map(|index| (self.edge_from[index], self.edge_to[index]))
    }
}

#[derive(Clone, Copy, Debug)]
struct Component {
    head: u8,
    tail: u8,
    size: u8,
    digit_mask: u16,
    minimum_rank: u8,
    maximum_rank: u8,
}

#[derive(Clone, Debug)]
struct Cover {
    paths: Vec<Vec<u8>>,
    digit_order: [u8; SIDE],
    row_morph: u16,
    column_morph: u16,
}

#[derive(Debug)]
struct Classification {
    count: u64,
    capped: bool,
    first_solution: [u8; 81],
    second_solution: Option<[u8; 81]>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct EdgeMask([u64; EDGE_WORDS]);

impl EdgeMask {
    const EMPTY: Self = Self([0; EDGE_WORDS]);

    fn insert(&mut self, edge: usize) {
        self.0[edge / 64] |= 1u64 << (edge % 64);
    }

    fn is_subset_of(self, other: Self) -> bool {
        self.0
            .iter()
            .zip(other.0)
            .all(|(selected, common)| selected & !common == 0)
    }

    fn count(self) -> u32 {
        self.0.iter().map(|word| word.count_ones()).sum()
    }
}

#[derive(Debug)]
struct DirectedEdgeUniverse {
    edges: Vec<(u8, u8)>,
    index: [[u16; CELLS]; CELLS],
}

impl DirectedEdgeUniverse {
    fn new() -> Self {
        let mut edges = Vec::with_capacity(DIRECTED_EDGES);
        let mut index = [[u16::MAX; CELLS]; CELLS];
        for left in 0..CELLS {
            for right in left + 1..CELLS {
                let row_distance = (left / SIDE).abs_diff(right / SIDE);
                let column_distance = (left % SIDE).abs_diff(right % SIDE);
                if row_distance <= 1 && column_distance <= 1 {
                    for (from, to) in [(left, right), (right, left)] {
                        index[from][to] = edges.len() as u16;
                        edges.push((from as u8, to as u8));
                    }
                }
            }
        }
        assert_eq!(edges.len(), DIRECTED_EDGES);
        Self { edges, index }
    }

    fn selected_mask(&self, paths: &[Vec<u8>]) -> EdgeMask {
        let mut selected = EdgeMask::EMPTY;
        for path in paths {
            for pair in path.windows(2) {
                let edge = self.index[pair[0] as usize][pair[1] as usize];
                assert_ne!(edge, u16::MAX, "thermo step is not king-adjacent");
                selected.insert(edge as usize);
            }
        }
        selected
    }

    fn common_true_mask(&self, first: &[u8; CELLS], second: &[u8; CELLS]) -> EdgeMask {
        let mut common = EdgeMask::EMPTY;
        for (edge, &(from, to)) in self.edges.iter().enumerate() {
            if first[from as usize] < first[to as usize]
                && second[from as usize] < second[to as usize]
            {
                common.insert(edge);
            }
        }
        common
    }
}

#[derive(Clone, Copy, Debug)]
struct WitnessCut {
    id: u64,
    common_true: EdgeMask,
}

#[derive(Debug)]
struct WitnessCache {
    cuts: Vec<WitnessCut>,
    ids_by_mask: BTreeMap<EdgeMask, u64>,
    limit: usize,
    next_id: u64,
    admitted: u64,
    minimum_common: u32,
}

impl WitnessCache {
    fn new(limit: usize) -> Self {
        Self {
            cuts: Vec::with_capacity(limit),
            ids_by_mask: BTreeMap::new(),
            limit,
            next_id: 1,
            admitted: 0,
            minimum_common: 0,
        }
    }

    fn find(&self, selected: EdgeMask) -> (Option<u64>, u64) {
        let mut probes = 0u64;
        for witness in self.cuts.iter().rev() {
            probes += 1;
            if selected.is_subset_of(witness.common_true) {
                return (Some(witness.id), probes);
            }
        }
        (None, probes)
    }

    fn insert(&mut self, common_true: EdgeMask) -> Option<u64> {
        if let Some(&id) = self.ids_by_mask.get(&common_true) {
            return Some(id);
        }
        if self.limit == 0 {
            return None;
        }
        if self.cuts.len() == self.limit {
            let strength = common_true.count();
            if strength <= self.minimum_common {
                return None;
            }
            let weakest_index = self
                .cuts
                .iter()
                .position(|witness| witness.common_true.count() == self.minimum_common)
                .expect("minimum cache strength is represented");
            let removed = self.cuts.swap_remove(weakest_index);
            self.ids_by_mask.remove(&removed.common_true);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.admitted += 1;
        self.cuts.push(WitnessCut { id, common_true });
        self.ids_by_mask.insert(common_true, id);
        self.minimum_common = self
            .cuts
            .iter()
            .map(|witness| witness.common_true.count())
            .min()
            .expect("inserted cache is non-empty");
        Some(id)
    }
}

#[derive(Default, Debug)]
struct SearchStats {
    mandatory_nodes: u64,
    mandatory_degree_prunes: u64,
    mandatory_spatial_prunes: u64,
    isolate_prunes: u64,
    reversal_prunes: u64,
    skeletons: u64,
    merge_nodes: u64,
    merge_spatial_prunes: u64,
    merge_structure_prunes: u64,
    early_noncanonical_prunes: u64,
    noncanonical_prunes: u64,
    maximal_covers: u64,
}

#[derive(Clone, Default, Debug)]
struct ScopeCounts {
    maximal_covers: u64,
    duplicate_layouts: u64,
    classified_layouts: u64,
    multiple_layouts: u64,
    unique_layouts: u64,
    cut_screened_layouts: u64,
    solver_calls: u64,
    cut_cache_probes: u64,
}

#[derive(Debug)]
struct Options {
    input: PathBuf,
    output: Option<PathBuf>,
    start_line: usize,
    end_line: usize,
    max_eligible: Option<usize>,
    min_paths: usize,
    max_paths: usize,
    stop_on_first: bool,
    emit_multiples: bool,
    deduplicate_layouts: bool,
    witness_cache_limit: usize,
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
    let edge_universe = DirectedEdgeUniverse::new();
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
        writeln!(
            writer,
            "{{\"schema\":\"thermo-17c-maximal-v2\",\"min_paths\":{},\"max_paths\":{},\"start_line\":{},\"end_line\":{},\"max_eligible\":{},\"emit_multiples\":{},\"deduplicate_layouts\":{},\"witness_cache_limit\":{},\"prerequisite\":\"all lower path counts below min_paths excluded\"}}",
            options.min_paths,
            options.max_paths,
            options.start_line,
            options.end_line,
            options
                .max_eligible
                .map_or_else(|| "null".to_owned(), |limit| limit.to_string()),
            options.emit_multiples,
            options.deduplicate_layouts,
            options.witness_cache_limit
        )
        .map_err(|error| format!("cannot write output: {error}"))?;
    }

    let mut totals = SearchStats::default();
    let mut scopes = BTreeMap::<String, ScopeCounts>::new();
    let mut witness_cache = WitnessCache::new(options.witness_cache_limit);
    let mut eligible_records = 0u64;
    let mut found_unique = false;
    let mut stopped = false;

    for (line_number, puzzle) in &puzzles {
        if *line_number < options.start_line || *line_number > options.end_line {
            continue;
        }
        if !puzzle.eligible(options.max_paths) {
            continue;
        }
        if options
            .max_eligible
            .is_some_and(|limit| eligible_records as usize >= limit)
        {
            break;
        }
        eligible_records += 1;
        // Catalogue representatives are inequivalent as 17-given classics, but
        // their thermo projections need not be.  Keeping this set per source
        // bounds memory without affecting completeness: a repeated layout is
        // harmless and is normally discharged by the global witness cache.
        let mut seen = BTreeSet::<Vec<u8>>::new();
        let supports = EdgeSupports::new(puzzle, &axis);
        let mut local = SearchStats::default();
        let mut callback_error = None;
        let exhausted = enumerate_maximal_covers(
            puzzle,
            &axis,
            &supports,
            options.min_paths,
            options.max_paths,
            &mut local,
            |cover| {
                let partition = compact_partition_from_paths(&cover.paths);
                let scope = scopes.entry(partition.clone()).or_default();
                scope.maximal_covers += 1;
                if options.deduplicate_layouts {
                    let key = canonical_layout_key(&cover.paths);
                    if !seen.insert(key) {
                        scope.duplicate_layouts += 1;
                        return true;
                    }
                }
                scope.classified_layouts += 1;
                let cached_witness = if options.witness_cache_limit == 0 {
                    None
                } else {
                    let selected = edge_universe.selected_mask(&cover.paths);
                    let (witness, probes) = witness_cache.find(selected);
                    scope.cut_cache_probes += probes;
                    witness
                };
                if let Some(witness_id) = cached_witness {
                    scope.cut_screened_layouts += 1;
                    scope.multiple_layouts += 1;
                    if options.emit_multiples
                        && let Err(error) = write_screened_candidate(
                            output.as_mut(),
                            *line_number,
                            puzzle,
                            &partition,
                            &cover,
                            witness_id,
                        )
                    {
                        callback_error = Some(error);
                        return false;
                    }
                    return true;
                }
                scope.solver_calls += 1;
                match classify_cover(puzzle, &cover, &axis) {
                    Ok(classification) => {
                        let mut witness_id = None;
                        if classification.count == 1 {
                            scope.unique_layouts += 1;
                            found_unique = true;
                        } else {
                            scope.multiple_layouts += 1;
                            if options.witness_cache_limit != 0 {
                                let second = classification
                                    .second_solution
                                    .expect("cap-two multiple has two witnesses");
                                let common = edge_universe
                                    .common_true_mask(&classification.first_solution, &second);
                                witness_id = witness_cache.insert(common);
                                if let Some(id) = witness_id
                                    && let Err(error) = write_witness_cut(
                                        output.as_mut(),
                                        id,
                                        *line_number,
                                        &partition,
                                        &cover,
                                        common,
                                        &classification.first_solution,
                                        &second,
                                    )
                                {
                                    callback_error = Some(error);
                                    return false;
                                }
                            }
                        }
                        if (classification.count == 1 || options.emit_multiples)
                            && let Err(error) = write_candidate(
                                output.as_mut(),
                                *line_number,
                                puzzle,
                                &partition,
                                &cover,
                                &classification,
                                witness_id,
                            )
                        {
                            callback_error = Some(error);
                            return false;
                        }
                        !(classification.count == 1 && options.stop_on_first)
                    }
                    Err(error) => {
                        callback_error = Some(format!("line {line_number}: {error}"));
                        false
                    }
                }
            },
        );
        if let Some(error) = callback_error {
            return Err(error);
        }
        add_stats(&mut totals, &local);
        if !exhausted && found_unique && options.stop_on_first {
            stopped = true;
            break;
        }
        if options.progress_every != 0
            && (eligible_records as usize).is_multiple_of(options.progress_every)
        {
            eprintln!(
                "eligible={} mandatory_nodes={} skeletons={} merge_nodes={} maximal={} classified={} screened={} solver_calls={} witness_cuts={}/{} unique={} elapsed={:.3}s",
                eligible_records,
                totals.mandatory_nodes,
                totals.skeletons,
                totals.merge_nodes,
                totals.maximal_covers,
                scopes
                    .values()
                    .map(|scope| scope.classified_layouts)
                    .sum::<u64>(),
                scopes
                    .values()
                    .map(|scope| scope.cut_screened_layouts)
                    .sum::<u64>(),
                scopes.values().map(|scope| scope.solver_calls).sum::<u64>(),
                witness_cache.cuts.len(),
                witness_cache.admitted,
                scopes
                    .values()
                    .map(|scope| scope.unique_layouts)
                    .sum::<u64>(),
                started.elapsed().as_secs_f64()
            );
        }
    }

    for partition in partitions_in_range(options.min_paths, options.max_paths) {
        let key = compact_partition(&partition);
        let scope = scopes.get(&key).cloned().unwrap_or_default();
        let record = format!(
            "{{\"type\":\"partition-summary\",\"partition\":\"{key}\",\"maximal_covers\":{},\"within_source_duplicates\":{},\"classified_layout_occurrences\":{},\"cut_screened_layouts\":{},\"solver_calls\":{},\"multiple_layouts\":{},\"unique_layouts\":{}}}",
            scope.maximal_covers,
            scope.duplicate_layouts,
            scope.classified_layouts,
            scope.cut_screened_layouts,
            scope.solver_calls,
            scope.multiple_layouts,
            scope.unique_layouts
        );
        if let Some(writer) = output.as_mut() {
            writeln!(writer, "{record}")
                .map_err(|error| format!("cannot write output: {error}"))?;
        }
    }

    let complete = options.max_eligible.is_none()
        && options.start_line == 1
        && options.end_line >= puzzles.len()
        && !stopped;
    let summary = format!(
        "{{\"type\":\"summary\",\"complete\":{complete},\"records\":{},\"eligible_records\":{eligible_records},\"mandatory_nodes\":{},\"mandatory_degree_prunes\":{},\"mandatory_spatial_prunes\":{},\"isolate_prunes\":{},\"reversal_prunes\":{},\"skeletons\":{},\"merge_nodes\":{},\"merge_spatial_prunes\":{},\"merge_structure_prunes\":{},\"early_noncanonical_prunes\":{},\"noncanonical_prunes\":{},\"maximal_covers\":{},\"classified_layout_occurrences\":{},\"cut_screened_layouts\":{},\"solver_calls\":{},\"cut_cache_probes\":{},\"witness_cuts_retained\":{},\"witness_cuts_admitted\":{},\"multiple_layouts\":{},\"unique_layouts\":{}}}",
        puzzles.len(),
        totals.mandatory_nodes,
        totals.mandatory_degree_prunes,
        totals.mandatory_spatial_prunes,
        totals.isolate_prunes,
        totals.reversal_prunes,
        totals.skeletons,
        totals.merge_nodes,
        totals.merge_spatial_prunes,
        totals.merge_structure_prunes,
        totals.early_noncanonical_prunes,
        totals.noncanonical_prunes,
        totals.maximal_covers,
        scopes
            .values()
            .map(|scope| scope.classified_layouts)
            .sum::<u64>(),
        scopes
            .values()
            .map(|scope| scope.cut_screened_layouts)
            .sum::<u64>(),
        scopes.values().map(|scope| scope.solver_calls).sum::<u64>(),
        scopes
            .values()
            .map(|scope| scope.cut_cache_probes)
            .sum::<u64>(),
        witness_cache.cuts.len(),
        witness_cache.admitted,
        scopes
            .values()
            .map(|scope| scope.multiple_layouts)
            .sum::<u64>(),
        scopes
            .values()
            .map(|scope| scope.unique_layouts)
            .sum::<u64>()
    );
    println!("{summary}");
    if let Some(writer) = output.as_mut() {
        writeln!(writer, "{summary}").map_err(|error| format!("cannot write output: {error}"))?;
        writer
            .flush()
            .map_err(|error| format!("cannot flush output: {error}"))?;
    }
    eprintln!("total elapsed {:.3}s", started.elapsed().as_secs_f64());
    Ok(())
}

fn add_stats(total: &mut SearchStats, local: &SearchStats) {
    total.mandatory_nodes += local.mandatory_nodes;
    total.mandatory_degree_prunes += local.mandatory_degree_prunes;
    total.mandatory_spatial_prunes += local.mandatory_spatial_prunes;
    total.isolate_prunes += local.isolate_prunes;
    total.reversal_prunes += local.reversal_prunes;
    total.skeletons += local.skeletons;
    total.merge_nodes += local.merge_nodes;
    total.merge_spatial_prunes += local.merge_spatial_prunes;
    total.merge_structure_prunes += local.merge_structure_prunes;
    total.early_noncanonical_prunes += local.early_noncanonical_prunes;
    total.noncanonical_prunes += local.noncanonical_prunes;
    total.maximal_covers += local.maximal_covers;
}

fn enumerate_maximal_covers<F>(
    puzzle: &Puzzle,
    axis: &AxisMorphs,
    supports: &EdgeSupports,
    min_paths: usize,
    max_paths: usize,
    stats: &mut SearchStats,
    mut callback: F,
) -> bool
where
    F: FnMut(Cover) -> bool,
{
    // A digit cannot occur twice on one strictly increasing thermometer.  Its
    // maximum multiplicity is therefore a per-record lower bound on the path
    // count.  Raising the DFS floor to that bound removes branches which could
    // never enter the requested range, without removing any cover.
    let record_min_paths = min_paths.max(
        puzzle
            .occurrences
            .iter()
            .map(Vec::len)
            .max()
            .expect("nine digit occurrence lists"),
    );
    let mut search = CoverSearch {
        puzzle,
        axis,
        supports,
        min_paths: record_min_paths,
        max_paths,
        stats,
        callback: &mut callback,
    };
    let mut digit_order = [NO_VERTEX; SIDE];
    let mut mandatory_edges = [u16::MAX; SIDE - 1];
    search.mandatory_dfs(
        0,
        0,
        Forest::empty(),
        &mut digit_order,
        &mut mandatory_edges,
        MorphSet::all(),
        MorphSet::all(),
    )
}

struct CoverSearch<'a, F> {
    puzzle: &'a Puzzle,
    axis: &'a AxisMorphs,
    supports: &'a EdgeSupports,
    min_paths: usize,
    max_paths: usize,
    stats: &'a mut SearchStats,
    callback: &'a mut F,
}

impl<F> CoverSearch<'_, F>
where
    F: FnMut(Cover) -> bool,
{
    #[allow(clippy::too_many_arguments)]
    fn mandatory_dfs(
        &mut self,
        depth: usize,
        used_digits: u16,
        forest: Forest,
        digit_order: &mut [u8; SIDE],
        mandatory_edges: &mut [u16; SIDE - 1],
        row_support: MorphSet,
        column_support: MorphSet,
    ) -> bool {
        self.stats.mandatory_nodes += 1;

        // Global reversal sends the first rank to the last.  Our canonical
        // half has first < last.  As soon as every still-unused symbol is at
        // most the first one, no completion of this prefix can survive that
        // test, so do not build the rest of its physical-edge tree.
        if depth != 0 && depth != SIDE && !reversal_prefix_can_survive(digit_order[0], used_digits)
        {
            self.stats.reversal_prunes += 1;
            return true;
        }
        let remaining_edges = (SIDE - 1).saturating_sub(forest.edge_count as usize);
        let minimum_isolates = forest.isolated_count().saturating_sub(2 * remaining_edges);
        let maximum_extra_edges = 9 - self.min_paths;
        if minimum_isolates > 2 * maximum_extra_edges {
            self.stats.isolate_prunes += 1;
            return true;
        }

        if depth == SIDE {
            debug_assert!(digit_order[0] < digit_order[SIDE - 1]);
            if forest.isolated_count() > 2 * maximum_extra_edges {
                self.stats.isolate_prunes += 1;
                return true;
            }
            let mut rank = [NO_VERTEX; SIDE];
            for (position, &digit) in digit_order.iter().enumerate() {
                rank[digit as usize] = position as u8;
            }
            self.stats.skeletons += 1;
            return self.merge_dfs(
                forest,
                digit_order,
                &rank,
                mandatory_edges,
                row_support,
                column_support,
                None,
            );
        }

        if depth == 0 {
            for digit in 0..SIDE {
                digit_order[0] = digit as u8;
                if !self.mandatory_dfs(
                    1,
                    1u16 << digit,
                    forest,
                    digit_order,
                    mandatory_edges,
                    row_support,
                    column_support,
                ) {
                    return false;
                }
            }
            return true;
        }

        let previous_digit = digit_order[depth - 1] as usize;
        for digit in 0..SIDE {
            if used_digits & (1u16 << digit) != 0 {
                continue;
            }
            for &from in &self.puzzle.occurrences[previous_digit] {
                if forest.next[from as usize] != NO_VERTEX {
                    self.stats.mandatory_degree_prunes += 1;
                    continue;
                }
                for &to in &self.puzzle.occurrences[digit] {
                    let Some(next_forest) = forest.add_edge(from, to) else {
                        self.stats.mandatory_degree_prunes += 1;
                        continue;
                    };
                    let Some((next_rows, next_columns)) =
                        self.supports
                            .add_edge(row_support, column_support, from, to)
                    else {
                        self.stats.mandatory_spatial_prunes += 1;
                        continue;
                    };
                    digit_order[depth] = digit as u8;
                    mandatory_edges[depth - 1] = edge_code(from, to);
                    if !self.mandatory_dfs(
                        depth + 1,
                        used_digits | (1u16 << digit),
                        next_forest,
                        digit_order,
                        mandatory_edges,
                        next_rows,
                        next_columns,
                    ) {
                        return false;
                    }
                }
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn merge_dfs(
        &mut self,
        forest: Forest,
        digit_order: &[u8; SIDE],
        rank: &[u8; SIDE],
        mandatory_edges: &[u16; SIDE - 1],
        row_support: MorphSet,
        column_support: MorphSet,
        last_edge: Option<u16>,
    ) -> bool {
        self.stats.merge_nodes += 1;
        let Some(components) = forest.components(&self.puzzle.digits, rank) else {
            self.stats.merge_structure_prunes += 1;
            return true;
        };
        let path_count = components.len();
        if path_count < self.min_paths {
            self.stats.merge_structure_prunes += 1;
            return true;
        }
        let remaining_merges = path_count - self.min_paths;
        if forest.isolated_count() > 2 * remaining_merges {
            self.stats.isolate_prunes += 1;
            return true;
        }

        let mut merges = Vec::new();
        let mut has_supported_merge = false;
        for left in 0..components.len() {
            for right in left + 1..components.len() {
                let a = components[left];
                let b = components[right];
                if a.digit_mask & b.digit_mask != 0 || a.size + b.size > 9 {
                    continue;
                }
                let endpoints = if a.maximum_rank < b.minimum_rank {
                    Some((a.tail, b.head))
                } else if b.maximum_rank < a.minimum_rank {
                    Some((b.tail, a.head))
                } else {
                    None
                };
                let Some((from, to)) = endpoints else {
                    continue;
                };
                let code = edge_code(from, to);
                let from_rank = rank[self.puzzle.digits[from as usize] as usize];
                let to_rank = rank[self.puzzle.digits[to as usize] as usize];
                let Some(next_forest) = forest.add_edge(from, to) else {
                    self.stats.merge_structure_prunes += 1;
                    continue;
                };
                let Some((next_rows, next_columns)) =
                    self.supports
                        .add_edge(row_support, column_support, from, to)
                else {
                    self.stats.merge_spatial_prunes += 1;
                    continue;
                };
                has_supported_merge = true;
                // The distinguished edge chosen for each consecutive rank
                // pair is canonical only when it has the smallest edge code
                // in that category.  A smaller extra merge makes that
                // impossible forever, so reject its descendant subtree.  It
                // still counts for maximality above: otherwise removing the
                // option could incorrectly turn its parent into a cover.
                if !extra_merge_preserves_mandatory_canonical(
                    code,
                    from_rank,
                    to_rank,
                    mandatory_edges,
                ) {
                    self.stats.early_noncanonical_prunes += 1;
                    continue;
                }
                merges.push(MergeOption {
                    code,
                    forest: next_forest,
                    rows: next_rows,
                    columns: next_columns,
                });
            }
        }
        merges.sort_by_key(|option| option.code);

        let is_cover = path_count >= self.min_paths
            && path_count <= self.max_paths
            && components.iter().all(|component| component.size >= 2);
        if is_cover && !has_supported_merge {
            if !mandatory_choice_is_canonical(&forest, &self.puzzle.digits, rank, mandatory_edges) {
                self.stats.noncanonical_prunes += 1;
                return true;
            }
            let row_morph = row_support.first().expect("non-empty row support");
            let column_morph = column_support.first().expect("non-empty column support");
            let cover = self.realize_cover(&forest, digit_order, row_morph, column_morph);
            self.stats.maximal_covers += 1;
            return (self.callback)(cover);
        }
        if path_count == self.min_paths {
            return true;
        }

        for option in merges {
            if last_edge.is_some_and(|previous| option.code <= previous) {
                continue;
            }
            if !self.merge_dfs(
                option.forest,
                digit_order,
                rank,
                mandatory_edges,
                option.rows,
                option.columns,
                Some(option.code),
            ) {
                return false;
            }
        }
        true
    }

    fn realize_cover(
        &self,
        forest: &Forest,
        digit_order: &[u8; SIDE],
        row_morph: usize,
        column_morph: usize,
    ) -> Cover {
        let rows = self.axis.permutations[row_morph];
        let columns = self.axis.permutations[column_morph];
        let mut paths = forest
            .vertex_paths()
            .into_iter()
            .map(|path| {
                path.into_iter()
                    .map(|vertex| morph_cell(self.puzzle.cells[vertex as usize], &rows, &columns))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        paths.sort_by_key(|path| (Reverse(path.len()), path.clone()));
        debug_assert!(
            paths
                .iter()
                .all(|path| path.len() >= 2 && is_king_path(path))
        );
        Cover {
            paths,
            digit_order: *digit_order,
            row_morph: row_morph as u16,
            column_morph: column_morph as u16,
        }
    }
}

#[derive(Clone, Copy)]
struct MergeOption {
    code: u16,
    forest: Forest,
    rows: MorphSet,
    columns: MorphSet,
}

fn mandatory_choice_is_canonical(
    forest: &Forest,
    digits: &[u8; CLUES],
    rank: &[u8; SIDE],
    mandatory_edges: &[u16; SIDE - 1],
) -> bool {
    let mut minimum = [u16::MAX; SIDE - 1];
    for (from, to) in forest.edges() {
        let from_rank = rank[digits[from as usize] as usize];
        let to_rank = rank[digits[to as usize] as usize];
        if to_rank == from_rank + 1 {
            minimum[from_rank as usize] = minimum[from_rank as usize].min(edge_code(from, to));
        }
    }
    minimum == *mandatory_edges
}

fn reversal_prefix_can_survive(first_digit: u8, used_digits: u16) -> bool {
    (first_digit as usize + 1..SIDE).any(|digit| used_digits & (1u16 << digit) == 0)
}

fn extra_merge_preserves_mandatory_canonical(
    code: u16,
    from_rank: u8,
    to_rank: u8,
    mandatory_edges: &[u16; SIDE - 1],
) -> bool {
    to_rank != from_rank + 1 || code >= mandatory_edges[from_rank as usize]
}

fn edge_code(from: u8, to: u8) -> u16 {
    u16::from(from) * CLUES as u16 + u16::from(to)
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

fn target_givens(puzzle: &Puzzle, cover: &Cover, axis: &AxisMorphs) -> [u8; CELLS] {
    let rows = axis.permutations[cover.row_morph as usize];
    let columns = axis.permutations[cover.column_morph as usize];
    let mut rank = [0u8; SIDE];
    for (position, &digit) in cover.digit_order.iter().enumerate() {
        rank[digit as usize] = position as u8 + 1;
    }
    let mut givens = [0u8; CELLS];
    for vertex in 0..CLUES {
        let cell = morph_cell(puzzle.cells[vertex], &rows, &columns);
        givens[cell as usize] = rank[puzzle.digits[vertex] as usize];
    }
    givens
}

fn write_candidate(
    mut output: Option<&mut BufWriter<File>>,
    line_number: usize,
    puzzle: &Puzzle,
    partition: &str,
    cover: &Cover,
    classification: &Classification,
    witness_cut_id: Option<u64>,
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
    let witness_cut_id = witness_cut_id.map_or_else(|| "null".to_owned(), |id| id.to_string());
    let record = format!(
        "{{\"type\":\"candidate\",\"classification_source\":\"solver\",\"witness_cut_id\":{witness_cut_id},\"multiplicity\":\"{multiplicity}\",\"count\":{},\"capped\":{},\"source_line\":{line_number},\"source_puzzle\":\"{}\",\"partition\":\"{partition}\",\"row_morph\":{},\"column_morph\":{},\"digit_order\":\"{}\",\"paths\":\"{}\",\"first_solution\":\"{}\",\"second_solution\":{second_solution}}}",
        classification.count,
        classification.capped,
        puzzle.encoded,
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

fn write_screened_candidate(
    mut output: Option<&mut BufWriter<File>>,
    line_number: usize,
    puzzle: &Puzzle,
    partition: &str,
    cover: &Cover,
    witness_cut_id: u64,
) -> Result<(), String> {
    let record = format!(
        "{{\"type\":\"candidate\",\"classification_source\":\"witness-cut\",\"witness_cut_id\":{witness_cut_id},\"multiplicity\":\"multiple\",\"count\":2,\"capped\":true,\"source_line\":{line_number},\"source_puzzle\":\"{}\",\"partition\":\"{partition}\",\"row_morph\":{},\"column_morph\":{},\"digit_order\":\"{}\",\"paths\":\"{}\"}}",
        puzzle.encoded,
        cover.row_morph,
        cover.column_morph,
        cover
            .digit_order
            .iter()
            .map(|digit| (digit + 1).to_string())
            .collect::<Vec<_>>()
            .join(","),
        compact_paths(&cover.paths),
    );
    if let Some(writer) = output.as_mut() {
        writeln!(writer, "{record}").map_err(|error| format!("cannot write output: {error}"))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_witness_cut(
    mut output: Option<&mut BufWriter<File>>,
    id: u64,
    line_number: usize,
    partition: &str,
    cover: &Cover,
    common_true: EdgeMask,
    first_solution: &[u8; CELLS],
    second_solution: &[u8; CELLS],
) -> Result<(), String> {
    let record = format!(
        "{{\"type\":\"witness-cut\",\"id\":{id},\"source_line\":{line_number},\"partition\":\"{partition}\",\"paths\":\"{}\",\"cut_length\":{},\"first_solution\":\"{}\",\"second_solution\":\"{}\"}}",
        compact_paths(&cover.paths),
        DIRECTED_EDGES as u32 - common_true.count(),
        compact_solution(first_solution),
        compact_solution(second_solution),
    );
    if let Some(writer) = output.as_mut() {
        writeln!(writer, "{record}").map_err(|error| format!("cannot write output: {error}"))?;
    }
    Ok(())
}

fn canonical_layout_key(paths: &[Vec<u8>]) -> Vec<u8> {
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
            let mut key = Vec::with_capacity(CLUES + paths.len());
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

fn compact_paths(paths: &[Vec<u8>]) -> String {
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

fn compact_partition_from_paths(paths: &[Vec<u8>]) -> String {
    let lengths = paths.iter().map(Vec::len).collect::<Vec<_>>();
    compact_partition(&lengths)
}

fn compact_partition(partition: &[usize]) -> String {
    partition
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join("+")
}

fn partitions_in_range(min_paths: usize, max_paths: usize) -> Vec<Vec<usize>> {
    let mut result = Vec::new();
    for count in (min_paths..=max_paths).rev() {
        enumerate_partitions(17, count, 9, &mut Vec::new(), &mut result);
    }
    result
}

fn enumerate_partitions(
    remaining: usize,
    count: usize,
    maximum: usize,
    prefix: &mut Vec<usize>,
    result: &mut Vec<Vec<usize>>,
) {
    if count == 0 {
        if remaining == 0 {
            result.push(prefix.clone());
        }
        return;
    }
    let upper = maximum.min(remaining.saturating_sub(2 * (count - 1)));
    for length in (2..=upper).rev() {
        prefix.push(length);
        enumerate_partitions(remaining - length, count - 1, length, prefix, result);
        prefix.pop();
    }
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
    let mut start_line = 1usize;
    let mut end_line = usize::MAX;
    let mut max_eligible = None;
    let mut min_paths = 4usize;
    let mut max_paths = 8usize;
    let mut stop_on_first = false;
    let mut emit_multiples = false;
    let mut deduplicate_layouts = false;
    let mut witness_cache_limit = 0usize;
    let mut progress_every = 100usize;
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
            "--max-eligible" => {
                max_eligible = Some(parse_usize(
                    next_value(&mut arguments, "--max-eligible")?,
                    "--max-eligible",
                )?)
            }
            "--min-paths" => {
                min_paths = parse_usize(next_value(&mut arguments, "--min-paths")?, "--min-paths")?
            }
            "--max-paths" => {
                max_paths = parse_usize(next_value(&mut arguments, "--max-paths")?, "--max-paths")?
            }
            "--progress-every" => {
                progress_every = parse_usize(
                    next_value(&mut arguments, "--progress-every")?,
                    "--progress-every",
                )?
            }
            "--stop-on-first" => stop_on_first = true,
            "--emit-multiples" => emit_multiples = true,
            "--deduplicate-layouts" => deduplicate_layouts = true,
            "--witness-cache-limit" => {
                witness_cache_limit = parse_usize(
                    next_value(&mut arguments, "--witness-cache-limit")?,
                    "--witness-cache-limit",
                )?
            }
            "--help" | "-h" => {
                println!(
                    "Usage: thermo-17c-maximal --input FILE [--output JSONL] [--min-paths 3|4] [--max-paths 3..8] [--start-line N] [--end-line N] [--max-eligible N] [--stop-on-first] [--emit-multiples] [--deduplicate-layouts] [--witness-cache-limit N] [--progress-every N]"
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
    if !(3..=8).contains(&min_paths) || !(3..=8).contains(&max_paths) || min_paths > max_paths {
        return Err("path range must satisfy 3 <= min-paths <= max-paths <= 8".to_owned());
    }
    Ok(Options {
        input,
        output,
        start_line,
        end_line,
        max_eligible,
        min_paths,
        max_paths,
        stop_on_first,
        emit_multiples,
        deduplicate_layouts,
        witness_cache_limit,
        progress_every,
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
    fn partitions_cover_the_expected_remaining_forty() {
        let partitions = partitions_in_range(4, 8);
        assert_eq!(partitions.len(), 40);
        assert_eq!(partitions.iter().filter(|part| part.len() == 4).count(), 16);
        assert_eq!(partitions.iter().filter(|part| part.len() == 5).count(), 13);
        assert_eq!(partitions.iter().filter(|part| part.len() == 6).count(), 7);
        assert_eq!(partitions.iter().filter(|part| part.len() == 7).count(), 3);
        assert_eq!(partitions.iter().filter(|part| part.len() == 8).count(), 1);
    }

    #[test]
    fn eight_edges_always_leave_nine_components() {
        let mut forest = Forest::empty();
        for edge in 0..8u8 {
            forest = forest.add_edge(2 * edge, 2 * edge + 1).unwrap();
        }
        let digits = [0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8];
        let rank = [0, 1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(forest.components(&digits, &rank).unwrap().len(), 9);
    }

    #[test]
    fn component_merge_rejects_repeated_digits() {
        let mut forest = Forest::empty();
        forest = forest.add_edge(0, 1).unwrap();
        forest = forest.add_edge(2, 3).unwrap();
        let mut digits = [0u8; CLUES];
        digits[0] = 0;
        digits[1] = 1;
        digits[2] = 1;
        digits[3] = 2;
        for (index, digit) in digits.iter_mut().enumerate().skip(4) {
            *digit = (index % SIDE) as u8;
        }
        let rank = [0, 1, 2, 3, 4, 5, 6, 7, 8];
        let components = forest.components(&digits, &rank).unwrap();
        assert_ne!(components[0].digit_mask & components[1].digit_mask, 0);
    }

    #[test]
    fn canonical_key_ignores_component_order_and_global_reversal() {
        let paths = vec![vec![0, 1, 2], vec![10, 11], vec![20, 21]];
        let mut reversed = paths
            .iter()
            .rev()
            .map(|path| path.iter().rev().copied().collect::<Vec<_>>())
            .collect::<Vec<_>>();
        reversed.rotate_left(1);
        assert_eq!(
            canonical_layout_key(&paths),
            canonical_layout_key(&reversed)
        );
    }

    #[test]
    fn eligible_requires_all_digits_and_enough_paths_for_multiplicity() {
        let all_nine = format!("{}{}", "12345678912345678", ".".repeat(64));
        let puzzle = Puzzle::parse(&all_nine, 1).unwrap();
        assert!(puzzle.eligible(4));
        assert!(!puzzle.eligible(1));
    }

    #[test]
    fn reversal_prefix_prunes_exactly_when_no_larger_last_digit_remains() {
        assert!(!reversal_prefix_can_survive(8, 1 << 8));
        assert!(!reversal_prefix_can_survive(5, 0b1_1110_0000));
        assert!(reversal_prefix_can_survive(5, 0b0_1110_0000));
        assert!(reversal_prefix_can_survive(0, 1));
    }

    #[test]
    fn early_canonical_prune_only_rejects_a_smaller_adjacent_rank_edge() {
        let mut mandatory = [u16::MAX; SIDE - 1];
        mandatory[3] = 100;
        assert!(!extra_merge_preserves_mandatory_canonical(
            99, 3, 4, &mandatory
        ));
        assert!(extra_merge_preserves_mandatory_canonical(
            100, 3, 4, &mandatory
        ));
        assert!(extra_merge_preserves_mandatory_canonical(
            1, 3, 5, &mandatory
        ));
    }

    #[test]
    fn witness_cut_screen_is_exactly_common_satisfied_edges() {
        let universe = DirectedEdgeUniverse::new();
        let encoded =
            "831274569964358271257961843682743195793125486415896327578612934126439758349587612";
        let first: [u8; CELLS] = encoded
            .bytes()
            .map(|byte| byte - b'0')
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let second = first.map(|digit| match digit {
            1 => 2,
            2 => 1,
            other => other,
        });
        let common = universe.common_true_mask(&first, &second);
        let common_edge = universe
            .edges
            .iter()
            .position(|&(from, to)| {
                first[from as usize] < first[to as usize]
                    && second[from as usize] < second[to as usize]
            })
            .unwrap();
        let rejected_edge = (0..DIRECTED_EDGES)
            .find(|&edge| {
                let mut selected = EdgeMask::EMPTY;
                selected.insert(edge);
                !selected.is_subset_of(common)
            })
            .unwrap();

        let mut cache = WitnessCache::new(1);
        let id = cache.insert(common).unwrap();
        let mut selected = EdgeMask::EMPTY;
        selected.insert(common_edge);
        assert_eq!(cache.find(selected).0, Some(id));
        selected = EdgeMask::EMPTY;
        selected.insert(rejected_edge);
        assert_eq!(cache.find(selected).0, None);
    }

    #[test]
    fn bounded_witness_cache_retains_stronger_common_masks() {
        let mut cache = WitnessCache::new(2);
        let mut one = EdgeMask::EMPTY;
        one.insert(0);
        let mut two = one;
        two.insert(1);
        let mut three = two;
        three.insert(2);
        assert!(cache.insert(one).is_some());
        assert!(cache.insert(two).is_some());
        assert!(cache.insert(three).is_some());
        assert_eq!(cache.cuts.len(), 2);
        assert!(!cache.ids_by_mask.contains_key(&one));
        assert!(cache.ids_by_mask.contains_key(&two));
        assert!(cache.ids_by_mask.contains_key(&three));
    }
}
