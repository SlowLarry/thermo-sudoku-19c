use std::collections::HashMap;
use std::fmt;

const ALL: u16 = 0x01ff;
const NO_CELL: u8 = u8::MAX;
const DEFAULT_EXTENSION_PREFIX_SOLUTIONS: u64 = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Thermometer {
    cells: [u8; 9],
    length: u8,
}

impl Thermometer {
    pub fn cells(&self) -> &[u8] {
        &self.cells[..self.length as usize]
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layout {
    thermometers: Vec<Thermometer>,
    thermo_degree: [u8; 81],
    thermo_of: [u8; 81],
    all_thermos: u64,
}

impl Layout {
    pub fn new(paths: &[Vec<u8>]) -> Result<Self, LayoutError> {
        Self::from_paths(paths.iter().map(Vec::as_slice))
    }

    fn from_paths<'a, I>(paths: I) -> Result<Self, LayoutError>
    where
        I: IntoIterator<Item = &'a [u8]>,
        I::IntoIter: ExactSizeIterator,
    {
        let paths = paths.into_iter();
        let thermo_count = paths.len();
        let mut occupied = [false; 81];
        let mut thermo_degree = [0u8; 81];
        let mut thermo_of = [NO_CELL; 81];
        let mut thermometers = Vec::with_capacity(thermo_count);

        for (thermo_index, path) in paths.enumerate() {
            if !(2..=9).contains(&path.len()) {
                return Err(LayoutError::InvalidLength {
                    thermo: thermo_index,
                    length: path.len(),
                });
            }

            let mut local = [false; 81];
            for (position, &cell) in path.iter().enumerate() {
                if cell >= 81 {
                    return Err(LayoutError::CellOutOfRange {
                        thermo: thermo_index,
                        position,
                        cell,
                    });
                }
                if local[cell as usize] {
                    return Err(LayoutError::RepeatedCell {
                        thermo: thermo_index,
                        cell,
                    });
                }
                if occupied[cell as usize] {
                    return Err(LayoutError::Overlap { cell });
                }
                local[cell as usize] = true;
            }

            for position in 1..path.len() {
                if !king_adjacent(path[position - 1], path[position]) {
                    return Err(LayoutError::NonAdjacent {
                        thermo: thermo_index,
                        from: path[position - 1],
                        to: path[position],
                    });
                }
            }

            for &cell in path {
                occupied[cell as usize] = true;
                thermo_of[cell as usize] = thermo_index as u8;
            }
            for edge in path.windows(2) {
                thermo_degree[edge[0] as usize] += 1;
                thermo_degree[edge[1] as usize] += 1;
            }
            let mut cells = [NO_CELL; 9];
            cells[..path.len()].copy_from_slice(path);
            thermometers.push(Thermometer {
                cells,
                length: path.len() as u8,
            });
        }

        Ok(Self {
            thermometers,
            thermo_degree,
            thermo_of,
            all_thermos: if thermo_count == 0 {
                0
            } else {
                (1u64 << thermo_count) - 1
            },
        })
    }

    pub fn empty() -> Self {
        Self {
            thermometers: Vec::new(),
            thermo_degree: [0; 81],
            thermo_of: [NO_CELL; 81],
            all_thermos: 0,
        }
    }

    pub fn thermometers(&self) -> &[Thermometer] {
        &self.thermometers
    }

    pub fn covered_cells(&self) -> usize {
        self.thermometers.iter().map(|t| t.cells().len()).sum()
    }

    fn with_two_cell_extension(&self, bulb: u8, tip: u8) -> Self {
        debug_assert!(king_adjacent(bulb, tip));
        debug_assert_eq!(self.thermo_of[bulb as usize], NO_CELL);
        debug_assert_eq!(self.thermo_of[tip as usize], NO_CELL);
        let mut layout = self.clone();
        let thermo_index = layout.thermometers.len();
        debug_assert!(thermo_index < 40);
        let mut cells = [NO_CELL; 9];
        cells[0] = bulb;
        cells[1] = tip;
        layout.thermometers.push(Thermometer { cells, length: 2 });
        layout.thermo_degree[bulb as usize] = 1;
        layout.thermo_degree[tip as usize] = 1;
        layout.thermo_of[bulb as usize] = thermo_index as u8;
        layout.thermo_of[tip as usize] = thermo_index as u8;
        layout.all_thermos |= 1u64 << thermo_index;
        layout
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutError {
    InvalidLength {
        thermo: usize,
        length: usize,
    },
    CellOutOfRange {
        thermo: usize,
        position: usize,
        cell: u8,
    },
    RepeatedCell {
        thermo: usize,
        cell: u8,
    },
    Overlap {
        cell: u8,
    },
    NonAdjacent {
        thermo: usize,
        from: u8,
        to: u8,
    },
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InvalidLength { thermo, length } => {
                write!(
                    f,
                    "thermometer {thermo} has length {length}; expected 2..=9"
                )
            }
            Self::CellOutOfRange {
                thermo,
                position,
                cell,
            } => write!(
                f,
                "thermometer {thermo}, position {position}: cell {cell} is outside 0..=80"
            ),
            Self::RepeatedCell { thermo, cell } => {
                write!(f, "thermometer {thermo} repeats cell {cell}")
            }
            Self::Overlap { cell } => write!(f, "cell {cell} occurs in multiple thermometers"),
            Self::NonAdjacent { thermo, from, to } => {
                write!(f, "thermometer {thermo} has non-adjacent step {from}->{to}")
            }
        }
    }
}

impl std::error::Error for LayoutError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProblemError {
    InvalidGiven { cell: usize, digit: u8 },
    Layout(LayoutError),
}

impl fmt::Display for ProblemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGiven { cell, digit } => {
                write!(f, "given at cell {cell} is {digit}; expected 0..=9")
            }
            Self::Layout(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ProblemError {}

impl From<LayoutError> for ProblemError {
    fn from(value: LayoutError) -> Self {
        Self::Layout(value)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SolveStats {
    pub nodes: u64,
    pub branches: u64,
    pub propagation_rounds: u64,
    pub thermo_revisions: u64,
    pub max_depth: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SolveResult {
    /// Number found, capped at the requested limit.
    pub count: u64,
    /// True when search stopped at the limit, so the exact count may be larger.
    pub capped: bool,
    pub first_solution: Option<[u8; 81]>,
    pub second_solution: Option<[u8; 81]>,
    pub stats: SolveStats,
}

/// An exact prefix of the complete solutions for one fixed problem.
///
/// When `exhausted` is true, `solutions` contains every solution. When
/// `capped` is true, search found one additional solution beyond the returned
/// prefix and stopped, so more solutions exist. The two flags are always
/// complements.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SolutionBatch {
    pub solutions: Vec<[u8; 81]>,
    pub exhausted: bool,
    pub capped: bool,
    pub stats: SolveStats,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Multiplicity {
    Zero,
    Unique,
    Multiple,
}

impl SolveResult {
    pub fn multiplicity(&self) -> Multiplicity {
        match self.count {
            0 => Multiplicity::Zero,
            1 if !self.capped => Multiplicity::Unique,
            _ => Multiplicity::Multiple,
        }
    }
}

/// One legal directed king-neighbour inequality disjoint from the base layout.
///
/// `count` is saturated at two. Counts zero and one are exact precisely when
/// `exact` is true; two always means "at least two".
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TwoCellExtension {
    pub bulb: u8,
    pub tip: u8,
    pub count: u8,
    pub exact: bool,
    pub first_witness: Option<u32>,
    pub second_witness: Option<u32>,
}

impl TwoCellExtension {
    /// Returns `None` only for a partial collective result whose zero/one
    /// upper bound has not yet been proved. Completed public screens do not
    /// expose that state.
    pub fn multiplicity(&self) -> Option<Multiplicity> {
        match (self.count, self.exact) {
            (0, true) => Some(Multiplicity::Zero),
            (1, true) => Some(Multiplicity::Unique),
            (2.., _) => Some(Multiplicity::Multiple),
            _ => None,
        }
    }
}

/// Collective classification of every legal disjoint two-cell extension.
///
/// The witness pool is a compact, independently checkable certificate for all
/// extensions classified as multiple: each such edge indexes two distinct base
/// solutions that satisfy its directed inequality.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TwoCellScreenResult {
    pub base_solutions_visited: u64,
    pub base_exhausted: bool,
    pub collective_solution_limit: Option<u64>,
    pub fallback_searches: u32,
    pub extensions: Vec<TwoCellExtension>,
    pub witness_solutions: Vec<[u8; 81]>,
    pub stats: SolveStats,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NineEightScreenResult {
    pub compatible_templates: u8,
    pub screen: TwoCellScreenResult,
}

impl TwoCellScreenResult {
    pub fn zero_count(&self) -> usize {
        self.extensions
            .iter()
            .filter(|extension| extension.multiplicity() == Some(Multiplicity::Zero))
            .count()
    }

    pub fn unique_count(&self) -> usize {
        self.extensions
            .iter()
            .filter(|extension| extension.multiplicity() == Some(Multiplicity::Unique))
            .count()
    }

    pub fn multiple_count(&self) -> usize {
        self.extensions.len() - self.zero_count() - self.unique_count()
    }
}

const fn make_cell_house_bits() -> [u32; 81] {
    let mut bits = [0u32; 81];
    let mut cell = 0usize;
    while cell < 81 {
        let row = cell / 9;
        let col = cell % 9;
        let box_index = (row / 3) * 3 + col / 3;
        bits[cell] = (1u32 << row) | (1u32 << (9 + col)) | (1u32 << (18 + box_index));
        cell += 1;
    }
    bits
}

const CELL_HOUSE_BITS: [u32; 81] = make_cell_house_bits();

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
                    let box_col = (box_index % 3) * 3;
                    ((box_row + position / 3) * 9 + box_col + position % 3) as u8
                }
            };
            position += 1;
        }
        house += 1;
    }
    cells
}

const HOUSE_CELLS: [[u8; 9]; 27] = make_house_cells();
const BOX_HOUSES: u32 = 0x01ff << 18;

const fn push_peer_const(
    output: &mut [u8; 20],
    seen: &mut [bool; 81],
    count: &mut usize,
    cell: usize,
) {
    if !seen[cell] {
        seen[cell] = true;
        output[*count] = cell as u8;
        *count += 1;
    }
}

const fn make_peers() -> [[u8; 20]; 81] {
    let mut result = [[NO_CELL; 20]; 81];
    let mut cell = 0usize;
    while cell < 81 {
        let row = cell / 9;
        let col = cell % 9;
        let mut seen = [false; 81];
        seen[cell] = true;
        let mut count = 0usize;

        let mut index = 0usize;
        while index < 9 {
            push_peer_const(&mut result[cell], &mut seen, &mut count, row * 9 + index);
            push_peer_const(&mut result[cell], &mut seen, &mut count, index * 9 + col);
            index += 1;
        }

        let box_row = (row / 3) * 3;
        let box_col = (col / 3) * 3;
        let mut dr = 0usize;
        while dr < 3 {
            let mut dc = 0usize;
            while dc < 3 {
                push_peer_const(
                    &mut result[cell],
                    &mut seen,
                    &mut count,
                    (box_row + dr) * 9 + box_col + dc,
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

const PEERS: [[u8; 20]; 81] = make_peers();
#[cfg(test)]
const ALL_HOUSES: u32 = (1u32 << 27) - 1;

#[derive(Clone, Copy, Debug, Default)]
struct Work {
    single_lo: u64,
    single_hi: u32,
    dirty_houses: u32,
    dirty_thermos: u64,
}

#[derive(Clone, Copy)]
struct SearchOptions {
    limit: u64,
    capture_solutions: bool,
}

struct ExtensionAccumulator {
    extensions: Vec<TwoCellExtension>,
    pending: Vec<usize>,
    witness_solutions: Vec<[u8; 81]>,
    base_solutions_visited: u64,
    solution_limit: Option<u64>,
    budget_reached: bool,
}

impl ExtensionAccumulator {
    fn new(layout: &Layout, solution_limit: Option<u64>) -> Self {
        let mut extensions = Vec::new();
        for bulb in 0u8..81 {
            if layout.thermo_of[bulb as usize] != NO_CELL {
                continue;
            }
            for tip in 0u8..81 {
                if layout.thermo_of[tip as usize] == NO_CELL && king_adjacent(bulb, tip) {
                    extensions.push(TwoCellExtension {
                        bulb,
                        tip,
                        count: 0,
                        exact: false,
                        first_witness: None,
                        second_witness: None,
                    });
                }
            }
        }
        Self {
            pending: (0..extensions.len()).collect(),
            extensions,
            witness_solutions: Vec::new(),
            base_solutions_visited: 0,
            solution_limit,
            budget_reached: false,
        }
    }

    /// Returns true while at least one edge still needs a second witness.
    fn observe(&mut self, state: &[u16; 81]) -> bool {
        self.base_solutions_visited += 1;
        let mut witness = None;
        let mut write = 0usize;
        for read in 0..self.pending.len() {
            let index = self.pending[read];
            let satisfies = {
                let extension = &self.extensions[index];
                state[extension.bulb as usize] < state[extension.tip as usize]
            };
            if satisfies {
                let witness = *witness.get_or_insert_with(|| {
                    let witness = self.witness_solutions.len() as u32;
                    self.witness_solutions.push(masks_to_solution(state));
                    witness
                });
                let extension = &mut self.extensions[index];
                match extension.count {
                    0 => extension.first_witness = Some(witness),
                    1 => extension.second_witness = Some(witness),
                    _ => unreachable!("saturated extensions are removed from pending"),
                }
                extension.count += 1;
            }
            if self.extensions[index].count < 2 {
                self.pending[write] = index;
                write += 1;
            }
        }
        self.pending.truncate(write);
        if self.pending.is_empty() {
            return false;
        }
        if self
            .solution_limit
            .is_some_and(|limit| self.base_solutions_visited >= limit)
        {
            self.budget_reached = true;
            return false;
        }
        true
    }
}

impl Work {
    #[inline]
    fn add_single(&mut self, cell: usize) {
        if cell < 64 {
            self.single_lo |= 1u64 << cell;
        } else {
            self.single_hi |= 1u32 << (cell - 64);
        }
    }

    #[inline]
    fn pop_single(&mut self) -> Option<usize> {
        if self.single_lo != 0 {
            let cell = self.single_lo.trailing_zeros() as usize;
            self.single_lo &= self.single_lo - 1;
            Some(cell)
        } else if self.single_hi != 0 {
            let cell = self.single_hi.trailing_zeros() as usize;
            self.single_hi &= self.single_hi - 1;
            Some(cell + 64)
        } else {
            None
        }
    }
}

#[inline(always)]
fn restrict_domain(
    layout: &Layout,
    state: &mut [u16; 81],
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
    let thermo = layout.thermo_of[cell];
    if thermo != NO_CELL {
        work.dirty_thermos |= 1u64 << thermo;
    }
    if !old.is_power_of_two() && next.is_power_of_two() {
        work.add_single(cell);
    }
    true
}

#[derive(Clone, Debug)]
pub struct Solver {
    givens: [u8; 81],
    layout: Layout,
}

impl Solver {
    pub fn new(givens: [u8; 81], paths: &[Vec<u8>]) -> Result<Self, ProblemError> {
        Self::from_layout(givens, Layout::new(paths)?)
    }

    fn from_layout(givens: [u8; 81], layout: Layout) -> Result<Self, ProblemError> {
        for (cell, &digit) in givens.iter().enumerate() {
            if digit > 9 {
                return Err(ProblemError::InvalidGiven { cell, digit });
            }
        }
        Ok(Self { givens, layout })
    }

    pub fn blank(paths: &[Vec<u8>]) -> Result<Self, ProblemError> {
        Self::new([0; 81], paths)
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn classify(&self) -> SolveResult {
        self.count_up_to(2)
    }

    pub fn count_up_to(&self, limit: u64) -> SolveResult {
        self.count_up_to_internal(limit, true)
    }

    /// Return at most `limit` distinct complete solutions.
    ///
    /// Search continues after filling the batch until it either exhausts the
    /// remaining tree or finds one additional solution. Consequently a batch
    /// containing exactly `limit` solutions can still report `exhausted`.
    pub fn enumerate_up_to(&self, limit: usize) -> SolutionBatch {
        assert!(limit > 0, "solution batch limit must be positive");
        let mut result = SolutionBatch {
            solutions: Vec::with_capacity(limit.min(1024)),
            exhausted: true,
            capped: false,
            stats: SolveStats::default(),
        };
        let Some((state, work)) = self.initial_search_state() else {
            return result;
        };

        let mut cell_order = std::array::from_fn(|cell| cell as u8);
        result.exhausted = self.search_batch(state, work, limit, 0, &mut result, &mut cell_order);
        result.capped = !result.exhausted;
        result
    }

    /// Classify every directed king-neighbour two-cell thermometer that is
    /// disjoint from the current layout. A short collective prefix supplies
    /// witnesses for common edges, then unresolved edges are classified
    /// independently with cap two.
    pub fn screen_two_cell_extensions(&self) -> TwoCellScreenResult {
        self.screen_two_cell_extensions_hybrid(DEFAULT_EXTENSION_PREFIX_SOLUTIONS)
    }

    /// Hybrid collective/independent screen with a caller-selected number of
    /// base solutions in the collective prefix. A zero limit is equivalent to
    /// independent cap-two classification of every edge.
    pub fn screen_two_cell_extensions_hybrid(
        &self,
        collective_solution_limit: u64,
    ) -> TwoCellScreenResult {
        self.screen_two_cell_extensions_internal(Some(collective_solution_limit), true)
    }

    /// Exact reference strategy that only enumerates base solutions, stopping
    /// early if every legal extension has acquired two witnesses.
    pub fn screen_two_cell_extensions_collective(&self) -> TwoCellScreenResult {
        self.screen_two_cell_extensions_internal(None, false)
    }

    fn screen_two_cell_extensions_internal(
        &self,
        collective_solution_limit: Option<u64>,
        finish_individually: bool,
    ) -> TwoCellScreenResult {
        let mut accumulator = ExtensionAccumulator::new(&self.layout, collective_solution_limit);
        let mut stats = SolveStats::default();
        if accumulator.extensions.is_empty() {
            return TwoCellScreenResult {
                base_solutions_visited: 0,
                // No search is needed for a vacuous extension universe, so
                // the base's own solution set has not been exhausted.
                base_exhausted: false,
                collective_solution_limit,
                fallback_searches: 0,
                extensions: accumulator.extensions,
                witness_solutions: accumulator.witness_solutions,
                stats,
            };
        }

        let base_exhausted = if collective_solution_limit == Some(0) {
            false
        } else if let Some((state, work)) = self.initial_search_state() {
            let mut cell_order = std::array::from_fn(|cell| cell as u8);
            self.search_extensions(
                state,
                work,
                0,
                &mut accumulator,
                &mut stats,
                &mut cell_order,
            )
        } else {
            true
        };
        let mut fallback_searches = 0u32;
        if finish_individually && !base_exhausted && !accumulator.pending.is_empty() {
            fallback_searches = self.finish_extensions_individually(&mut accumulator, &mut stats);
        } else {
            for extension in &mut accumulator.extensions {
                extension.exact = base_exhausted && extension.count < 2;
            }
        }
        debug_assert!(
            base_exhausted || accumulator.pending.is_empty() || accumulator.budget_reached
        );
        debug_assert!(
            accumulator
                .extensions
                .iter()
                .all(|extension| extension.multiplicity().is_some())
        );

        TwoCellScreenResult {
            base_solutions_visited: accumulator.base_solutions_visited,
            base_exhausted,
            collective_solution_limit,
            fallback_searches,
            extensions: accumulator.extensions,
            witness_solutions: accumulator.witness_solutions,
            stats,
        }
    }

    fn finish_extensions_individually(
        &self,
        accumulator: &mut ExtensionAccumulator,
        stats: &mut SolveStats,
    ) -> u32 {
        finish_extensions_for_solvers(std::slice::from_ref(self), accumulator, stats)
    }

    fn count_up_to_internal(&self, limit: u64, capture_solutions: bool) -> SolveResult {
        assert!(
            limit >= 2,
            "solution limit must be at least two to classify 0 / 1 / 2+"
        );
        let mut result = SolveResult {
            count: 0,
            capped: false,
            first_solution: None,
            second_solution: None,
            stats: SolveStats::default(),
        };
        let Some((state, work)) = self.initial_search_state() else {
            return result;
        };

        let mut cell_order = std::array::from_fn(|cell| cell as u8);
        let options = SearchOptions {
            limit,
            capture_solutions,
        };
        self.search(state, work, options, 0, &mut result, &mut cell_order);
        result.capped = result.count >= limit;
        result
    }

    fn initial_search_state(&self) -> Option<([u16; 81], Work)> {
        let mut state = [ALL; 81];
        let mut work = Work {
            dirty_thermos: self.layout.all_thermos,
            ..Work::default()
        };
        for (cell, &digit) in self.givens.iter().enumerate() {
            if digit != 0
                && !restrict_domain(
                    &self.layout,
                    &mut state,
                    &mut work,
                    cell,
                    bit_for_digit(digit),
                )
            {
                return None;
            }
        }
        Some((state, work))
    }

    fn search(
        &self,
        mut state: [u16; 81],
        mut work: Work,
        options: SearchOptions,
        depth: u8,
        result: &mut SolveResult,
        cell_order: &mut [u8; 81],
    ) {
        if result.count >= options.limit {
            return;
        }
        result.stats.nodes += 1;
        result.stats.max_depth = result.stats.max_depth.max(depth);
        if !self.propagate(&mut state, &mut work, &mut result.stats) {
            return;
        }

        let Some(cell) = choose_branch_cell(&state, &self.layout, cell_order) else {
            result.count += 1;
            if options.capture_solutions {
                let solution = masks_to_solution(&state);
                if result.first_solution.is_none() {
                    result.first_solution = Some(solution);
                } else if result.second_solution.is_none() {
                    result.second_solution = Some(solution);
                }
            }
            return;
        };

        result.stats.branches += 1;
        let mut choices = state[cell];
        while choices != 0 && result.count < options.limit {
            let value = low_bit(choices);
            choices &= choices - 1;
            let mut child = state;
            let mut child_work = Work::default();
            if restrict_domain(&self.layout, &mut child, &mut child_work, cell, value) {
                self.search(child, child_work, options, depth + 1, result, cell_order);
            }
        }
    }

    /// Returns true when the complete subtree was exhausted. A false result
    /// means an additional solution beyond the requested batch was found.
    fn search_batch(
        &self,
        mut state: [u16; 81],
        mut work: Work,
        limit: usize,
        depth: u8,
        result: &mut SolutionBatch,
        cell_order: &mut [u8; 81],
    ) -> bool {
        result.stats.nodes += 1;
        result.stats.max_depth = result.stats.max_depth.max(depth);
        if !self.propagate(&mut state, &mut work, &mut result.stats) {
            return true;
        }

        let Some(cell) = choose_branch_cell(&state, &self.layout, cell_order) else {
            if result.solutions.len() == limit {
                return false;
            }
            result.solutions.push(masks_to_solution(&state));
            return true;
        };

        result.stats.branches += 1;
        let mut choices = state[cell];
        while choices != 0 {
            let value = low_bit(choices);
            choices &= choices - 1;
            let mut child = state;
            let mut child_work = Work::default();
            if restrict_domain(&self.layout, &mut child, &mut child_work, cell, value)
                && !self.search_batch(child, child_work, limit, depth + 1, result, cell_order)
            {
                return false;
            }
        }
        true
    }

    /// Returns true when the complete subtree was exhausted, or false when
    /// every extension acquired two witnesses and the collective screen could
    /// stop early.
    fn search_extensions(
        &self,
        mut state: [u16; 81],
        mut work: Work,
        depth: u8,
        accumulator: &mut ExtensionAccumulator,
        stats: &mut SolveStats,
        cell_order: &mut [u8; 81],
    ) -> bool {
        stats.nodes += 1;
        stats.max_depth = stats.max_depth.max(depth);
        if !self.propagate(&mut state, &mut work, stats) {
            return true;
        }
        let Some(cell) = choose_branch_cell(&state, &self.layout, cell_order) else {
            return accumulator.observe(&state);
        };

        stats.branches += 1;
        let mut choices = state[cell];
        while choices != 0 {
            let value = low_bit(choices);
            choices &= choices - 1;
            let mut child = state;
            let mut child_work = Work::default();
            if restrict_domain(&self.layout, &mut child, &mut child_work, cell, value)
                && !self.search_extensions(
                    child,
                    child_work,
                    depth + 1,
                    accumulator,
                    stats,
                    cell_order,
                )
            {
                return false;
            }
        }
        true
    }

    fn propagate(&self, state: &mut [u16; 81], work: &mut Work, stats: &mut SolveStats) -> bool {
        loop {
            if let Some(cell) = work.pop_single() {
                stats.propagation_rounds += 1;
                let value = state[cell];
                debug_assert!(value.is_power_of_two());
                for &peer in &PEERS[cell] {
                    if !restrict_domain(&self.layout, state, work, peer as usize, ALL & !value) {
                        return false;
                    }
                }
                continue;
            }

            if work.dirty_thermos != 0 {
                stats.propagation_rounds += 1;
                stats.thermo_revisions += 1;
                let thermo_index = work.dirty_thermos.trailing_zeros() as usize;
                let thermo_bit = 1u64 << thermo_index;
                work.dirty_thermos &= !thermo_bit;
                if !revise_thermo(
                    &self.layout,
                    state,
                    work,
                    self.layout.thermometers[thermo_index].cells(),
                ) {
                    return false;
                }
                // The two directional passes are already a fixed point for the
                // chain. Candidate changes made during them requeued this same
                // thermometer, so suppress that redundant self-revision.
                work.dirty_thermos &= !thermo_bit;
                continue;
            }

            if work.dirty_houses != 0 {
                stats.propagation_rounds += 1;
                let dirty_boxes = work.dirty_houses & BOX_HOUSES;
                let house = if dirty_boxes != 0 {
                    dirty_boxes.trailing_zeros() as usize
                } else {
                    work.dirty_houses.trailing_zeros() as usize
                };
                work.dirty_houses &= !(1u32 << house);
                if !revise_house(&self.layout, state, work, house) {
                    return false;
                }
                continue;
            }

            return true;
        }
    }
}

/// Specialized exact screen for a disjoint length-9 plus length-8 base.
///
/// The length-9 path fixes digits 1 through 9. The length-8 path has nine
/// disjoint possibilities, one for each omitted digit. We therefore solve a
/// union of at most nine ordinary 17-given Sudokus instead of repeatedly
/// propagating the two long thermometer constraints.
pub fn screen_nine_eight_extensions(
    path_nine: &[u8],
    path_eight: &[u8],
    collective_solution_limit: u64,
) -> Result<NineEightScreenResult, LayoutError> {
    let base_layout = Layout::from_paths([path_nine, path_eight].into_iter())?;
    if path_nine.len() != 9 {
        return Err(LayoutError::InvalidLength {
            thermo: 0,
            length: path_nine.len(),
        });
    }
    if path_eight.len() != 8 {
        return Err(LayoutError::InvalidLength {
            thermo: 1,
            length: path_eight.len(),
        });
    }

    let solvers = nine_eight_template_solvers(path_nine, path_eight);
    let compatible_templates = solvers.len() as u8;
    let mut accumulator = ExtensionAccumulator::new(&base_layout, Some(collective_solution_limit));
    let mut stats = SolveStats::default();
    let mut base_exhausted = solvers.is_empty();

    if !solvers.is_empty() && collective_solution_limit == 0 {
        base_exhausted = false;
    } else if !solvers.is_empty() {
        base_exhausted = true;
        for solver in &solvers {
            if accumulator.pending.is_empty() {
                base_exhausted = false;
                break;
            }
            let exhausted = if let Some((state, work)) = solver.initial_search_state() {
                let mut cell_order = std::array::from_fn(|cell| cell as u8);
                solver.search_extensions(
                    state,
                    work,
                    0,
                    &mut accumulator,
                    &mut stats,
                    &mut cell_order,
                )
            } else {
                true
            };
            if !exhausted {
                base_exhausted = false;
                break;
            }
        }
    }

    let fallback_searches = if !base_exhausted && !accumulator.pending.is_empty() {
        finish_extensions_for_solvers(&solvers, &mut accumulator, &mut stats)
    } else {
        for extension in &mut accumulator.extensions {
            extension.exact = base_exhausted && extension.count < 2;
        }
        0
    };
    debug_assert!(base_exhausted || accumulator.pending.is_empty());
    debug_assert!(
        accumulator
            .extensions
            .iter()
            .all(|extension| extension.multiplicity().is_some())
    );

    Ok(NineEightScreenResult {
        compatible_templates,
        screen: TwoCellScreenResult {
            base_solutions_visited: accumulator.base_solutions_visited,
            base_exhausted,
            collective_solution_limit: Some(collective_solution_limit),
            fallback_searches,
            extensions: accumulator.extensions,
            witness_solutions: accumulator.witness_solutions,
            stats,
        },
    })
}

fn nine_eight_template_solvers(path_nine: &[u8], path_eight: &[u8]) -> Vec<Solver> {
    let mut solvers = Vec::with_capacity(9);
    for omitted in 1u8..=9 {
        let mut givens = [0u8; 81];
        let mut rows = [0u16; 9];
        let mut columns = [0u16; 9];
        let mut boxes = [0u16; 9];
        let mut valid = true;
        for (position, &cell) in path_nine.iter().enumerate() {
            valid &= add_template_given(
                &mut givens,
                &mut rows,
                &mut columns,
                &mut boxes,
                cell,
                position as u8 + 1,
            );
        }
        for (position, &cell) in path_eight.iter().enumerate() {
            let ordinal = position as u8 + 1;
            let digit = if ordinal < omitted {
                ordinal
            } else {
                ordinal + 1
            };
            valid &= add_template_given(
                &mut givens,
                &mut rows,
                &mut columns,
                &mut boxes,
                cell,
                digit,
            );
        }
        if valid {
            solvers.push(Solver {
                givens,
                layout: Layout::empty(),
            });
        }
    }
    solvers
}

fn add_template_given(
    givens: &mut [u8; 81],
    rows: &mut [u16; 9],
    columns: &mut [u16; 9],
    boxes: &mut [u16; 9],
    cell: u8,
    digit: u8,
) -> bool {
    let row = (cell / 9) as usize;
    let column = (cell % 9) as usize;
    let box_index = (row / 3) * 3 + column / 3;
    let bit = bit_for_digit(digit);
    if rows[row] & bit != 0 || columns[column] & bit != 0 || boxes[box_index] & bit != 0 {
        return false;
    }
    rows[row] |= bit;
    columns[column] |= bit;
    boxes[box_index] |= bit;
    givens[cell as usize] = digit;
    true
}

fn finish_extensions_for_solvers(
    solvers: &[Solver],
    accumulator: &mut ExtensionAccumulator,
    stats: &mut SolveStats,
) -> u32 {
    let pending = std::mem::take(&mut accumulator.pending);
    let mut witness_map: HashMap<[u8; 81], u32> = accumulator
        .witness_solutions
        .iter()
        .copied()
        .enumerate()
        .map(|(index, solution)| (solution, index as u32))
        .collect();
    let mut searches = 0u32;

    for &index in &pending {
        let (bulb, tip) = {
            let extension = &accumulator.extensions[index];
            (extension.bulb, extension.tip)
        };
        let mut witnesses = [None, None];
        let mut witness_count = 0usize;
        for base_solver in solvers {
            if witness_count == 2 {
                break;
            }
            searches += 1;
            let solver = Solver {
                givens: base_solver.givens,
                layout: base_solver.layout.with_two_cell_extension(bulb, tip),
            };
            let result = solver.count_up_to_internal(2, true);
            merge_stats(stats, &result.stats);
            for solution in [result.first_solution, result.second_solution]
                .into_iter()
                .flatten()
            {
                let witness = intern_witness(
                    &mut accumulator.witness_solutions,
                    &mut witness_map,
                    solution,
                );
                if !witnesses[..witness_count].contains(&Some(witness)) {
                    witnesses[witness_count] = Some(witness);
                    witness_count += 1;
                    if witness_count == 2 {
                        break;
                    }
                }
            }
        }
        let extension = &mut accumulator.extensions[index];
        extension.count = witness_count as u8;
        extension.exact = witness_count < 2;
        extension.first_witness = witnesses[0];
        extension.second_witness = witnesses[1];
    }
    searches
}

/// Enforce generalized arc consistency on one strictly increasing chain.
///
/// For `left < right`, every right value above `min(left)` has support, and
/// every left value below `max(right)` has support. A forward lower-bound pass
/// followed by a backward upper-bound pass is therefore the exact fixed point
/// for a path of inequalities, including domains with holes.
fn revise_thermo(layout: &Layout, state: &mut [u16; 81], work: &mut Work, cells: &[u8]) -> bool {
    for edge in cells.windows(2) {
        let left = edge[0] as usize;
        let right = edge[1] as usize;
        let left_min = low_bit(state[left]);
        if left_min == 0 {
            return false;
        }
        let greater_than_left_min = ALL & !(left_min.wrapping_shl(1).wrapping_sub(1));
        if !restrict_domain(layout, state, work, right, greater_than_left_min) {
            return false;
        }
    }

    for edge in cells.windows(2).rev() {
        let left = edge[0] as usize;
        let right = edge[1] as usize;
        let right_max = high_bit(state[right]);
        if right_max == 0 {
            return false;
        }
        if !restrict_domain(layout, state, work, left, right_max.wrapping_sub(1)) {
            return false;
        }
    }
    true
}

#[inline(always)]
fn remove_domain_bits(
    layout: &Layout,
    state: &mut [u16; 81],
    work: &mut Work,
    cell: usize,
    remove: u16,
) -> bool {
    remove == 0 || restrict_domain(layout, state, work, cell, ALL & !remove)
}

/// Revise one row, column, or box. Hidden singles, missing-digit conflicts,
/// pointing, and claiming are all computed nine digits at a time with bitwise
/// unions instead of separate per-digit scans.
fn revise_house(layout: &Layout, state: &mut [u16; 81], work: &mut Work, house: usize) -> bool {
    let mut once = 0u16;
    let mut twice = 0u16;
    for position in 0..9 {
        let domain = state[house_cell(house, position)];
        twice |= once & domain;
        once |= domain;
    }
    if once != ALL {
        return false;
    }

    let unique = once & !twice;
    if unique != 0 {
        for position in 0..9 {
            let cell = house_cell(house, position);
            let forced = state[cell] & unique;
            if forced == 0 {
                continue;
            }
            if !forced.is_power_of_two() || !restrict_domain(layout, state, work, cell, forced) {
                return false;
            }
        }
    }

    match house {
        // Row claiming: a digit confined to one stack in the row can be
        // removed from the other two rows of that box.
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
                    for col in stack * 3..stack * 3 + 3 {
                        if !remove_domain_bits(layout, state, work, other_row * 9 + col, confined) {
                            return false;
                        }
                    }
                }
            }
        }
        // Column claiming: the row analogue with bands and stacks exchanged.
        9..=17 => {
            let col = house - 9;
            let mut segments = [0u16; 3];
            for (band, segment) in segments.iter_mut().enumerate() {
                for offset in 0..3 {
                    *segment |= state[(band * 3 + offset) * 9 + col];
                }
            }
            for band in 0..3 {
                let confined =
                    segments[band] & !(segments[(band + 1) % 3] | segments[(band + 2) % 3]);
                if confined == 0 {
                    continue;
                }
                let box_col = (col / 3) * 3;
                for other_col in box_col..box_col + 3 {
                    if other_col == col {
                        continue;
                    }
                    for row in band * 3..band * 3 + 3 {
                        if !remove_domain_bits(layout, state, work, row * 9 + other_col, confined) {
                            return false;
                        }
                    }
                }
            }
        }
        // Box pointing: a digit confined to one mini-row/mini-column in the
        // box can be removed from the rest of that full row/column.
        _ => {
            let box_index = house - 18;
            let box_row = (box_index / 3) * 3;
            let box_col = (box_index % 3) * 3;
            let mut mini_rows = [0u16; 3];
            let mut mini_cols = [0u16; 3];
            for dr in 0..3 {
                for dc in 0..3 {
                    let domain = state[(box_row + dr) * 9 + box_col + dc];
                    mini_rows[dr] |= domain;
                    mini_cols[dc] |= domain;
                }
            }
            for dr in 0..3 {
                let confined = mini_rows[dr] & !(mini_rows[(dr + 1) % 3] | mini_rows[(dr + 2) % 3]);
                if confined != 0 {
                    let row = box_row + dr;
                    for col in 0..9 {
                        if col / 3 == box_col / 3 {
                            continue;
                        }
                        if !remove_domain_bits(layout, state, work, row * 9 + col, confined) {
                            return false;
                        }
                    }
                }
            }
            for dc in 0..3 {
                let confined = mini_cols[dc] & !(mini_cols[(dc + 1) % 3] | mini_cols[(dc + 2) % 3]);
                if confined != 0 {
                    let col = box_col + dc;
                    for row in 0..9 {
                        if row / 3 == box_row / 3 {
                            continue;
                        }
                        if !remove_domain_bits(layout, state, work, row * 9 + col, confined) {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

#[cfg(test)]
fn propagate_reference(layout: &Layout, state: &mut [u16; 81]) -> bool {
    loop {
        let mut changed = false;

        for cell in 0..81 {
            let mask = state[cell];
            if mask == 0 {
                return false;
            }
            if mask.is_power_of_two() {
                for &peer in &PEERS[cell] {
                    let peer = peer as usize;
                    let next = state[peer] & !mask;
                    if next == 0 {
                        return false;
                    }
                    if next != state[peer] {
                        state[peer] = next;
                        changed = true;
                    }
                }
            }
        }

        for house in 0..27 {
            for digit_index in 0..9 {
                let bit = 1u16 << digit_index;
                let mut location = NO_CELL;
                let mut count = 0u8;
                for position in 0..9 {
                    let cell = house_cell(house, position);
                    if state[cell] & bit != 0 {
                        location = cell as u8;
                        count += 1;
                    }
                }
                if count == 0 {
                    return false;
                }
                if count == 1 && state[location as usize] != bit {
                    state[location as usize] = bit;
                    changed = true;
                }
            }
        }

        if !propagate_locked_candidates(state, &mut changed) {
            return false;
        }

        for thermo in &layout.thermometers {
            let before = *state;
            let mut ignored_work = Work::default();
            if !revise_thermo(layout, state, &mut ignored_work, thermo.cells()) {
                return false;
            }
            changed |= *state != before;
        }

        if !changed {
            return true;
        }
    }
}

#[cfg(test)]
fn propagate_locked_candidates(state: &mut [u16; 81], changed: &mut bool) -> bool {
    // A digit confined to one row/column inside a box can be removed from the
    // rest of that row/column.
    for box_index in 0..9 {
        let box_row = (box_index / 3) * 3;
        let box_col = (box_index % 3) * 3;
        for digit_index in 0..9 {
            let bit = 1u16 << digit_index;
            let mut row_mask = 0u16;
            let mut col_mask = 0u16;
            for dr in 0..3 {
                for dc in 0..3 {
                    let cell = (box_row + dr) * 9 + box_col + dc;
                    if state[cell] & bit != 0 {
                        row_mask |= 1 << dr;
                        col_mask |= 1 << dc;
                    }
                }
            }
            if row_mask == 0 || col_mask == 0 {
                return false;
            }
            if row_mask.is_power_of_two() {
                let row = box_row + row_mask.trailing_zeros() as usize;
                for col in 0..9 {
                    if col / 3 == box_col / 3 {
                        continue;
                    }
                    if !remove_candidate(state, row * 9 + col, bit, changed) {
                        return false;
                    }
                }
            }
            if col_mask.is_power_of_two() {
                let col = box_col + col_mask.trailing_zeros() as usize;
                for row in 0..9 {
                    if row / 3 == box_row / 3 {
                        continue;
                    }
                    if !remove_candidate(state, row * 9 + col, bit, changed) {
                        return false;
                    }
                }
            }
        }
    }

    // A digit confined to one box inside a row/column can be removed from the
    // other cells of that box.
    for row in 0..9 {
        for digit_index in 0..9 {
            let bit = 1u16 << digit_index;
            let mut stacks = 0u16;
            for col in 0..9 {
                if state[row * 9 + col] & bit != 0 {
                    stacks |= 1 << (col / 3);
                }
            }
            if stacks == 0 {
                return false;
            }
            if stacks.is_power_of_two() {
                let stack = stacks.trailing_zeros() as usize;
                let box_row = (row / 3) * 3;
                for rr in box_row..box_row + 3 {
                    if rr == row {
                        continue;
                    }
                    for col in stack * 3..stack * 3 + 3 {
                        if !remove_candidate(state, rr * 9 + col, bit, changed) {
                            return false;
                        }
                    }
                }
            }
        }
    }

    for col in 0..9 {
        for digit_index in 0..9 {
            let bit = 1u16 << digit_index;
            let mut bands = 0u16;
            for row in 0..9 {
                if state[row * 9 + col] & bit != 0 {
                    bands |= 1 << (row / 3);
                }
            }
            if bands == 0 {
                return false;
            }
            if bands.is_power_of_two() {
                let band = bands.trailing_zeros() as usize;
                let box_col = (col / 3) * 3;
                for cc in box_col..box_col + 3 {
                    if cc == col {
                        continue;
                    }
                    for row in band * 3..band * 3 + 3 {
                        if !remove_candidate(state, row * 9 + cc, bit, changed) {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

#[cfg(test)]
#[inline]
fn remove_candidate(state: &mut [u16; 81], cell: usize, bit: u16, changed: &mut bool) -> bool {
    let next = state[cell] & !bit;
    if next == 0 {
        return false;
    }
    if next != state[cell] {
        state[cell] = next;
        *changed = true;
    }
    true
}

fn choose_branch_cell(
    state: &[u16; 81],
    layout: &Layout,
    cell_order: &mut [u8; 81],
) -> Option<usize> {
    // Keep an evolving cell permutation across the entire search. Moving fixed
    // cells and the selected branch cell to the front preserves useful order
    // information after backtracking, instead of restarting row-major at every
    // node.
    let mut unresolved = 0usize;
    for scan in 0..81 {
        let cell = cell_order[scan] as usize;
        if state[cell].is_power_of_two() {
            cell_order.swap(unresolved, scan);
            unresolved += 1;
        }
    }
    if unresolved == 81 {
        return None;
    }

    let mut best_index = unresolved;
    let mut best_size = u32::MAX;
    let mut best_degree = 0u8;
    for (index, &ordered_cell) in cell_order.iter().enumerate().skip(unresolved) {
        let cell = ordered_cell as usize;
        let size = state[cell].count_ones();
        let degree = layout.thermo_degree[cell];
        if size < best_size || (size == best_size && degree > best_degree) {
            best_index = index;
            best_size = size;
            best_degree = degree;
            // Two candidates on an interior thermo cell is the globally best
            // possible MRV/degree key, so no later cell can displace it.
            if size == 2 && degree == 2 {
                break;
            }
        }
    }
    cell_order.swap(unresolved, best_index);
    Some(cell_order[unresolved] as usize)
}

#[cfg(test)]
fn increasing_templates(length: usize) -> Vec<[u16; 9]> {
    fn generate(
        length: usize,
        position: usize,
        next_digit: u8,
        current: &mut [u16; 9],
        output: &mut Vec<[u16; 9]>,
    ) {
        if position == length {
            output.push(*current);
            return;
        }
        let remaining = length - position;
        let max_digit = 10 - remaining as u8;
        for digit in next_digit..=max_digit {
            current[position] = bit_for_digit(digit);
            generate(length, position + 1, digit + 1, current, output);
        }
    }

    let mut output = Vec::new();
    generate(length, 0, 1, &mut [0; 9], &mut output);
    output
}

#[inline]
fn house_cell(house: usize, position: usize) -> usize {
    HOUSE_CELLS[house][position] as usize
}

#[inline]
fn king_adjacent(a: u8, b: u8) -> bool {
    let ar = (a / 9) as i16;
    let ac = (a % 9) as i16;
    let br = (b / 9) as i16;
    let bc = (b % 9) as i16;
    let dr = (ar - br).abs();
    let dc = (ac - bc).abs();
    (dr != 0 || dc != 0) && dr <= 1 && dc <= 1
}

#[inline]
fn bit_for_digit(digit: u8) -> u16 {
    1u16 << (digit - 1)
}

#[inline]
fn low_bit(mask: u16) -> u16 {
    mask & mask.wrapping_neg()
}

#[inline]
fn high_bit(mask: u16) -> u16 {
    if mask == 0 {
        0
    } else {
        1u16 << (u16::BITS - 1 - mask.leading_zeros())
    }
}

fn merge_stats(total: &mut SolveStats, addition: &SolveStats) {
    total.nodes += addition.nodes;
    total.branches += addition.branches;
    total.propagation_rounds += addition.propagation_rounds;
    total.thermo_revisions += addition.thermo_revisions;
    total.max_depth = total.max_depth.max(addition.max_depth);
}

fn intern_witness(
    pool: &mut Vec<[u8; 81]>,
    indices: &mut HashMap<[u8; 81], u32>,
    solution: [u8; 81],
) -> u32 {
    if let Some(&index) = indices.get(&solution) {
        return index;
    }
    let index = pool.len() as u32;
    pool.push(solution);
    indices.insert(solution, index);
    index
}

fn masks_to_solution(state: &[u16; 81]) -> [u8; 81] {
    let mut solution = [0u8; 81];
    for (cell, &mask) in state.iter().enumerate() {
        debug_assert!(mask.is_power_of_two());
        solution[cell] = mask.trailing_zeros() as u8 + 1;
    }
    solution
}

/// C ABI for the Python `ctypes` adapter.
///
/// Returns a capped non-negative solution count, or a negative error code:
/// -1 null/invalid pointers, -2 malformed offsets, -3 invalid layout/givens.
///
/// # Safety
///
/// `offsets` must reference `thermo_count + 1` readable `u16` values. Its last
/// value determines how many readable bytes `cells` must reference. A non-null
/// `givens` must reference 81 readable bytes, and a non-null `first_solution`
/// must reference 81 writable bytes. All pointed-to memory must remain valid
/// for the duration of the call and must not overlap writable output.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn thermo_sudoku_count_up_to(
    givens: *const u8,
    cells: *const u8,
    offsets: *const u16,
    thermo_count: usize,
    limit: u64,
    first_solution: *mut u8,
) -> i64 {
    if !(2..=i64::MAX as u64).contains(&limit) || offsets.is_null() || thermo_count > 40 {
        return -1;
    }
    let offsets_slice = unsafe { std::slice::from_raw_parts(offsets, thermo_count + 1) };
    if offsets_slice.first().copied() != Some(0) {
        return -2;
    }
    let total_cells = offsets_slice[thermo_count] as usize;
    let cell_slice = if total_cells == 0 {
        &[]
    } else {
        if cells.is_null() {
            return -1;
        }
        unsafe { std::slice::from_raw_parts(cells, total_cells) }
    };
    for window in offsets_slice.windows(2) {
        let start = window[0] as usize;
        let end = window[1] as usize;
        if start > end || end > total_cells {
            return -2;
        }
    }
    let givens_array = if givens.is_null() {
        [0u8; 81]
    } else {
        let mut array = [0u8; 81];
        array.copy_from_slice(unsafe { std::slice::from_raw_parts(givens, 81) });
        array
    };

    let path_slices = offsets_slice.windows(2).map(|window| {
        let start = window[0] as usize;
        let end = window[1] as usize;
        &cell_slice[start..end]
    });
    let Ok(layout) = Layout::from_paths(path_slices) else {
        return -3;
    };
    let Ok(solver) = Solver::from_layout(givens_array, layout) else {
        return -3;
    };
    let result = solver.count_up_to_internal(limit, !first_solution.is_null());
    if !first_solution.is_null()
        && let Some(solution) = result.first_solution
    {
        unsafe {
            std::ptr::copy_nonoverlapping(solution.as_ptr(), first_solution, 81);
        }
    }
    result.count as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLUE_20: &[&[u8]] = &[
        &[18, 27, 28, 19, 20, 11, 12, 13, 4],
        &[57, 48, 49],
        &[59, 68, 69, 60, 61, 52, 53, 44],
    ];
    const KNOWN_THREE: &[&[u8]] = &[
        &[19, 29, 28, 20, 11, 12, 13, 3, 4],
        &[77, 69, 78, 70, 62, 53, 44, 52],
        &[41, 51],
    ];

    fn paths(raw: &[&[u8]]) -> Vec<Vec<u8>> {
        raw.iter().map(|path| path.to_vec()).collect()
    }

    fn assert_solution_satisfies(solution: &[u8; 81], layout: &[Vec<u8>]) {
        for house in 0..27 {
            let mut digits = 0u16;
            for position in 0..9 {
                let digit = solution[house_cell(house, position)];
                assert!((1..=9).contains(&digit));
                digits |= bit_for_digit(digit);
            }
            assert_eq!(digits, ALL);
        }
        for path in layout {
            for edge in path.windows(2) {
                assert!(solution[edge[0] as usize] < solution[edge[1] as usize]);
            }
        }
    }

    #[test]
    fn template_counts_are_binomial() {
        let expected = [0, 0, 36, 84, 126, 126, 84, 36, 9, 1];
        for (length, &count) in expected.iter().enumerate().skip(2) {
            assert_eq!(increasing_templates(length).len(), count);
        }
    }

    fn template_revision(domains: &mut [u16], templates: &[[u16; 9]]) -> bool {
        let mut supports = [0u16; 9];
        let mut active = 0usize;
        'template: for template in templates {
            for (position, domain) in domains.iter().enumerate() {
                if domain & template[position] == 0 {
                    continue 'template;
                }
            }
            active += 1;
            for position in 0..domains.len() {
                supports[position] |= template[position];
            }
        }
        if active == 0 {
            return false;
        }
        for (domain, support) in domains.iter_mut().zip(supports) {
            *domain &= support;
        }
        true
    }

    fn assert_linear_matches_templates(domains: &[u16], templates: &[[u16; 9]]) {
        let mut expected = domains.to_vec();
        let expected_feasible = template_revision(&mut expected, templates);

        let mut state = [ALL; 81];
        state[..domains.len()].copy_from_slice(domains);
        let cells: Vec<u8> = (0..domains.len() as u8).collect();
        let layout = Layout::new(std::slice::from_ref(&cells)).unwrap();
        let before = state;
        let mut work = Work::default();
        let observed_feasible = revise_thermo(&layout, &mut state, &mut work, &cells);

        assert_eq!(observed_feasible, expected_feasible, "domains={domains:?}");
        if expected_feasible {
            assert_eq!(&state[..domains.len()], expected.as_slice());
            assert_eq!(state != before, expected.as_slice() != domains);
        }
    }

    #[test]
    fn linear_thermo_matches_all_two_cell_domain_pairs() {
        let templates = increasing_templates(2);
        for left in 0..=ALL {
            for right in 0..=ALL {
                assert_linear_matches_templates(&[left, right], &templates);
            }
        }
    }

    #[test]
    fn linear_thermo_matches_template_gac_on_long_sparse_domains() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        for length in 2..=9 {
            let templates = increasing_templates(length);
            for _ in 0..2_000 {
                let mut domains = vec![0u16; length];
                for domain in &mut domains {
                    seed ^= seed << 7;
                    seed ^= seed >> 9;
                    seed ^= seed << 8;
                    *domain = (seed as u16) & ALL;
                }
                assert_linear_matches_templates(&domains, &templates);
            }
        }
    }

    #[test]
    fn linear_thermo_handles_bound_cascades_and_holes() {
        let all = vec![ALL; 9];
        assert_linear_matches_templates(&all, &increasing_templates(9));

        let sparse = [
            bit_for_digit(2) | bit_for_digit(8),
            bit_for_digit(1) | bit_for_digit(3) | bit_for_digit(9),
            bit_for_digit(2) | bit_for_digit(4) | bit_for_digit(8),
        ];
        assert_linear_matches_templates(&sparse, &increasing_templates(3));

        let length_four = increasing_templates(4);
        assert_linear_matches_templates(
            &[
                bit_for_digit(4),
                bit_for_digit(1) | bit_for_digit(5),
                bit_for_digit(2) | bit_for_digit(6),
                bit_for_digit(3) | bit_for_digit(7),
            ],
            &length_four,
        );
        assert_linear_matches_templates(
            &[
                bit_for_digit(3) | bit_for_digit(7),
                bit_for_digit(4) | bit_for_digit(8),
                bit_for_digit(5) | bit_for_digit(9),
                bit_for_digit(6),
            ],
            &length_four,
        );
        let length_two = increasing_templates(2);
        assert_linear_matches_templates(&[bit_for_digit(9), ALL], &length_two);
        assert_linear_matches_templates(&[ALL, bit_for_digit(1)], &length_two);
    }

    fn incremental_closure(layout: &Layout, state: &mut [u16; 81]) -> bool {
        if state.contains(&0) {
            return false;
        }
        let solver = Solver {
            givens: [0; 81],
            layout: layout.clone(),
        };
        let mut work = Work {
            dirty_houses: ALL_HOUSES,
            dirty_thermos: layout.all_thermos,
            ..Work::default()
        };
        for (cell, domain) in state.iter().enumerate() {
            if domain.is_power_of_two() {
                work.add_single(cell);
            }
        }
        solver.propagate(state, &mut work, &mut SolveStats::default())
    }

    fn next_random(seed: &mut u64) -> u16 {
        *seed ^= *seed << 7;
        *seed ^= *seed >> 9;
        *seed ^= *seed << 8;
        (*seed as u16) & ALL
    }

    #[test]
    fn incremental_propagation_matches_full_scan_on_satisfiable_states() {
        let layout = Layout::new(&paths(BLUE_20)).unwrap();
        let solution =
            b"873195624926784513145236897238619745564827139791453268689371452457962381312548976";
        let mut seed = 0xd1b5_4a32_d192_ed03u64;

        for _ in 0..2_000 {
            let mut initial = [0u16; 81];
            for cell in 0..81 {
                let solution_bit = bit_for_digit(solution[cell] - b'0');
                initial[cell] = solution_bit | next_random(&mut seed);
            }

            let mut expected = initial;
            let mut observed = initial;
            assert!(propagate_reference(&layout, &mut expected));
            assert!(incremental_closure(&layout, &mut observed));
            assert_eq!(observed, expected);

            let fixed_point = observed;
            assert!(incremental_closure(&layout, &mut observed));
            assert_eq!(observed, fixed_point);
        }
    }

    #[test]
    fn incremental_propagation_matches_full_scan_on_arbitrary_states() {
        let layout = Layout::new(&paths(KNOWN_THREE)).unwrap();
        let mut seed = 0x94d0_49bb_1331_11ebu64;

        for _ in 0..2_000 {
            let mut initial = [0u16; 81];
            for domain in &mut initial {
                *domain = next_random(&mut seed);
            }

            let mut expected = initial;
            let mut observed = initial;
            let expected_feasible = propagate_reference(&layout, &mut expected);
            let observed_feasible = incremental_closure(&layout, &mut observed);
            assert_eq!(observed_feasible, expected_feasible, "initial={initial:?}");
            if expected_feasible {
                assert_eq!(observed, expected);
            }
        }
    }

    #[test]
    fn blue_twenty_is_unique() {
        let result = Solver::blank(&paths(BLUE_20)).unwrap().classify();
        assert_eq!(result.multiplicity(), Multiplicity::Unique);
        assert_eq!(result.count, 1);
        assert!(result.first_solution.is_some());
        assert!(result.second_solution.is_none());
    }

    #[test]
    fn known_nineteen_has_three_solutions() {
        let solver = Solver::blank(&paths(KNOWN_THREE)).unwrap();
        let classified = solver.classify();
        assert_eq!(classified.multiplicity(), Multiplicity::Multiple);
        assert_eq!(classified.count, 2);
        assert!(classified.capped);
        assert!(classified.first_solution.is_some());
        assert!(classified.second_solution.is_some());

        let counted = solver.count_up_to(4);
        assert_eq!(counted.count, 3);
        assert!(!counted.capped);
    }

    #[test]
    fn enumeration_exhausts_blue_with_its_unique_valid_solution() {
        let layout = paths(BLUE_20);
        let batch = Solver::blank(&layout).unwrap().enumerate_up_to(2);

        assert_eq!(batch.solutions.len(), 1);
        assert!(batch.exhausted);
        assert!(!batch.capped);
        assert!(batch.stats.nodes > 0);
        assert_solution_satisfies(&batch.solutions[0], &layout);
    }

    #[test]
    fn enumeration_returns_all_known_three_solutions_distinct_and_valid() {
        let layout = paths(KNOWN_THREE);
        let batch = Solver::blank(&layout).unwrap().enumerate_up_to(3);

        assert_eq!(batch.solutions.len(), 3);
        assert!(batch.exhausted);
        assert!(!batch.capped);
        for solution in &batch.solutions {
            assert_solution_satisfies(solution, &layout);
        }
        for left in 0..batch.solutions.len() {
            for right in left + 1..batch.solutions.len() {
                assert_ne!(batch.solutions[left], batch.solutions[right]);
            }
        }
    }

    #[test]
    fn enumeration_reports_a_cap_only_after_finding_an_extra_solution() {
        let solver = Solver::blank(&paths(KNOWN_THREE)).unwrap();
        let capped = solver.enumerate_up_to(2);
        assert_eq!(capped.solutions.len(), 2);
        assert!(!capped.exhausted);
        assert!(capped.capped);

        let exact = solver.enumerate_up_to(3);
        assert_eq!(exact.solutions.len(), 3);
        assert!(exact.exhausted);
        assert!(!exact.capped);
    }

    #[test]
    fn collective_two_cell_screen_matches_individual_searches() {
        let base = paths(&KNOWN_THREE[..2]);
        let screen = Solver::blank(&base)
            .unwrap()
            .screen_two_cell_extensions_collective();

        for extension in &screen.extensions {
            let mut extended = base.clone();
            extended.push(vec![extension.bulb, extension.tip]);
            let individual = Solver::blank(&extended).unwrap().classify();
            assert_eq!(extension.count as u64, individual.count);
            assert_eq!(extension.exact, !individual.capped);

            if let Some(first) = extension.first_witness {
                let solution = &screen.witness_solutions[first as usize];
                assert_solution_satisfies(solution, &extended);
            }
            if let Some(second) = extension.second_witness {
                let solution = &screen.witness_solutions[second as usize];
                assert_solution_satisfies(solution, &extended);
                assert_ne!(extension.first_witness, extension.second_witness);
            }
        }
        assert_eq!(screen.unique_count(), 0);
        let known_edge = screen
            .extensions
            .iter()
            .find(|extension| extension.bulb == 41 && extension.tip == 51)
            .unwrap();
        assert_eq!(known_edge.count, 2);
        assert!(!known_edge.exact);
    }

    #[test]
    fn hybrid_two_cell_screen_matches_collective_reference() {
        let base = paths(&KNOWN_THREE[..2]);
        let solver = Solver::blank(&base).unwrap();
        let reference = solver.screen_two_cell_extensions_collective();

        for prefix in [0, 1, 8, 64] {
            let screen = solver.screen_two_cell_extensions_hybrid(prefix);
            assert_eq!(screen.extensions.len(), reference.extensions.len());
            for (observed, expected) in screen.extensions.iter().zip(&reference.extensions) {
                assert_eq!(
                    (observed.bulb, observed.tip, observed.count, observed.exact),
                    (expected.bulb, expected.tip, expected.count, expected.exact)
                );
                let mut extended = base.clone();
                extended.push(vec![observed.bulb, observed.tip]);
                if let Some(first) = observed.first_witness {
                    assert_solution_satisfies(&screen.witness_solutions[first as usize], &extended);
                }
                if let Some(second) = observed.second_witness {
                    assert_solution_satisfies(
                        &screen.witness_solutions[second as usize],
                        &extended,
                    );
                    assert_ne!(observed.first_witness, observed.second_witness);
                }
            }
        }
    }

    #[test]
    fn specialized_nine_eight_screen_matches_generic_thermos() {
        let base = paths(&KNOWN_THREE[..2]);
        let generic = Solver::blank(&base)
            .unwrap()
            .screen_two_cell_extensions_hybrid(128);
        let specialized = screen_nine_eight_extensions(&base[0], &base[1], 128).unwrap();
        assert_eq!(specialized.compatible_templates, 9);
        assert_eq!(
            specialized.screen.extensions.len(),
            generic.extensions.len()
        );
        for (observed, expected) in specialized
            .screen
            .extensions
            .iter()
            .zip(&generic.extensions)
        {
            assert_eq!(
                (observed.bulb, observed.tip, observed.count, observed.exact),
                (expected.bulb, expected.tip, expected.count, expected.exact)
            );
        }
    }

    #[test]
    fn collective_screen_reports_exact_zero_and_unique_extensions() {
        let base = paths(BLUE_20);
        let screen = Solver::blank(&base).unwrap().screen_two_cell_extensions();
        assert!(screen.base_exhausted);
        assert_eq!(screen.base_solutions_visited, 1);
        assert!(screen.zero_count() > 0);
        assert!(screen.unique_count() > 0);
        assert_eq!(screen.multiple_count(), 0);
        assert!(screen.extensions.iter().all(|extension| extension.exact));
    }

    #[test]
    fn collective_screen_handles_an_unsatisfiable_base() {
        let base = vec![(0u8..9).collect(), (9u8..18).collect()];
        let screen = Solver::blank(&base).unwrap().screen_two_cell_extensions();
        assert!(screen.base_exhausted);
        assert_eq!(screen.base_solutions_visited, 0);
        assert_eq!(screen.unique_count(), 0);
        assert_eq!(screen.multiple_count(), 0);
        assert!(
            screen
                .extensions
                .iter()
                .all(|extension| extension.count == 0 && extension.exact)
        );
    }

    #[test]
    fn vacuous_extension_universe_does_not_claim_base_exhaustion() {
        let mut base = Vec::new();
        for row in 0u8..9 {
            for column in (0u8..8).step_by(2) {
                let cell = row * 9 + column;
                base.push(vec![cell, cell + 1]);
            }
        }
        for row in (0u8..8).step_by(2) {
            base.push(vec![row * 9 + 8, (row + 1) * 9 + 8]);
        }
        let screen = Solver::blank(&base).unwrap().screen_two_cell_extensions();
        assert!(screen.extensions.is_empty());
        assert!(!screen.base_exhausted);
        assert_eq!(screen.base_solutions_visited, 0);
    }

    #[test]
    fn two_forced_rows_are_unsatisfiable() {
        let layout = vec![(0u8..9).collect(), (9u8..18).collect()];
        let result = Solver::blank(&layout).unwrap().classify();
        assert_eq!(result.multiplicity(), Multiplicity::Zero);
    }

    #[test]
    fn rejects_overlap_and_bad_steps() {
        assert!(matches!(
            Layout::new(&[vec![0, 1], vec![1, 2]]),
            Err(LayoutError::Overlap { cell: 1 })
        ));
        assert!(matches!(
            Layout::new(&[vec![0, 2]]),
            Err(LayoutError::NonAdjacent { .. })
        ));
    }

    #[test]
    fn classic_given_grid_works() {
        let text =
            b"534678912672195348198342567859761423426853791713924856961537284287419635345286179";
        let mut givens = [0u8; 81];
        for (index, byte) in text.iter().enumerate() {
            givens[index] = byte - b'0';
        }
        let result = Solver::new(givens, &[]).unwrap().classify();
        assert_eq!(result.multiplicity(), Multiplicity::Unique);
    }

    #[test]
    fn ffi_accepts_an_empty_layout_without_a_cells_pointer() {
        let offsets = [0u16];
        let count = unsafe {
            thermo_sudoku_count_up_to(
                std::ptr::null(),
                std::ptr::null(),
                offsets.as_ptr(),
                0,
                2,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(count, 2);
    }

    #[test]
    fn ffi_returns_a_requested_solution_witness() {
        let text =
            b"534678912672195348198342567859761423426853791713924856961537284287419635345286179";
        let mut givens = [0u8; 81];
        for (index, byte) in text.iter().enumerate() {
            givens[index] = byte - b'0';
        }
        let offsets = [0u16];
        let mut witness = [0u8; 81];
        let count = unsafe {
            thermo_sudoku_count_up_to(
                givens.as_ptr(),
                std::ptr::null(),
                offsets.as_ptr(),
                0,
                2,
                witness.as_mut_ptr(),
            )
        };
        assert_eq!(count, 1);
        assert_eq!(witness, givens);
    }
}
