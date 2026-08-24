//! Exact Sudoku solving for overlapping and branching thermometer graphs.
//!
//! [`Solver`](crate::Solver) remains the optimized implementation for
//! cell-disjoint paths.  This module flattens overlapping paths to their
//! directed `<` comparisons and propagates those binary arcs incrementally.

use std::fmt;

use super::{
    ALL, BOX_HOUSES, CELL_HOUSE_BITS, LayoutError, PEERS, ProblemError, SearchOptions,
    SolutionBatch, SolveResult, SolveStats, Solver, bit_for_digit, high_bit, house_cell, low_bit,
    masks_to_solution,
};

/// Maximum number of distinct comparisons in one overlapping graph.
///
/// An exact row-mask dynamic program gives 46 as the maximum number of
/// undirected king-neighbour pairs induced by 17 cells on a 9x9 grid.  A target
/// solution orients each such pair in exactly one direction, so one `u64`
/// covers every graph required by the exact 17-cell search with 18 spare bits.
/// The constructor rejects larger graphs instead of risking an overflowing
/// shift or silently dropping a constraint.
pub const MAX_COMPARISONS: usize = 64;

/// A deduplicated directed graph of local strict inequalities.
///
/// Each pair `(lower, upper)` means `digit(lower) < digit(upper)`.  Opposite
/// comparisons are intentionally accepted; their strict cycle is detected as
/// unsatisfiable by propagation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComparisonLayout {
    comparisons: Vec<(u8, u8)>,
    incident: [u64; 81],
    degree: [u8; 81],
    all_comparisons: u64,
    max_degree: u8,
}

impl ComparisonLayout {
    /// Build a graph from explicit directed king-neighbour comparisons.
    /// Duplicate comparisons are removed while preserving first-seen order.
    pub fn new(comparisons: &[(u8, u8)]) -> Result<Self, ComparisonLayoutError> {
        let mut validated = Vec::with_capacity(comparisons.len().min(MAX_COMPARISONS));
        for (comparison, &(lower, upper)) in comparisons.iter().enumerate() {
            if lower >= 81 {
                return Err(ComparisonLayoutError::ComparisonCellOutOfRange {
                    comparison,
                    endpoint: 0,
                    cell: lower,
                });
            }
            if upper >= 81 {
                return Err(ComparisonLayoutError::ComparisonCellOutOfRange {
                    comparison,
                    endpoint: 1,
                    cell: upper,
                });
            }
            if !super::king_adjacent(lower, upper) {
                return Err(ComparisonLayoutError::NonAdjacentComparison {
                    comparison,
                    lower,
                    upper,
                });
            }
            validated.push((lower, upper));
        }
        Self::from_validated(validated)
    }

    /// Flatten thermometer paths into directed comparisons.
    ///
    /// Paths may overlap one another and may share bulbs, tips, or interior
    /// cells.  A single path still has the ordinary thermometer rules: length
    /// 2 through 9, no repeated cell, and king-neighbour steps.
    pub fn from_paths(paths: &[Vec<u8>]) -> Result<Self, ComparisonLayoutError> {
        let edge_capacity = paths.iter().map(|path| path.len().saturating_sub(1)).sum();
        let mut comparisons = Vec::with_capacity(edge_capacity);
        for (path_index, path) in paths.iter().enumerate() {
            if !(2..=9).contains(&path.len()) {
                return Err(ComparisonLayoutError::InvalidPathLength {
                    path: path_index,
                    length: path.len(),
                });
            }
            let mut local = [false; 81];
            for (position, &cell) in path.iter().enumerate() {
                if cell >= 81 {
                    return Err(ComparisonLayoutError::PathCellOutOfRange {
                        path: path_index,
                        position,
                        cell,
                    });
                }
                if local[cell as usize] {
                    return Err(ComparisonLayoutError::RepeatedPathCell {
                        path: path_index,
                        cell,
                    });
                }
                local[cell as usize] = true;
            }
            for (position, edge) in path.windows(2).enumerate() {
                let lower = edge[0];
                let upper = edge[1];
                if !super::king_adjacent(lower, upper) {
                    return Err(ComparisonLayoutError::NonAdjacentPathStep {
                        path: path_index,
                        position,
                        lower,
                        upper,
                    });
                }
                comparisons.push((lower, upper));
            }
        }
        Self::from_validated(comparisons)
    }

