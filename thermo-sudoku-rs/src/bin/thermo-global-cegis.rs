//! Target-free CEGIS pilot for the relaxed sixteen-comparison problem.
//!
//! The master chooses directed king-adjacent comparisons and carries a solved
//! classic Sudoku satisfying them. A checker searches exactly for another
//! solution. Every counterexample pair contributes a globally valid cut: a
//! future comparison set must contain an edge which is not true in both grids.

use std::collections::HashSet;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::{BufWriter, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::time::Duration;

const CELLS: usize = 81;
const MAX_BUDGET: usize = 16;
const UNDIRECTED_KING_PAIRS: usize = 272;
const DIRECTED_EDGES: usize = UNDIRECTED_KING_PAIRS * 2;
const EDGE_WORDS: usize = DIRECTED_EDGES.div_ceil(64);
const ALL_DIGITS: u16 = 0x01ff;
const ALL_HOUSES: u32 = (1u32 << 27) - 1;
const NO_CELL: u8 = u8::MAX;
const CHECKPOINT_HEADER: &str = "# thermo-global-cegis-v1";
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

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

    fn without(self, forbidden: Self) -> Self {
        let mut result = self;
        for (word, blocked) in result.0.iter_mut().zip(forbidden.0) {
            *word &= !blocked;
        }
        result
    }

    fn count(self) -> usize {
        self.0.iter().map(|word| word.count_ones() as usize).sum()
    }

    fn is_empty(self) -> bool {
        self.0.iter().all(|&word| word == 0)
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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct DirectedEdge {
    lower: u8,
    upper: u8,
}

fn directed_edges() -> Vec<DirectedEdge> {
    let mut edges = Vec::with_capacity(DIRECTED_EDGES);
    for left in 0..CELLS {
        for right in left + 1..CELLS {
            let row_distance = (left / 9).abs_diff(right / 9);
            let column_distance = (left % 9).abs_diff(right % 9);
            if row_distance <= 1 && column_distance <= 1 {
                edges.push(DirectedEdge {
                    lower: left as u8,
                    upper: right as u8,
                });
                edges.push(DirectedEdge {
                    lower: right as u8,
                    upper: left as u8,
                });
            }
        }
    }
    assert_eq!(edges.len(), DIRECTED_EDGES);
    edges
}

fn edge_true(edge: DirectedEdge, grid: &[u8; CELLS]) -> bool {
    grid[edge.lower as usize] < grid[edge.upper as usize]
}

#[derive(Clone, Copy, Debug, Default)]
struct Work {
    singles_low: u64,
    singles_high: u32,
    dirty_houses: u32,
    dirty_comparisons: u32,
}

impl Work {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SearchStatus {
    Exhausted,
    Stopped,
    NodeLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VisitControl {
    Continue,
    Stop,
}

struct SudokuSearch<'a> {
    comparisons: &'a [DirectedEdge],
    incident: [u32; CELLS],
    degree: [u8; CELLS],
    node_limit: Option<u64>,
    nodes: u64,
    branch_bias: Option<&'a [u8; CELLS]>,
}

impl<'a> SudokuSearch<'a> {
    fn new(comparisons: &'a [DirectedEdge]) -> Self {
        assert!(comparisons.len() < u32::BITS as usize);
        let mut incident = [0u32; CELLS];
        let mut degree = [0u8; CELLS];
        for (index, edge) in comparisons.iter().enumerate() {
            let bit = 1u32 << index;
            incident[edge.lower as usize] |= bit;
            incident[edge.upper as usize] |= bit;
            degree[edge.lower as usize] += 1;
            degree[edge.upper as usize] += 1;
        }
        Self {
            comparisons,
            incident,
            degree,
            node_limit: None,
            nodes: 0,
            branch_bias: None,
        }
    }

    fn visit<F>(
        mut self,
        node_limit: Option<u64>,
        branch_bias: Option<&'a [u8; CELLS]>,
        mut visitor: F,
    ) -> (SearchStatus, u64)
    where
        F: FnMut([u8; CELLS]) -> VisitControl,
    {
        self.node_limit = node_limit;
        self.branch_bias = branch_bias;
        let mut state = [ALL_DIGITS; CELLS];
        let mut work = Work {
            dirty_houses: ALL_HOUSES,
            dirty_comparisons: if self.comparisons.is_empty() {
                0
            } else {
                (1u32 << self.comparisons.len()) - 1
            },
            ..Work::default()
        };
        for &edge in self.comparisons {
            if !restrict(
                &self,
                &mut state,
                &mut work,
                edge.lower as usize,
                ALL_DIGITS & !(1 << 8),
            ) || !restrict(
                &self,
                &mut state,
                &mut work,
                edge.upper as usize,
                ALL_DIGITS & !1,
            ) {
                return (SearchStatus::Exhausted, self.nodes);
            }
        }
        let mut cell_order = std::array::from_fn(|cell| cell as u8);
        let status = self.search(state, work, &mut cell_order, &mut visitor);
        (status, self.nodes)
    }

    fn search<F>(
        &mut self,
        mut state: [u16; CELLS],
        mut work: Work,
        cell_order: &mut [u8; CELLS],
        visitor: &mut F,
    ) -> SearchStatus
    where
        F: FnMut([u8; CELLS]) -> VisitControl,
    {
        if self.node_limit.is_some_and(|limit| self.nodes >= limit) {
            return SearchStatus::NodeLimit;
        }
        self.nodes += 1;
        if !propagate(self, &mut state, &mut work) {
            return SearchStatus::Exhausted;
        }
        let Some(cell) = choose_branch_cell(&state, &self.degree, cell_order) else {
            return match visitor(domains_to_grid(&state)) {
                VisitControl::Continue => SearchStatus::Exhausted,
                VisitControl::Stop => SearchStatus::Stopped,
            };
        };

        let candidates = state[cell];
        let bias_bit = self.branch_bias.map_or(0, |grid| bit_for_digit(grid[cell]));
        let mut ordinary = candidates & !bias_bit;
        while ordinary != 0 {
            let value = low_bit(ordinary);
            ordinary &= ordinary - 1;
            let mut child = state;
            let mut child_work = Work::default();
            if restrict(self, &mut child, &mut child_work, cell, value) {
                match self.search(child, child_work, cell_order, visitor) {
                    SearchStatus::Exhausted => {}
                    terminal => return terminal,
                }
            }
        }
        if candidates & bias_bit != 0 {
            let mut child = state;
            let mut child_work = Work::default();
            if restrict(self, &mut child, &mut child_work, cell, bias_bit) {
                return self.search(child, child_work, cell_order, visitor);
            }
        }
        SearchStatus::Exhausted
    }
}

fn restrict(
    search: &SudokuSearch<'_>,
    state: &mut [u16; CELLS],
    work: &mut Work,
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
    work.dirty_comparisons |= search.incident[cell];
    if !old.is_power_of_two() && next.is_power_of_two() {
        work.add_single(cell);
    }
    true
}

fn propagate(search: &SudokuSearch<'_>, state: &mut [u16; CELLS], work: &mut Work) -> bool {
    loop {
        if let Some(cell) = work.pop_single() {
            let value = state[cell];
            for &peer in &PEERS[cell] {
                if !restrict(search, state, work, peer as usize, ALL_DIGITS & !value) {
                    return false;
                }
            }
            continue;
        }
        if work.dirty_comparisons != 0 {
            let index = work.dirty_comparisons.trailing_zeros() as usize;
            let bit = 1u32 << index;
            work.dirty_comparisons &= !bit;
            if !revise_comparison(search, state, work, search.comparisons[index]) {
                return false;
            }
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
            if !revise_house(search, state, work, house) {
                return false;
            }
            continue;
        }
        return true;
    }
}

fn revise_comparison(
    search: &SudokuSearch<'_>,
    state: &mut [u16; CELLS],
    work: &mut Work,
    edge: DirectedEdge,
) -> bool {
    let lower = edge.lower as usize;
    let upper = edge.upper as usize;
    let lower_min = low_bit(state[lower]);
    if lower_min == 0 {
        return false;
    }
    let greater = ALL_DIGITS & !(lower_min.wrapping_shl(1).wrapping_sub(1));
    if !restrict(search, state, work, upper, greater) {
        return false;
    }
    let upper_max = high_bit(state[upper]);
    upper_max != 0 && restrict(search, state, work, lower, upper_max.wrapping_sub(1))
}

fn revise_house(
    search: &SudokuSearch<'_>,
    state: &mut [u16; CELLS],
    work: &mut Work,
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
                && (!forced.is_power_of_two() || !restrict(search, state, work, cell, forced))
            {
                return false;
            }
        }
    }
    match house {
        0..=8 => revise_row_locks(search, state, work, house),
        9..=17 => revise_column_locks(search, state, work, house - 9),
        _ => revise_box_locks(search, state, work, house - 18),
    }
}

fn revise_row_locks(
    search: &SudokuSearch<'_>,
    state: &mut [u16; CELLS],
    work: &mut Work,
    row: usize,
) -> bool {
    let mut segments = [0u16; 3];
    for (stack, segment) in segments.iter_mut().enumerate() {
        for offset in 0..3 {
            *segment |= state[row * 9 + stack * 3 + offset];
        }
    }
    for stack in 0..3 {
        let confined = segments[stack] & !(segments[(stack + 1) % 3] | segments[(stack + 2) % 3]);
        if confined == 0 {
            continue;
        }
        let box_row = (row / 3) * 3;
        for other_row in box_row..box_row + 3 {
            if other_row == row {
                continue;
            }
            for column in stack * 3..stack * 3 + 3 {
                if !restrict(
                    search,
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
    true
}

fn revise_column_locks(
    search: &SudokuSearch<'_>,
    state: &mut [u16; CELLS],
    work: &mut Work,
    column: usize,
) -> bool {
    let mut segments = [0u16; 3];
    for (band, segment) in segments.iter_mut().enumerate() {
        for offset in 0..3 {
            *segment |= state[(band * 3 + offset) * 9 + column];
        }
    }
    for band in 0..3 {
        let confined = segments[band] & !(segments[(band + 1) % 3] | segments[(band + 2) % 3]);
        if confined == 0 {
            continue;
        }
        let box_column = (column / 3) * 3;
        for other_column in box_column..box_column + 3 {
            if other_column == column {
                continue;
            }
            for row in band * 3..band * 3 + 3 {
                if !restrict(
                    search,
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
    true
}

fn revise_box_locks(
    search: &SudokuSearch<'_>,
    state: &mut [u16; CELLS],
    work: &mut Work,
    box_index: usize,
) -> bool {
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
                if column / 3 != box_column / 3
                    && !restrict(
                        search,
                        state,
                        work,
                        row * 9 + column,
                        ALL_DIGITS & !confined,
                    )
                {
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
                if row / 3 != box_row / 3
                    && !restrict(
                        search,
                        state,
                        work,
                        row * 9 + column,
                        ALL_DIGITS & !confined,
                    )
                {
                    return false;
                }
            }
        }
    }
    true
}

fn choose_branch_cell(
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

fn house_cell(house: usize, position: usize) -> usize {
    HOUSE_CELLS[house][position] as usize
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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct GridPair {
    first: [u8; CELLS],
    second: [u8; CELLS],
}

impl GridPair {
    fn new(left: [u8; CELLS], right: [u8; CELLS]) -> Result<Self, String> {
        if left == right {
            return Err("a counterexample pair must contain two different grids".into());
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

fn pair_cut(pair: &GridPair, edges: &[DirectedEdge]) -> EdgeSet {
    let mut cut = EdgeSet::default();
    for (edge_id, &edge) in edges.iter().enumerate() {
        if !(edge_true(edge, &pair.first) && edge_true(edge, &pair.second)) {
            cut.insert(edge_id);
        }
    }
    cut
}

fn selected_set(selected: &[usize]) -> EdgeSet {
    let mut result = EdgeSet::default();
    for &edge in selected {
        result.insert(edge);
    }
    result
}

fn selected_edges(selected: &[usize], edges: &[DirectedEdge]) -> Vec<DirectedEdge> {
    selected.iter().map(|&edge| edges[edge]).collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FeasibilityStatus {
    Satisfiable,
    Unsatisfiable,
    NodeLimit,
}

#[derive(Debug)]
struct FeasibilityResult {
    status: FeasibilityStatus,
    witness: Option<[u8; CELLS]>,
    nodes: u64,
}

fn find_one(
    selected: &[usize],
    edges: &[DirectedEdge],
    node_limit: Option<u64>,
) -> FeasibilityResult {
    let comparisons = selected_edges(selected, edges);
    let mut witness = None;
    let (status, nodes) = SudokuSearch::new(&comparisons).visit(node_limit, None, |grid| {
        witness = Some(grid);
        VisitControl::Stop
    });
    let status = match status {
        SearchStatus::Stopped => FeasibilityStatus::Satisfiable,
        SearchStatus::Exhausted => FeasibilityStatus::Unsatisfiable,
        SearchStatus::NodeLimit => FeasibilityStatus::NodeLimit,
    };
    FeasibilityResult {
        status,
        witness,
        nodes,
    }
}

#[derive(Debug)]
struct MasterCandidate {
    selected: Vec<usize>,
    witness: [u8; CELLS],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MasterStatus {
    Candidate,
    Exhausted,
    MasterNodeLimit,
    SudokuNodeLimit,
}

#[derive(Debug)]
struct MasterResult {
    status: MasterStatus,
    candidate: Option<MasterCandidate>,
    nodes: u64,
    sudoku_nodes: u64,
}

struct JointMaster<'a> {
    cuts: &'a [EdgeSet],
    edges: &'a [DirectedEdge],
    budget: usize,
    master_node_limit: Option<u64>,
    sudoku_node_limit: Option<u64>,
    nodes: u64,
    sudoku_nodes: u64,
    terminal_limit: Option<MasterStatus>,
    preferred: EdgeSet,
}

impl<'a> JointMaster<'a> {
    #[cfg(test)]
    fn solve(
        cuts: &'a [EdgeSet],
        edges: &'a [DirectedEdge],
        budget: usize,
        master_node_limit: Option<u64>,
        sudoku_node_limit: Option<u64>,
    ) -> MasterResult {
        Self::solve_with_hint(
            cuts,
            edges,
            budget,
            master_node_limit,
            sudoku_node_limit,
            None,
            &[],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn solve_with_hint(
        cuts: &'a [EdgeSet],
        edges: &'a [DirectedEdge],
        budget: usize,
        master_node_limit: Option<u64>,
        sudoku_node_limit: Option<u64>,
        root_hint: Option<[u8; CELLS]>,
        preferred_edges: &[usize],
    ) -> MasterResult {
        let preferred = selected_set(preferred_edges);
        let mut master = Self {
            cuts,
            edges,
            budget,
            master_node_limit,
            sudoku_node_limit,
            nodes: 0,
            sudoku_nodes: 0,
            terminal_limit: None,
            preferred,
        };
        let root = if let Some(witness) = root_hint {
            FeasibilityResult {
                status: FeasibilityStatus::Satisfiable,
                witness: Some(witness),
                nodes: 0,
            }
        } else {
            master.find_one_counted(&[])
        };
        let candidate = match root.status {
            FeasibilityStatus::Satisfiable => {
                let mut selected = Vec::new();
                let mut active = master.cuts.to_vec();
                master.search(
                    &mut selected,
                    EdgeSet::default(),
                    EdgeSet::default(),
                    root.witness.expect("SAT result has a witness"),
                    &mut active,
                )
            }
            FeasibilityStatus::Unsatisfiable => None,
            FeasibilityStatus::NodeLimit => {
                master.terminal_limit = Some(MasterStatus::SudokuNodeLimit);
                None
            }
        };
        let status = if candidate.is_some() {
            MasterStatus::Candidate
        } else if let Some(limit) = master.terminal_limit {
            limit
        } else {
            MasterStatus::Exhausted
        };
        MasterResult {
            status,
            candidate,
            nodes: master.nodes,
            sudoku_nodes: master.sudoku_nodes,
        }
    }

    fn find_one_counted(&mut self, selected: &[usize]) -> FeasibilityResult {
        let remaining = self
            .sudoku_node_limit
            .map(|limit| limit.saturating_sub(self.sudoku_nodes));
        let result = find_one(selected, self.edges, remaining);
        self.sudoku_nodes += result.nodes;
        result
    }

    fn search(
        &mut self,
        selected: &mut Vec<usize>,
        selected_bits: EdgeSet,
        forbidden: EdgeSet,
        witness: [u8; CELLS],
        active: &mut [EdgeSet],
    ) -> Option<MasterCandidate> {
        if self
            .master_node_limit
            .is_some_and(|limit| self.nodes >= limit)
        {
            self.terminal_limit = Some(MasterStatus::MasterNodeLimit);
            return None;
        }
        self.nodes += 1;

        let mut pivot = None::<EdgeSet>;
        let mut pivot_size = usize::MAX;
        let mut coverage = [0usize; DIRECTED_EDGES];
        for (sample_index, &cut) in active.iter().enumerate() {
            let available = cut.without(forbidden);
            if available.is_empty() {
                return None;
            }
            let available_size = available.count();
            if available_size < pivot_size {
                pivot = Some(available);
                pivot_size = available_size;
            }
            if sample_index < 512 {
                for edge in available.iter() {
                    coverage[edge] += 1;
                }
            }
        }
        let Some(pivot) = pivot else {
            let mut padded = selected.clone();
            let mut padded_bits = selected_bits;
            for edge_id in self.preferred.iter() {
                if padded.len() == self.budget {
                    break;
                }
                if !padded_bits.contains(edge_id) && edge_true(self.edges[edge_id], &witness) {
                    padded.push(edge_id);
                    padded_bits.insert(edge_id);
                }
            }
            for (edge_id, &edge) in self.edges.iter().enumerate() {
                if padded.len() == self.budget {
                    break;
                }
                if !padded_bits.contains(edge_id) && edge_true(edge, &witness) {
                    padded.push(edge_id);
                    padded_bits.insert(edge_id);
                }
            }
            if padded.len() != self.budget {
                return None;
            }
            return Some(MasterCandidate {
                selected: padded,
                witness,
            });
        };
        if selected.len() == self.budget {
            return None;
        }

        let mut choices = pivot.iter().collect::<Vec<_>>();
        choices.sort_unstable_by(|&left, &right| {
            let left_true = edge_true(self.edges[left], &witness);
            let right_true = edge_true(self.edges[right], &witness);
            let left_preferred = self.preferred.contains(left);
            let right_preferred = self.preferred.contains(right);
            right_true
                .cmp(&left_true)
                .then_with(|| right_preferred.cmp(&left_preferred))
                .then_with(|| coverage[right].cmp(&coverage[left]))
                .then_with(|| left.cmp(&right))
        });

        let mut sibling_forbidden = forbidden;
        for edge_id in choices {
            if sibling_forbidden.contains(edge_id) {
                continue;
            }
            selected.push(edge_id);
            let mut child_selected_bits = selected_bits;
            child_selected_bits.insert(edge_id);
            let child_witness = if edge_true(self.edges[edge_id], &witness) {
                Some(witness)
            } else if selected_bits.contains(edge_id ^ 1) {
                None
            } else {
                let feasibility = self.find_one_counted(selected);
                match feasibility.status {
                    FeasibilityStatus::Satisfiable => feasibility.witness,
                    FeasibilityStatus::Unsatisfiable => None,
                    FeasibilityStatus::NodeLimit => {
                        self.terminal_limit = Some(MasterStatus::SudokuNodeLimit);
                        selected.pop();
                        return None;
                    }
                }
            };
            if let Some(child_witness) = child_witness {
                let child_active_len = partition_unhit(active, edge_id);
                if let Some(candidate) = self.search(
                    selected,
                    child_selected_bits,
                    sibling_forbidden,
                    child_witness,
                    &mut active[..child_active_len],
                ) {
                    return Some(candidate);
                }
            }
            selected.pop();
            if self.terminal_limit.is_some() {
                return None;
            }
            sibling_forbidden.insert(edge_id);
        }
        None
    }
}

fn partition_unhit(active: &mut [EdgeSet], edge_id: usize) -> usize {
    let mut left = 0usize;
    let mut right = active.len();
    while left < right {
        while left < right && !active[left].contains(edge_id) {
            left += 1;
        }
        while left < right && active[right - 1].contains(edge_id) {
            right -= 1;
        }
        if left < right {
            active.swap(left, right - 1);
            left += 1;
            right -= 1;
        }
    }
    left
}

#[derive(Debug)]
struct OracleResult {
    alternatives: Vec<[u8; CELLS]>,
    nodes: u64,
    exhausted: bool,
    node_limit_hit: bool,
}

fn find_alternatives(
    candidate: &MasterCandidate,
    edges: &[DirectedEdge],
    node_limit: Option<u64>,
    batch: usize,
) -> OracleResult {
    let comparisons = selected_edges(&candidate.selected, edges);
    let mut alternatives = Vec::with_capacity(batch);
    let (status, nodes) =
        SudokuSearch::new(&comparisons).visit(node_limit, Some(&candidate.witness), |grid| {
            if grid != candidate.witness {
                alternatives.push(grid);
                if alternatives.len() == batch {
                    return VisitControl::Stop;
                }
            }
            VisitControl::Continue
        });
    OracleResult {
        alternatives,
        nodes,
        exhausted: status == SearchStatus::Exhausted,
        node_limit_hit: status == SearchStatus::NodeLimit,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PairMode {
    Anchor,
    All,
}

#[derive(Debug)]
struct Options {
    budget: usize,
    max_iterations: usize,
    master_node_limit: Option<u64>,
    master_sudoku_node_limit: Option<u64>,
    oracle_node_limit: Option<u64>,
    oracle_batch: usize,
    pair_mode: PairMode,
    checkpoint: Option<PathBuf>,
    checkpoint_every: usize,
    direct_pairs: Vec<String>,
    score3_seed: bool,
    progress_every: usize,
    summary_only: bool,
    output: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            budget: MAX_BUDGET,
            max_iterations: 100,
            master_node_limit: Some(2_000_000),
            master_sudoku_node_limit: Some(5_000_000),
            oracle_node_limit: Some(5_000_000),
            oracle_batch: 32,
            pair_mode: PairMode::All,
            checkpoint: None,
            checkpoint_every: 1,
            direct_pairs: Vec::new(),
            score3_seed: true,
            progress_every: 1,
            summary_only: false,
            output: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunStatus {
    RelaxedUnique,
    RelaxedExcluded,
    MasterNodeLimit,
    MasterSudokuNodeLimit,
    OracleNodeLimit,
    IterationLimit,
}

impl RunStatus {
    fn label(self) -> &'static str {
        match self {
            Self::RelaxedUnique => "relaxed-unique-witness",
            Self::RelaxedExcluded => "relaxed-exact-budget-excluded",
            Self::MasterNodeLimit => "master-node-limit",
            Self::MasterSudokuNodeLimit => "master-sudoku-node-limit",
            Self::OracleNodeLimit => "oracle-node-limit",
            Self::IterationLimit => "iteration-limit",
        }
    }
}

#[derive(Debug)]
struct IterationLog {
    iteration: usize,
    pair_cuts_before: usize,
    master_nodes: u64,
    master_sudoku_nodes: u64,
    swap_pairs_added: usize,
    oracle_nodes: u64,
    alternatives: usize,
    oracle_pairs_added: usize,
    oracle_exhausted: bool,
    oracle_node_limit_hit: bool,
}

#[derive(Debug)]
struct RunReport {
    status: RunStatus,
    initial_pairs: usize,
    final_pairs: usize,
    duplicate_pairs: usize,
    iterations: Vec<IterationLog>,
    total_master_nodes: u64,
    total_master_sudoku_nodes: u64,
    total_oracle_nodes: u64,
    final_candidate: Option<MasterCandidate>,
    elapsed_seconds: f64,
}

fn run() -> Result<(), String> {
    let options = parse_options()?;
    let started = Instant::now();
    let edges = directed_edges();
    let mut pairs = if let Some(path) = &options.checkpoint {
        load_checkpoint(path, options.budget)?
    } else {
        Vec::new()
    };
    let mut seen = pairs.iter().copied().collect::<HashSet<_>>();
    let mut duplicate_pairs = 0usize;

    for encoded in &options.direct_pairs {
        let pair = parse_pair(encoded)?;
        validate_pair(&pair)?;
        if seen.insert(pair) {
            pairs.push(pair);
        } else {
            duplicate_pairs += 1;
        }
    }
    let score3 = if options.score3_seed {
        Some(score3_seed(&edges)?)
    } else {
        None
    };
    if let Some(seed) = &score3 {
        for &pair in &seed.pairs {
            if seen.insert(pair) {
                pairs.push(pair);
            } else {
                duplicate_pairs += 1;
            }
        }
    }
    let initial_pairs = pairs.len();
    let mut cuts = pairs
        .iter()
        .map(|pair| pair_cut(pair, &edges))
        .collect::<Vec<_>>();
    write_checkpoint_if_requested(&options, &pairs)?;
    let mut persisted_pairs = pairs.len();

    let mut report = RunReport {
        status: RunStatus::IterationLimit,
        initial_pairs,
        final_pairs: initial_pairs,
        duplicate_pairs,
        iterations: Vec::new(),
        total_master_nodes: 0,
        total_master_sudoku_nodes: 0,
        total_oracle_nodes: 0,
        final_candidate: None,
        elapsed_seconds: 0.0,
    };

    for iteration in 0..options.max_iterations {
        let master = JointMaster::solve_with_hint(
            &cuts,
            &edges,
            options.budget,
            options.master_node_limit,
            options.master_sudoku_node_limit,
            score3.as_ref().map(|seed| seed.solutions[0]),
            score3
                .as_ref()
                .map_or(&[][..], |seed| seed.selected.as_slice()),
        );
        report.total_master_nodes += master.nodes;
        report.total_master_sudoku_nodes += master.sudoku_nodes;
        let pair_cuts_before = pairs.len();
        let Some(candidate) = master.candidate else {
            report.status = match master.status {
                MasterStatus::Exhausted => RunStatus::RelaxedExcluded,
                MasterStatus::MasterNodeLimit => RunStatus::MasterNodeLimit,
                MasterStatus::SudokuNodeLimit => RunStatus::MasterSudokuNodeLimit,
                MasterStatus::Candidate => unreachable!(),
            };
            break;
        };
        validate_candidate(&candidate, &cuts, &edges, options.budget)?;

        let mut swap_pairs_added = 0usize;
        let candidate_bits = selected_set(&candidate.selected);
        let mut invalidated_by_swap_seed = false;
        for digit in 1..=8 {
            let swapped = swap_digits(candidate.witness, digit, digit + 1);
            let pair = GridPair::new(candidate.witness, swapped)?;
            if seen.insert(pair) {
                let cut = pair_cut(&pair, &edges);
                invalidated_by_swap_seed |= !cut.intersects(candidate_bits);
                pairs.push(pair);
                cuts.push(cut);
                swap_pairs_added += 1;
            }
        }
        if invalidated_by_swap_seed {
            if pairs.len() != persisted_pairs && checkpoint_due(iteration, options.checkpoint_every)
            {
                write_checkpoint_if_requested(&options, &pairs)?;
                persisted_pairs = pairs.len();
            }
            if iteration % options.progress_every == 0 {
                eprintln!(
                    "global-cegis iteration={iteration} cuts={pair_cuts_before} master_nodes={} master_sudoku_nodes={} swap_pairs_added={swap_pairs_added} checker=deferred",
                    master.nodes, master.sudoku_nodes
                );
            }
            report.iterations.push(IterationLog {
                iteration,
                pair_cuts_before,
                master_nodes: master.nodes,
                master_sudoku_nodes: master.sudoku_nodes,
                swap_pairs_added,
                oracle_nodes: 0,
                alternatives: 0,
                oracle_pairs_added: 0,
                oracle_exhausted: false,
                oracle_node_limit_hit: false,
            });
            continue;
        }

        let oracle = find_alternatives(
            &candidate,
            &edges,
            options.oracle_node_limit,
            options.oracle_batch,
        );
        report.total_oracle_nodes += oracle.nodes;
        for alternative in &oracle.alternatives {
            validate_sudoku(alternative)?;
            if candidate
                .selected
                .iter()
                .any(|&edge| !edge_true(edges[edge], alternative))
            {
                return Err("internal error: checker returned a comparison violation".into());
            }
        }
        let pool = std::iter::once(candidate.witness)
            .chain(oracle.alternatives.iter().copied())
            .collect::<Vec<_>>();
        let mut oracle_pairs_added = 0usize;
        match options.pair_mode {
            PairMode::Anchor => {
                for &alternative in &oracle.alternatives {
                    let pair = GridPair::new(candidate.witness, alternative)?;
                    let cut = pair_cut(&pair, &edges);
                    if cut.intersects(candidate_bits) {
                        return Err("internal error: an anchor pair cut is hit by the checked comparison set".into());
                    }
                    if seen.insert(pair) {
                        cuts.push(cut);
                        pairs.push(pair);
                        oracle_pairs_added += 1;
                    }
                }
            }
            PairMode::All => {
                for left in 0..pool.len() {
                    for right in left + 1..pool.len() {
                        let pair = GridPair::new(pool[left], pool[right])?;
                        let cut = pair_cut(&pair, &edges);
                        if cut.intersects(candidate_bits) {
                            return Err("internal error: a checker pair cut is hit by the checked comparison set".into());
                        }
                        if seen.insert(pair) {
                            cuts.push(cut);
                            pairs.push(pair);
                            oracle_pairs_added += 1;
                        }
                    }
                }
            }
        }
        if pairs.len() != persisted_pairs && checkpoint_due(iteration, options.checkpoint_every) {
            write_checkpoint_if_requested(&options, &pairs)?;
            persisted_pairs = pairs.len();
        }
        if iteration % options.progress_every == 0 {
            eprintln!(
                "global-cegis iteration={iteration} cuts={pair_cuts_before} selected={} master_nodes={} master_sudoku_nodes={} swap_pairs_added={swap_pairs_added} oracle_nodes={} alternatives={} oracle_pairs_added={oracle_pairs_added} exhausted={} node_limit={}",
                candidate.selected.len(),
                master.nodes,
                master.sudoku_nodes,
                oracle.nodes,
                oracle.alternatives.len(),
                oracle.exhausted,
                oracle.node_limit_hit
            );
        }
        report.iterations.push(IterationLog {
            iteration,
            pair_cuts_before,
            master_nodes: master.nodes,
            master_sudoku_nodes: master.sudoku_nodes,
            swap_pairs_added,
            oracle_nodes: oracle.nodes,
            alternatives: oracle.alternatives.len(),
            oracle_pairs_added,
            oracle_exhausted: oracle.exhausted,
            oracle_node_limit_hit: oracle.node_limit_hit,
        });

        if oracle.alternatives.is_empty() {
            if oracle.exhausted {
                report.status = RunStatus::RelaxedUnique;
                report.final_candidate = Some(candidate);
            } else {
                report.status = RunStatus::OracleNodeLimit;
            }
            break;
        }
        if oracle_pairs_added == 0 {
            return Err("internal error: checker alternatives produced no new pair cut".into());
        }
    }

    report.final_pairs = pairs.len();
    report.duplicate_pairs = duplicate_pairs;
    report.elapsed_seconds = started.elapsed().as_secs_f64();
    if pairs.len() != persisted_pairs {
        write_checkpoint_if_requested(&options, &pairs)?;
    }
    let output = format_report(&options, &report, &edges, &pairs);
    if let Some(path) = &options.output {
        fs::write(path, output)
            .map_err(|error| format!("cannot write output {}: {error}", path.display()))?;
    } else {
        print!("{output}");
    }
    Ok(())
}

fn checkpoint_due(iteration: usize, checkpoint_every: usize) -> bool {
    iteration % checkpoint_every == checkpoint_every - 1
}

fn validate_candidate(
    candidate: &MasterCandidate,
    cuts: &[EdgeSet],
    edges: &[DirectedEdge],
    budget: usize,
) -> Result<(), String> {
    validate_sudoku(&candidate.witness)
        .map_err(|error| format!("internal master witness is invalid: {error}"))?;
    if candidate.selected.len() != budget {
        return Err(format!(
            "internal master selected {} edges, expected exactly {budget}",
            candidate.selected.len()
        ));
    }
    let mut seen = EdgeSet::default();
    for &edge in &candidate.selected {
        if edge >= edges.len() || seen.contains(edge) {
            return Err("internal master selected an invalid or repeated edge".into());
        }
        if !edge_true(edges[edge], &candidate.witness) {
            return Err("internal master selected an edge false in its witness".into());
        }
        seen.insert(edge);
    }
    if cuts.iter().any(|cut| !cut.intersects(seen)) {
        return Err("internal master candidate misses a learned pair cut".into());
    }
    Ok(())
}

fn swap_digits(mut grid: [u8; CELLS], left: u8, right: u8) -> [u8; CELLS] {
    for digit in &mut grid {
        if *digit == left {
            *digit = right;
        } else if *digit == right {
            *digit = left;
        }
    }
    grid
}

#[derive(Debug)]
struct Score3Seed {
    selected: Vec<usize>,
    solutions: Vec<[u8; CELLS]>,
    pairs: Vec<GridPair>,
}

fn score3_seed(edges: &[DirectedEdge]) -> Result<Score3Seed, String> {
    let paths: [&[usize]; 3] = [
        &[19, 29, 28, 20, 11, 12, 13, 3, 4],
        &[77, 69, 78, 70, 62, 53, 44, 52],
        &[41, 51],
    ];
    let mut selected = Vec::with_capacity(MAX_BUDGET);
    for path in paths {
        for step in path.windows(2) {
            let lower = step[0] as u8;
            let upper = step[1] as u8;
            let edge = edges
                .iter()
                .position(|candidate| candidate.lower == lower && candidate.upper == upper)
                .ok_or_else(|| format!("score-3 seed has non-king step {lower}->{upper}"))?;
            selected.push(edge);
        }
    }
    if selected.len() != MAX_BUDGET {
        return Err("internal score-3 seed does not contain sixteen comparisons".into());
    }
    let comparisons = selected_edges(&selected, edges);
    let mut solutions = Vec::new();
    let (status, _) = SudokuSearch::new(&comparisons).visit(None, None, |grid| {
        solutions.push(grid);
        if solutions.len() == 4 {
            VisitControl::Stop
        } else {
            VisitControl::Continue
        }
    });
    if status != SearchStatus::Exhausted || solutions.len() != 3 {
        return Err(format!(
            "score-3 seed expected exactly 3 solutions, got {} with status {status:?}",
            solutions.len()
        ));
    }
    let mut pairs = Vec::new();
    for left in 0..solutions.len() {
        for right in left + 1..solutions.len() {
            pairs.push(GridPair::new(solutions[left], solutions[right])?);
        }
    }
    Ok(Score3Seed {
        selected,
        solutions,
        pairs,
    })
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options::default();
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--budget" => {
                options.budget = parse_usize("--budget", arguments.next())?;
                if options.budget > MAX_BUDGET {
                    return Err(format!("--budget must be at most {MAX_BUDGET}"));
                }
            }
            "--max-iterations" => {
                options.max_iterations = parse_usize("--max-iterations", arguments.next())?;
            }
            "--master-node-limit" => {
                options.master_node_limit = parse_limit("--master-node-limit", arguments.next())?;
            }
            "--master-sudoku-node-limit" => {
                options.master_sudoku_node_limit =
                    parse_limit("--master-sudoku-node-limit", arguments.next())?;
            }
            "--oracle-node-limit" => {
                options.oracle_node_limit = parse_limit("--oracle-node-limit", arguments.next())?;
            }
            "--oracle-batch" => {
                options.oracle_batch = parse_usize("--oracle-batch", arguments.next())?;
                if options.oracle_batch == 0 {
                    return Err("--oracle-batch must be positive".into());
                }
            }
            "--pair-mode" => {
                options.pair_mode = match require_value("--pair-mode", arguments.next())?.as_str() {
                    "anchor" => PairMode::Anchor,
                    "all" => PairMode::All,
                    value => {
                        return Err(format!(
                            "invalid --pair-mode {value:?}; expected anchor or all"
                        ));
                    }
                };
            }
            "--checkpoint" => {
                options.checkpoint = Some(PathBuf::from(require_value(
                    "--checkpoint",
                    arguments.next(),
                )?));
            }
            "--checkpoint-every" => {
                options.checkpoint_every = parse_usize("--checkpoint-every", arguments.next())?;
                if options.checkpoint_every == 0 {
                    return Err("--checkpoint-every must be positive".into());
                }
            }
            "--pair" => {
                options
                    .direct_pairs
                    .push(require_value("--pair", arguments.next())?);
            }
            "--no-score3-seed" => options.score3_seed = false,
            "--progress-every" => {
                options.progress_every = parse_usize("--progress-every", arguments.next())?;
                if options.progress_every == 0 {
                    return Err("--progress-every must be positive".into());
                }
            }
            "--summary-only" => options.summary_only = true,
            "--output" => {
                options.output = Some(PathBuf::from(require_value("--output", arguments.next())?));
            }
            _ => return Err(format!("unknown option {argument:?}; use --help")),
        }
    }
    if let (Some(checkpoint), Some(output)) = (&options.checkpoint, &options.output)
        && destinations_equal(checkpoint, output)?
    {
        return Err("--checkpoint and --output must name different files".into());
    }
    Ok(options)
}

fn destinations_equal(left: &Path, right: &Path) -> Result<bool, String> {
    fn resolve(path: &Path) -> Result<PathBuf, String> {
        if path.exists() {
            return fs::canonicalize(path)
                .map_err(|error| format!("cannot resolve path {}: {error}", path.display()));
        }
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
        let parent = parent.unwrap_or_else(|| Path::new("."));
        let parent = fs::canonicalize(parent)
            .map_err(|error| format!("cannot resolve parent of {}: {error}", path.display()))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| format!("path {} has no file name", path.display()))?;
        Ok(parent.join(file_name))
    }

    let left = resolve(left)?;
    let right = resolve(right)?;
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

fn require_value(option: &str, value: Option<String>) -> Result<String, String> {
    value.ok_or_else(|| format!("{option} requires a value"))
}

fn parse_usize(option: &str, value: Option<String>) -> Result<usize, String> {
    let value = require_value(option, value)?;
    value
        .parse()
        .map_err(|_| format!("invalid value for {option}: {value:?}"))
}

fn parse_limit(option: &str, value: Option<String>) -> Result<Option<u64>, String> {
    let value = require_value(option, value)?;
    if value == "none" || value == "unlimited" {
        return Ok(None);
    }
    value
        .parse()
        .map(Some)
        .map_err(|_| format!("invalid value for {option}: {value:?}"))
}

fn print_help() {
    println!(
        "thermo-global-cegis - target-free relaxed comparison CEGIS pilot\n\
\n\
Usage: thermo-global-cegis [OPTIONS]\n\
\n\
  --budget N                       exact comparison count (default 16, max 16)\n\
  --max-iterations N               CEGIS refinement cap (default 100)\n\
  --master-node-limit N|none       hitting-search nodes per iteration\n\
  --master-sudoku-node-limit N|none  feasibility nodes per master call\n\
  --oracle-node-limit N|none       second-solution nodes per checker call\n\
  --oracle-batch N                 alternatives per checker call (default 32)\n\
  --pair-mode all|anchor           learn all batch pairs or witness pairs\n\
  --checkpoint PATH                load/save checksummed pair-cut checkpoint\n\
  --checkpoint-every N            persist every N refinements (default 1)\n\
  --pair GRID|GRID                 add one solved-Sudoku counterexample pair\n\
  --no-score3-seed                 omit the built-in exact-3 9+8+2 seed\n\
  --progress-every N               stderr progress period\n\
  --summary-only                   omit per-iteration records from output\n\
  --output PATH                    write the final report instead of stdout\n\
\n\
Node or iteration limits always produce an explicitly inconclusive result."
    );
}

fn parse_grid(text: &str) -> Result<[u8; CELLS], String> {
    let mut digits = Vec::with_capacity(CELLS);
    for character in text.chars() {
        match character {
            '1'..='9' => digits.push(character as u8 - b'0'),
            '/' | ',' | ';' | ':' | '-' | '_' | '[' | ']' | '(' | ')' => {}
            character if character.is_whitespace() => {}
            '0' | '.' => return Err("solved grids cannot contain blanks".into()),
            _ => return Err(format!("unexpected grid character {character:?}")),
        }
    }
    if digits.len() != CELLS {
        return Err(format!("expected 81 digits, found {}", digits.len()));
    }
    let mut grid = [0u8; CELLS];
    grid.copy_from_slice(&digits);
    Ok(grid)
}

fn format_grid(grid: &[u8; CELLS]) -> String {
    grid.iter().map(|digit| char::from(b'0' + digit)).collect()
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
        let bit = bit_for_digit(digit);
        if seen & bit != 0 {
            return Err(format!("digit {digit} is repeated"));
        }
        seen |= bit;
    }
    Ok(())
}

fn parse_pair(text: &str) -> Result<GridPair, String> {
    let (left, right) = text
        .split_once('|')
        .ok_or_else(|| "a pair must have the form GRID|GRID".to_string())?;
    if right.contains('|') {
        return Err("a pair must contain exactly one '|' separator".into());
    }
    let pair = GridPair::new(parse_grid(left)?, parse_grid(right)?)?;
    validate_pair(&pair)?;
    Ok(pair)
}

fn validate_pair(pair: &GridPair) -> Result<(), String> {
    validate_sudoku(&pair.first).map_err(|error| format!("first pair grid: {error}"))?;
    validate_sudoku(&pair.second).map_err(|error| format!("second pair grid: {error}"))?;
    if pair.first >= pair.second {
        return Err("pair grids are not distinct and canonically ordered".into());
    }
    Ok(())
}

fn pairs_checksum(pairs: &[GridPair]) -> u64 {
    let mut hash = FNV_OFFSET;
    for pair in pairs {
        for byte in pair.first {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash ^= 0xfe;
        hash = hash.wrapping_mul(FNV_PRIME);
        for byte in pair.second {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn edges_checksum(edges: &[DirectedEdge]) -> u64 {
    let mut hash = FNV_OFFSET;
    for edge in edges {
        hash ^= u64::from(edge.lower);
        hash = hash.wrapping_mul(FNV_PRIME);
        hash ^= u64::from(edge.upper);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn write_checkpoint_if_requested(options: &Options, pairs: &[GridPair]) -> Result<(), String> {
    let Some(path) = &options.checkpoint else {
        return Ok(());
    };
    write_checkpoint(path, options.budget, pairs)
}

fn write_checkpoint(path: &Path, budget: usize, pairs: &[GridPair]) -> Result<(), String> {
    let checksum = pairs_checksum(pairs);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock error while checkpointing: {error}"))?
        .as_nanos();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("checkpoint path {} has no UTF-8 file name", path.display()))?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));
    let file = fs::File::create(&temporary).map_err(|error| {
        format!(
            "cannot create temporary checkpoint {}: {error}",
            temporary.display()
        )
    })?;
    let mut writer = BufWriter::with_capacity(1 << 20, &file);
    writeln!(writer, "{CHECKPOINT_HEADER}")
        .and_then(|_| writeln!(writer, "# budget={budget}"))
        .and_then(|_| writeln!(writer, "# directed_edges={DIRECTED_EDGES}"))
        .and_then(|_| writeln!(writer, "# pairs={}", pairs.len()))
        .and_then(|_| writeln!(writer, "# fnv1a64={checksum:016x}"))
        .map_err(|error| format!("cannot write checkpoint {}: {error}", temporary.display()))?;
    let mut line = [0u8; CELLS * 2 + 2];
    line[CELLS] = b'|';
    line[CELLS * 2 + 1] = b'\n';
    for pair in pairs {
        for (target, digit) in line[..CELLS].iter_mut().zip(pair.first) {
            *target = b'0' + digit;
        }
        for (target, digit) in line[CELLS + 1..CELLS * 2 + 1].iter_mut().zip(pair.second) {
            *target = b'0' + digit;
        }
        writer
            .write_all(&line)
            .map_err(|error| format!("cannot write checkpoint {}: {error}", temporary.display()))?;
    }
    writeln!(
        writer,
        "# end pairs={} fnv1a64={checksum:016x}",
        pairs.len()
    )
    .and_then(|_| writer.flush())
    .map_err(|error| format!("cannot finish checkpoint {}: {error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync checkpoint {}: {error}", temporary.display()))?;
    drop(writer);
    drop(file);
    replace_file(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination).map_err(|error| {
        format!(
            "cannot atomically replace checkpoint {} with {}: {error}",
            destination.display(),
            source.display()
        )
    })
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    const REPLACE_ATTEMPTS: usize = 200;
    const RETRY_DELAY: Duration = Duration::from_millis(100);
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }
    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut last_error = None;
    for attempt in 0..REPLACE_ATTEMPTS {
        // SAFETY: both pointers reference live, NUL-terminated UTF-16 buffers
        // for the duration of the call; flags request a same-volume replace.
        let succeeded = unsafe {
            MoveFileExW(
                source_wide.as_ptr(),
                destination_wide.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if succeeded != 0 {
            return Ok(());
        }
        last_error = Some(std::io::Error::last_os_error());
        if attempt + 1 != REPLACE_ATTEMPTS {
            std::thread::sleep(RETRY_DELAY);
        }
    }
    Err(format!(
        "cannot atomically replace checkpoint {} with {} after {REPLACE_ATTEMPTS} attempts: {}",
        destination.display(),
        source.display(),
        last_error.expect("at least one replacement attempt")
    ))
}

fn load_checkpoint(path: &Path, budget: usize) -> Result<Vec<GridPair>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("cannot read checkpoint {}: {error}", path.display()))?;
    let mut lines = contents.lines();
    if lines.next() != Some(CHECKPOINT_HEADER) {
        return Err(format!(
            "checkpoint {} has the wrong or missing schema header",
            path.display()
        ));
    }
    let mut declared_budget = None;
    let mut declared_edges = None;
    let mut declared_pairs = None;
    let mut declared_checksum = None;
    let mut footer = None;
    let mut pairs = Vec::new();
    for line in lines {
        if let Some(value) = line.strip_prefix("# budget=") {
            declared_budget = value.parse::<usize>().ok();
        } else if let Some(value) = line.strip_prefix("# directed_edges=") {
            declared_edges = value.parse::<usize>().ok();
        } else if let Some(value) = line.strip_prefix("# pairs=") {
            declared_pairs = value.parse::<usize>().ok();
        } else if let Some(value) = line.strip_prefix("# fnv1a64=") {
            declared_checksum = u64::from_str_radix(value, 16).ok();
        } else if let Some(value) = line.strip_prefix("# end pairs=") {
            let (count, checksum) = value
                .split_once(" fnv1a64=")
                .ok_or_else(|| format!("malformed checkpoint footer in {}", path.display()))?;
            footer = Some((
                count.parse::<usize>().map_err(|_| {
                    format!("invalid checkpoint footer count in {}", path.display())
                })?,
                u64::from_str_radix(checksum, 16).map_err(|_| {
                    format!("invalid checkpoint footer checksum in {}", path.display())
                })?,
            ));
        } else if line.starts_with('#') || line.trim().is_empty() {
            return Err(format!(
                "unexpected checkpoint metadata line {line:?} in {}",
                path.display()
            ));
        } else {
            if footer.is_some() {
                return Err(format!(
                    "data occurs after checkpoint footer in {}",
                    path.display()
                ));
            }
            pairs.push(parse_pair(line).map_err(|error| {
                format!("invalid checkpoint pair in {}: {error}", path.display())
            })?);
        }
    }
    let expected_checksum = pairs_checksum(&pairs);
    let expected_count = pairs.len();
    if declared_budget != Some(budget)
        || declared_edges != Some(DIRECTED_EDGES)
        || declared_pairs != Some(expected_count)
        || declared_checksum != Some(expected_checksum)
        || footer != Some((expected_count, expected_checksum))
    {
        return Err(format!(
            "checkpoint {} metadata/checksum mismatch (expected budget={budget}, edges={DIRECTED_EDGES}, pairs={expected_count}, fnv1a64={expected_checksum:016x})",
            path.display()
        ));
    }
    let unique = pairs.iter().copied().collect::<HashSet<_>>();
    if unique.len() != pairs.len() {
        return Err(format!(
            "checkpoint {} contains duplicate pairs",
            path.display()
        ));
    }
    Ok(pairs)
}

fn format_report(
    options: &Options,
    report: &RunReport,
    edges: &[DirectedEdge],
    pairs: &[GridPair],
) -> String {
    let mut output = String::new();
    writeln!(output, "thermo-global-cegis-v1").unwrap();
    writeln!(
        output,
        "model=target-free-relaxed-overlapping-directed-king-comparisons"
    )
    .unwrap();
    writeln!(output, "selection_cardinality=exactly").unwrap();
    writeln!(output, "budget={}", options.budget).unwrap();
    writeln!(output, "directed_edge_universe={DIRECTED_EDGES}").unwrap();
    writeln!(
        output,
        "directed_edge_order=unordered-cell-pairs-lexicographic-forward-then-reverse-v1"
    )
    .unwrap();
    writeln!(
        output,
        "directed_edge_order_fnv1a64={:016x}",
        edges_checksum(edges)
    )
    .unwrap();
    writeln!(output, "pair_cut_semantics=edge-not-true-in-both-grids").unwrap();
    writeln!(
        output,
        "pair_mode={}",
        match options.pair_mode {
            PairMode::Anchor => "anchor",
            PairMode::All => "all",
        }
    )
    .unwrap();
    writeln!(output, "score3_seed={}", options.score3_seed).unwrap();
    writeln!(
        output,
        "score3_seed_layout=19,29,28,20,11,12,13,3,4|77,69,78,70,62,53,44,52|41,51"
    )
    .unwrap();
    writeln!(output, "max_iterations={}", options.max_iterations).unwrap();
    writeln!(
        output,
        "master_node_limit={}",
        format_limit(options.master_node_limit)
    )
    .unwrap();
    writeln!(
        output,
        "master_sudoku_node_limit={}",
        format_limit(options.master_sudoku_node_limit)
    )
    .unwrap();
    writeln!(
        output,
        "oracle_node_limit={}",
        format_limit(options.oracle_node_limit)
    )
    .unwrap();
    writeln!(output, "oracle_batch={}", options.oracle_batch).unwrap();
    writeln!(
        output,
        "exact_budget_covers_at_most_budget_by_witness_true_padding=true"
    )
    .unwrap();
    writeln!(output, "checkpoint_integrity=count-fnv1a64-footer").unwrap();
    writeln!(output, "checkpoint_write_atomic=true").unwrap();
    writeln!(
        output,
        "checkpoint_every_completed_refinements={}",
        options.checkpoint_every
    )
    .unwrap();
    writeln!(output, "checkpoint_clean_exit_flush=true").unwrap();
    writeln!(
        output,
        "checkpoint_crash_loss_max_completed_refinements={}",
        options.checkpoint_every - 1
    )
    .unwrap();
    if let Some(path) = &options.checkpoint {
        writeln!(output, "pair_witness_checkpoint={}", path.display()).unwrap();
        writeln!(
            output,
            "pair_witness_checkpoint_fnv1a64={:016x}",
            pairs_checksum(pairs)
        )
        .unwrap();
        writeln!(output, "pair_witness_pairs_persisted=true").unwrap();
        writeln!(output, "pair_witness_persisted_pairs={}", pairs.len()).unwrap();
    } else {
        writeln!(output, "pair_witness_checkpoint=none").unwrap();
        writeln!(output, "pair_witness_pairs_persisted=false").unwrap();
        writeln!(output, "pair_witness_persisted_pairs=0").unwrap();
    }
    writeln!(output, "result={}", report.status.label()).unwrap();
    let relaxed_conclusion = match report.status {
        RunStatus::RelaxedUnique => "unique-witness-exists",
        RunStatus::RelaxedExcluded => "excluded-at-exact-budget",
        _ => "inconclusive",
    };
    writeln!(output, "relaxed_model_conclusion={relaxed_conclusion}").unwrap();
    let nonoverlapping_conclusion =
        if report.status == RunStatus::RelaxedExcluded && options.budget == MAX_BUDGET {
            "excluded-by-relaxation"
        } else {
            "inconclusive"
        };
    writeln!(
        output,
        "nonoverlapping_19c_conclusion={nonoverlapping_conclusion}"
    )
    .unwrap();
    writeln!(
        output,
        "geometric_19c_pilot_inconclusive={}",
        nonoverlapping_conclusion == "inconclusive"
    )
    .unwrap();
    writeln!(output, "proof_trace_available=false").unwrap();
    writeln!(
        output,
        "solver_exhaustive_uniqueness_check={}",
        matches!(report.status, RunStatus::RelaxedUnique)
    )
    .unwrap();
    writeln!(
        output,
        "recheckable_positive_witness={}",
        matches!(report.status, RunStatus::RelaxedUnique)
    )
    .unwrap();
    writeln!(output, "initial_pair_cuts={}", report.initial_pairs).unwrap();
    writeln!(output, "final_pair_cuts={}", report.final_pairs).unwrap();
    writeln!(output, "input_duplicate_pairs={}", report.duplicate_pairs).unwrap();
    writeln!(output, "iterations={}", report.iterations.len()).unwrap();
    writeln!(output, "total_master_nodes={}", report.total_master_nodes).unwrap();
    writeln!(
        output,
        "total_master_sudoku_nodes={}",
        report.total_master_sudoku_nodes
    )
    .unwrap();
    writeln!(output, "total_oracle_nodes={}", report.total_oracle_nodes).unwrap();
    writeln!(output, "elapsed_seconds={:.6}", report.elapsed_seconds).unwrap();
    if let Some(candidate) = &report.final_candidate {
        writeln!(output, "witness_target={}", format_grid(&candidate.witness)).unwrap();
        writeln!(
            output,
            "selected_edge_ids={}",
            join_usize(&candidate.selected, ";")
        )
        .unwrap();
        let comparisons = candidate
            .selected
            .iter()
            .map(|&edge| format!("{}<{}", edges[edge].lower, edges[edge].upper))
            .collect::<Vec<_>>()
            .join(";");
        writeln!(output, "comparisons={comparisons}").unwrap();
        writeln!(output, "checker_exhausted=true").unwrap();
    }
    if !options.summary_only {
        for item in &report.iterations {
            writeln!(
                output,
                "iteration={} cuts_before={} master_nodes={} master_sudoku_nodes={} swap_pairs_added={} oracle_nodes={} alternatives={} oracle_pairs_added={} oracle_exhausted={} oracle_node_limit_hit={}",
                item.iteration,
                item.pair_cuts_before,
                item.master_nodes,
                item.master_sudoku_nodes,
                item.swap_pairs_added,
                item.oracle_nodes,
                item.alternatives,
                item.oracle_pairs_added,
                item.oracle_exhausted,
                item.oracle_node_limit_hit
            )
            .unwrap();
        }
    }
    output
}

fn join_usize(values: &[usize], separator: &str) -> String {
    values
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(separator)
}

fn format_limit(limit: Option<u64>) -> String {
    limit.map_or_else(|| "none".to_string(), |value| value.to_string())
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
    use std::time::{SystemTime, UNIX_EPOCH};

    const CANONICAL: &str = concat!(
        "123456789",
        "456789123",
        "789123456",
        "234567891",
        "567891234",
        "891234567",
        "345678912",
        "678912345",
        "912345678"
    );

    fn canonical() -> [u8; CELLS] {
        parse_grid(CANONICAL).unwrap()
    }

    fn ids_for_paths(edges: &[DirectedEdge], paths: &[&[usize]]) -> Vec<usize> {
        paths
            .iter()
            .flat_map(|path| path.windows(2))
            .map(|step| {
                edges
                    .iter()
                    .position(|edge| {
                        edge.lower as usize == step[0] && edge.upper as usize == step[1]
                    })
                    .unwrap()
            })
            .collect()
    }

    fn count_up_to(
        selected: &[usize],
        edges: &[DirectedEdge],
        cap: usize,
    ) -> (usize, bool, Vec<[u8; CELLS]>) {
        let comparisons = selected_edges(selected, edges);
        let mut solutions = Vec::new();
        let (status, _) = SudokuSearch::new(&comparisons).visit(None, None, |grid| {
            solutions.push(grid);
            if solutions.len() == cap {
                VisitControl::Stop
            } else {
                VisitControl::Continue
            }
        });
        (
            solutions.len(),
            status == SearchStatus::Exhausted,
            solutions,
        )
    }

    #[test]
    fn directed_universe_has_544_paired_reversals() {
        let edges = directed_edges();
        assert_eq!(edges.len(), 544);
        for pair in edges.chunks_exact(2) {
            assert_eq!(pair[0].lower, pair[1].upper);
            assert_eq!(pair[0].upper, pair[1].lower);
        }
        assert_eq!(edges.iter().copied().collect::<HashSet<_>>().len(), 544);
    }

    #[test]
    fn in_place_cut_partition_preserves_every_cut() {
        let mut active = (0..97usize)
            .map(|index| {
                let mut cut = EdgeSet::default();
                for edge in 0..DIRECTED_EDGES {
                    if (index * 37 + edge * 13) % 11 < 4 {
                        cut.insert(edge);
                    }
                }
                cut
            })
            .collect::<Vec<_>>();
        let mut expected = active.clone();
        expected.sort_unstable();
        let edge_id = 317;
        let kept = partition_unhit(&mut active, edge_id);
        assert!(active[..kept].iter().all(|cut| !cut.contains(edge_id)));
        assert!(active[kept..].iter().all(|cut| cut.contains(edge_id)));
        active.sort_unstable();
        assert_eq!(active, expected);
    }

    #[test]
    fn pair_cut_is_exactly_edges_not_true_in_both() {
        let edges = directed_edges();
        let first = canonical();
        let second = swap_digits(first, 1, 2);
        let pair = GridPair::new(first, second).unwrap();
        let cut = pair_cut(&pair, &edges);
        for (edge_id, &edge) in edges.iter().enumerate() {
            assert_eq!(
                cut.contains(edge_id),
                !(edge_true(edge, &first) && edge_true(edge, &second))
            );
        }
        let equality_grid = parse_grid(
            "326891745985674123714523869832769514697415238451238697243157986178946352569382471",
        )
        .unwrap();
        let equality_pair = GridPair::new(equality_grid, swap_digits(equality_grid, 1, 2)).unwrap();
        let equality_cut = pair_cut(&equality_pair, &edges);
        let equal_pair = edges
            .chunks_exact(2)
            .position(|directions| {
                equality_grid[directions[0].lower as usize]
                    == equality_grid[directions[0].upper as usize]
            })
            .unwrap();
        assert!(equality_cut.contains(equal_pair * 2));
        assert!(equality_cut.contains(equal_pair * 2 + 1));
    }

    #[test]
    fn adjacent_symbol_swap_cuts_are_disjoint_under_witness() {
        let edges = directed_edges();
        let witness = canonical();
        let mut union = EdgeSet::default();
        for digit in 1..=8 {
            let pair = GridPair::new(witness, swap_digits(witness, digit, digit + 1)).unwrap();
            let cut = pair_cut(&pair, &edges);
            let mut under_witness = EdgeSet::default();
            for (edge_id, &edge) in edges.iter().enumerate() {
                if cut.contains(edge_id) && edge_true(edge, &witness) {
                    assert_eq!(witness[edge.lower as usize], digit);
                    assert_eq!(witness[edge.upper as usize], digit + 1);
                    under_witness.insert(edge_id);
                }
            }
            assert!(!under_witness.is_empty());
            assert!(!under_witness.intersects(union));
            for edge in under_witness.iter() {
                union.insert(edge);
            }
        }
    }

    #[test]
    fn blank_find_one_and_node_limit_are_distinct() {
        let edges = directed_edges();
        let found = find_one(&[], &edges, None);
        assert_eq!(found.status, FeasibilityStatus::Satisfiable);
        validate_sudoku(&found.witness.unwrap()).unwrap();
        let limited = find_one(&[], &edges, Some(0));
        assert_eq!(limited.status, FeasibilityStatus::NodeLimit);
        assert_eq!(limited.nodes, 0);
    }

    #[test]
    fn master_pads_to_exactly_sixteen_true_edges() {
        let edges = directed_edges();
        let result = JointMaster::solve(&[], &edges, 16, None, None);
        assert_eq!(result.status, MasterStatus::Candidate);
        let candidate = result.candidate.unwrap();
        validate_candidate(&candidate, &[], &edges, 16).unwrap();
        assert_eq!(candidate.selected.len(), 16);
    }

    #[test]
    fn tiny_joint_master_matches_brute_force() {
        let edges = directed_edges();
        let mut first = EdgeSet::default();
        first.insert(0);
        let mut second = EdgeSet::default();
        second.insert(2);
        let cuts = [first, second];
        let one = JointMaster::solve(&cuts, &edges, 1, None, None);
        assert_eq!(one.status, MasterStatus::Exhausted);
        let two = JointMaster::solve(&cuts, &edges, 2, None, None);
        assert_eq!(two.status, MasterStatus::Candidate);
        validate_candidate(&two.candidate.unwrap(), &cuts, &edges, 2).unwrap();
        let limited = JointMaster::solve(&cuts, &edges, 2, Some(0), None);
        assert_eq!(limited.status, MasterStatus::MasterNodeLimit);
    }

    #[test]
    fn score3_seed_is_three_pairwise_counterexamples() {
        let edges = directed_edges();
        let seed = score3_seed(&edges).unwrap();
        assert_eq!(seed.pairs.len(), 3);
        assert_eq!(seed.pairs.iter().copied().collect::<HashSet<_>>().len(), 3);
        let cuts = seed
            .pairs
            .iter()
            .map(|pair| pair_cut(pair, &edges))
            .collect::<Vec<_>>();
        let master = JointMaster::solve_with_hint(
            &cuts,
            &edges,
            16,
            None,
            None,
            Some(seed.solutions[0]),
            &seed.selected,
        );
        let candidate = master.candidate.unwrap();
        assert_eq!(candidate.witness, seed.solutions[0]);
        assert_eq!(
            candidate
                .selected
                .iter()
                .filter(|edge| seed.selected.contains(edge))
                .count(),
            15
        );
    }

    #[test]
    fn all_pair_batch_cuts_are_missed_by_checked_set() {
        let edges = directed_edges();
        let paths: [&[usize]; 3] = [
            &[19, 29, 28, 20, 11, 12, 13, 3, 4],
            &[77, 69, 78, 70, 62, 53, 44, 52],
            &[41, 51],
        ];
        let selected = ids_for_paths(&edges, &paths);
        let (count, exhausted, solutions) = count_up_to(&selected, &edges, 4);
        assert_eq!((count, exhausted), (3, true));
        let selected_bits = selected_set(&selected);
        let mut pair_count = 0;
        for left in 0..solutions.len() {
            for right in left + 1..solutions.len() {
                let pair = GridPair::new(solutions[left], solutions[right]).unwrap();
                assert!(!pair_cut(&pair, &edges).intersects(selected_bits));
                pair_count += 1;
            }
        }
        assert_eq!(pair_count, solutions.len() * (solutions.len() - 1) / 2);
    }

    #[test]
    fn blue_full_is_unique_and_every_leave_one_out_is_multiple() {
        let edges = directed_edges();
        let paths: [&[usize]; 3] = [
            &[18, 27, 28, 19, 20, 11, 12, 13, 4],
            &[57, 48, 49],
            &[59, 68, 69, 60, 61, 52, 53, 44],
        ];
        let selected = ids_for_paths(&edges, &paths);
        assert_eq!(selected.len(), 17);
        let (count, exhausted, _) = count_up_to(&selected, &edges, 2);
        assert_eq!((count, exhausted), (1, true));
        for omitted in 0..selected.len() {
            let subset = selected
                .iter()
                .enumerate()
                .filter_map(|(index, &edge)| (index != omitted).then_some(edge))
                .collect::<Vec<_>>();
            let (count, _, _) = count_up_to(&subset, &edges, 2);
            assert_eq!(count, 2, "leave-one-out index {omitted}");
        }
    }

    #[test]
    #[ignore = "8,448-case exhaustive differential regression; run explicitly"]
    fn score3_full_one_edge_replacement_neighborhood() {
        let edges = directed_edges();
        let paths: [&[usize]; 3] = [
            &[19, 29, 28, 20, 11, 12, 13, 3, 4],
            &[77, 69, 78, 70, 62, 53, 44, 52],
            &[41, 51],
        ];
        let original = ids_for_paths(&edges, &paths);
        let original_set = original.iter().copied().collect::<HashSet<_>>();
        let mut zero = 0usize;
        let mut unique = 0usize;
        let mut multiple = 0usize;
        for omitted in 0..original.len() {
            for replacement in 0..edges.len() {
                if original_set.contains(&replacement) {
                    continue;
                }
                let mut selected = original.clone();
                selected[omitted] = replacement;
                let (count, exhausted, _) = count_up_to(&selected, &edges, 2);
                match (count, exhausted) {
                    (0, true) => zero += 1,
                    (1, true) => unique += 1,
                    (2, _) => multiple += 1,
                    outcome => panic!("unexpected capped-count outcome {outcome:?}"),
                }
            }
        }
        assert_eq!((zero, unique, multiple), (722, 0, 7_726));
    }

    #[test]
    fn checkpoint_round_trip_and_integrity_rejections() {
        let first = canonical();
        let second = swap_digits(first, 1, 2);
        let pair = GridPair::new(first, second).unwrap();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "thermo-global-cegis-test-{}-{nonce}.txt",
            std::process::id()
        ));
        write_checkpoint(&path, 16, &[pair]).unwrap();
        assert_eq!(load_checkpoint(&path, 16).unwrap(), vec![pair]);
        assert!(load_checkpoint(&path, 15).is_err());

        let valid = fs::read_to_string(&path).unwrap();
        let truncated = valid
            .lines()
            .take(valid.lines().count() - 1)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, truncated).unwrap();
        assert!(load_checkpoint(&path, 16).is_err());

        write_checkpoint(&path, 16, &[pair, pair]).unwrap();
        assert!(load_checkpoint(&path, 16).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn checkpoint_and_output_aliases_are_detected() {
        let current = env::current_dir().unwrap();
        let name = "thermo-global-cegis-alias-test.txt";
        assert!(destinations_equal(Path::new(name), &current.join(name)).unwrap());
    }

    #[test]
    fn checkpoint_batch_schedule_is_one_based() {
        assert!(checkpoint_due(0, 1));
        assert!(checkpoint_due(99, 1));
        assert!(!checkpoint_due(0, 10));
        assert!(!checkpoint_due(8, 10));
        assert!(checkpoint_due(9, 10));
        assert!(checkpoint_due(19, 10));
    }

    #[test]
    fn non_sixteen_budget_does_not_claim_19c_exclusion() {
        let options = Options {
            budget: 0,
            score3_seed: false,
            ..Options::default()
        };
        let report = RunReport {
            status: RunStatus::RelaxedExcluded,
            initial_pairs: 0,
            final_pairs: 0,
            duplicate_pairs: 0,
            iterations: Vec::new(),
            total_master_nodes: 0,
            total_master_sudoku_nodes: 0,
            total_oracle_nodes: 0,
            final_candidate: None,
            elapsed_seconds: 0.0,
        };
        let rendered = format_report(&options, &report, &directed_edges(), &[]);
        assert!(rendered.contains("result=relaxed-exact-budget-excluded"));
        assert!(rendered.contains("nonoverlapping_19c_conclusion=inconclusive"));
        assert!(rendered.contains("checkpoint_every_completed_refinements=1"));
        assert!(rendered.contains("checkpoint_crash_loss_max_completed_refinements=0"));
        assert!(!rendered.contains("result=relaxed-16-excluded"));
    }
}
