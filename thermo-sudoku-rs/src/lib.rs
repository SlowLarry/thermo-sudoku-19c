use std::fmt;
use std::sync::OnceLock;

const ALL: u16 = 0x01ff;
const NO_CELL: u8 = u8::MAX;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Thermometer {
    cells: Vec<u8>,
    templates: Vec<[u16; 9]>,
}

impl Thermometer {
    pub fn cells(&self) -> &[u8] {
        &self.cells
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layout {
    thermometers: Vec<Thermometer>,
}

impl Layout {
    pub fn new(paths: &[Vec<u8>]) -> Result<Self, LayoutError> {
        let mut occupied = [false; 81];
        let mut thermometers = Vec::with_capacity(paths.len());

        for (thermo_index, path) in paths.iter().enumerate() {
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
            }
            thermometers.push(Thermometer {
                cells: path.clone(),
                templates: increasing_templates(path.len()),
            });
        }

        Ok(Self { thermometers })
    }

    pub fn empty() -> Self {
        Self {
            thermometers: Vec::new(),
        }
    }

    pub fn thermometers(&self) -> &[Thermometer] {
        &self.thermometers
    }

    pub fn covered_cells(&self) -> usize {
        self.thermometers.iter().map(|t| t.cells.len()).sum()
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

#[derive(Clone, Debug)]
pub struct Solver {
    givens: [u8; 81],
    layout: Layout,
}

impl Solver {
    pub fn new(givens: [u8; 81], paths: &[Vec<u8>]) -> Result<Self, ProblemError> {
        for (cell, &digit) in givens.iter().enumerate() {
            if digit > 9 {
                return Err(ProblemError::InvalidGiven { cell, digit });
            }
        }
        Ok(Self {
            givens,
            layout: Layout::new(paths)?,
        })
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
        assert!(
            limit >= 2,
            "solution limit must be at least two to classify 0 / 1 / 2+"
        );
        let mut state = [ALL; 81];
        for (cell, &digit) in self.givens.iter().enumerate() {
            if digit != 0 {
                state[cell] = bit_for_digit(digit);
            }
        }

        let mut result = SolveResult {
            count: 0,
            capped: false,
            first_solution: None,
            second_solution: None,
            stats: SolveStats::default(),
        };
        self.search(state, limit, 0, &mut result);
        result.capped = result.count >= limit;
        result
    }

    fn search(&self, mut state: [u16; 81], limit: u64, depth: u8, result: &mut SolveResult) {
        if result.count >= limit {
            return;
        }
        result.stats.nodes += 1;
        result.stats.max_depth = result.stats.max_depth.max(depth);
        if !self.propagate(&mut state, &mut result.stats) {
            return;
        }

        let Some(cell) = choose_branch_cell(&state, &self.layout) else {
            let solution = masks_to_solution(&state);
            result.count += 1;
            if result.first_solution.is_none() {
                result.first_solution = Some(solution);
            } else if result.second_solution.is_none() {
                result.second_solution = Some(solution);
            }
            return;
        };

        result.stats.branches += 1;
        let mut choices = state[cell];
        while choices != 0 && result.count < limit {
            let value = low_bit(choices);
            choices &= choices - 1;
            let mut child = state;
            child[cell] = value;
            self.search(child, limit, depth + 1, result);
        }
    }

    fn propagate(&self, state: &mut [u16; 81], stats: &mut SolveStats) -> bool {
        loop {
            stats.propagation_rounds += 1;
            let mut changed = false;

            // Naked singles and peer eliminations.
            for cell in 0..81 {
                let mask = state[cell];
                if mask == 0 {
                    return false;
                }
                if mask.is_power_of_two() {
                    for &peer in &peers()[cell] {
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

            // Hidden singles and missing-digit contradictions.
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

            // Locked candidates in both box->line and line->box directions.
            if !propagate_locked_candidates(state, &mut changed) {
                return false;
            }

            // Exact generalized arc consistency for each increasing sequence.
            for thermo in &self.layout.thermometers {
                stats.thermo_revisions += 1;
                let mut supports = [0u16; 9];
                let mut active = 0usize;
                'template: for template in &thermo.templates {
                    for (position, &cell) in thermo.cells.iter().enumerate() {
                        if state[cell as usize] & template[position] == 0 {
                            continue 'template;
                        }
                    }
                    active += 1;
                    for position in 0..thermo.cells.len() {
                        supports[position] |= template[position];
                    }
                }
                if active == 0 {
                    return false;
                }
                for (position, &cell) in thermo.cells.iter().enumerate() {
                    let cell = cell as usize;
                    let next = state[cell] & supports[position];
                    if next == 0 {
                        return false;
                    }
                    if next != state[cell] {
                        state[cell] = next;
                        changed = true;
                    }
                }
            }

            if !changed {
                return true;
            }
        }
    }
}

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

fn choose_branch_cell(state: &[u16; 81], layout: &Layout) -> Option<usize> {
    let mut thermo_cell = [false; 81];
    for thermo in &layout.thermometers {
        for &cell in &thermo.cells {
            thermo_cell[cell as usize] = true;
        }
    }

    let mut best: Option<(u32, bool, usize)> = None;
    for (cell, &mask) in state.iter().enumerate() {
        let size = mask.count_ones();
        if size <= 1 {
            continue;
        }
        // Prefer smaller domains, then thermo cells, then row-major order.
        let key = (size, !thermo_cell[cell], cell);
        if best.is_none_or(|current| key < current) {
            best = Some(key);
        }
    }
    best.map(|(_, _, cell)| cell)
}

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

fn peers() -> &'static [[u8; 20]; 81] {
    static PEERS: OnceLock<[[u8; 20]; 81]> = OnceLock::new();
    PEERS.get_or_init(|| {
        let mut result = [[NO_CELL; 20]; 81];
        for cell in 0..81 {
            let row = cell / 9;
            let col = cell % 9;
            let mut seen = [false; 81];
            seen[cell] = true;
            let mut count = 0;
            for c in 0..9 {
                push_peer(&mut result[cell], &mut seen, &mut count, row * 9 + c);
            }
            for r in 0..9 {
                push_peer(&mut result[cell], &mut seen, &mut count, r * 9 + col);
            }
            let box_row = (row / 3) * 3;
            let box_col = (col / 3) * 3;
            for dr in 0..3 {
                for dc in 0..3 {
                    push_peer(
                        &mut result[cell],
                        &mut seen,
                        &mut count,
                        (box_row + dr) * 9 + box_col + dc,
                    );
                }
            }
            debug_assert_eq!(count, 20);
        }
        result
    })
}

fn push_peer(output: &mut [u8; 20], seen: &mut [bool; 81], count: &mut usize, cell: usize) {
    if !seen[cell] {
        seen[cell] = true;
        output[*count] = cell as u8;
        *count += 1;
    }
}

#[inline]
fn house_cell(house: usize, position: usize) -> usize {
    match house {
        0..=8 => house * 9 + position,
        9..=17 => position * 9 + (house - 9),
        _ => {
            let box_index = house - 18;
            let box_row = (box_index / 3) * 3;
            let box_col = (box_index % 3) * 3;
            (box_row + position / 3) * 9 + box_col + position % 3
        }
    }
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
    let mut paths = Vec::with_capacity(thermo_count);
    for window in offsets_slice.windows(2) {
        let start = window[0] as usize;
        let end = window[1] as usize;
        if start > end || end > total_cells {
            return -2;
        }
        paths.push(cell_slice[start..end].to_vec());
    }
    let givens_array = if givens.is_null() {
        [0u8; 81]
    } else {
        let mut array = [0u8; 81];
        array.copy_from_slice(unsafe { std::slice::from_raw_parts(givens, 81) });
        array
    };

    let Ok(solver) = Solver::new(givens_array, &paths) else {
        return -3;
    };
    let result = solver.count_up_to(limit);
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

    #[test]
    fn template_counts_are_binomial() {
        let expected = [0, 0, 36, 84, 126, 126, 84, 36, 9, 1];
        for (length, &count) in expected.iter().enumerate().skip(2) {
            assert_eq!(increasing_templates(length).len(), count);
        }
    }

    #[test]
    fn blue_twenty_is_unique() {
        let result = Solver::blank(&paths(BLUE_20)).unwrap().classify();
        assert_eq!(result.multiplicity(), Multiplicity::Unique);
        assert_eq!(result.count, 1);
    }

    #[test]
    fn known_nineteen_has_three_solutions() {
        let solver = Solver::blank(&paths(KNOWN_THREE)).unwrap();
        let classified = solver.classify();
        assert_eq!(classified.multiplicity(), Multiplicity::Multiple);
        assert_eq!(classified.count, 2);
        assert!(classified.capped);

        let counted = solver.count_up_to(4);
        assert_eq!(counted.count, 3);
        assert!(!counted.capped);
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
}