    fn from_validated(input: Vec<(u8, u8)>) -> Result<Self, ComparisonLayoutError> {
        let mut seen = [[false; 81]; 81];
        let mut comparisons = Vec::with_capacity(input.len().min(MAX_COMPARISONS));
        for (lower, upper) in input {
            let slot = &mut seen[lower as usize][upper as usize];
            if !*slot {
                *slot = true;
                comparisons.push((lower, upper));
            }
        }
        if comparisons.len() > MAX_COMPARISONS {
            return Err(ComparisonLayoutError::TooManyComparisons {
                count: comparisons.len(),
                maximum: MAX_COMPARISONS,
            });
        }

        let mut incident = [0u64; 81];
        let mut degree = [0u8; 81];
        let all_comparisons = if comparisons.len() == MAX_COMPARISONS {
            u64::MAX
        } else {
            (1u64 << comparisons.len()) - 1
        };
        for (comparison, &(lower, upper)) in comparisons.iter().enumerate() {
            for cell in [lower as usize, upper as usize] {
                // A cell has eight king neighbours and at most two directed
                // comparisons per neighbour.  Deduplication makes 16 exact.
                debug_assert!(degree[cell] < 16);
                incident[cell] |= 1u64 << comparison;
                degree[cell] += 1;
            }
        }
        let max_degree = degree.iter().copied().max().unwrap_or(0);

        Ok(Self {
            comparisons,
            incident,
            degree,
            all_comparisons,
            max_degree,
        })
    }

    pub fn comparisons(&self) -> &[(u8, u8)] {
        &self.comparisons
    }

    pub fn comparison_count(&self) -> usize {
        self.comparisons.len()
    }

    pub fn covered_cells(&self) -> usize {
        self.degree.iter().filter(|&&degree| degree != 0).count()
    }

    #[inline(always)]
    fn mark_incident(&self, work: &mut ComparisonWork, cell: usize) {
        work.dirty_comparisons |= self.incident[cell];
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComparisonLayoutError {
    InvalidPathLength {
        path: usize,
        length: usize,
    },
    PathCellOutOfRange {
        path: usize,
        position: usize,
        cell: u8,
    },
    RepeatedPathCell {
        path: usize,
        cell: u8,
    },
    NonAdjacentPathStep {
        path: usize,
        position: usize,
        lower: u8,
        upper: u8,
    },
    ComparisonCellOutOfRange {
        comparison: usize,
        endpoint: usize,
        cell: u8,
    },
    NonAdjacentComparison {
        comparison: usize,
        lower: u8,
        upper: u8,
    },
    TooManyComparisons {
        count: usize,
        maximum: usize,
    },
}

impl fmt::Display for ComparisonLayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InvalidPathLength { path, length } => {
                write!(
                    f,
                    "thermometer path {path} has length {length}; expected 2..=9"
                )
            }
            Self::PathCellOutOfRange {
                path,
                position,
                cell,
            } => write!(
                f,
                "thermometer path {path}, position {position}: cell {cell} is outside 0..=80"
            ),
            Self::RepeatedPathCell { path, cell } => {
                write!(f, "thermometer path {path} repeats cell {cell}")
            }
            Self::NonAdjacentPathStep {
                path,
                position,
                lower,
                upper,
            } => write!(
                f,
                "thermometer path {path}, step {position}: cells {lower}->{upper} are not king-adjacent"
            ),
            Self::ComparisonCellOutOfRange {
                comparison,
                endpoint,
                cell,
            } => write!(
                f,
                "comparison {comparison}, endpoint {endpoint}: cell {cell} is outside 0..=80"
            ),
            Self::NonAdjacentComparison {
                comparison,
                lower,
                upper,
            } => write!(
                f,
                "comparison {comparison}: cells {lower}->{upper} are not king-adjacent"
            ),
            Self::TooManyComparisons { count, maximum } => write!(
                f,
                "comparison graph has {count} distinct edges; maximum supported is {maximum}"
            ),
        }
    }
}

impl std::error::Error for ComparisonLayoutError {}

/// Validation failure for a sparse target projection.
///
/// A projection uses zero for an unconstrained cell and `1..=9` for a target
/// digit.  It is deliberately separate from Sudoku givens: the target is not
/// imposed on the search, but is used to distinguish the known catalogue
/// completion from alternatives.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TargetProjectionError {
    Empty,
    InvalidDigit { cell: usize, digit: u8 },
}

impl fmt::Display for TargetProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Empty => write!(f, "target projection has no specified cells"),
            Self::InvalidDigit { cell, digit } => write!(
                f,
                "target projection at cell {cell} is {digit}; expected 0..=9"
            ),
        }
    }
}

impl std::error::Error for TargetProjectionError {}

/// Result of searching for a solution outside a sparse target projection.
///
/// Search stops as soon as it finds either a differing solution or two
/// solutions agreeing with the projection.  Otherwise it exhausts the whole
/// solution space.  Thus `exhausted && target_matching_solutions == 1` is an
/// exact uniqueness result, while `alternative_solution.is_some()` proves a
/// solution exists outside the target projection.  A caller that already
/// knows one target-matching solution exists can combine that fact with a
/// differing witness to prove multiplicity immediately.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetAlternativeResult {
    /// Number of target-matching solutions encountered, capped at two.
    pub target_matching_solutions: u8,
    /// First solution agreeing with every nonzero target cell, when seen.
    pub target_matching_solution: Option<[u8; 81]>,
    /// First solution disagreeing with at least one nonzero target cell.
    pub alternative_solution: Option<[u8; 81]>,
    /// True only when the complete solution space was searched.
    pub exhausted: bool,
    pub stats: SolveStats,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComparisonProblemError {
    InvalidGiven { cell: usize, digit: u8 },
    Layout(ComparisonLayoutError),
}

impl fmt::Display for ComparisonProblemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGiven { cell, digit } => {
                write!(f, "given at cell {cell} is {digit}; expected 0..=9")
            }
            Self::Layout(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ComparisonProblemError {}

impl From<ComparisonLayoutError> for ComparisonProblemError {
    fn from(value: ComparisonLayoutError) -> Self {
        Self::Layout(value)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ComparisonWork {
    single_lo: u64,
    single_hi: u32,
    dirty_houses: u32,
    dirty_comparisons: u64,
}

impl ComparisonWork {
    #[inline(always)]
    fn add_single(&mut self, cell: usize) {
        if cell < 64 {
            self.single_lo |= 1u64 << cell;
        } else {
            self.single_hi |= 1u32 << (cell - 64);
        }
    }

    #[inline(always)]
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

    #[inline(always)]
    fn clear_comparison(&mut self, comparison: usize) {
        self.dirty_comparisons &= !(1u64 << comparison);
    }

    #[inline]
    fn pop_comparison(&mut self) -> Option<usize> {
        if self.dirty_comparisons == 0 {
            None
        } else {
            let comparison = self.dirty_comparisons.trailing_zeros() as usize;
            self.dirty_comparisons &= self.dirty_comparisons - 1;
            Some(comparison)
        }
    }
}

/// Exact capped solver for arbitrary overlapping local thermometer graphs.
///
/// Constructing from paths automatically delegates cell-disjoint layouts to
/// the original chain-specialized [`Solver`].  Overlapping paths and explicit
/// comparison graphs use incremental binary `<` arc consistency.
#[derive(Clone, Debug)]
pub struct ComparisonSolver {
    givens: [u8; 81],
    layout: ComparisonLayout,
    disjoint_fast_path: Option<Solver>,
}

impl ComparisonSolver {
    pub fn new(givens: [u8; 81], comparisons: &[(u8, u8)]) -> Result<Self, ComparisonProblemError> {
        validate_givens(&givens)?;
        Ok(Self {
            givens,
            layout: ComparisonLayout::new(comparisons)?,
            disjoint_fast_path: None,
        })
    }

    pub fn blank(comparisons: &[(u8, u8)]) -> Result<Self, ComparisonProblemError> {
        Self::new([0; 81], comparisons)
    }

    pub fn from_paths(givens: [u8; 81], paths: &[Vec<u8>]) -> Result<Self, ComparisonProblemError> {
        validate_givens(&givens)?;
        let layout = ComparisonLayout::from_paths(paths)?;
        let disjoint_fast_path = match Solver::new(givens, paths) {
            Ok(solver) => Some(solver),
            Err(ProblemError::Layout(LayoutError::Overlap { .. })) => None,
            Err(error) => unreachable!("comparison validation disagreed with Solver: {error}"),
        };
        Ok(Self {
            givens,
            layout,
            disjoint_fast_path,
        })
    }

    pub fn blank_paths(paths: &[Vec<u8>]) -> Result<Self, ComparisonProblemError> {
        Self::from_paths([0; 81], paths)
    }

    pub fn layout(&self) -> &ComparisonLayout {
        &self.layout
    }

    pub fn uses_disjoint_path_fast_path(&self) -> bool {
        self.disjoint_fast_path.is_some()
    }

    pub fn classify(&self) -> SolveResult {
        self.count_up_to(2)
    }

    pub fn count_up_to(&self, limit: u64) -> SolveResult {
        if let Some(solver) = &self.disjoint_fast_path {
            return solver.count_up_to(limit);
        }
        self.count_up_to_internal(limit, true)
    }

    /// Find the first solution that differs from a sparse target projection.
    ///
    /// Zero target entries are ignored.  Unlike [`Self::count_up_to`], this
    /// search does not spend time finding a second arbitrary solution after a
    /// differing witness has already been found.  It still searches exactly:
    /// if no differing witness (and no second target match) is found,
    /// `exhausted` is true and the target-matching count is exact.
    ///
    /// The comparison backend is used even for a solver constructed from
    /// disjoint paths, because the chain-specialized backend has no
    /// target-aware stopping rule.
    pub fn find_target_alternative(
        &self,
        target: &[u8; 81],
    ) -> Result<TargetAlternativeResult, TargetProjectionError> {
        validate_target_projection(target)?;
        let mut result = TargetAlternativeResult {
            target_matching_solutions: 0,
            target_matching_solution: None,
            alternative_solution: None,
            exhausted: true,
            stats: SolveStats::default(),
        };
        let Some((state, work)) = self.initial_search_state() else {
            return Ok(result);
        };

        let mut cell_order = std::array::from_fn(|cell| cell as u8);
        result.exhausted =
            self.search_target_alternative(state, work, target, 0, &mut result, &mut cell_order);
        Ok(result)
    }

    pub fn enumerate_up_to(&self, limit: usize) -> SolutionBatch {
        if let Some(solver) = &self.disjoint_fast_path {
            return solver.enumerate_up_to(limit);
        }
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
        let options = SearchOptions {
            limit,
            capture_solutions,
        };
        let mut cell_order = std::array::from_fn(|cell| cell as u8);
        self.search(state, work, options, 0, &mut result, &mut cell_order);
        result.capped = result.count >= limit;
        result
    }

    fn initial_search_state(&self) -> Option<([u16; 81], ComparisonWork)> {
        let mut state = [ALL; 81];
        let mut work = ComparisonWork {
            dirty_comparisons: self.layout.all_comparisons,
            ..ComparisonWork::default()
        };
        for (cell, &digit) in self.givens.iter().enumerate() {
            if digit != 0
                && !restrict_comparison_domain(
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
        mut work: ComparisonWork,
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

        let Some(cell) = choose_comparison_branch_cell(&state, &self.layout, cell_order) else {
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
            let mut child_work = ComparisonWork::default();
            if restrict_comparison_domain(&self.layout, &mut child, &mut child_work, cell, value) {
                self.search(child, child_work, options, depth + 1, result, cell_order);
            }
        }
    }

    fn search_batch(
        &self,
        mut state: [u16; 81],
        mut work: ComparisonWork,
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

        let Some(cell) = choose_comparison_branch_cell(&state, &self.layout, cell_order) else {
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
            let mut child_work = ComparisonWork::default();
            if restrict_comparison_domain(&self.layout, &mut child, &mut child_work, cell, value)
                && !self.search_batch(child, child_work, limit, depth + 1, result, cell_order)
            {
                return false;
            }
        }
        true
    }

    /// Return true iff this entire subtree was exhausted.
    fn search_target_alternative(
        &self,
        mut state: [u16; 81],
        mut work: ComparisonWork,
        target: &[u8; 81],
        depth: u8,
        result: &mut TargetAlternativeResult,
        cell_order: &mut [u8; 81],
    ) -> bool {
        result.stats.nodes += 1;
        result.stats.max_depth = result.stats.max_depth.max(depth);
        if !self.propagate(&mut state, &mut work, &mut result.stats) {
            return true;
        }

        let Some(cell) = choose_comparison_branch_cell(&state, &self.layout, cell_order) else {
            let solution = masks_to_solution(&state);
            if target
                .iter()
                .zip(solution)
                .any(|(&expected, observed)| expected != 0 && expected != observed)
            {
                result.alternative_solution = Some(solution);
                return false;
            }
            result.target_matching_solutions += 1;
            if result.target_matching_solution.is_none() {
                result.target_matching_solution = Some(solution);
            }
            // Two solutions agreeing with the projection already prove
            // multiplicity, so there is no reason to search for an alternative.
            return result.target_matching_solutions < 2;
        };

        result.stats.branches += 1;
        let mut choices = state[cell];
        while choices != 0 {
            let value = low_bit(choices);
            choices &= choices - 1;
            let mut child = state;
            let mut child_work = ComparisonWork::default();
            if restrict_comparison_domain(&self.layout, &mut child, &mut child_work, cell, value)
                && !self.search_target_alternative(
                    child,
                    child_work,
                    target,
                    depth + 1,
                    result,
                    cell_order,
                )
            {
                return false;
            }
        }
        true
    }

    fn propagate(
        &self,
        state: &mut [u16; 81],
        work: &mut ComparisonWork,
        stats: &mut SolveStats,
    ) -> bool {
        loop {
            if let Some(cell) = work.pop_single() {
                stats.propagation_rounds += 1;
                let value = state[cell];
                debug_assert!(value.is_power_of_two());
                for &peer in &PEERS[cell] {
                    if !restrict_comparison_domain(
                        &self.layout,
                        state,
                        work,
                        peer as usize,
                        ALL & !value,
                    ) {
                        return false;
                    }
                }
                continue;
            }

            if let Some(comparison) = work.pop_comparison() {
                stats.propagation_rounds += 1;
                stats.thermo_revisions += 1;
                if !revise_comparison(&self.layout, state, work, comparison) {
                    return false;
                }
                // Both directions of this binary arc were revised. Changes to
                // its endpoints requeued it along with their other incident
                // arcs, so suppress only this redundant self-revision.
                work.clear_comparison(comparison);
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
                if !revise_comparison_house(&self.layout, state, work, house) {
                    return false;
                }
                continue;
            }

            return true;
        }
    }
}

fn validate_givens(givens: &[u8; 81]) -> Result<(), ComparisonProblemError> {
    for (cell, &digit) in givens.iter().enumerate() {
        if digit > 9 {
            return Err(ComparisonProblemError::InvalidGiven { cell, digit });
        }
    }
    Ok(())
}

fn validate_target_projection(target: &[u8; 81]) -> Result<(), TargetProjectionError> {
    let mut specified = false;
    for (cell, &digit) in target.iter().enumerate() {
        if digit > 9 {
            return Err(TargetProjectionError::InvalidDigit { cell, digit });
        }
        specified |= digit != 0;
    }
    if !specified {
        return Err(TargetProjectionError::Empty);
    }
    Ok(())
}

#[inline(always)]
fn restrict_comparison_domain(
    layout: &ComparisonLayout,
    state: &mut [u16; 81],
    work: &mut ComparisonWork,
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
    layout.mark_incident(work, cell);
    if !old.is_power_of_two() && next.is_power_of_two() {
        work.add_single(cell);
    }
    true
}

/// Exact generalized arc revision for `lower < upper`, including holes.
#[inline(always)]
fn revise_comparison(
    layout: &ComparisonLayout,
    state: &mut [u16; 81],
    work: &mut ComparisonWork,
    comparison: usize,
) -> bool {
    let (lower, upper) = layout.comparisons[comparison];
    let lower = lower as usize;
    let upper = upper as usize;

    let lower_min = low_bit(state[lower]);
    if lower_min == 0 {
        return false;
    }
    let supported_upper = ALL & !(lower_min.wrapping_shl(1).wrapping_sub(1));
    if !restrict_comparison_domain(layout, state, work, upper, supported_upper) {
        return false;
    }

    let upper_max = high_bit(state[upper]);
    if upper_max == 0 {
        return false;
    }
    restrict_comparison_domain(layout, state, work, lower, upper_max.wrapping_sub(1))
}

#[inline(always)]
fn remove_comparison_domain_bits(
    layout: &ComparisonLayout,
    state: &mut [u16; 81],
    work: &mut ComparisonWork,
    cell: usize,
    remove: u16,
) -> bool {
    remove == 0 || restrict_comparison_domain(layout, state, work, cell, ALL & !remove)
}

/// Comparison-backend copy of the optimized Sudoku house revision.  Keeping
/// it monomorphic avoids adding an abstraction to the established path solver.
fn revise_comparison_house(
    layout: &ComparisonLayout,
    state: &mut [u16; 81],
    work: &mut ComparisonWork,
    house: usize,
) -> bool {
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
            if !forced.is_power_of_two()
                || !restrict_comparison_domain(layout, state, work, cell, forced)
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
                    for col in stack * 3..stack * 3 + 3 {
                        if !remove_comparison_domain_bits(
                            layout,
                            state,
                            work,
                            other_row * 9 + col,
                            confined,
                        ) {
                            return false;
                        }
                    }
                }
            }
        }
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
                        if !remove_comparison_domain_bits(
                            layout,
                            state,
                            work,
                            row * 9 + other_col,
                            confined,
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
                        if !remove_comparison_domain_bits(
                            layout,
                            state,
                            work,
                            row * 9 + col,
                            confined,
                        ) {
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
                        if !remove_comparison_domain_bits(
                            layout,
                            state,
                            work,
                            row * 9 + col,
                            confined,
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

fn choose_comparison_branch_cell(
    state: &[u16; 81],
    layout: &ComparisonLayout,
    cell_order: &mut [u8; 81],
) -> Option<usize> {
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
        let degree = layout.degree[cell];
        if size < best_size || (size == best_size && degree > best_degree) {
            best_index = index;
            best_size = size;
            best_degree = degree;
            if size == 2 && degree == layout.max_degree {
                break;
            }
        }
    }
    cell_order.swap(unresolved, best_index);
    Some(cell_order[unresolved] as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Multiplicity;

    const KNOWN_THREE: &[&[u8]] = &[
        &[19, 29, 28, 20, 11, 12, 13, 3, 4],
        &[77, 69, 78, 70, 62, 53, 44, 52],
        &[41, 51],
    ];

    fn paths(raw: &[&[u8]]) -> Vec<Vec<u8>> {
        raw.iter().map(|path| path.to_vec()).collect()
    }

    fn flattened(paths: &[Vec<u8>]) -> Vec<(u8, u8)> {
        paths
            .iter()
            .flat_map(|path| path.windows(2).map(|edge| (edge[0], edge[1])))
            .collect()
    }

    fn satisfies(solution: &[u8; 81], comparisons: &[(u8, u8)]) -> bool {
        comparisons
            .iter()
            .all(|&(lower, upper)| solution[lower as usize] < solution[upper as usize])
    }

    #[test]
    fn capacity_covers_the_densest_seventeen_cell_induced_graph() {
        // Contiguous row widths 3,5,5,4 realize 46 induced king edges.
        let cells = [
            1, 2, 3, 9, 10, 11, 12, 13, 18, 19, 20, 21, 22, 27, 28, 29, 30,
        ];
        let mut comparisons = Vec::new();
        for (index, &lower) in cells.iter().enumerate() {
            for &upper in &cells[index + 1..] {
                if super::super::king_adjacent(lower, upper) {
                    comparisons.push((lower, upper));
                }
            }
        }
        assert_eq!(comparisons.len(), 46);
        assert!(comparisons.len() <= MAX_COMPARISONS);
        assert_eq!(
            ComparisonLayout::new(&comparisons)
                .unwrap()
                .comparison_count(),
            46
        );
    }

    #[test]
    fn row_mask_dp_proves_the_seventeen_cell_king_edge_maximum() {
        // A 9-bit mask describes the occupied columns in one row. Horizontal
        // edges are internal to a mask; vertical and diagonal edges depend
        // only on two consecutive row masks. This DP therefore exhausts all
        // 2^81 cell sets without enumerating them explicitly.
        const NEGATIVE: i16 = i16::MIN / 2;
        let row_masks = (0usize..512)
            .map(|mask| {
                (
                    mask,
                    mask.count_ones() as usize,
                    (mask & (mask >> 1)).count_ones() as i16,
                )
            })
            .collect::<Vec<_>>();
        let mut previous = vec![[NEGATIVE; 512]; 18];
        previous[0][0] = 0;
        for _row in 0..9 {
            let mut next = vec![[NEGATIVE; 512]; 18];
            for used in 0..=17 {
                for (previous_mask, &score) in previous[used].iter().enumerate() {
                    if score == NEGATIVE {
                        continue;
                    }
                    for &(current_mask, added, horizontal) in &row_masks {
                        if used + added > 17 {
                            continue;
                        }
                        let between = ((previous_mask & current_mask).count_ones()
                            + (((previous_mask << 1) & 0x1ff) & current_mask).count_ones()
                            + ((previous_mask >> 1) & current_mask).count_ones())
                            as i16;
                        let candidate = score + horizontal + between;
                        next[used + added][current_mask] =
                            next[used + added][current_mask].max(candidate);
                    }
                }
            }
            previous = next;
        }
        assert_eq!(previous[17].iter().copied().max(), Some(46));
    }

    #[test]
    fn capacity_is_enforced_before_a_dirty_bit_can_overflow() {
        let mut comparisons = Vec::new();
        'outer: for lower in 0u8..81 {
            for upper in 0u8..81 {
                if super::super::king_adjacent(lower, upper) {
                    comparisons.push((lower, upper));
                    if comparisons.len() == MAX_COMPARISONS + 1 {
                        break 'outer;
                    }
                }
            }
        }
        assert!(matches!(
            ComparisonLayout::new(&comparisons),
            Err(ComparisonLayoutError::TooManyComparisons {
                count: 65,
                maximum: 64
            })
        ));
    }

    #[test]
    fn explicit_and_path_duplicates_are_deduplicated() {
        let explicit = ComparisonLayout::new(&[(0, 1), (0, 1), (1, 10)]).unwrap();
        assert_eq!(explicit.comparisons(), &[(0, 1), (1, 10)]);

        let overlapping = vec![vec![0, 1, 10], vec![0, 1], vec![1, 10]];
        let paths = ComparisonLayout::from_paths(&overlapping).unwrap();
        assert_eq!(paths.comparisons(), explicit.comparisons());
    }

    #[test]
    fn binary_revision_matches_brute_force_supports_for_all_domain_pairs() {
        let layout = ComparisonLayout::new(&[(0, 1)]).unwrap();
        for lower_domain in 0..=ALL {
            for upper_domain in 0..=ALL {
                let mut supported_lower = 0u16;
                let mut supported_upper = 0u16;
                for lower_digit in 0..9 {
                    let lower_bit = 1u16 << lower_digit;
                    if lower_domain & lower_bit == 0 {
                        continue;
                    }
                    for upper_digit in lower_digit + 1..9 {
                        let upper_bit = 1u16 << upper_digit;
                        if upper_domain & upper_bit != 0 {
                            supported_lower |= lower_bit;
                            supported_upper |= upper_bit;
                        }
                    }
                }

                let expected_feasible = supported_lower != 0 && supported_upper != 0;
                let mut state = [ALL; 81];
                state[0] = lower_domain;
                state[1] = upper_domain;
                let observed =
                    revise_comparison(&layout, &mut state, &mut ComparisonWork::default(), 0);
                assert_eq!(observed, expected_feasible);
                if expected_feasible {
                    assert_eq!(state[0], supported_lower);
                    assert_eq!(state[1], supported_upper);
                }
            }
        }
    }

    #[test]
    fn shared_paths_form_branches_and_diamonds() {
        let branch = vec![vec![0, 10, 20], vec![10, 19]];
        let branch_solver = ComparisonSolver::blank_paths(&branch).unwrap();
        assert!(!branch_solver.uses_disjoint_path_fast_path());
        assert_eq!(branch_solver.layout().comparison_count(), 3);
        let branch_result = branch_solver.classify();
        assert_eq!(branch_result.multiplicity(), Multiplicity::Multiple);
        assert!(satisfies(
            branch_result.first_solution.as_ref().unwrap(),
            branch_solver.layout().comparisons()
        ));

        let diamond = vec![vec![0, 1, 10], vec![0, 9, 10]];
        let diamond_solver = ComparisonSolver::blank_paths(&diamond).unwrap();
        assert!(!diamond_solver.uses_disjoint_path_fast_path());
        assert_eq!(diamond_solver.layout().comparison_count(), 4);
        let diamond_result = diamond_solver.classify();
        assert_eq!(diamond_result.multiplicity(), Multiplicity::Multiple);
        assert!(satisfies(
            diamond_result.second_solution.as_ref().unwrap(),
            diamond_solver.layout().comparisons()
        ));
    }

    #[test]
    fn strict_cycles_are_unsatisfiable() {
        let two_cycle = ComparisonSolver::blank(&[(0, 1), (1, 0)])
            .unwrap()
            .classify();
        assert_eq!(two_cycle.multiplicity(), Multiplicity::Zero);

        let triangle = ComparisonSolver::blank(&[(0, 1), (1, 9), (9, 0)])
            .unwrap()
            .classify();
        assert_eq!(triangle.multiplicity(), Multiplicity::Zero);
    }

    #[test]
    fn disjoint_paths_delegate_to_the_original_solver_exactly() {
        let paths = paths(KNOWN_THREE);
        let original = Solver::blank(&paths).unwrap();
        let comparison = ComparisonSolver::blank_paths(&paths).unwrap();
        assert!(comparison.uses_disjoint_path_fast_path());
        assert_eq!(comparison.classify(), original.classify());
        assert_eq!(comparison.enumerate_up_to(3), original.enumerate_up_to(3));
    }

    #[test]
    fn sparse_target_search_stops_on_a_differing_solution() {
        let paths = paths(KNOWN_THREE);
        let reference = Solver::blank(&paths).unwrap().enumerate_up_to(4);
        assert!(reference.exhausted);
        assert_eq!(reference.solutions.len(), 3);
        let known = reference.solutions[0];
        let differing_cell = (0..81)
            .find(|&cell| {
                reference.solutions[1..]
                    .iter()
                    .any(|solution| solution[cell] != known[cell])
            })
            .unwrap();
        let mut target = [0u8; 81];
        target[differing_cell] = known[differing_cell];
        let mut specified = 1;
        for cell in 0..81 {
            if specified == 17 {
                break;
            }
            if target[cell] == 0 {
                target[cell] = known[cell];
                specified += 1;
            }
        }

        let comparisons = flattened(&paths);
        let result = ComparisonSolver::blank(&comparisons)
            .unwrap()
            .find_target_alternative(&target)
            .unwrap();
        let alternative = result.alternative_solution.unwrap();
        assert!(!result.exhausted);
        assert!(satisfies(&alternative, &comparisons));
        assert!(
            target
                .iter()
                .zip(alternative)
                .any(|(&expected, observed)| expected != 0 && expected != observed)
        );
    }

    #[test]
    fn sparse_target_search_proves_an_exact_unique_result() {
        let paths = paths(KNOWN_THREE);
        let known = Solver::blank(&paths).unwrap().enumerate_up_to(1).solutions[0];
        let solver = ComparisonSolver::new(known, &flattened(&paths)).unwrap();
        let mut target = [0u8; 81];
        target[..17].copy_from_slice(&known[..17]);

        let result = solver.find_target_alternative(&target).unwrap();
        assert!(result.exhausted);
        assert_eq!(result.target_matching_solutions, 1);
        assert_eq!(result.target_matching_solution, Some(known));
        assert_eq!(result.alternative_solution, None);
    }

    #[test]
    fn sparse_target_search_stops_after_two_matching_solutions() {
        let paths = paths(KNOWN_THREE);
        let reference = Solver::blank(&paths).unwrap().enumerate_up_to(4);
        let common_cell = (0..81)
            .find(|&cell| {
                reference
                    .solutions
                    .iter()
                    .all(|solution| solution[cell] == reference.solutions[0][cell])
            })
            .unwrap();
        let mut target = [0u8; 81];
        target[common_cell] = reference.solutions[0][common_cell];

        let result = ComparisonSolver::blank(&flattened(&paths))
            .unwrap()
            .find_target_alternative(&target)
            .unwrap();
        assert!(!result.exhausted);
        assert_eq!(result.target_matching_solutions, 2);
        assert!(result.target_matching_solution.is_some());
        assert_eq!(result.alternative_solution, None);
    }

    #[test]
    fn target_projection_validation_is_explicit() {
        let solver = ComparisonSolver::blank(&[(0, 1)]).unwrap();
        assert_eq!(
            solver.find_target_alternative(&[0; 81]),
            Err(TargetProjectionError::Empty)
        );
        let mut invalid = [0; 81];
        invalid[37] = 10;
        assert_eq!(
            solver.find_target_alternative(&invalid),
            Err(TargetProjectionError::InvalidDigit {
                cell: 37,
                digit: 10
            })
        );
    }

    #[test]
    fn graph_search_matches_filtering_a_complete_solution_set() {
        let base_paths = paths(KNOWN_THREE);
        let reference = Solver::blank(&base_paths).unwrap().enumerate_up_to(4);
        assert!(reference.exhausted);
        assert_eq!(reference.solutions.len(), 3);

        let mut all_edges = Vec::new();
        for lower in 0u8..81 {
            for upper in 0u8..81 {
                if super::super::king_adjacent(lower, upper) {
                    all_edges.push((lower, upper));
                }
            }
        }
        let base_comparisons = flattened(&base_paths);
        let mut seed = 0xd1b5_4a32_d192_ed03u64;
        for case in 0..32 {
            let mut comparisons = base_comparisons.clone();
            for _ in 0..case % 13 {
                seed ^= seed << 7;
                seed ^= seed >> 9;
                seed ^= seed << 8;
                comparisons.push(all_edges[seed as usize % all_edges.len()]);
            }

            let mut expected: Vec<_> = reference
                .solutions
                .iter()
                .copied()
                .filter(|solution| satisfies(solution, &comparisons))
                .collect();
            let batch = ComparisonSolver::blank(&comparisons)
                .unwrap()
                .enumerate_up_to(4);
            assert!(batch.exhausted, "case {case}");
            let mut observed = batch.solutions;
            expected.sort_unstable();
            observed.sort_unstable();
            assert_eq!(observed, expected, "case {case}");
        }
    }
}
