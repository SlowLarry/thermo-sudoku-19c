//! Proof-oriented CNF master for non-overlapping thermometer layouts.
//!
//! It converts a validated `thermo-global-cegis-v1` grid-pair checkpoint into
//! a deterministic DIMACS master, strictly validates complete SAT models, and
//! can run a bounded CaDiCaL-compatible solve/check/learn sidecar loop.  A
//! satisfying assignment is a classic Sudoku together with a vertex-disjoint
//! union of directed king paths covering at most 19 cells and hitting every
//! supplied pair cut. The optional exact-9+8+2 scope adds a labeled component
//! encoding without changing the default formula. An UNSAT proof excludes the
//! topology scope named in that emitted CNF; only the default scope addresses
//! every disjoint layout covering at most 19 cells.

use std::collections::{HashSet, hash_map::RandomState};
use std::env;
use std::fs::{self, OpenOptions};
use std::hash::{BuildHasher, Hash};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitCode, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::time::Duration;

use thermo_sudoku::{Multiplicity, Solver};

const CELLS: usize = 81;
const DIGITS: usize = 9;
const DIRECTED_EDGES: usize = 544;
const UNDIRECTED_EDGES: usize = DIRECTED_EDGES / 2;
const COVER_LIMIT: usize = 19;
const CHECKPOINT_HEADER: &str = "# thermo-global-cegis-v1";
const ACTIVE_CUTS_HEADER: &str = "# thermo-topology-active-cuts-v1";
const ACTIVE_CUTS_HEADER_V2: &str = "# thermo-topology-active-cuts-v2";
const CNF_SCHEMA: &str = "thermo-topology-cnf-v2";
const EXACT_982_CNF_SCHEMA: &str = "thermo-topology-cnf-exact-9+8+2-v1";
const BRIDGE_PROTOCOL: &str = "thermo-cadical-bridge-v1";
const MAX_BRIDGE_RESPONSE_BYTES: usize = 1 << 20;
const DEFAULT_ORACLE_BATCH: usize = 32;
const MAX_ORACLE_BATCH: usize = 1024;
const DEFAULT_LAZY_ACTIVE_SEED: usize = 0;
const DEFAULT_LAZY_VIOLATION_BATCH: usize = 256;
const BASE_CLAUSE_COUNT: usize = 57_384;
const CHECKPOINT_PAIR_LINE_BYTES: u64 = (CELLS * 2 + 2) as u64;
// This is exactly one 1,000-iteration, all-pair, batch-64 continuation. Larger
// configurations grow in bounded chunks instead of reserving their possibly
// enormous theoretical maximum before the first solve.
const MAX_EAGER_REFINEMENT_RESERVE: usize = 2_080_000;
const RECORD_GROWTH_CHUNK: usize = 262_144;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

// Deterministic DIMACS variable ranges (all inclusive):
//   1..729       digit(cell, digit)
//   730..1273    selected directed edge
//   1274..1354   occupied cell
//   1355..2874   Sinz at-most-19 auxiliaries (80 * 19)
//   2875..7226   adjacent-symbol-swap witnesses (8 * 544)
const DIGIT_BASE: i32 = 1;
const EDGE_BASE: i32 = DIGIT_BASE + (CELLS * DIGITS) as i32;
const OCCUPIED_BASE: i32 = EDGE_BASE + DIRECTED_EDGES as i32;
const SEQUENTIAL_BASE: i32 = OCCUPIED_BASE + CELLS as i32;
const SEQUENTIAL_VARIABLES: usize = (CELLS - 1) * COVER_LIMIT;
const SWAP_BASE: i32 = SEQUENTIAL_BASE + SEQUENTIAL_VARIABLES as i32;
const SWAP_VARIABLES: usize = 8 * DIRECTED_EDGES;
const VARIABLE_COUNT: i32 = SWAP_BASE + SWAP_VARIABLES as i32 - 1;

// The exact-9+8+2 scope appends variables, leaving every generic variable ID
// stable.  Three cell labels identify the length-9, length-8, and length-2
// components. Source variables count directed path components. Full unary
// threshold counters make each requested cardinality exact rather than merely
// bounding it from above.
const EXACT_982_LABEL_BASE: i32 = VARIABLE_COUNT + 1;
const EXACT_982_LABEL_VARIABLES: usize = 3 * CELLS;
const EXACT_982_SOURCE_BASE: i32 = EXACT_982_LABEL_BASE + EXACT_982_LABEL_VARIABLES as i32;
const EXACT_982_SOURCE_VARIABLES: usize = CELLS;
const EXACT_982_COUNTER_BASE: i32 = EXACT_982_SOURCE_BASE + EXACT_982_SOURCE_VARIABLES as i32;
const EXACT_982_LABEL_9_COUNTER_VARIABLES: usize = CELLS * (9 + 1);
const EXACT_982_LABEL_8_COUNTER_VARIABLES: usize = CELLS * (8 + 1);
const EXACT_982_LABEL_2_COUNTER_VARIABLES: usize = CELLS * (2 + 1);
const EXACT_982_SOURCE_COUNTER_VARIABLES: usize = CELLS * (3 + 1);
const EXACT_982_COUNTER_VARIABLES: usize = EXACT_982_LABEL_9_COUNTER_VARIABLES
    + EXACT_982_LABEL_8_COUNTER_VARIABLES
    + EXACT_982_LABEL_2_COUNTER_VARIABLES
    + EXACT_982_SOURCE_COUNTER_VARIABLES;
const EXACT_982_VARIABLE_COUNT: i32 =
    EXACT_982_COUNTER_BASE + EXACT_982_COUNTER_VARIABLES as i32 - 1;
const EXACT_982_EXTRA_CLAUSE_COUNT: usize = 12_575;

type Grid = [u8; CELLS];
type Clause = Vec<i32>;

const PACKED_GRID_BYTES: usize = CELLS.div_ceil(2);
const EMPTY_INDEX_SLOT: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DirectedEdge {
    lower: u8,
    upper: u8,
}

/// A solved grid stored as two four-bit digits per byte.  The first cell in
/// each byte occupies the high nibble, so bytewise ordering is the same as
/// the canonical lexicographic ordering of unpacked grids.  The unused low
/// nibble of the final byte is always zero.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct PackedGrid([u8; PACKED_GRID_BYTES]);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct GridPair {
    first: PackedGrid,
    second: PackedGrid,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PairCut([u64; DIRECTED_EDGES.div_ceil(64)]);

#[derive(Debug)]
struct Checkpoint {
    budget: usize,
    checksum: u64,
    pairs: Vec<GridPair>,
    cuts: Vec<PairCut>,
    /// Index of the first pair that materializes each deduplicated cut.
    /// This is stable because checkpoints are append-only.
    cut_witnesses: Vec<u32>,
    /// Exact membership indexes.  Slots contain indices into the canonical,
    /// append-ordered vectors above; hashes only choose a probe sequence and
    /// full key equality resolves every collision.
    pair_index: FlatIndex,
    cut_index: FlatIndex,
}

struct FlatIndex {
    slots: Vec<u32>,
    hash_builder: RandomState,
}

impl std::fmt::Debug for FlatIndex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FlatIndex")
            .field("buckets", &self.slots.len())
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ActiveCutPool {
    indices: Vec<usize>,
    mask: Vec<bool>,
}

#[derive(Clone, Debug)]
struct LazyCutOptions {
    manifest: PathBuf,
    active_seed: usize,
    violation_batch: Option<usize>,
}

struct LazyCutRuntime {
    options: LazyCutOptions,
    active: ActiveCutPool,
    _manifest_lock: RunLock,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SatStatus {
    Satisfiable,
    Unsatisfiable,
    Unknown,
}

#[derive(Debug)]
struct SatResult {
    status: SatStatus,
    assignment: Option<Vec<bool>>,
}

#[derive(Debug)]
struct DecodedCandidate {
    target: Grid,
    selected: Vec<usize>,
    paths: Vec<Vec<u8>>,
    covered_cells: usize,
}

impl PackedGrid {
    fn new(grid: Grid) -> Self {
        debug_assert!(grid.iter().all(|digit| (1..=9).contains(digit)));
        let mut packed = [0u8; PACKED_GRID_BYTES];
        for (cell, digit) in grid.into_iter().enumerate() {
            let shift = if cell.is_multiple_of(2) { 4 } else { 0 };
            packed[cell / 2] |= digit << shift;
        }
        Self(packed)
    }

    fn digit(self, cell: usize) -> u8 {
        debug_assert!(cell < CELLS);
        let shift = if cell.is_multiple_of(2) { 4 } else { 0 };
        (self.0[cell / 2] >> shift) & 0x0f
    }

    #[cfg(test)]
    fn unpack(self) -> Grid {
        std::array::from_fn(|cell| self.digit(cell))
    }

    fn write_ascii(self, output: &mut [u8]) {
        debug_assert_eq!(output.len(), CELLS);
        for (cell, byte) in output.iter_mut().enumerate() {
            *byte = b'0' + self.digit(cell);
        }
    }
}

impl FlatIndex {
    fn new() -> Self {
        Self {
            slots: Vec::new(),
            hash_builder: RandomState::new(),
        }
    }

    fn buckets_for_entries(entries: usize) -> Result<usize, String> {
        if entries == 0 {
            return Ok(0);
        }
        // Keep linear probing at or below 75% load.  Power-of-two tables make
        // the probe sequence cheap, while u32 slots cap the supported pool
        // explicitly rather than allowing a platform-dependent usize limit.
        let required = entries
            .checked_mul(4)
            .and_then(|value| value.checked_add(2))
            .ok_or_else(|| "checkpoint index capacity overflow".to_string())?
            / 3;
        required
            .max(8)
            .checked_next_power_of_two()
            .ok_or_else(|| "checkpoint index table is too large".to_string())
    }

    fn hash<T: Hash>(&self, value: &T) -> u64 {
        self.hash_builder.hash_one(value)
    }

    fn find<T: Eq + Hash>(&self, values: &[T], value: &T) -> Option<usize> {
        if self.slots.is_empty() {
            return None;
        }
        let mask = self.slots.len() - 1;
        let mut bucket = self.hash(value) as usize & mask;
        loop {
            let stored = self.slots[bucket];
            if stored == EMPTY_INDEX_SLOT {
                return None;
            }
            let index = stored as usize;
            debug_assert!(index < values.len());
            if values[index] == *value {
                return Some(index);
            }
            bucket = (bucket + 1) & mask;
        }
    }

    fn contains<T: Eq + Hash>(&self, values: &[T], value: &T) -> bool {
        self.find(values, value).is_some()
    }

    fn reserve<T: Eq + Hash>(&mut self, values: &[T], entries: usize) -> Result<(), String> {
        if entries > EMPTY_INDEX_SLOT as usize {
            return Err(format!(
                "checkpoint has {entries} records, exceeding the exact u32 index limit"
            ));
        }
        let buckets = Self::buckets_for_entries(entries)?;
        if buckets <= self.slots.len() {
            return Ok(());
        }
        let mut replacement = Vec::new();
        replacement
            .try_reserve_exact(buckets)
            .map_err(|error| format!("cannot allocate checkpoint index: {error}"))?;
        replacement.resize(buckets, EMPTY_INDEX_SLOT);
        let old = std::mem::replace(&mut self.slots, replacement);
        for &stored in &old {
            if stored != EMPTY_INDEX_SLOT {
                self.insert_position(values, stored as usize)?;
            }
        }
        Ok(())
    }

    fn insert_position<T: Eq + Hash>(&mut self, values: &[T], index: usize) -> Result<(), String> {
        let stored = u32::try_from(index)
            .map_err(|_| "checkpoint exact index exhausted u32 positions".to_string())?;
        let value = values
            .get(index)
            .ok_or_else(|| "checkpoint exact index received an invalid position".to_string())?;
        if self.slots.is_empty() {
            return Err("checkpoint exact index was not reserved before insertion".into());
        }
        let mask = self.slots.len() - 1;
        let mut bucket = self.hash(value) as usize & mask;
        loop {
            if self.slots[bucket] == EMPTY_INDEX_SLOT {
                self.slots[bucket] = stored;
                return Ok(());
            }
            bucket = (bucket + 1) & mask;
        }
    }
}

fn try_reserve_exact<T>(values: &mut Vec<T>, capacity: usize, name: &str) -> Result<(), String> {
    values
        .try_reserve_exact(capacity.saturating_sub(values.len()))
        .map_err(|error| format!("cannot reserve {name} capacity {capacity}: {error}"))
}

fn indexed_insert<T: Copy + Eq + Hash>(
    values: &mut Vec<T>,
    index: &mut FlatIndex,
    value: T,
) -> Result<bool, String> {
    if index.contains(values, &value) {
        return Ok(false);
    }
    let new_len = values
        .len()
        .checked_add(1)
        .ok_or_else(|| "checkpoint record count overflow".to_string())?;
    index.reserve(values, new_len)?;
    values.push(value);
    index.insert_position(values, new_len - 1)?;
    Ok(true)
}

impl Checkpoint {
    fn insert_pair(&mut self, pair: GridPair) -> Result<bool, String> {
        indexed_insert(&mut self.pairs, &mut self.pair_index, pair)
    }

    fn insert_cut(&mut self, cut: PairCut, witness: usize) -> Result<bool, String> {
        if self.cut_index.contains(&self.cuts, &cut) {
            return Ok(false);
        }
        if witness >= self.pairs.len()
            || self
                .cut_witnesses
                .last()
                .is_some_and(|&previous| previous as usize >= witness)
        {
            return Err("cut witness is invalid or breaks first-occurrence order".into());
        }
        let witness = u32::try_from(witness)
            .map_err(|_| "cut witness index exceeds the supported u32 range".to_string())?;
        if !indexed_insert(&mut self.cuts, &mut self.cut_index, cut)? {
            return Err("cut index changed during insertion".into());
        }
        self.cut_witnesses.push(witness);
        Ok(true)
    }

    fn reserve_records(&mut self, capacity: usize) -> Result<(), String> {
        if capacity > EMPTY_INDEX_SLOT as usize {
            return Err(format!(
                "checkpoint capacity {capacity} exceeds the supported u32 index range"
            ));
        }
        try_reserve_exact(&mut self.pairs, capacity, "pair")?;
        try_reserve_exact(&mut self.cuts, capacity, "cut")?;
        try_reserve_exact(&mut self.cut_witnesses, capacity, "cut-witness")?;
        self.pair_index.reserve(&self.pairs, capacity)?;
        self.cut_index.reserve(&self.cuts, capacity)?;
        Ok(())
    }

    fn reserve_for_append(
        &mut self,
        additional_pairs: usize,
        additional_cuts: usize,
    ) -> Result<(), String> {
        let required_pairs = self
            .pairs
            .len()
            .checked_add(additional_pairs)
            .ok_or_else(|| "pair capacity overflow".to_string())?;
        let required_cuts = self
            .cuts
            .len()
            .checked_add(additional_cuts)
            .ok_or_else(|| "cut capacity overflow".to_string())?;
        let rounded = |required: usize| -> Result<usize, String> {
            required
                .checked_add(RECORD_GROWTH_CHUNK - 1)
                .map(|value| value / RECORD_GROWTH_CHUNK * RECORD_GROWTH_CHUNK)
                .ok_or_else(|| "checkpoint growth capacity overflow".to_string())
        };
        if required_pairs > self.pairs.capacity() {
            try_reserve_exact(&mut self.pairs, rounded(required_pairs)?, "pair")?;
        }
        if required_cuts > self.cuts.capacity() {
            let capacity = rounded(required_cuts)?;
            try_reserve_exact(&mut self.cuts, capacity, "cut")?;
        }
        if required_cuts > self.cut_witnesses.capacity() {
            let capacity = rounded(required_cuts)?;
            try_reserve_exact(&mut self.cut_witnesses, capacity, "cut-witness")?;
        }
        self.pair_index.reserve(&self.pairs, required_pairs)?;
        self.cut_index.reserve(&self.cuts, required_cuts)?;
        Ok(())
    }
}

impl GridPair {
    fn new(left: Grid, right: Grid) -> Result<Self, String> {
        if left == right {
            return Err("a learned pair must contain two different grids".into());
        }
        Ok(if left < right {
            Self {
                first: PackedGrid::new(left),
                second: PackedGrid::new(right),
            }
        } else {
            Self {
                first: PackedGrid::new(right),
                second: PackedGrid::new(left),
            }
        })
    }
}

fn digit_var(cell: usize, digit_index: usize) -> i32 {
    debug_assert!(cell < CELLS && digit_index < DIGITS);
    DIGIT_BASE + (cell * DIGITS + digit_index) as i32
}

fn edge_var(edge: usize) -> i32 {
    debug_assert!(edge < DIRECTED_EDGES);
    EDGE_BASE + edge as i32
}

fn occupied_var(cell: usize) -> i32 {
    debug_assert!(cell < CELLS);
    OCCUPIED_BASE + cell as i32
}

/// `prefix` is 0..80 for prefixes ending at cell 0..79, and `count` is
/// 0..19 for the proposition "this prefix contains at least count + 1 cells".
fn sequential_var(prefix: usize, count: usize) -> i32 {
    debug_assert!(prefix < CELLS - 1 && count < COVER_LIMIT);
    SEQUENTIAL_BASE + (prefix * COVER_LIMIT + count) as i32
}

fn swap_var(digit_index: usize, edge: usize) -> i32 {
    debug_assert!(digit_index < 8 && edge < DIRECTED_EDGES);
    SWAP_BASE + (digit_index * DIRECTED_EDGES + edge) as i32
}

fn exact_982_label_var(label: usize, cell: usize) -> i32 {
    debug_assert!(label < 3 && cell < CELLS);
    EXACT_982_LABEL_BASE + (label * CELLS + cell) as i32
}

fn exact_982_source_var(cell: usize) -> i32 {
    debug_assert!(cell < CELLS);
    EXACT_982_SOURCE_BASE + cell as i32
}

/// Full unary threshold counter variable: among inputs through `prefix`, at
/// least `threshold` are true. `threshold` is one-based and `width` is k + 1
/// for an exact-k counter.
fn exact_counter_var(base: i32, prefix: usize, threshold: usize, width: usize) -> i32 {
    debug_assert!(threshold > 0 && threshold <= width);
    base + (prefix * width + threshold - 1) as i32
}

fn directed_edges() -> Vec<DirectedEdge> {
    let mut result = Vec::with_capacity(DIRECTED_EDGES);
    for left in 0..CELLS {
        for right in left + 1..CELLS {
            let row_distance = (left / 9).abs_diff(right / 9);
            let column_distance = (left % 9).abs_diff(right % 9);
            if row_distance <= 1 && column_distance <= 1 {
                result.push(DirectedEdge {
                    lower: left as u8,
                    upper: right as u8,
                });
                result.push(DirectedEdge {
                    lower: right as u8,
                    upper: left as u8,
                });
            }
        }
    }
    assert_eq!(result.len(), DIRECTED_EDGES);
    result
}

fn push_at_most_one(clauses: &mut Vec<Clause>, variables: &[i32]) {
    for left in 0..variables.len() {
        for right in left + 1..variables.len() {
            clauses.push(vec![-variables[left], -variables[right]]);
        }
    }
}

fn push_exactly_one(clauses: &mut Vec<Clause>, variables: &[i32]) {
    clauses.push(variables.to_vec());
    push_at_most_one(clauses, variables);
}

fn classic_sudoku_clauses(clauses: &mut Vec<Clause>) {
    for cell in 0..CELLS {
        push_exactly_one(
            clauses,
            &(0..DIGITS)
                .map(|digit| digit_var(cell, digit))
                .collect::<Vec<_>>(),
        );
    }

    for digit in 0..DIGITS {
        for row in 0..9 {
            push_exactly_one(
                clauses,
                &(0..9)
                    .map(|column| digit_var(row * 9 + column, digit))
                    .collect::<Vec<_>>(),
            );
        }
        for column in 0..9 {
            push_exactly_one(
                clauses,
                &(0..9)
                    .map(|row| digit_var(row * 9 + column, digit))
                    .collect::<Vec<_>>(),
            );
        }
        for box_index in 0..9 {
            let box_row = (box_index / 3) * 3;
            let box_column = (box_index % 3) * 3;
            push_exactly_one(
                clauses,
                &(0..9)
                    .map(|position| {
                        digit_var(
                            (box_row + position / 3) * 9 + box_column + position % 3,
                            digit,
                        )
                    })
                    .collect::<Vec<_>>(),
            );
        }
    }
}

fn comparison_clauses(clauses: &mut Vec<Clause>, edges: &[DirectedEdge]) {
    for (edge_id, edge) in edges.iter().enumerate() {
        let selected = edge_var(edge_id);
        for lower_digit in 0..DIGITS {
            for upper_digit in 0..=lower_digit {
                clauses.push(vec![
                    -selected,
                    -digit_var(edge.lower as usize, lower_digit),
                    -digit_var(edge.upper as usize, upper_digit),
                ]);
            }
        }
    }
}

fn topology_clauses(clauses: &mut Vec<Clause>, edges: &[DirectedEdge]) {
    let mut incoming = vec![Vec::<i32>::new(); CELLS];
    let mut outgoing = vec![Vec::<i32>::new(); CELLS];
    let mut incident = vec![Vec::<i32>::new(); CELLS];

    for (edge_id, edge) in edges.iter().enumerate() {
        let selected = edge_var(edge_id);
        let lower = edge.lower as usize;
        let upper = edge.upper as usize;
        outgoing[lower].push(selected);
        incoming[upper].push(selected);
        incident[lower].push(selected);
        incident[upper].push(selected);
        clauses.push(vec![-selected, occupied_var(lower)]);
        clauses.push(vec![-selected, occupied_var(upper)]);
    }

    for cell in 0..CELLS {
        let mut occupied_implies_incident = vec![-occupied_var(cell)];
        occupied_implies_incident.extend(incident[cell].iter().copied());
        clauses.push(occupied_implies_incident);
        push_at_most_one(clauses, &incoming[cell]);
        push_at_most_one(clauses, &outgoing[cell]);
    }

    // A pair of opposite arcs is a directed 2-cycle.  Longer directed cycles
    // are impossible because selected arcs also impose strict digit increase.
    for pair in 0..UNDIRECTED_EDGES {
        clauses.push(vec![-edge_var(2 * pair), -edge_var(2 * pair + 1)]);
    }
}

fn coverage_clauses(clauses: &mut Vec<Clause>) {
    // Sinz sequential-counter encoding of sum(occupied[0..81]) <= 19.
    clauses.push(vec![-occupied_var(0), sequential_var(0, 0)]);
    for cell in 1..CELLS - 1 {
        clauses.push(vec![-occupied_var(cell), sequential_var(cell, 0)]);
        clauses.push(vec![-sequential_var(cell - 1, 0), sequential_var(cell, 0)]);
        for count in 1..COVER_LIMIT {
            clauses.push(vec![
                -occupied_var(cell),
                -sequential_var(cell - 1, count - 1),
                sequential_var(cell, count),
            ]);
            clauses.push(vec![
                -sequential_var(cell - 1, count),
                sequential_var(cell, count),
            ]);
        }
    }
    for cell in 1..CELLS {
        clauses.push(vec![
            -occupied_var(cell),
            -sequential_var(cell - 1, COVER_LIMIT - 1),
        ]);
    }
}

/// Append a propagation-complete definition of every prefix threshold through
/// k + 1, then require the final count to be at least k and not at least k + 1.
/// The returned value is the first unused variable after this counter.
fn exact_cardinality_clauses(
    clauses: &mut Vec<Clause>,
    variables: &[i32],
    count: usize,
    base: i32,
) -> i32 {
    assert!(!variables.is_empty() && count > 0 && count < variables.len());
    let width = count + 1;

    let first = exact_counter_var(base, 0, 1, width);
    clauses.push(vec![-variables[0], first]);
    clauses.push(vec![variables[0], -first]);
    for threshold in 2..=width {
        clauses.push(vec![-exact_counter_var(base, 0, threshold, width)]);
    }

    for (prefix, &input) in variables.iter().enumerate().skip(1) {
        let current = exact_counter_var(base, prefix, 1, width);
        let previous = exact_counter_var(base, prefix - 1, 1, width);
        // current <-> previous OR input
        clauses.push(vec![-previous, current]);
        clauses.push(vec![-input, current]);
        clauses.push(vec![-current, previous, input]);

        for threshold in 2..=width {
            let current = exact_counter_var(base, prefix, threshold, width);
            let previous = exact_counter_var(base, prefix - 1, threshold, width);
            let previous_lower = exact_counter_var(base, prefix - 1, threshold - 1, width);
            // current <-> previous OR (input AND previous_lower)
            clauses.push(vec![-previous, current]);
            clauses.push(vec![-input, -previous_lower, current]);
            clauses.push(vec![-current, previous, input]);
            clauses.push(vec![-current, previous, previous_lower]);
        }
    }

    let last = variables.len() - 1;
    clauses.push(vec![exact_counter_var(base, last, count, width)]);
    clauses.push(vec![-exact_counter_var(base, last, count + 1, width)]);
    base + (variables.len() * width) as i32
}

fn exact_982_geometry_clauses(clauses: &mut Vec<Clause>, edges: &[DirectedEdge]) {
    // Every occupied cell has exactly one of the three length labels, and no
    // unoccupied cell can carry one. Exact label counts sum to the existing
    // 19-cell coverage limit, so these clauses also force exactly 19 occupied
    // cells.
    for cell in 0..CELLS {
        let labels = (0..3)
            .map(|label| exact_982_label_var(label, cell))
            .collect::<Vec<_>>();
        for &label in &labels {
            clauses.push(vec![-label, occupied_var(cell)]);
        }
        let mut occupied_implies_label = vec![-occupied_var(cell)];
        occupied_implies_label.extend(labels.iter().copied());
        clauses.push(occupied_implies_label);
        push_at_most_one(clauses, &labels);
    }

    // A selected edge cannot cross a component label. Since both endpoints
    // are occupied, the conditional equivalences force equal labels.
    let mut incoming = vec![Vec::<i32>::new(); CELLS];
    for (edge_id, edge) in edges.iter().enumerate() {
        let selected = edge_var(edge_id);
        let lower = edge.lower as usize;
        let upper = edge.upper as usize;
        incoming[upper].push(selected);
        for label in 0..3 {
            let lower_label = exact_982_label_var(label, lower);
            let upper_label = exact_982_label_var(label, upper);
            clauses.push(vec![-selected, -lower_label, upper_label]);
            clauses.push(vec![-selected, -upper_label, lower_label]);
        }
    }

    // A source is exactly an occupied cell with no selected incoming edge.
    // Strict digit increase makes directed cycles impossible, so sources count
    // the path components of the selected graph.
    for (cell, incoming) in incoming.iter().enumerate() {
        let source = exact_982_source_var(cell);
        clauses.push(vec![-source, occupied_var(cell)]);
        for &selected in incoming {
            clauses.push(vec![-source, -selected]);
        }
        let mut source_if_no_incoming = vec![-occupied_var(cell)];
        source_if_no_incoming.extend(incoming.iter().copied());
        source_if_no_incoming.push(source);
        clauses.push(source_if_no_incoming);
    }

    let before_counters = clauses.len();
    let mut counter_base = EXACT_982_COUNTER_BASE;
    for (label, count) in [(0, 9), (1, 8), (2, 2)] {
        let variables = (0..CELLS)
            .map(|cell| exact_982_label_var(label, cell))
            .collect::<Vec<_>>();
        counter_base = exact_cardinality_clauses(clauses, &variables, count, counter_base);
    }
    let sources = (0..CELLS).map(exact_982_source_var).collect::<Vec<_>>();
    counter_base = exact_cardinality_clauses(clauses, &sources, 3, counter_base);
    debug_assert_eq!(counter_base - 1, EXACT_982_VARIABLE_COUNT);
    debug_assert_eq!(clauses.len() - before_counters, 8_038);
}

fn adjacent_swap_necessity_clauses(clauses: &mut Vec<Clause>, edges: &[DirectedEdge]) {
    // If swapping symbols d and d+1 in the existential target is not to leave
    // every selected comparison unchanged, at least one selected arc must
    // join exactly d to d+1.  The auxiliaries are one-way witnesses; the long
    // clause forces one real witness for each d.
    for digit_index in 0..8 {
        let mut witnesses = Vec::with_capacity(DIRECTED_EDGES);
        for (edge_id, edge) in edges.iter().enumerate() {
            let witness = swap_var(digit_index, edge_id);
            witnesses.push(witness);
            clauses.push(vec![-witness, edge_var(edge_id)]);
            clauses.push(vec![-witness, digit_var(edge.lower as usize, digit_index)]);
            clauses.push(vec![
                -witness,
                digit_var(edge.upper as usize, digit_index + 1),
            ]);
        }
        clauses.push(witnesses);
    }
}

fn less_or_equal_digit_clauses(clauses: &mut Vec<Clause>, left: usize, right: usize) {
    for left_digit in 0..DIGITS {
        for right_digit in 0..left_digit {
            clauses.push(vec![
                -digit_var(left, left_digit),
                -digit_var(right, right_digit),
            ]);
        }
    }
}

fn d4_complement_symmetry_clauses(clauses: &mut Vec<Clause>) {
    // Version 1 chooses an orbit representative under the square's D4 action
    // and simultaneous digit complement plus directed-arc reversal. Cell 0 is
    // a minimum corner, complement chooses its digit at most 5, and the final
    // diagonal reflection orders its east/south neighbours.
    for corner in [8, 72, 80] {
        less_or_equal_digit_clauses(clauses, 0, corner);
    }
    less_or_equal_digit_clauses(clauses, 1, 9);
    for digit_index in 5..DIGITS {
        clauses.push(vec![-digit_var(0, digit_index)]);
    }
}

fn base_clauses(edges: &[DirectedEdge], symmetry_break: SymmetryBreak) -> Vec<Clause> {
    base_clauses_for_scope(edges, symmetry_break, TopologyScope::AtMost19)
}

fn base_clauses_for_scope(
    edges: &[DirectedEdge],
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
) -> Vec<Clause> {
    let mut clauses = Vec::new();
    classic_sudoku_clauses(&mut clauses);
    comparison_clauses(&mut clauses, edges);
    topology_clauses(&mut clauses, edges);
    coverage_clauses(&mut clauses);
    adjacent_swap_necessity_clauses(&mut clauses, edges);
    if topology_scope == TopologyScope::Exact982 {
        exact_982_geometry_clauses(&mut clauses, edges);
    }
    if symmetry_break == SymmetryBreak::D4ComplementV1 {
        d4_complement_symmetry_clauses(&mut clauses);
    }
    clauses
}

fn pair_cut(pair: &GridPair, edges: &[DirectedEdge]) -> PairCut {
    let mut cut = PairCut([0; DIRECTED_EDGES.div_ceil(64)]);
    for (edge_id, edge) in edges.iter().enumerate() {
        let lower = edge.lower as usize;
        let upper = edge.upper as usize;
        if !(pair.first.digit(lower) < pair.first.digit(upper)
            && pair.second.digit(lower) < pair.second.digit(upper))
        {
            cut.0[edge_id / 64] |= 1u64 << (edge_id % 64);
        }
    }
    cut
}

fn pair_clause(cut: PairCut) -> Clause {
    let mut clause = Vec::new();
    for edge_id in 0..DIRECTED_EDGES {
        if cut.0[edge_id / 64] & (1u64 << (edge_id % 64)) != 0 {
            clause.push(edge_var(edge_id));
        }
    }
    clause
}

fn validate_unit(grid: &Grid, cells: impl Iterator<Item = usize>) -> bool {
    let mut seen = 0u16;
    for cell in cells {
        let digit = grid[cell];
        if !(1..=9).contains(&digit) {
            return false;
        }
        let bit = 1u16 << (digit - 1);
        if seen & bit != 0 {
            return false;
        }
        seen |= bit;
    }
    seen == 0x01ff
}

fn validate_sudoku(grid: &Grid) -> bool {
    for row in 0..9 {
        if !validate_unit(grid, (0..9).map(|column| row * 9 + column)) {
            return false;
        }
    }
    for column in 0..9 {
        if !validate_unit(grid, (0..9).map(|row| row * 9 + column)) {
            return false;
        }
    }
    for box_index in 0..9 {
        let box_row = (box_index / 3) * 3;
        let box_column = (box_index % 3) * 3;
        if !validate_unit(
            grid,
            (0..9).map(|position| (box_row + position / 3) * 9 + box_column + position % 3),
        ) {
            return false;
        }
    }
    true
}

fn parse_grid(text: &str) -> Result<Grid, String> {
    if text.len() != CELLS || !text.bytes().all(|byte| (b'1'..=b'9').contains(&byte)) {
        return Err("expected exactly 81 ASCII digits 1-9".into());
    }
    let mut grid = [0u8; CELLS];
    for (cell, byte) in text.bytes().enumerate() {
        grid[cell] = byte - b'0';
    }
    if !validate_sudoku(&grid) {
        return Err("grid is not a solved classic Sudoku".into());
    }
    Ok(grid)
}

fn fnv_byte(hash: &mut u64, byte: u8) {
    *hash ^= u64::from(byte);
    *hash = hash.wrapping_mul(FNV_PRIME);
}

fn edges_checksum(edges: &[DirectedEdge]) -> u64 {
    let mut checksum = FNV_OFFSET;
    for edge in edges {
        fnv_byte(&mut checksum, edge.lower);
        fnv_byte(&mut checksum, edge.upper);
    }
    checksum
}

fn load_checkpoint_with_reserve(
    path: &Path,
    additional_capacity: usize,
) -> Result<Checkpoint, String> {
    let file =
        fs::File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let file_size = file
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?
        .len();
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut raw_line = String::new();
    reader
        .read_line(&mut raw_line)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let header = raw_line
        .strip_suffix('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .unwrap_or(&raw_line);
    if header != CHECKPOINT_HEADER {
        return Err("wrong or missing checkpoint schema header".into());
    }

    let mut budget = None;
    let mut declared_edges = None;
    let mut declared_pairs = None;
    let mut declared_checksum = None;
    let mut footer = None;
    let mut data_started = false;
    let mut checksum = FNV_OFFSET;
    let mut checkpoint = Checkpoint {
        budget: 0,
        checksum: FNV_OFFSET,
        pairs: Vec::new(),
        cuts: Vec::new(),
        cut_witnesses: Vec::new(),
        pair_index: FlatIndex::new(),
        cut_index: FlatIndex::new(),
    };
    let edges = directed_edges();
    let mut line_number = 1usize;

    loop {
        raw_line.clear();
        let bytes = reader
            .read_line(&mut raw_line)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if bytes == 0 {
            break;
        }
        line_number += 1;
        let line = raw_line
            .strip_suffix('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .unwrap_or(&raw_line);
        if line.is_empty() {
            return Err(format!("line {line_number}: blank lines are not allowed"));
        }
        if let Some(value) = line.strip_prefix("# end pairs=") {
            if footer.is_some() {
                return Err(format!("line {line_number}: duplicate footer"));
            }
            let (count, hash) = value
                .split_once(" fnv1a64=")
                .ok_or_else(|| format!("line {line_number}: malformed footer"))?;
            footer = Some((
                count
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_number}: invalid footer count"))?,
                u64::from_str_radix(hash, 16)
                    .map_err(|_| format!("line {line_number}: invalid footer checksum"))?,
            ));
            continue;
        }
        if footer.is_some() {
            return Err(format!("line {line_number}: data after footer"));
        }
        if let Some(value) = line.strip_prefix("# budget=") {
            if data_started || budget.is_some() {
                return Err(format!("line {line_number}: misplaced or duplicate budget"));
            }
            budget = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_number}: invalid budget"))?,
            );
            continue;
        }
        if let Some(value) = line.strip_prefix("# directed_edges=") {
            if data_started || declared_edges.is_some() {
                return Err(format!(
                    "line {line_number}: misplaced or duplicate directed_edges"
                ));
            }
            declared_edges = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_number}: invalid directed_edges"))?,
            );
            continue;
        }
        if let Some(value) = line.strip_prefix("# pairs=") {
            if data_started || declared_pairs.is_some() {
                return Err(format!("line {line_number}: misplaced or duplicate pairs"));
            }
            let count = value
                .parse::<usize>()
                .map_err(|_| format!("line {line_number}: invalid pair count"))?;
            let count_u64 = u64::try_from(count)
                .map_err(|_| format!("line {line_number}: pair count is too large"))?;
            if count_u64 > file_size / CHECKPOINT_PAIR_LINE_BYTES {
                return Err(format!(
                    "line {line_number}: pair count is incompatible with checkpoint file size"
                ));
            }
            let capacity = count
                .checked_add(additional_capacity)
                .ok_or_else(|| "checkpoint reserve capacity overflow".to_string())?;
            checkpoint.reserve_records(capacity)?;
            declared_pairs = Some(count);
            continue;
        }
        if let Some(value) = line.strip_prefix("# fnv1a64=") {
            if data_started || declared_checksum.is_some() {
                return Err(format!(
                    "line {line_number}: misplaced or duplicate checksum"
                ));
            }
            declared_checksum = Some(
                u64::from_str_radix(value, 16)
                    .map_err(|_| format!("line {line_number}: invalid checksum"))?,
            );
            continue;
        }
        if line.starts_with('#') {
            return Err(format!("line {line_number}: unexpected metadata {line:?}"));
        }

        data_started = true;
        let (first_text, second_text) = line
            .split_once('|')
            .ok_or_else(|| format!("line {line_number}: missing pair separator"))?;
        if second_text.contains('|') {
            return Err(format!("line {line_number}: too many pair separators"));
        }
        if first_text >= second_text {
            return Err(format!(
                "line {line_number}: pair grids are not distinct and canonically ordered"
            ));
        }
        let first = parse_grid(first_text)
            .map_err(|error| format!("line {line_number}, first grid: {error}"))?;
        let second = parse_grid(second_text)
            .map_err(|error| format!("line {line_number}, second grid: {error}"))?;
        for &byte in &first {
            fnv_byte(&mut checksum, byte);
        }
        fnv_byte(&mut checksum, 0xfe);
        for &byte in &second {
            fnv_byte(&mut checksum, byte);
        }
        fnv_byte(&mut checksum, 0xff);
        let pair =
            GridPair::new(first, second).map_err(|error| format!("line {line_number}: {error}"))?;
        let pair_position = checkpoint.pairs.len();
        if !checkpoint.insert_pair(pair)? {
            return Err(format!("line {line_number}: duplicate grid-pair record"));
        }
        let cut = pair_cut(&pair, &edges);
        checkpoint.insert_cut(cut, pair_position)?;
    }

    let expected = (checkpoint.pairs.len(), checksum);
    if declared_edges != Some(DIRECTED_EDGES) {
        return Err(format!(
            "checkpoint declares {:?} directed edges, expected {DIRECTED_EDGES}",
            declared_edges
        ));
    }
    if declared_pairs != Some(checkpoint.pairs.len())
        || declared_checksum != Some(checksum)
        || footer != Some(expected)
        || budget.is_none()
    {
        return Err(format!(
            "checkpoint metadata/checksum mismatch (computed pairs={}, fnv1a64={checksum:016x})",
            checkpoint.pairs.len()
        ));
    }
    checkpoint.budget = budget.expect("checked above");
    checkpoint.checksum = checksum;
    debug_assert_eq!(checkpoint.cuts.len(), checkpoint.cut_witnesses.len());
    Ok(checkpoint)
}

fn load_checkpoint(path: &Path) -> Result<Checkpoint, String> {
    load_checkpoint_with_reserve(path, 0)
}

fn extend_pairs_checksum(mut checksum: u64, pairs: &[GridPair]) -> u64 {
    for pair in pairs {
        for cell in 0..CELLS {
            fnv_byte(&mut checksum, pair.first.digit(cell));
        }
        fnv_byte(&mut checksum, 0xfe);
        for cell in 0..CELLS {
            fnv_byte(&mut checksum, pair.second.digit(cell));
        }
        fnv_byte(&mut checksum, 0xff);
    }
    checksum
}

fn pairs_checksum(pairs: &[GridPair]) -> u64 {
    extend_pairs_checksum(FNV_OFFSET, pairs)
}

fn format_grid(grid: &Grid) -> String {
    grid.iter().map(|digit| char::from(b'0' + digit)).collect()
}

fn format_packed_grid(grid: PackedGrid) -> String {
    (0..CELLS)
        .map(|cell| char::from(b'0' + grid.digit(cell)))
        .collect()
}

fn write_checkpoint(checkpoint: &Checkpoint, output: &Path) -> Result<(), String> {
    let checksum = pairs_checksum(&checkpoint.pairs);
    if checksum != checkpoint.checksum {
        return Err("internal checkpoint checksum is stale".into());
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock error while checkpointing: {error}"))?
        .as_nanos();
    let file_name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "checkpoint path {} has no UTF-8 file name",
                output.display()
            )
        })?;
    let temporary =
        output.with_file_name(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));
    let file = fs::File::create(&temporary)
        .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
    let mut writer = BufWriter::with_capacity(1 << 20, &file);
    writeln!(writer, "{CHECKPOINT_HEADER}")
        .and_then(|_| writeln!(writer, "# budget={}", checkpoint.budget))
        .and_then(|_| writeln!(writer, "# directed_edges={DIRECTED_EDGES}"))
        .and_then(|_| writeln!(writer, "# pairs={}", checkpoint.pairs.len()))
        .and_then(|_| writeln!(writer, "# fnv1a64={checksum:016x}"))
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    let mut line = [0u8; CELLS * 2 + 2];
    line[CELLS] = b'|';
    line[CELLS * 2 + 1] = b'\n';
    for pair in &checkpoint.pairs {
        pair.first.write_ascii(&mut line[..CELLS]);
        pair.second.write_ascii(&mut line[CELLS + 1..CELLS * 2 + 1]);
        writer
            .write_all(&line)
            .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    }
    writeln!(
        writer,
        "# end pairs={} fnv1a64={checksum:016x}",
        checkpoint.pairs.len()
    )
    .and_then(|_| writer.flush())
    .map_err(|error| format!("cannot finish {}: {error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", temporary.display()))?;
    drop(writer);
    drop(file);
    replace_file(&temporary, output).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination).map_err(|error| {
        format!(
            "cannot atomically replace {} with {}: {error}",
            destination.display(),
            source.display()
        )
    })?;
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let directory = fs::File::open(parent).map_err(|error| {
        format!(
            "cannot open checkpoint directory {}: {error}",
            parent.display()
        )
    })?;
    directory.sync_all().map_err(|error| {
        format!(
            "cannot sync checkpoint directory {} after atomic replace: {error}",
            parent.display()
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
        // for this call. The flags request a same-volume, write-through replace.
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
        "cannot atomically replace {} with {} after {REPLACE_ATTEMPTS} attempts: {}",
        destination.display(),
        source.display(),
        last_error.expect("at least one replacement attempt")
    ))
}

impl ActiveCutPool {
    fn from_indices(pool_len: usize, indices: Vec<usize>) -> Result<Self, String> {
        let mut mask = vec![false; pool_len];
        for &index in &indices {
            if index >= pool_len {
                return Err(format!(
                    "active cut index {index} exceeds pool size {pool_len}"
                ));
            }
            if std::mem::replace(&mut mask[index], true) {
                return Err(format!("duplicate active cut index {index}"));
            }
        }
        Ok(Self { indices, mask })
    }

    fn extend_pool(&mut self, pool_len: usize) -> Result<(), String> {
        if pool_len < self.mask.len() {
            return Err("pair-cut pool unexpectedly shrank".into());
        }
        self.mask.resize(pool_len, false);
        Ok(())
    }

    fn activate(&mut self, index: usize) -> Result<(), String> {
        let slot = self
            .mask
            .get_mut(index)
            .ok_or_else(|| format!("active cut index {index} is outside the full pool"))?;
        if *slot {
            return Err(format!("pair cut {index} is already active"));
        }
        *slot = true;
        self.indices.push(index);
        Ok(())
    }

    fn validate(&self, pool_len: usize) -> Result<(), String> {
        if self.mask.len() != pool_len
            || self.mask.iter().filter(|&&active| active).count() != self.indices.len()
            || self
                .indices
                .iter()
                .any(|&index| index >= pool_len || !self.mask[index])
        {
            return Err("active-cut indices and membership mask disagree".into());
        }
        Ok(())
    }
}

fn evenly_spaced_cut_indices(pool_len: usize, requested: usize) -> Vec<usize> {
    let count = requested.min(pool_len);
    if count == 0 {
        return Vec::new();
    }
    (0..count)
        .map(|position| position * pool_len / count)
        .collect()
}

fn active_cuts_checksum(checkpoint: &Checkpoint, indices: &[usize]) -> Result<u64, String> {
    let mut checksum = FNV_OFFSET;
    for &index in indices {
        let witness_index = *checkpoint
            .cut_witnesses
            .get(index)
            .ok_or_else(|| format!("active cut index {index} has no checkpoint witness"))?
            as usize;
        for byte in (index as u64).to_le_bytes() {
            fnv_byte(&mut checksum, byte);
        }
        let pair = checkpoint
            .pairs
            .get(witness_index)
            .ok_or_else(|| format!("active cut {index} has an invalid witness index"))?;
        for cell in 0..CELLS {
            fnv_byte(&mut checksum, pair.first.digit(cell));
        }
        fnv_byte(&mut checksum, 0xfe);
        for cell in 0..CELLS {
            fnv_byte(&mut checksum, pair.second.digit(cell));
        }
        fnv_byte(&mut checksum, 0xff);
    }
    Ok(checksum)
}

fn write_active_cuts_manifest(
    checkpoint: &Checkpoint,
    active: &ActiveCutPool,
    output: &Path,
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
) -> Result<(), String> {
    active.validate(checkpoint.cuts.len())?;
    let checksum = active_cuts_checksum(checkpoint, &active.indices)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock error while saving active cuts: {error}"))?
        .as_nanos();
    let file_name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            format!(
                "active-cut path {} has no UTF-8 file name",
                output.display()
            )
        })?;
    let temporary =
        output.with_file_name(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));
    let file = fs::File::create(&temporary)
        .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
    let mut writer = BufWriter::with_capacity(1 << 20, &file);
    let edge_checksum = edges_checksum(&directed_edges());
    let header = match topology_scope {
        TopologyScope::AtMost19 => ACTIVE_CUTS_HEADER,
        TopologyScope::Exact982 => ACTIVE_CUTS_HEADER_V2,
    };
    writeln!(writer, "{header}")
        .and_then(|_| writeln!(writer, "# cnf_schema={}", topology_scope.cnf_schema()))
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    if topology_scope == TopologyScope::Exact982 {
        writeln!(writer, "# topology_scope={}", topology_scope.as_str())
            .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    }
    writeln!(writer, "# symmetry_break={}", symmetry_break.as_str())
        .and_then(|_| writeln!(writer, "# edge_order_fnv1a64={edge_checksum:016x}"))
        .and_then(|_| writeln!(writer, "# directed_edges={DIRECTED_EDGES}"))
        .and_then(|_| writeln!(writer, "# pool_pairs={}", checkpoint.pairs.len()))
        .and_then(|_| writeln!(writer, "# pool_unique_cuts={}", checkpoint.cuts.len()))
        .and_then(|_| writeln!(writer, "# pool_fnv1a64={:016x}", checkpoint.checksum))
        .and_then(|_| writeln!(writer, "# active_cuts={}", active.indices.len()))
        .and_then(|_| writeln!(writer, "# fnv1a64={checksum:016x}"))
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    for &index in &active.indices {
        let witness = checkpoint.pairs[checkpoint.cut_witnesses[index] as usize];
        writeln!(
            writer,
            "{index}|{}|{}",
            format_packed_grid(witness.first),
            format_packed_grid(witness.second)
        )
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    }
    writeln!(
        writer,
        "# end active_cuts={} fnv1a64={checksum:016x}",
        active.indices.len()
    )
    .and_then(|_| writer.flush())
    .map_err(|error| format!("cannot finish {}: {error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", temporary.display()))?;
    drop(writer);
    drop(file);
    replace_file(&temporary, output).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

fn load_active_cuts_manifest(
    path: &Path,
    checkpoint: &Checkpoint,
    edges: &[DirectedEdge],
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
) -> Result<ActiveCutPool, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut lines = contents.lines().enumerate();
    let header = lines.next().map(|(_, line)| line);
    let legacy = header == Some(ACTIVE_CUTS_HEADER);
    if !legacy && header != Some(ACTIVE_CUTS_HEADER_V2) {
        return Err("wrong or missing active-cut manifest schema header".into());
    }
    let mut declared_edges = None;
    let mut declared_schema = None;
    let mut declared_topology_scope = None;
    let mut declared_symmetry = None;
    let mut declared_edge_checksum = None;
    let mut declared_pool_pairs = None;
    let mut declared_pool_cuts = None;
    let mut declared_pool_checksum = None;
    let mut declared_active = None;
    let mut declared_checksum = None;
    let mut footer = None;
    let mut data_started = false;
    let mut indices = Vec::new();

    for (zero_line, line) in lines {
        let line_number = zero_line + 1;
        if line.is_empty() {
            return Err(format!("line {line_number}: blank lines are not allowed"));
        }
        if let Some(value) = line.strip_prefix("# end active_cuts=") {
            if footer.is_some() {
                return Err(format!("line {line_number}: duplicate footer"));
            }
            let (count, hash) = value
                .split_once(" fnv1a64=")
                .ok_or_else(|| format!("line {line_number}: malformed footer"))?;
            footer = Some((
                count
                    .parse::<usize>()
                    .map_err(|_| format!("line {line_number}: invalid footer count"))?,
                u64::from_str_radix(hash, 16)
                    .map_err(|_| format!("line {line_number}: invalid footer checksum"))?,
            ));
            continue;
        }
        if footer.is_some() {
            return Err(format!("line {line_number}: data after footer"));
        }
        let metadata = [
            ("# directed_edges=", &mut declared_edges),
            ("# pool_pairs=", &mut declared_pool_pairs),
            ("# pool_unique_cuts=", &mut declared_pool_cuts),
            ("# active_cuts=", &mut declared_active),
        ];
        let mut handled = false;
        for (prefix, slot) in metadata {
            if let Some(value) = line.strip_prefix(prefix) {
                if data_started || slot.is_some() {
                    return Err(format!(
                        "line {line_number}: misplaced or duplicate {prefix}"
                    ));
                }
                *slot = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| format!("line {line_number}: invalid {prefix}"))?,
                );
                handled = true;
                break;
            }
        }
        if handled {
            continue;
        }
        if let Some(value) = line.strip_prefix("# cnf_schema=") {
            if data_started || declared_schema.replace(value.to_string()).is_some() {
                return Err(format!(
                    "line {line_number}: misplaced or duplicate CNF schema"
                ));
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("# topology_scope=") {
            if data_started || declared_topology_scope.replace(value.to_string()).is_some() {
                return Err(format!(
                    "line {line_number}: misplaced or duplicate topology scope"
                ));
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("# symmetry_break=") {
            if data_started || declared_symmetry.replace(value.to_string()).is_some() {
                return Err(format!(
                    "line {line_number}: misplaced or duplicate symmetry mode"
                ));
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("# edge_order_fnv1a64=") {
            if data_started || declared_edge_checksum.is_some() {
                return Err(format!(
                    "line {line_number}: misplaced or duplicate edge checksum"
                ));
            }
            declared_edge_checksum = Some(
                u64::from_str_radix(value, 16)
                    .map_err(|_| format!("line {line_number}: invalid edge checksum"))?,
            );
            continue;
        }
        if let Some(value) = line.strip_prefix("# pool_fnv1a64=") {
            if data_started || declared_pool_checksum.is_some() {
                return Err(format!(
                    "line {line_number}: misplaced or duplicate pool checksum"
                ));
            }
            declared_pool_checksum = Some(
                u64::from_str_radix(value, 16)
                    .map_err(|_| format!("line {line_number}: invalid pool checksum"))?,
            );
            continue;
        }
        if let Some(value) = line.strip_prefix("# fnv1a64=") {
            if data_started || declared_checksum.is_some() {
                return Err(format!(
                    "line {line_number}: misplaced or duplicate active checksum"
                ));
            }
            declared_checksum = Some(
                u64::from_str_radix(value, 16)
                    .map_err(|_| format!("line {line_number}: invalid active checksum"))?,
            );
            continue;
        }
        if line.starts_with('#') {
            return Err(format!("line {line_number}: unexpected metadata {line:?}"));
        }
        data_started = true;
        let mut fields = line.split('|');
        let index = fields
            .next()
            .ok_or_else(|| format!("line {line_number}: missing cut index"))?
            .parse::<usize>()
            .map_err(|_| format!("line {line_number}: invalid cut index"))?;
        let first_text = fields
            .next()
            .ok_or_else(|| format!("line {line_number}: missing first witness grid"))?;
        let second_text = fields
            .next()
            .ok_or_else(|| format!("line {line_number}: missing second witness grid"))?;
        if fields.next().is_some() || first_text >= second_text {
            return Err(format!("line {line_number}: malformed active-cut witness"));
        }
        let pair = GridPair::new(
            parse_grid(first_text)
                .map_err(|error| format!("line {line_number}, first grid: {error}"))?,
            parse_grid(second_text)
                .map_err(|error| format!("line {line_number}, second grid: {error}"))?,
        )
        .map_err(|error| format!("line {line_number}: {error}"))?;
        let expected_witness = checkpoint
            .cut_witnesses
            .get(index)
            .and_then(|&pair_index| checkpoint.pairs.get(pair_index as usize))
            .ok_or_else(|| format!("line {line_number}: cut index {index} is outside the pool"))?;
        if &pair != expected_witness || pair_cut(&pair, edges) != checkpoint.cuts[index] {
            return Err(format!(
                "line {line_number}: witness does not match checkpoint cut {index}"
            ));
        }
        indices.push(index);
    }

    let pool_pairs = declared_pool_pairs
        .ok_or_else(|| "active-cut manifest has no pool pair count".to_string())?;
    let pool_cuts = declared_pool_cuts
        .ok_or_else(|| "active-cut manifest has no pool cut count".to_string())?;
    let pool_checksum = declared_pool_checksum
        .ok_or_else(|| "active-cut manifest has no pool checksum".to_string())?;
    let scope_metadata_valid = if legacy {
        topology_scope == TopologyScope::AtMost19
            && declared_schema.as_deref() == Some(CNF_SCHEMA)
            && declared_topology_scope.is_none()
    } else {
        declared_schema.as_deref() == Some(topology_scope.cnf_schema())
            && declared_topology_scope.as_deref() == Some(topology_scope.as_str())
    };
    if !scope_metadata_valid
        || declared_symmetry.as_deref() != Some(symmetry_break.as_str())
        || declared_edge_checksum != Some(edges_checksum(edges))
        || declared_edges != Some(DIRECTED_EDGES)
        || pool_pairs > checkpoint.pairs.len()
        || pool_cuts > checkpoint.cuts.len()
        || checkpoint
            .cut_witnesses
            .partition_point(|&pair| (pair as usize) < pool_pairs)
            != pool_cuts
        || pairs_checksum(&checkpoint.pairs[..pool_pairs]) != pool_checksum
        || indices.iter().any(|&index| index >= pool_cuts)
    {
        return Err(
            "active-cut manifest does not describe this checkpoint or an append-only prefix".into(),
        );
    }
    let active = ActiveCutPool::from_indices(checkpoint.cuts.len(), indices)?;
    let checksum = active_cuts_checksum(checkpoint, &active.indices)?;
    if declared_active != Some(active.indices.len())
        || declared_checksum != Some(checksum)
        || footer != Some((active.indices.len(), checksum))
    {
        return Err("active-cut manifest metadata/checksum mismatch".into());
    }
    Ok(active)
}

fn parse_sat_result(text: &str, variable_count: i32) -> Result<SatResult, String> {
    let mut status = None;
    let mut values = vec![None::<bool>; variable_count as usize + 1];
    let mut saw_literal = false;
    for (line_index, raw_line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('c') {
            continue;
        }
        if let Some(value) = line.strip_prefix("s ") {
            let parsed = match value.trim() {
                "SATISFIABLE" => SatStatus::Satisfiable,
                "UNSATISFIABLE" => SatStatus::Unsatisfiable,
                "UNKNOWN" => SatStatus::Unknown,
                other => {
                    return Err(format!(
                        "model line {line_number}: unknown status {other:?}"
                    ));
                }
            };
            if status.replace(parsed).is_some_and(|old| old != parsed) {
                return Err("model contains conflicting status lines".into());
            }
            continue;
        }
        let Some(literals) = line.strip_prefix('v') else {
            return Err(format!(
                "model line {line_number}: expected a comment, status, or witness line"
            ));
        };
        if !literals.is_empty() && !literals.starts_with(char::is_whitespace) {
            return Err(format!(
                "model line {line_number}: malformed witness prefix"
            ));
        }
        for token in literals.split_whitespace() {
            let literal = token.parse::<i32>().map_err(|_| {
                format!("model line {line_number}: invalid literal token {token:?}")
            })?;
            if literal == 0 {
                continue;
            }
            let variable = literal.unsigned_abs() as usize;
            if variable > variable_count as usize {
                return Err(format!(
                    "model line {line_number}: variable {variable} exceeds {variable_count}"
                ));
            }
            saw_literal = true;
            let value = literal > 0;
            if values[variable]
                .replace(value)
                .is_some_and(|old| old != value)
            {
                return Err(format!(
                    "model line {line_number}: variable {variable} has conflicting values"
                ));
            }
        }
    }

    let status = status.ok_or_else(|| "model has no status line".to_string())?;
    let assignment = if status == SatStatus::Satisfiable {
        if !saw_literal {
            return Err("SAT model contains no witness literals".into());
        }
        let missing = (1..=variable_count as usize)
            .filter(|&variable| values[variable].is_none())
            .take(8)
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(format!(
                "SAT model is partial; first missing variables are {missing:?}"
            ));
        }
        Some(
            values
                .into_iter()
                .map(|value| value.unwrap_or(false))
                .collect(),
        )
    } else {
        if saw_literal {
            return Err("non-SAT result unexpectedly contains witness literals".into());
        }
        None
    };
    Ok(SatResult { status, assignment })
}

fn clause_satisfied(clause: &[i32], assignment: &[bool]) -> bool {
    clause.iter().any(|&literal| {
        let value = assignment[literal.unsigned_abs() as usize];
        if literal > 0 { value } else { !value }
    })
}

fn selected_edge_mask(assignment: &[bool]) -> PairCut {
    let mut mask = PairCut([0; DIRECTED_EDGES.div_ceil(64)]);
    for edge_id in 0..DIRECTED_EDGES {
        if assignment[edge_var(edge_id) as usize] {
            mask.0[edge_id / 64] |= 1u64 << (edge_id % 64);
        }
    }
    mask
}

fn pair_cut_satisfied(cut: PairCut, selected: PairCut) -> bool {
    cut.0
        .iter()
        .zip(selected.0)
        .any(|(cut_word, selected_word)| cut_word & selected_word != 0)
}

fn violated_inactive_cut_indices(
    cuts: &[PairCut],
    active: &ActiveCutPool,
    selected: PairCut,
    limit: Option<usize>,
) -> Result<(Vec<usize>, usize), String> {
    active.validate(cuts.len())?;
    let mut violated = Vec::new();
    for (index, &cut) in cuts.iter().enumerate() {
        if !active.mask[index] && !pair_cut_satisfied(cut, selected) {
            let length = cut.0.iter().map(|word| word.count_ones()).sum::<u32>();
            violated.push((length, index));
        }
    }
    let total = violated.len();
    // Shorter positive clauses are normally stronger. The pool ID is the
    // stable deterministic tie-breaker and is also what the manifest records.
    violated.sort_unstable();
    if let Some(limit) = limit {
        violated.truncate(limit);
    }
    Ok((
        violated.into_iter().map(|(_, index)| index).collect(),
        total,
    ))
}

#[cfg(test)]
fn decode_candidate_with_base(
    required_cuts: &[PairCut],
    assignment: &[bool],
    edges: &[DirectedEdge],
    base: &[Clause],
) -> Result<DecodedCandidate, String> {
    decode_candidate_with_scope_and_base(
        required_cuts,
        assignment,
        edges,
        base,
        TopologyScope::AtMost19,
    )
}

fn decode_candidate_with_scope_and_base(
    required_cuts: &[PairCut],
    assignment: &[bool],
    edges: &[DirectedEdge],
    base: &[Clause],
    topology_scope: TopologyScope,
) -> Result<DecodedCandidate, String> {
    let variable_count = topology_scope.variable_count();
    if assignment.len() != variable_count as usize + 1 {
        return Err(format!(
            "assignment has {} entries, expected {}",
            assignment.len(),
            variable_count + 1
        ));
    }
    if let Some((index, _)) = base
        .iter()
        .enumerate()
        .find(|(_, clause)| !clause_satisfied(clause, assignment))
    {
        return Err(format!("model violates base CNF clause {}", index + 1));
    }
    let selected_mask = selected_edge_mask(assignment);
    for (cut_index, &cut) in required_cuts.iter().enumerate() {
        if !pair_cut_satisfied(cut, selected_mask) {
            return Err(format!(
                "model violates checkpoint pair-cut clause {}",
                cut_index + 1
            ));
        }
    }

    let mut target = [0u8; CELLS];
    for (cell, target_digit) in target.iter_mut().enumerate() {
        let selected_digits = (0..DIGITS)
            .filter(|&digit| assignment[digit_var(cell, digit) as usize])
            .collect::<Vec<_>>();
        if selected_digits.len() != 1 {
            return Err(format!(
                "model selects {} digits for cell {cell}",
                selected_digits.len()
            ));
        }
        *target_digit = selected_digits[0] as u8 + 1;
    }
    if !validate_sudoku(&target) {
        return Err("decoded target is not a solved classic Sudoku".into());
    }

    let selected = (0..DIRECTED_EDGES)
        .filter(|&edge| assignment[edge_var(edge) as usize])
        .collect::<Vec<_>>();
    let mut incoming = [None::<usize>; CELLS];
    let mut outgoing = [None::<usize>; CELLS];
    let mut occupied = [false; CELLS];
    for &edge_id in &selected {
        let edge = edges[edge_id];
        let lower = edge.lower as usize;
        let upper = edge.upper as usize;
        if target[lower] >= target[upper] {
            return Err(format!(
                "selected edge {edge_id} ({lower}<{upper}) does not increase in the target"
            ));
        }
        if outgoing[lower].replace(edge_id).is_some() {
            return Err(format!("cell {lower} has two outgoing selected edges"));
        }
        if incoming[upper].replace(edge_id).is_some() {
            return Err(format!("cell {upper} has two incoming selected edges"));
        }
        occupied[lower] = true;
        occupied[upper] = true;
    }

    let covered_cells = occupied.iter().filter(|&&value| value).count();
    if covered_cells > COVER_LIMIT {
        return Err(format!(
            "decoded layout covers {covered_cells} cells, exceeding {COVER_LIMIT}"
        ));
    }
    let mut visited_edges = vec![false; DIRECTED_EDGES];
    let mut paths = Vec::new();
    for cell in 0..CELLS {
        if incoming[cell].is_some() || outgoing[cell].is_none() {
            continue;
        }
        let mut path = vec![cell as u8];
        let mut current = cell;
        while let Some(edge_id) = outgoing[current] {
            if visited_edges[edge_id] {
                return Err("selected graph contains a directed cycle".into());
            }
            visited_edges[edge_id] = true;
            let next = edges[edge_id].upper;
            path.push(next);
            current = next as usize;
            if path.len() > 9 {
                return Err("decoded thermometer has more than nine cells".into());
            }
        }
        paths.push(path);
    }
    if selected.iter().any(|&edge| !visited_edges[edge]) {
        return Err("selected graph contains an unrooted directed cycle".into());
    }
    paths.sort_unstable();
    let solver = Solver::blank(&paths)
        .map_err(|error| format!("decoded paths fail the thermo layout validator: {error}"))?;
    if solver.layout().covered_cells() != covered_cells {
        return Err("decoded paths and occupied-cell count disagree".into());
    }
    let candidate = DecodedCandidate {
        target,
        selected,
        paths,
        covered_cells,
    };
    topology_scope.validates_candidate(&candidate)?;
    Ok(candidate)
}

#[cfg(test)]
fn decode_candidate(
    checkpoint: &Checkpoint,
    assignment: &[bool],
) -> Result<DecodedCandidate, String> {
    let edges = directed_edges();
    let base = base_clauses(&edges, SymmetryBreak::None);
    decode_candidate_with_base(&checkpoint.cuts, assignment, &edges, &base)
}

fn format_paths(paths: &[Vec<u8>]) -> String {
    paths
        .iter()
        .map(|path| path.iter().map(u8::to_string).collect::<Vec<_>>().join(","))
        .collect::<Vec<_>>()
        .join("|")
}

fn format_candidate_body(candidate: &DecodedCandidate, edges: &[DirectedEdge]) -> String {
    let selected_edges = candidate
        .selected
        .iter()
        .map(|&edge_id| {
            let edge = edges[edge_id];
            format!("{}<{}", edge.lower, edge.upper)
        })
        .collect::<Vec<_>>()
        .join(";");
    let selected_edge_ids = candidate
        .selected
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(";");
    format!(
        "target={}\nthermos={}\ncovered_cells={}\nthermometer_count={}\ncomparison_count={}\nselected_edge_ids={}\nselected_edges={}\n",
        format_grid(&candidate.target),
        format_paths(&candidate.paths),
        candidate.covered_cells,
        candidate.paths.len(),
        candidate.selected.len(),
        selected_edge_ids,
        selected_edges
    )
}

fn format_candidate(candidate: &DecodedCandidate, edges: &[DirectedEdge]) -> String {
    format!("status=sat\n{}", format_candidate_body(candidate, edges))
}

fn invoke_sat(
    executable: &Path,
    cnf: &Path,
    model: &Path,
    proof: Option<&Path>,
    conflicts: Option<u64>,
    variable_count: i32,
) -> Result<SatResult, String> {
    if model.exists() {
        fs::remove_file(model)
            .map_err(|error| format!("cannot remove stale model {}: {error}", model.display()))?;
    }
    if let Some(proof) = proof
        && proof.exists()
    {
        fs::remove_file(proof)
            .map_err(|error| format!("cannot remove stale proof {}: {error}", proof.display()))?;
    }
    let mut command = Command::new(executable);
    command.arg("-q").arg("-w").arg(model);
    if let Some(conflicts) = conflicts {
        command.arg("-c").arg(conflicts.to_string());
    }
    command.arg(cnf);
    if let Some(proof) = proof {
        command.arg(proof);
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot run {}: {error}", executable.display()))?;
    let code = output.status.code();
    if !matches!(code, Some(0 | 10 | 20)) {
        return Err(format!(
            "SAT process exited with {:?}: {}",
            code,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let model_text = if model.exists() {
        fs::read_to_string(model)
            .map_err(|error| format!("cannot read model {}: {error}", model.display()))?
    } else {
        String::from_utf8(output.stdout)
            .map_err(|_| "SAT solver stdout is not valid UTF-8".to_string())?
    };
    let result = parse_sat_result(&model_text, variable_count)?;
    let expected_code = match result.status {
        SatStatus::Satisfiable => 10,
        SatStatus::Unsatisfiable => 20,
        SatStatus::Unknown => 0,
    };
    if code != Some(expected_code) {
        return Err(format!(
            "SAT result {:?} disagrees with process exit code {:?}",
            result.status, code
        ));
    }
    Ok(result)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BridgeMetadata {
    cadical: String,
    revision: String,
    library_sha256: String,
    prefer_selected: bool,
}

fn parse_bridge_ready(
    line: &str,
    expected_variables: usize,
    expected_clauses: usize,
    expected_prefer_selected: bool,
) -> Result<BridgeMetadata, String> {
    let tokens = line.split_whitespace().collect::<Vec<_>>();
    if tokens.len() != 8 || tokens[0] != "READY" || tokens[1] != BRIDGE_PROTOCOL {
        return Err(format!("malformed bridge READY response {line:?}"));
    }
    let field = |index: usize, name: &str| -> Result<&str, String> {
        tokens[index]
            .strip_prefix(name)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("malformed bridge READY field {name:?}"))
    };
    let variables = field(2, "variables=")?
        .parse::<usize>()
        .map_err(|_| "invalid bridge variable count".to_string())?;
    let clauses = field(3, "clauses=")?
        .parse::<usize>()
        .map_err(|_| "invalid bridge clause count".to_string())?;
    let cadical = field(4, "cadical=")?.to_string();
    let revision = field(5, "revision=")?.to_string();
    let library_sha256 = field(6, "library_sha256=")?.to_string();
    let prefer_selected = match field(7, "prefer_selected=")? {
        "0" => false,
        "1" => true,
        _ => return Err("invalid bridge prefer_selected flag".into()),
    };
    if variables != expected_variables {
        return Err(format!(
            "bridge loaded {variables} variables, expected {expected_variables}"
        ));
    }
    if clauses != expected_clauses {
        return Err(format!(
            "bridge loaded {clauses} clauses, expected {expected_clauses}"
        ));
    }
    if prefer_selected != expected_prefer_selected {
        return Err("bridge phase-hint mode disagrees with the requested mode".into());
    }
    Ok(BridgeMetadata {
        cadical,
        revision,
        library_sha256,
        prefer_selected,
    })
}

fn parse_bridge_model(line: &str, variable_count: usize) -> Result<Vec<bool>, String> {
    let mut tokens = line.split_whitespace();
    if tokens.next() != Some("MODEL") {
        return Err(format!("expected bridge MODEL response, got {line:?}"));
    }
    let mut assignment = vec![false; variable_count + 1];
    for (variable, value) in assignment.iter_mut().enumerate().skip(1) {
        let token = tokens
            .next()
            .ok_or_else(|| format!("bridge model ends before variable {variable}"))?;
        let literal = token
            .parse::<i32>()
            .map_err(|_| format!("invalid bridge model literal {token:?}"))?;
        if literal.unsigned_abs() as usize != variable {
            return Err(format!(
                "bridge model position {variable} contains literal {literal}"
            ));
        }
        *value = literal > 0;
    }
    if tokens.next() != Some("0") || tokens.next().is_some() {
        return Err("bridge model has a missing terminator or trailing data".into());
    }
    Ok(assignment)
}

struct IncrementalBridge {
    child: Option<Child>,
    input: Option<BufWriter<ChildStdin>>,
    output: BufReader<ChildStdout>,
    initial_clauses: usize,
    added_clauses: usize,
    variable_count: usize,
    metadata: BridgeMetadata,
    executable: PathBuf,
}

impl IncrementalBridge {
    fn spawn(
        executable: &Path,
        cnf: &Path,
        variable_count: usize,
        clauses: usize,
        prefer_selected: bool,
    ) -> Result<Self, String> {
        let executable = fs::canonicalize(executable).map_err(|error| {
            format!(
                "cannot resolve bridge executable {}: {error}",
                executable.display()
            )
        })?;
        let mut command = Command::new(&executable);
        command
            .arg("--cnf")
            .arg(cnf)
            .arg("--variables")
            .arg(variable_count.to_string())
            .arg("--clauses")
            .arg(clauses.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Inheriting stderr prevents a verbose or failing child from ever
            // blocking on an undrained diagnostic pipe.
            .stderr(Stdio::inherit());
        if prefer_selected {
            command.arg("--prefer-selected");
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot start bridge {}: {error}", executable.display()))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| "bridge stdin pipe was not created".to_string())?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| "bridge stdout pipe was not created".to_string())?;
        let mut bridge = Self {
            child: Some(child),
            input: Some(BufWriter::new(input)),
            output: BufReader::new(output),
            initial_clauses: clauses,
            added_clauses: 0,
            variable_count,
            metadata: BridgeMetadata {
                cadical: String::new(),
                revision: String::new(),
                library_sha256: String::new(),
                prefer_selected,
            },
            executable,
        };
        let ready = bridge.read_line("READY")?;
        bridge.metadata = parse_bridge_ready(&ready, variable_count, clauses, prefer_selected)?;
        Ok(bridge)
    }

    fn read_line(&mut self, expected: &str) -> Result<String, String> {
        let mut line = String::new();
        let bytes = Read::by_ref(&mut self.output)
            .take((MAX_BRIDGE_RESPONSE_BYTES + 1) as u64)
            .read_line(&mut line)
            .map_err(|error| format!("cannot read bridge {expected} response: {error}"))?;
        if bytes == 0 {
            let status = self
                .child
                .as_mut()
                .and_then(|child| child.try_wait().ok().flatten());
            return Err(format!(
                "bridge closed stdout while waiting for {expected} (status={status:?})"
            ));
        }
        if line.len() > MAX_BRIDGE_RESPONSE_BYTES {
            return Err(format!("bridge {expected} response is too long"));
        }
        if !line.ends_with('\n') {
            return Err(format!("bridge {expected} response is truncated"));
        }
        while line.ends_with(['\n', '\r']) {
            line.pop();
        }
        if let Some(message) = line.strip_prefix("ERROR ") {
            return Err(format!("bridge rejected command: {message}"));
        }
        Ok(line)
    }

    fn send_line(&mut self, line: &str) -> Result<(), String> {
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| "bridge input is already closed".to_string())?;
        writeln!(input, "{line}")
            .and_then(|_| input.flush())
            .map_err(|error| format!("cannot write bridge command: {error}"))
    }

    fn solve(&mut self, conflicts: Option<u64>) -> Result<SatResult, String> {
        let limit = match conflicts {
            Some(value) if value <= i32::MAX as u64 => value.to_string(),
            Some(value) => {
                return Err(format!(
                    "conflict limit {value} exceeds the bridge maximum {}",
                    i32::MAX
                ));
            }
            None => "-1".to_string(),
        };
        self.send_line(&format!("SOLVE {limit}"))?;
        let response = self.read_line("RESULT")?;
        match response.as_str() {
            "RESULT UNSAT" => Ok(SatResult {
                status: SatStatus::Unsatisfiable,
                assignment: None,
            }),
            "RESULT UNKNOWN" => Ok(SatResult {
                status: SatStatus::Unknown,
                assignment: None,
            }),
            _ => {
                let expected = format!("RESULT SAT {}", self.variable_count);
                if response != expected {
                    return Err(format!("malformed bridge result {response:?}"));
                }
                let model = self.read_line("MODEL")?;
                Ok(SatResult {
                    status: SatStatus::Satisfiable,
                    assignment: Some(parse_bridge_model(&model, self.variable_count)?),
                })
            }
        }
    }

    fn add_clause(&mut self, clause: &[i32]) -> Result<(), String> {
        if clause.len() > self.variable_count
            || clause
                .iter()
                .any(|literal| *literal == 0 || literal.unsigned_abs() > self.variable_count as u32)
        {
            return Err("refusing to send an invalid incremental clause".into());
        }
        let mut command = format!("ADD {}", clause.len());
        for literal in clause {
            command.push(' ');
            command.push_str(&literal.to_string());
        }
        command.push_str(" 0");
        self.send_line(&command)?;
        let response = self.read_line("ADDED")?;
        self.added_clauses += 1;
        let expected = format!(
            "ADDED {} {} {}",
            self.added_clauses,
            clause.len(),
            self.initial_clauses + self.added_clauses
        );
        if response != expected {
            return Err(format!(
                "malformed bridge acknowledgement {response:?}, expected {expected:?}"
            ));
        }
        Ok(())
    }

    fn total_clauses(&self) -> usize {
        self.initial_clauses + self.added_clauses
    }

    fn shutdown(&mut self) -> Result<(), String> {
        if self.child.is_none() {
            return Ok(());
        }
        self.send_line("QUIT")?;
        let response = self.read_line("BYE")?;
        let expected = format!("BYE {}", self.added_clauses);
        if response != expected {
            return Err(format!(
                "malformed bridge shutdown response {response:?}, expected {expected:?}"
            ));
        }
        self.input.take();
        let status = self
            .child
            .as_mut()
            .expect("checked above")
            .wait()
            .map_err(|error| format!("cannot wait for bridge shutdown: {error}"))?;
        self.child.take();
        if !status.success() {
            return Err(format!("bridge exited unsuccessfully after BYE: {status}"));
        }
        Ok(())
    }
}

impl Drop for IncrementalBridge {
    fn drop(&mut self) {
        // Closing stdin normally makes the bridge exit on EOF. Kill-and-wait
        // is the bounded fallback, including every error-return path.
        self.input.take();
        if let Some(mut child) = self.child.take() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

fn write_clause(writer: &mut impl Write, clause: &[i32]) -> std::io::Result<()> {
    for literal in clause {
        write!(writer, "{literal} ")?;
    }
    writeln!(writer, "0")
}

fn write_cnf(
    checkpoint: &Checkpoint,
    output: &Path,
    symmetry_break: SymmetryBreak,
) -> Result<(usize, usize), String> {
    let edges = directed_edges();
    let edge_checksum = edges_checksum(&edges);
    let base = base_clauses(&edges, symmetry_break);
    let clause_count = base.len() + checkpoint.cuts.len();
    let file = fs::File::create(output)
        .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "c {CNF_SCHEMA}")
        .and_then(|_| {
            writeln!(
                writer,
                "c model classic-sudoku plus disjoint-directed-king-paths"
            )
        })
        .and_then(|_| writeln!(writer, "c covered_cells_at_most {COVER_LIMIT}"))
        .and_then(|_| writeln!(writer, "c diagonal_crossings_without_shared_cells allowed"))
        .and_then(|_| writeln!(writer, "c symmetry_break {}", symmetry_break.as_str()))
        .and_then(|_| writeln!(writer, "c checkpoint_budget {}", checkpoint.budget))
        .and_then(|_| writeln!(writer, "c checkpoint_pairs {}", checkpoint.pairs.len()))
        .and_then(|_| writeln!(writer, "c unique_pair_cuts {}", checkpoint.cuts.len()))
        .and_then(|_| writeln!(writer, "c checkpoint_fnv1a64 {:016x}", checkpoint.checksum))
        .and_then(|_| writeln!(writer, "c digit_variables 1 729"))
        .and_then(|_| writeln!(writer, "c edge_variables 730 1273"))
        .and_then(|_| writeln!(writer, "c occupied_variables 1274 1354"))
        .and_then(|_| writeln!(writer, "c sequential_variables 1355 2874"))
        .and_then(|_| writeln!(writer, "c swap_witness_variables 2875 7226"))
        .and_then(|_| {
            writeln!(
                writer,
                "c edge_order lexicographic unordered cell pair then forward and reverse"
            )
        })
        .and_then(|_| writeln!(writer, "c edge_order_fnv1a64 {edge_checksum:016x}"))
        .and_then(|_| writeln!(writer, "c digit_var 1+9*cell+(digit-1)"))
        .and_then(|_| writeln!(writer, "c edge_var 730+edge_id"))
        .and_then(|_| writeln!(writer, "c occupied_var 1274+cell"))
        .and_then(|_| writeln!(writer, "c sequential_var 1355+19*prefix+count"))
        .and_then(|_| writeln!(writer, "c swap_var 2875+544*(digit-1)+edge_id"))
        .and_then(|_| writeln!(writer, "p cnf {VARIABLE_COUNT} {clause_count}"))
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;

    for clause in &base {
        write_clause(&mut writer, clause)
            .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    }
    for &cut in &checkpoint.cuts {
        write_clause(&mut writer, &pair_clause(cut))
            .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    }
    writer
        .flush()
        .map_err(|error| format!("cannot finish {}: {error}", output.display()))?;
    Ok((VARIABLE_COUNT as usize, clause_count))
}

fn write_exact_982_cnf(
    checkpoint: &Checkpoint,
    output: &Path,
    symmetry_break: SymmetryBreak,
) -> Result<(usize, usize), String> {
    let edges = directed_edges();
    let edge_checksum = edges_checksum(&edges);
    let topology_scope = TopologyScope::Exact982;
    let base = base_clauses_for_scope(&edges, symmetry_break, topology_scope);
    let clause_count = base.len() + checkpoint.cuts.len();
    let file = fs::File::create(output)
        .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "c {EXACT_982_CNF_SCHEMA}")
        .and_then(|_| writeln!(writer, "c topology_scope {}", topology_scope.as_str()))
        .and_then(|_| {
            writeln!(
                writer,
                "c model classic-sudoku plus exactly-three-disjoint-directed-king-paths"
            )
        })
        .and_then(|_| writeln!(writer, "c thermometer_lengths 9 8 2"))
        .and_then(|_| writeln!(writer, "c covered_cells_exactly 19"))
        .and_then(|_| writeln!(writer, "c diagonal_crossings_without_shared_cells allowed"))
        .and_then(|_| writeln!(writer, "c symmetry_break {}", symmetry_break.as_str()))
        .and_then(|_| writeln!(writer, "c checkpoint_budget {}", checkpoint.budget))
        .and_then(|_| writeln!(writer, "c checkpoint_pairs {}", checkpoint.pairs.len()))
        .and_then(|_| writeln!(writer, "c unique_pair_cuts {}", checkpoint.cuts.len()))
        .and_then(|_| writeln!(writer, "c checkpoint_fnv1a64 {:016x}", checkpoint.checksum))
        .and_then(|_| writeln!(writer, "c digit_variables 1 729"))
        .and_then(|_| writeln!(writer, "c edge_variables 730 1273"))
        .and_then(|_| writeln!(writer, "c occupied_variables 1274 1354"))
        .and_then(|_| writeln!(writer, "c sequential_variables 1355 2874"))
        .and_then(|_| writeln!(writer, "c swap_witness_variables 2875 7226"))
        .and_then(|_| writeln!(writer, "c component_label_variables 7227 7469"))
        .and_then(|_| writeln!(writer, "c path_source_variables 7470 7550"))
        .and_then(|_| writeln!(writer, "c exact_counter_variables 7551 9656"))
        .and_then(|_| {
            writeln!(
                writer,
                "c edge_order lexicographic unordered cell pair then forward and reverse"
            )
        })
        .and_then(|_| writeln!(writer, "c edge_order_fnv1a64 {edge_checksum:016x}"))
        .and_then(|_| writeln!(writer, "c digit_var 1+9*cell+(digit-1)"))
        .and_then(|_| writeln!(writer, "c edge_var 730+edge_id"))
        .and_then(|_| writeln!(writer, "c occupied_var 1274+cell"))
        .and_then(|_| writeln!(writer, "c sequential_var 1355+19*prefix+count"))
        .and_then(|_| writeln!(writer, "c swap_var 2875+544*(digit-1)+edge_id"))
        .and_then(|_| writeln!(writer, "c component_label_var 7227+81*label+cell"))
        .and_then(|_| writeln!(writer, "c path_source_var 7470+cell"))
        .and_then(|_| writeln!(writer, "p cnf {EXACT_982_VARIABLE_COUNT} {clause_count}"))
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;

    for clause in &base {
        write_clause(&mut writer, clause)
            .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    }
    for &cut in &checkpoint.cuts {
        write_clause(&mut writer, &pair_clause(cut))
            .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    }
    writer
        .flush()
        .map_err(|error| format!("cannot finish {}: {error}", output.display()))?;
    Ok((EXACT_982_VARIABLE_COUNT as usize, clause_count))
}

fn write_cnf_for_scope(
    checkpoint: &Checkpoint,
    output: &Path,
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
) -> Result<(usize, usize), String> {
    match topology_scope {
        TopologyScope::AtMost19 => write_cnf(checkpoint, output, symmetry_break),
        TopologyScope::Exact982 => write_exact_982_cnf(checkpoint, output, symmetry_break),
    }
}

/// Write the exact static formula currently held by a lazy bridge: the full
/// topology base plus only the active, globally valid pair cuts. The manifest
/// retains a solved-grid witness for every listed cut, so this smaller formula
/// can be regenerated and independently audited after an incremental UNSAT.
fn write_lazy_cnf(
    checkpoint: &Checkpoint,
    active: &ActiveCutPool,
    output: &Path,
    symmetry_break: SymmetryBreak,
) -> Result<(usize, usize), String> {
    active.validate(checkpoint.cuts.len())?;
    let edges = directed_edges();
    let edge_checksum = edges_checksum(&edges);
    let base = base_clauses(&edges, symmetry_break);
    let clause_count = base.len() + active.indices.len();
    let file = fs::File::create(output)
        .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "c {CNF_SCHEMA}")
        .and_then(|_| writeln!(writer, "c cut_pool_mode lazy-active-v1"))
        .and_then(|_| {
            writeln!(
                writer,
                "c model classic-sudoku plus disjoint-directed-king-paths"
            )
        })
        .and_then(|_| writeln!(writer, "c covered_cells_at_most {COVER_LIMIT}"))
        .and_then(|_| writeln!(writer, "c diagonal_crossings_without_shared_cells allowed"))
        .and_then(|_| writeln!(writer, "c symmetry_break {}", symmetry_break.as_str()))
        .and_then(|_| writeln!(writer, "c checkpoint_budget {}", checkpoint.budget))
        .and_then(|_| writeln!(writer, "c checkpoint_pairs {}", checkpoint.pairs.len()))
        .and_then(|_| writeln!(writer, "c full_unique_pair_cuts {}", checkpoint.cuts.len()))
        .and_then(|_| writeln!(writer, "c active_pair_cuts {}", active.indices.len()))
        .and_then(|_| writeln!(writer, "c checkpoint_fnv1a64 {:016x}", checkpoint.checksum))
        .and_then(|_| {
            writeln!(
                writer,
                "c active_fnv1a64 {:016x}",
                active_cuts_checksum(checkpoint, &active.indices)
                    .expect("active pool validated above")
            )
        })
        .and_then(|_| writeln!(writer, "c digit_variables 1 729"))
        .and_then(|_| writeln!(writer, "c edge_variables 730 1273"))
        .and_then(|_| writeln!(writer, "c occupied_variables 1274 1354"))
        .and_then(|_| writeln!(writer, "c sequential_variables 1355 2874"))
        .and_then(|_| writeln!(writer, "c swap_witness_variables 2875 7226"))
        .and_then(|_| {
            writeln!(
                writer,
                "c edge_order lexicographic unordered cell pair then forward and reverse"
            )
        })
        .and_then(|_| writeln!(writer, "c edge_order_fnv1a64 {edge_checksum:016x}"))
        .and_then(|_| writeln!(writer, "c digit_var 1+9*cell+(digit-1)"))
        .and_then(|_| writeln!(writer, "c edge_var 730+edge_id"))
        .and_then(|_| writeln!(writer, "c occupied_var 1274+cell"))
        .and_then(|_| writeln!(writer, "c sequential_var 1355+19*prefix+count"))
        .and_then(|_| writeln!(writer, "c swap_var 2875+544*(digit-1)+edge_id"))
        .and_then(|_| writeln!(writer, "p cnf {VARIABLE_COUNT} {clause_count}"))
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    for clause in &base {
        write_clause(&mut writer, clause)
            .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    }
    for &index in &active.indices {
        write_clause(&mut writer, &pair_clause(checkpoint.cuts[index]))
            .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    }
    writer
        .flush()
        .map_err(|error| format!("cannot finish {}: {error}", output.display()))?;
    Ok((VARIABLE_COUNT as usize, clause_count))
}

fn write_exact_982_lazy_cnf(
    checkpoint: &Checkpoint,
    active: &ActiveCutPool,
    output: &Path,
    symmetry_break: SymmetryBreak,
) -> Result<(usize, usize), String> {
    active.validate(checkpoint.cuts.len())?;
    let edges = directed_edges();
    let edge_checksum = edges_checksum(&edges);
    let topology_scope = TopologyScope::Exact982;
    let base = base_clauses_for_scope(&edges, symmetry_break, topology_scope);
    let clause_count = base.len() + active.indices.len();
    let file = fs::File::create(output)
        .map_err(|error| format!("cannot create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "c {EXACT_982_CNF_SCHEMA}")
        .and_then(|_| writeln!(writer, "c cut_pool_mode lazy-active-v1"))
        .and_then(|_| writeln!(writer, "c topology_scope {}", topology_scope.as_str()))
        .and_then(|_| {
            writeln!(
                writer,
                "c model classic-sudoku plus exactly-three-disjoint-directed-king-paths"
            )
        })
        .and_then(|_| writeln!(writer, "c thermometer_lengths 9 8 2"))
        .and_then(|_| writeln!(writer, "c covered_cells_exactly 19"))
        .and_then(|_| writeln!(writer, "c diagonal_crossings_without_shared_cells allowed"))
        .and_then(|_| writeln!(writer, "c symmetry_break {}", symmetry_break.as_str()))
        .and_then(|_| writeln!(writer, "c checkpoint_budget {}", checkpoint.budget))
        .and_then(|_| writeln!(writer, "c checkpoint_pairs {}", checkpoint.pairs.len()))
        .and_then(|_| writeln!(writer, "c full_unique_pair_cuts {}", checkpoint.cuts.len()))
        .and_then(|_| writeln!(writer, "c active_pair_cuts {}", active.indices.len()))
        .and_then(|_| writeln!(writer, "c checkpoint_fnv1a64 {:016x}", checkpoint.checksum))
        .and_then(|_| {
            writeln!(
                writer,
                "c active_fnv1a64 {:016x}",
                active_cuts_checksum(checkpoint, &active.indices)
                    .expect("active pool validated above")
            )
        })
        .and_then(|_| writeln!(writer, "c digit_variables 1 729"))
        .and_then(|_| writeln!(writer, "c edge_variables 730 1273"))
        .and_then(|_| writeln!(writer, "c occupied_variables 1274 1354"))
        .and_then(|_| writeln!(writer, "c sequential_variables 1355 2874"))
        .and_then(|_| writeln!(writer, "c swap_witness_variables 2875 7226"))
        .and_then(|_| writeln!(writer, "c component_label_variables 7227 7469"))
        .and_then(|_| writeln!(writer, "c path_source_variables 7470 7550"))
        .and_then(|_| writeln!(writer, "c exact_counter_variables 7551 9656"))
        .and_then(|_| {
            writeln!(
                writer,
                "c edge_order lexicographic unordered cell pair then forward and reverse"
            )
        })
        .and_then(|_| writeln!(writer, "c edge_order_fnv1a64 {edge_checksum:016x}"))
        .and_then(|_| writeln!(writer, "c digit_var 1+9*cell+(digit-1)"))
        .and_then(|_| writeln!(writer, "c edge_var 730+edge_id"))
        .and_then(|_| writeln!(writer, "c occupied_var 1274+cell"))
        .and_then(|_| writeln!(writer, "c sequential_var 1355+19*prefix+count"))
        .and_then(|_| writeln!(writer, "c swap_var 2875+544*(digit-1)+edge_id"))
        .and_then(|_| writeln!(writer, "c component_label_var 7227+81*label+cell"))
        .and_then(|_| writeln!(writer, "c path_source_var 7470+cell"))
        .and_then(|_| writeln!(writer, "p cnf {EXACT_982_VARIABLE_COUNT} {clause_count}"))
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    for clause in &base {
        write_clause(&mut writer, clause)
            .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    }
    for &index in &active.indices {
        write_clause(&mut writer, &pair_clause(checkpoint.cuts[index]))
            .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    }
    writer
        .flush()
        .map_err(|error| format!("cannot finish {}: {error}", output.display()))?;
    Ok((EXACT_982_VARIABLE_COUNT as usize, clause_count))
}

fn write_lazy_cnf_for_scope(
    checkpoint: &Checkpoint,
    active: &ActiveCutPool,
    output: &Path,
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
) -> Result<(usize, usize), String> {
    match topology_scope {
        TopologyScope::AtMost19 => write_lazy_cnf(checkpoint, active, output, symmetry_break),
        TopologyScope::Exact982 => {
            write_exact_982_lazy_cnf(checkpoint, active, output, symmetry_break)
        }
    }
}

fn find_once(haystack: &[u8], needle: &[u8], label: &str) -> Result<usize, String> {
    let matches = haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, window)| (window == needle).then_some(index))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err(format!(
            "CNF prefix contains {} occurrences of the expected {label}",
            matches.len()
        ));
    }
    Ok(matches[0])
}

/// Append zero or more newly unique pair clauses and patch the fixed prefix in
/// place. Pair records whose cut duplicates an existing cut update only the
/// checkpoint metadata. When a decimal field crosses a width boundary, fall
/// back to a deterministic full rewrite. In either case the result is
/// byte-for-byte identical to `write_cnf_for_scope(after)`.
#[allow(clippy::too_many_arguments)]
fn append_refinement_to_cnf(
    path: &Path,
    before_pairs: usize,
    before_cuts: usize,
    before_checksum: u64,
    after: &Checkpoint,
    new_cuts: &[PairCut],
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
) -> Result<(), String> {
    if after.pairs.len() <= before_pairs
        || after.cuts.len() != before_cuts + new_cuts.len()
        || after.cuts.get(before_cuts..) != Some(new_cuts)
        || after.checksum != extend_pairs_checksum(before_checksum, &after.pairs[before_pairs..])
    {
        return Err("invalid before/after state for incremental CNF append".into());
    }
    let base_count = topology_scope.base_clause_count(symmetry_break);
    let variable_count = topology_scope.variable_count();
    let old_pairs = format!("c checkpoint_pairs {before_pairs}\n");
    let new_pairs = format!("c checkpoint_pairs {}\n", after.pairs.len());
    let old_cuts = format!("c unique_pair_cuts {before_cuts}\n");
    let new_cuts_header = format!("c unique_pair_cuts {}\n", after.cuts.len());
    let old_checksum = format!("c checkpoint_fnv1a64 {before_checksum:016x}\n");
    let new_checksum = format!("c checkpoint_fnv1a64 {:016x}\n", after.checksum);
    let old_header = format!("p cnf {variable_count} {}\n", base_count + before_cuts);
    let new_header = format!("p cnf {variable_count} {}\n", base_count + after.cuts.len());
    if old_pairs.len() != new_pairs.len()
        || old_cuts.len() != new_cuts_header.len()
        || old_header.len() != new_header.len()
    {
        write_cnf_for_scope(after, path, symmetry_break, topology_scope)?;
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("cannot open {} for append: {error}", path.display()))?;
    let mut prefix = vec![0u8; 4096];
    let read = file
        .read(&mut prefix)
        .map_err(|error| format!("cannot read CNF prefix {}: {error}", path.display()))?;
    prefix.truncate(read);
    let pair_offset = find_once(&prefix, old_pairs.as_bytes(), "pair count")?;
    let cut_offset = find_once(&prefix, old_cuts.as_bytes(), "unique pair-cut count")?;
    let checksum_offset = find_once(&prefix, old_checksum.as_bytes(), "checkpoint checksum")?;
    let header_offset = find_once(&prefix, old_header.as_bytes(), "DIMACS header")?;

    file.seek(SeekFrom::End(0))
        .map_err(|error| format!("cannot seek {} for append: {error}", path.display()))?;
    {
        let mut append = BufWriter::with_capacity(1 << 20, &mut file);
        for &cut in new_cuts {
            write_clause(&mut append, &pair_clause(cut)).map_err(|error| {
                format!("cannot append pair clause to {}: {error}", path.display())
            })?;
        }
        append
            .flush()
            .map_err(|error| format!("cannot flush CNF append {}: {error}", path.display()))?;
    }
    for (offset, replacement) in [
        (pair_offset, new_pairs.as_bytes()),
        (cut_offset, new_cuts_header.as_bytes()),
        (checksum_offset, new_checksum.as_bytes()),
        (header_offset, new_header.as_bytes()),
    ] {
        file.seek(SeekFrom::Start(offset as u64))
            .and_then(|_| file.write_all(replacement))
            .map_err(|error| format!("cannot patch CNF prefix {}: {error}", path.display()))?;
    }
    // The CNF is a disposable cache reconstructed from the checksummed
    // checkpoint at every startup. Only the authoritative atomic checkpoint
    // needs a durable sync.
    file.flush()
        .map_err(|error| format!("cannot finish CNF append {}: {error}", path.display()))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_pair_to_cnf(
    path: &Path,
    before_pairs: usize,
    before_cuts: usize,
    before_checksum: u64,
    after: &Checkpoint,
    pair: &GridPair,
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
) -> Result<(), String> {
    if after.pairs.last() != Some(pair) || after.cuts.len() != before_cuts + 1 {
        return Err("invalid single-pair CNF append".into());
    }
    append_refinement_to_cnf(
        path,
        before_pairs,
        before_cuts,
        before_checksum,
        after,
        &after.cuts[before_cuts..],
        symmetry_break,
        topology_scope,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PairMode {
    Anchor,
    All,
}

impl PairMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Anchor => "anchor",
            Self::All => "all",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TopologyScope {
    AtMost19,
    Exact982,
}

impl TopologyScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::AtMost19 => "at-most-19",
            Self::Exact982 => "exact-9+8+2",
        }
    }

    fn cnf_schema(self) -> &'static str {
        match self {
            Self::AtMost19 => CNF_SCHEMA,
            Self::Exact982 => EXACT_982_CNF_SCHEMA,
        }
    }

    fn search_scope(self) -> &'static str {
        match self {
            Self::AtMost19 => {
                "classic-sudoku-plus-nonoverlapping-king-step-thermometers-at-most-19-covered-cells"
            }
            Self::Exact982 => {
                "classic-sudoku-plus-nonoverlapping-king-step-thermometers-exact-9+8+2"
            }
        }
    }

    fn variable_count(self) -> i32 {
        match self {
            Self::AtMost19 => VARIABLE_COUNT,
            Self::Exact982 => EXACT_982_VARIABLE_COUNT,
        }
    }

    fn base_clause_count(self, symmetry_break: SymmetryBreak) -> usize {
        BASE_CLAUSE_COUNT
            + match self {
                Self::AtMost19 => 0,
                Self::Exact982 => EXACT_982_EXTRA_CLAUSE_COUNT,
            }
            + symmetry_break.extra_clauses()
    }

    fn validates_candidate(self, candidate: &DecodedCandidate) -> Result<(), String> {
        if self == Self::AtMost19 {
            return Ok(());
        }
        let mut lengths = candidate.paths.iter().map(Vec::len).collect::<Vec<_>>();
        lengths.sort_unstable();
        if candidate.covered_cells != 19 || candidate.selected.len() != 16 || lengths != [2, 8, 9] {
            return Err(format!(
                "decoded topology is outside exact 9+8+2 scope: covered={} comparisons={} lengths={lengths:?}",
                candidate.covered_cells,
                candidate.selected.len()
            ));
        }
        Ok(())
    }
}

fn maximum_refinement_pairs(
    max_iterations: usize,
    oracle_batch: usize,
    pair_mode: PairMode,
) -> Result<usize, String> {
    let per_iteration = match pair_mode {
        PairMode::Anchor => oracle_batch,
        PairMode::All => {
            oracle_batch
                .checked_mul(oracle_batch + 1)
                .ok_or_else(|| "oracle pair-batch capacity overflow".to_string())?
                / 2
        }
    };
    max_iterations
        .checked_mul(per_iteration)
        .ok_or_else(|| "configured refinement capacity overflow".to_string())
}

fn eager_refinement_reserve(
    max_iterations: usize,
    oracle_batch: usize,
    pair_mode: PairMode,
) -> Result<usize, String> {
    Ok(
        maximum_refinement_pairs(max_iterations, oracle_batch, pair_mode)?
            .min(MAX_EAGER_REFINEMENT_RESERVE),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SymmetryBreak {
    None,
    D4ComplementV1,
}

impl SymmetryBreak {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::D4ComplementV1 => "d4-complement-v1",
        }
    }

    fn extra_clauses(self) -> usize {
        match self {
            Self::None => 0,
            Self::D4ComplementV1 => 148,
        }
    }
}

#[derive(Debug)]
enum Mode {
    Stats {
        checkpoint: PathBuf,
    },
    MergeCheckpoints {
        checkpoint: PathBuf,
        merge_checkpoints: Vec<PathBuf>,
        output: PathBuf,
    },
    Emit {
        checkpoint: PathBuf,
        output: PathBuf,
        symmetry_break: SymmetryBreak,
        topology_scope: TopologyScope,
    },
    EmitActive {
        checkpoint: PathBuf,
        active_cuts: PathBuf,
        output: PathBuf,
        symmetry_break: SymmetryBreak,
        topology_scope: TopologyScope,
    },
    Decode {
        checkpoint: PathBuf,
        model: PathBuf,
        output: Option<PathBuf>,
        symmetry_break: SymmetryBreak,
        topology_scope: TopologyScope,
    },
    Loop {
        checkpoint: PathBuf,
        next_checkpoint: PathBuf,
        sat_exe: PathBuf,
        cnf: PathBuf,
        model: PathBuf,
        proof: Option<PathBuf>,
        max_iterations: usize,
        conflicts: Option<u64>,
        symmetry_break: SymmetryBreak,
        topology_scope: TopologyScope,
    },
    IncrementalLoop {
        checkpoint: PathBuf,
        next_checkpoint: PathBuf,
        bridge_exe: PathBuf,
        cnf: PathBuf,
        max_iterations: usize,
        conflicts: Option<u64>,
        oracle_batch: usize,
        pair_mode: PairMode,
        prefer_selected: bool,
        checkpoint_every: usize,
        symmetry_break: SymmetryBreak,
        topology_scope: TopologyScope,
        lazy_cuts: Option<LazyCutOptions>,
    },
}

fn print_help() {
    println!(
        "thermo-topology-cnf [emit] --checkpoint PATH --output CNF\n\
             [--symmetry-break none|d4-complement-v1]\n\
             [--topology-scope at-most-19|exact-9+8+2]\n\
         thermo-topology-cnf emit-active --checkpoint PATH --active-cuts PATH\n\
             --output CNF [--symmetry-break none|d4-complement-v1]\n\
             [--topology-scope at-most-19|exact-9+8+2]\n\
         thermo-topology-cnf stats --checkpoint PATH\n\
         thermo-topology-cnf merge-checkpoints --checkpoint BASE\n\
             --merge-checkpoint PATH [--merge-checkpoint PATH ...] --output PATH\n\
         thermo-topology-cnf decode --checkpoint PATH --model MODEL [--output FILE]\n\
             [--symmetry-break none|d4-complement-v1]\n\
             [--topology-scope at-most-19|exact-9+8+2]\n\
         thermo-topology-cnf loop --checkpoint PATH --next-checkpoint PATH\n\
             --sat-exe PATH --cnf PATH [--model PATH] [--proof PATH]\n\
             [--max-iterations N] [--conflicts N]\n\
             [--symmetry-break none|d4-complement-v1]\n\
             [--topology-scope at-most-19|exact-9+8+2]\n\
         thermo-topology-cnf incremental-loop --checkpoint PATH\n\
             --next-checkpoint PATH --bridge-exe PATH --cnf PATH\n\
             [--max-iterations N] [--conflicts N] [--oracle-batch N]\n\
             [--pair-mode all|anchor] [--prefer-selected]\n\
             [--checkpoint-every N] [--symmetry-break none|d4-complement-v1]\n\
             [--topology-scope at-most-19|exact-9+8+2]\n\
             [--lazy-cuts ACTIVE-MANIFEST] [--lazy-active-seed N]\n\
             [--lazy-violation-batch N|all]\n\
         \n\
         `stats` reports exact pair-clause deduplication without writing a CNF.\n\
         `merge-checkpoints` appends distinct pairs from each validated v1 input\n\
         in command-line order, preserving the base checkpoint as an exact prefix.\n\
         `emit` writes the deterministic full topology master. `emit-active`\n\
         validates a lazy active-cut manifest and regenerates its exact small\n\
         static CNF. `decode` validates a\n\
         complete SAT competition-format model against that exact master and\n\
         emits its target and directed paths. `loop` runs a CaDiCaL-compatible\n\
         executable, validates every model, asks the exact thermo solver for\n\
         0/1/2+ solutions, and persists one new pair cut per non-unique\n\
         iteration. incremental-loop retains one CaDiCaL library session,\n\
         enumerates a batch of exact alternatives, and adds every new cut\n\
         monotonically. Resource-limit/UNKNOWN results are inconclusive."
    );
    println!(
        "\nSAT sidecar contract:\n\
         - the executable must accept CaDiCaL's `-q -w MODEL [-c N] CNF [PROOF]`;\n\
         - exit 10 means SAT, exit 20 UNSAT, and exit 0 UNKNOWN;\n\
         - SAT output must assign every variable declared by the CNF exactly\n\
           once (7226 by default, 9656 for exact-9+8+2) in\n\
           competition `s`/`v` format; partial, conflicting, out-of-range, or\n\
           clause-violating models are rejected;\n\
         - `--proof` is useful only when the final status is UNSAT. On SAT or\n\
           UNKNOWN it is not a negative certificate;\n\
         - each iteration starts a fresh SAT process. The CNF itself is kept\n\
         current by an exact in-place pair-clause append, but learned SAT\n\
         state is not retained across iterations.\n\
         \n\
         Persistent bridge contract:\n\
         - build tools/cadical-incremental-bridge.cpp against CaDiCaL;\n\
         - the strict v1 protocol returns every declared val result immediately\n\
           after SAT, before any subsequent clause addition;\n\
         - --conflicts is reset for each solve; values above INT_MAX are\n\
           rejected;\n\
         - incremental UNSAT is provisional, not proof-certified. Regenerate\n\
           the final static CNF and run a proof-producing solver before making\n\
           an exclusion claim;\n\
         - --prefer-selected changes phase hints only, not the formula;\n\
         - --lazy-cuts scans the complete validated pool after every SAT\n\
           model and invokes the thermo oracle only after zero pool cuts are\n\
           violated; its manifest binds active pool IDs to grid witnesses;\n\
         - every normal lazy-mode exit rewrites --cnf as the exact static\n\
           base-plus-active formula suitable for a fresh proof rerun;\n\
         - --symmetry-break d4-complement-v1 adds the versioned optional\n\
         D4-times-complement representative constraints and is off by\n\
         default;\n\
         - --topology-scope exact-9+8+2 restricts the master to three\n\
         cell-disjoint paths of those exact lengths. The default at-most-19\n\
         formula and its DIMACS bytes are unchanged."
    );
}

fn require_value(option: &str, index: &mut usize, arguments: &[String]) -> Result<String, String> {
    *index += 1;
    arguments
        .get(*index)
        .cloned()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_usize(option: &str, value: String) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("invalid value for {option}: {value:?}"))
}

fn parse_u64(option: &str, value: String) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid value for {option}: {value:?}"))
}

fn resolved_destination(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return fs::canonicalize(path)
            .map_err(|error| format!("cannot resolve {}: {error}", path.display()));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent)
        .map_err(|error| format!("cannot resolve parent of {}: {error}", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("path {} has no file name", path.display()))?;
    Ok(parent.join(name))
}

fn paths_equal(left: &Path, right: &Path) -> Result<bool, String> {
    let left = resolved_destination(left)?;
    let right = resolved_destination(right)?;
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

fn reject_collisions(paths: &[(&str, &Path)]) -> Result<(), String> {
    for left in 0..paths.len() {
        for right in left + 1..paths.len() {
            if paths_equal(paths[left].1, paths[right].1)? {
                return Err(format!(
                    "{} and {} must name different files",
                    paths[left].0, paths[right].0
                ));
            }
        }
    }
    Ok(())
}

struct RunLock {
    _file: fs::File,
    path: PathBuf,
}

impl RunLock {
    fn acquire(target: &Path, label: &str) -> Result<Self, String> {
        let target = resolved_destination(target)?;
        let file_name = target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{label} path {} has no UTF-8 file name", target.display()))?;
        let path = target.with_file_name(format!(".{file_name}.writer.lock"));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                format!(
                    "cannot open {label} writer lock {}: {error}",
                    path.display()
                )
            })?;
        file.try_lock().map_err(|error| {
            format!(
                "another process holds the {label} writer lock {}: {error}",
                path.display()
            )
        })?;
        file.set_len(0)
            .and_then(|_| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .and_then(|_| {
                writeln!(
                    file,
                    "pid={} target={}",
                    std::process::id(),
                    target.display()
                )
            })
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_data())
            .map_err(|error| {
                format!(
                    "cannot record {label} writer lock {}: {error}",
                    path.display()
                )
            })?;
        Ok(Self { _file: file, path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

fn parse_options() -> Result<Mode, String> {
    let mut arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
    {
        print_help();
        std::process::exit(0);
    }
    let command = if arguments
        .first()
        .is_some_and(|value| !value.starts_with('-'))
    {
        arguments.remove(0)
    } else {
        "emit".to_string()
    };
    let mut checkpoint = None;
    let mut merge_checkpoints = Vec::new();
    let mut output = None;
    let mut model = None;
    let mut next_checkpoint = None;
    let mut sat_exe = None;
    let mut bridge_exe = None;
    let mut cnf = None;
    let mut proof = None;
    let mut active_cuts = None;
    let mut lazy_cuts = None;
    let mut max_iterations = 1usize;
    let mut conflicts = None;
    let mut oracle_batch = DEFAULT_ORACLE_BATCH;
    let mut oracle_batch_set = false;
    let mut pair_mode = PairMode::All;
    let mut pair_mode_set = false;
    let mut prefer_selected = false;
    let mut checkpoint_every = 1usize;
    let mut checkpoint_every_set = false;
    let mut lazy_active_seed = DEFAULT_LAZY_ACTIVE_SEED;
    let mut lazy_active_seed_set = false;
    let mut lazy_violation_batch = Some(DEFAULT_LAZY_VIOLATION_BATCH);
    let mut lazy_violation_batch_set = false;
    let mut symmetry_break = SymmetryBreak::None;
    let mut symmetry_break_set = false;
    let mut topology_scope = TopologyScope::AtMost19;
    let mut topology_scope_set = false;
    let mut index = 0usize;
    while index < arguments.len() {
        let argument = &arguments[index];
        match argument.as_str() {
            "--checkpoint" => {
                checkpoint = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--merge-checkpoint" => {
                merge_checkpoints.push(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--output" => {
                output = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--model" => {
                model = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--next-checkpoint" => {
                next_checkpoint = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--sat-exe" => {
                sat_exe = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--bridge-exe" => {
                bridge_exe = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--cnf" => {
                cnf = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--proof" => {
                proof = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--active-cuts" => {
                active_cuts = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--lazy-cuts" => {
                lazy_cuts = Some(PathBuf::from(require_value(
                    argument, &mut index, &arguments,
                )?));
            }
            "--lazy-active-seed" => {
                lazy_active_seed_set = true;
                lazy_active_seed =
                    parse_usize(argument, require_value(argument, &mut index, &arguments)?)?;
            }
            "--lazy-violation-batch" => {
                lazy_violation_batch_set = true;
                let value = require_value(argument, &mut index, &arguments)?;
                lazy_violation_batch = if value == "all" {
                    None
                } else {
                    let parsed = parse_usize(argument, value)?;
                    if parsed == 0 {
                        return Err("--lazy-violation-batch must be positive or all".into());
                    }
                    Some(parsed)
                };
            }
            "--max-iterations" => {
                max_iterations =
                    parse_usize(argument, require_value(argument, &mut index, &arguments)?)?;
                if max_iterations == 0 {
                    return Err("--max-iterations must be positive".into());
                }
            }
            "--conflicts" => {
                conflicts = Some(parse_u64(
                    argument,
                    require_value(argument, &mut index, &arguments)?,
                )?);
            }
            "--oracle-batch" => {
                oracle_batch_set = true;
                oracle_batch =
                    parse_usize(argument, require_value(argument, &mut index, &arguments)?)?;
                if !(1..=MAX_ORACLE_BATCH).contains(&oracle_batch) {
                    return Err(format!(
                        "--oracle-batch must be between 1 and {MAX_ORACLE_BATCH}"
                    ));
                }
            }
            "--pair-mode" => {
                pair_mode_set = true;
                pair_mode = match require_value(argument, &mut index, &arguments)?.as_str() {
                    "anchor" => PairMode::Anchor,
                    "all" => PairMode::All,
                    value => {
                        return Err(format!(
                            "invalid --pair-mode {value:?}; expected anchor or all"
                        ));
                    }
                };
            }
            "--prefer-selected" => prefer_selected = true,
            "--checkpoint-every" => {
                checkpoint_every_set = true;
                checkpoint_every =
                    parse_usize(argument, require_value(argument, &mut index, &arguments)?)?;
                if checkpoint_every == 0 {
                    return Err("--checkpoint-every must be positive".into());
                }
            }
            "--symmetry-break" => {
                symmetry_break_set = true;
                symmetry_break = match require_value(argument, &mut index, &arguments)?.as_str() {
                    "none" => SymmetryBreak::None,
                    "d4-complement-v1" => SymmetryBreak::D4ComplementV1,
                    value => {
                        return Err(format!(
                            "invalid --symmetry-break {value:?}; expected none or d4-complement-v1"
                        ));
                    }
                };
            }
            "--topology-scope" => {
                topology_scope_set = true;
                topology_scope = match require_value(argument, &mut index, &arguments)?.as_str() {
                    "at-most-19" => TopologyScope::AtMost19,
                    "exact-9+8+2" => TopologyScope::Exact982,
                    value => {
                        return Err(format!(
                            "invalid --topology-scope {value:?}; expected at-most-19 or exact-9+8+2"
                        ));
                    }
                };
            }
            _ => return Err(format!("unknown option {argument:?}; use --help")),
        }
        index += 1;
    }

    let checkpoint = checkpoint.ok_or_else(|| "--checkpoint is required".to_string())?;
    let has_incremental_only_options = bridge_exe.is_some()
        || oracle_batch_set
        || pair_mode_set
        || prefer_selected
        || checkpoint_every_set
        || lazy_cuts.is_some()
        || lazy_active_seed_set
        || lazy_violation_batch_set;
    let has_merge_options = !merge_checkpoints.is_empty();
    match command.as_str() {
        "stats" => {
            if output.is_some()
                || model.is_some()
                || next_checkpoint.is_some()
                || sat_exe.is_some()
                || cnf.is_some()
                || proof.is_some()
                || conflicts.is_some()
                || max_iterations != 1
                || has_incremental_only_options
                || active_cuts.is_some()
                || symmetry_break_set
                || topology_scope_set
                || has_merge_options
            {
                return Err("stats accepts only --checkpoint".into());
            }
            Ok(Mode::Stats { checkpoint })
        }
        "merge-checkpoints" => {
            if model.is_some()
                || next_checkpoint.is_some()
                || sat_exe.is_some()
                || bridge_exe.is_some()
                || cnf.is_some()
                || proof.is_some()
                || conflicts.is_some()
                || max_iterations != 1
                || has_incremental_only_options
                || active_cuts.is_some()
                || symmetry_break_set
                || topology_scope_set
            {
                return Err(
                    "merge-checkpoints accepts only --checkpoint, --merge-checkpoint, and --output"
                        .into(),
                );
            }
            if merge_checkpoints.is_empty() {
                return Err("merge-checkpoints requires at least one --merge-checkpoint".into());
            }
            let output =
                output.ok_or_else(|| "--output is required for merge-checkpoints".to_string())?;
            let mut paths = vec![
                ("checkpoint", checkpoint.as_path()),
                ("output", output.as_path()),
            ];
            for path in &merge_checkpoints {
                paths.push(("merge-checkpoint", path.as_path()));
            }
            reject_collisions(&paths)?;
            Ok(Mode::MergeCheckpoints {
                checkpoint,
                merge_checkpoints,
                output,
            })
        }
        "emit" => {
            if model.is_some()
                || next_checkpoint.is_some()
                || sat_exe.is_some()
                || cnf.is_some()
                || proof.is_some()
                || conflicts.is_some()
                || max_iterations != 1
                || has_incremental_only_options
                || active_cuts.is_some()
                || has_merge_options
            {
                return Err(
                    "emit accepts only --checkpoint, --output, and --symmetry-break".into(),
                );
            }
            let output = output.ok_or_else(|| "--output is required for emit".to_string())?;
            reject_collisions(&[("checkpoint", &checkpoint), ("output", &output)])?;
            Ok(Mode::Emit {
                checkpoint,
                output,
                symmetry_break,
                topology_scope,
            })
        }
        "emit-active" => {
            if model.is_some()
                || next_checkpoint.is_some()
                || sat_exe.is_some()
                || bridge_exe.is_some()
                || cnf.is_some()
                || proof.is_some()
                || conflicts.is_some()
                || max_iterations != 1
                || oracle_batch_set
                || pair_mode_set
                || prefer_selected
                || checkpoint_every_set
                || lazy_cuts.is_some()
                || lazy_active_seed_set
                || lazy_violation_batch_set
                || has_merge_options
            {
                return Err(
                    "emit-active accepts only --checkpoint, --active-cuts, --output, and --symmetry-break"
                        .into(),
                );
            }
            let active_cuts = active_cuts
                .ok_or_else(|| "--active-cuts is required for emit-active".to_string())?;
            let output =
                output.ok_or_else(|| "--output is required for emit-active".to_string())?;
            reject_collisions(&[
                ("checkpoint", checkpoint.as_path()),
                ("active-cuts", active_cuts.as_path()),
                ("output", output.as_path()),
            ])?;
            Ok(Mode::EmitActive {
                checkpoint,
                active_cuts,
                output,
                symmetry_break,
                topology_scope,
            })
        }
        "decode" => {
            if next_checkpoint.is_some()
                || sat_exe.is_some()
                || cnf.is_some()
                || proof.is_some()
                || conflicts.is_some()
                || max_iterations != 1
                || has_incremental_only_options
                || active_cuts.is_some()
                || has_merge_options
            {
                return Err(
                    "decode accepts only --checkpoint, --model, --output, and --symmetry-break"
                        .into(),
                );
            }
            let model = model.ok_or_else(|| "--model is required for decode".to_string())?;
            let mut paths = vec![
                ("checkpoint", checkpoint.as_path()),
                ("model", model.as_path()),
            ];
            if let Some(output) = &output {
                paths.push(("output", output.as_path()));
            }
            reject_collisions(&paths)?;
            Ok(Mode::Decode {
                checkpoint,
                model,
                output,
                symmetry_break,
                topology_scope,
            })
        }
        "loop" => {
            if output.is_some()
                || has_incremental_only_options
                || active_cuts.is_some()
                || has_merge_options
            {
                return Err("loop does not accept output or incremental-loop options".into());
            }
            let next_checkpoint = next_checkpoint
                .ok_or_else(|| "--next-checkpoint is required for loop".to_string())?;
            let sat_exe = sat_exe.ok_or_else(|| "--sat-exe is required for loop".to_string())?;
            let cnf = cnf.ok_or_else(|| "--cnf is required for loop".to_string())?;
            let model = model.unwrap_or_else(|| cnf.with_extension("model"));
            let mut paths = vec![
                ("checkpoint", checkpoint.as_path()),
                ("sat-exe", sat_exe.as_path()),
                ("next-checkpoint", next_checkpoint.as_path()),
                ("cnf", cnf.as_path()),
                ("model", model.as_path()),
            ];
            if let Some(proof) = &proof {
                paths.push(("proof", proof.as_path()));
            }
            reject_collisions(&paths)?;
            Ok(Mode::Loop {
                checkpoint,
                next_checkpoint,
                sat_exe,
                cnf,
                model,
                proof,
                max_iterations,
                conflicts,
                symmetry_break,
                topology_scope,
            })
        }
        "incremental-loop" => {
            if output.is_some()
                || model.is_some()
                || sat_exe.is_some()
                || proof.is_some()
                || active_cuts.is_some()
                || has_merge_options
            {
                return Err(
                    "incremental-loop does not accept --output, --model, --sat-exe, or --proof"
                        .into(),
                );
            }
            if conflicts.is_some_and(|value| value > i32::MAX as u64) {
                return Err(format!(
                    "--conflicts must be at most {} for incremental-loop",
                    i32::MAX
                ));
            }
            let next_checkpoint = next_checkpoint
                .ok_or_else(|| "--next-checkpoint is required for incremental-loop".to_string())?;
            let bridge_exe = bridge_exe
                .ok_or_else(|| "--bridge-exe is required for incremental-loop".to_string())?;
            let cnf = cnf.ok_or_else(|| "--cnf is required for incremental-loop".to_string())?;
            if lazy_cuts.is_none() && (lazy_active_seed_set || lazy_violation_batch_set) {
                return Err(
                    "--lazy-active-seed and --lazy-violation-batch require --lazy-cuts".into(),
                );
            }
            let lazy_cuts = lazy_cuts.map(|manifest| LazyCutOptions {
                manifest,
                active_seed: lazy_active_seed,
                violation_batch: lazy_violation_batch,
            });
            let mut paths = vec![
                ("checkpoint", checkpoint.as_path()),
                ("bridge-exe", bridge_exe.as_path()),
                ("next-checkpoint", next_checkpoint.as_path()),
                ("cnf", cnf.as_path()),
            ];
            if let Some(lazy) = &lazy_cuts {
                paths.push(("lazy-cuts", lazy.manifest.as_path()));
            }
            reject_collisions(&paths)?;
            Ok(Mode::IncrementalLoop {
                checkpoint,
                next_checkpoint,
                bridge_exe,
                cnf,
                max_iterations,
                conflicts,
                oracle_batch,
                pair_mode,
                prefer_selected,
                checkpoint_every,
                symmetry_break,
                topology_scope,
                lazy_cuts,
            })
        }
        other => Err(format!(
            "unknown command {other:?}; expected stats, merge-checkpoints, emit, emit-active, decode, loop, or incremental-loop"
        )),
    }
}

fn run_emit(
    checkpoint_path: &Path,
    output: &Path,
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
) -> Result<(), String> {
    let checkpoint = load_checkpoint(checkpoint_path)?;
    let (variables, clauses) =
        write_cnf_for_scope(&checkpoint, output, symmetry_break, topology_scope)?;
    println!(
        "wrote {}: variables={variables} clauses={clauses} checkpoint_pairs={} unique_pair_cuts={} checkpoint_fnv1a64={:016x} symmetry_break={} topology_scope={}",
        output.display(),
        checkpoint.pairs.len(),
        checkpoint.cuts.len(),
        checkpoint.checksum,
        symmetry_break.as_str(),
        topology_scope.as_str()
    );
    Ok(())
}

fn run_emit_active(
    checkpoint_path: &Path,
    active_cuts_path: &Path,
    output: &Path,
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
) -> Result<(), String> {
    let checkpoint = load_checkpoint(checkpoint_path)?;
    let edges = directed_edges();
    let active = load_active_cuts_manifest(
        active_cuts_path,
        &checkpoint,
        &edges,
        symmetry_break,
        topology_scope,
    )?;
    let (variables, clauses) =
        write_lazy_cnf_for_scope(&checkpoint, &active, output, symmetry_break, topology_scope)?;
    println!(
        "wrote {}: variables={variables} clauses={clauses} checkpoint_pairs={} full_unique_pair_cuts={} active_pair_cuts={} checkpoint_fnv1a64={:016x} active_fnv1a64={:016x} symmetry_break={} topology_scope={}",
        output.display(),
        checkpoint.pairs.len(),
        checkpoint.cuts.len(),
        active.indices.len(),
        checkpoint.checksum,
        active_cuts_checksum(&checkpoint, &active.indices)?,
        symmetry_break.as_str(),
        topology_scope.as_str()
    );
    Ok(())
}

fn run_stats(checkpoint_path: &Path) -> Result<(), String> {
    let checkpoint = load_checkpoint(checkpoint_path)?;
    println!(
        "checkpoint_pairs={}\nunique_pair_cuts={}\nduplicate_pair_clauses={}\ncheckpoint_fnv1a64={:016x}",
        checkpoint.pairs.len(),
        checkpoint.cuts.len(),
        checkpoint.pairs.len() - checkpoint.cuts.len(),
        checkpoint.checksum
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CheckpointMergeStats {
    input_pairs: usize,
    added_pairs: usize,
    duplicate_pairs: usize,
    added_cuts: usize,
}

fn merge_checkpoint_data(
    destination: &mut Checkpoint,
    source: &Checkpoint,
    edges: &[DirectedEdge],
) -> Result<CheckpointMergeStats, String> {
    if destination.budget != source.budget {
        return Err(format!(
            "cannot merge checkpoint budget {} into budget {}",
            source.budget, destination.budget
        ));
    }
    destination.reserve_for_append(source.pairs.len(), source.cuts.len())?;
    let mut stats = CheckpointMergeStats {
        input_pairs: source.pairs.len(),
        ..CheckpointMergeStats::default()
    };
    for &pair in &source.pairs {
        let witness = destination.pairs.len();
        if !destination.insert_pair(pair)? {
            stats.duplicate_pairs += 1;
            continue;
        }
        stats.added_pairs += 1;
        stats.added_cuts += usize::from(destination.insert_cut(pair_cut(&pair, edges), witness)?);
    }
    destination.checksum = pairs_checksum(&destination.pairs);
    Ok(stats)
}

fn run_merge_checkpoints(
    checkpoint_path: &Path,
    merge_paths: &[PathBuf],
    output: &Path,
) -> Result<(), String> {
    let output_lock = RunLock::acquire(output, "merged checkpoint")?;
    let mut checkpoint = load_checkpoint(checkpoint_path)?;
    let base_pairs = checkpoint.pairs.len();
    let base_cuts = checkpoint.cuts.len();
    let edges = directed_edges();
    let mut total_added_pairs = 0usize;
    let mut total_duplicate_pairs = 0usize;
    let mut total_added_cuts = 0usize;
    for (index, path) in merge_paths.iter().enumerate() {
        let source = load_checkpoint(path)?;
        let stats = merge_checkpoint_data(&mut checkpoint, &source, &edges)?;
        total_added_pairs += stats.added_pairs;
        total_duplicate_pairs += stats.duplicate_pairs;
        total_added_cuts += stats.added_cuts;
        println!(
            "merge_input={} path={} input_pairs={} added_pairs={} duplicate_pairs={} added_unique_cuts={}",
            index + 1,
            path.display(),
            stats.input_pairs,
            stats.added_pairs,
            stats.duplicate_pairs,
            stats.added_cuts
        );
    }
    write_checkpoint(&checkpoint, output)?;
    println!(
        "status=checkpoints-merged\nbase_checkpoint={}\nbase_pairs={base_pairs}\nbase_unique_cuts={base_cuts}\nmerge_inputs={}\nadded_pairs={total_added_pairs}\nduplicate_pairs={total_duplicate_pairs}\nadded_unique_cuts={total_added_cuts}\noutput={}\noutput_pairs={}\noutput_unique_cuts={}\noutput_fnv1a64={:016x}\nbase_preserved_as_exact_prefix=true\nfirst_cut_witness_semantics=first-pair-occurrence-v1\nwriter_lock={}",
        checkpoint_path.display(),
        merge_paths.len(),
        output.display(),
        checkpoint.pairs.len(),
        checkpoint.cuts.len(),
        checkpoint.checksum,
        output_lock.path().display()
    );
    Ok(())
}

fn run_decode(
    checkpoint_path: &Path,
    model: &Path,
    output: Option<&Path>,
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
) -> Result<(), String> {
    let checkpoint = load_checkpoint(checkpoint_path)?;
    let text = fs::read_to_string(model)
        .map_err(|error| format!("cannot read model {}: {error}", model.display()))?;
    let result = parse_sat_result(&text, topology_scope.variable_count())?;
    let rendered = match result.status {
        SatStatus::Satisfiable => {
            let edges = directed_edges();
            let base = base_clauses_for_scope(&edges, symmetry_break, topology_scope);
            let candidate = decode_candidate_with_scope_and_base(
                &checkpoint.cuts,
                result.assignment.as_deref().expect("SAT has assignment"),
                &edges,
                &base,
                topology_scope,
            )?;
            format!(
                "{}symmetry_break={}\ntopology_scope={}\n",
                format_candidate(&candidate, &edges),
                symmetry_break.as_str(),
                topology_scope.as_str()
            )
        }
        SatStatus::Unsatisfiable => {
            format!(
                "status=unsat\nsymmetry_break={}\ntopology_scope={}\n",
                symmetry_break.as_str(),
                topology_scope.as_str()
            )
        }
        SatStatus::Unknown => format!(
            "status=unknown\nsymmetry_break={}\ntopology_scope={}\n",
            symmetry_break.as_str(),
            topology_scope.as_str()
        ),
    };
    if let Some(output) = output {
        fs::write(output, rendered)
            .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    } else {
        print!("{rendered}");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_loop(
    checkpoint_path: &Path,
    next_checkpoint: &Path,
    sat_exe: &Path,
    cnf: &Path,
    model: &Path,
    proof: Option<&Path>,
    max_iterations: usize,
    conflicts: Option<u64>,
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
) -> Result<(), String> {
    let mut checkpoint = load_checkpoint_with_reserve(checkpoint_path, max_iterations)?;
    let edges = directed_edges();
    let base = base_clauses_for_scope(&edges, symmetry_break, topology_scope);
    let (_, mut clauses) = write_cnf_for_scope(&checkpoint, cnf, symmetry_break, topology_scope)?;
    for iteration in 0..max_iterations {
        eprintln!(
            "topology-loop iteration={iteration} pairs={} unique_cuts={} clauses={clauses} symmetry_break={} topology_scope={}",
            checkpoint.pairs.len(),
            checkpoint.cuts.len(),
            symmetry_break.as_str(),
            topology_scope.as_str()
        );
        let sat = invoke_sat(
            sat_exe,
            cnf,
            model,
            proof,
            conflicts,
            topology_scope.variable_count(),
        )?;
        if sat.status != SatStatus::Unsatisfiable
            && let Some(proof) = proof
            && proof.exists()
        {
            fs::remove_file(proof).map_err(|error| {
                format!(
                    "cannot remove non-certificate proof {}: {error}",
                    proof.display()
                )
            })?;
        }
        match sat.status {
            SatStatus::Unknown => {
                write_checkpoint(&checkpoint, next_checkpoint)?;
                println!(
                    "status=inconclusive-sat-limit\nproof_certified=false\nglobal_19c_conclusion=false\niterations={}\npairs={}\ncheckpoint={}\nsymmetry_break={}\ntopology_scope={}\n",
                    iteration + 1,
                    checkpoint.pairs.len(),
                    next_checkpoint.display(),
                    symmetry_break.as_str(),
                    topology_scope.as_str()
                );
                println!("checkpoint_fnv1a64={:016x}", checkpoint.checksum);
                return Ok(());
            }
            SatStatus::Unsatisfiable => {
                write_checkpoint(&checkpoint, next_checkpoint)?;
                let proof_artifact_present = proof.is_some_and(Path::exists);
                let next_action = if proof_artifact_present {
                    "independently-verify-static-proof"
                } else {
                    "produce-and-independently-verify-static-proof"
                };
                println!(
                    "status=static-topology-unsat-provisional\nproof_certified=false\nglobal_19c_conclusion=false\nnext_action={}\niterations={}\npairs={}\ncheckpoint={}\nproof={}\nproof_artifact_present={}\nsymmetry_break={}\ntopology_scope={}\nnegative_exclusion_requires_symmetry_orbit_lemma={}\n",
                    next_action,
                    iteration + 1,
                    checkpoint.pairs.len(),
                    next_checkpoint.display(),
                    proof.map_or_else(|| "none".to_string(), |path| path.display().to_string()),
                    proof_artifact_present,
                    symmetry_break.as_str(),
                    topology_scope.as_str(),
                    symmetry_break != SymmetryBreak::None
                );
                println!("checkpoint_fnv1a64={:016x}", checkpoint.checksum);
                return Ok(());
            }
            SatStatus::Satisfiable => {}
        }

        let candidate = decode_candidate_with_scope_and_base(
            &checkpoint.cuts,
            sat.assignment.as_deref().expect("SAT has assignment"),
            &edges,
            &base,
            topology_scope,
        )?;
        eprintln!(
            "topology-loop iteration={iteration} selected={} covered={} thermometers={}",
            candidate.selected.len(),
            candidate.covered_cells,
            candidate.paths.len()
        );
        let solve = Solver::blank(&candidate.paths)
            .map_err(|error| format!("cannot build thermo oracle: {error}"))?
            .count_up_to(2);
        match solve.multiplicity() {
            Multiplicity::Zero => {
                return Err("SAT model target is not accepted by the thermo oracle".into());
            }
            Multiplicity::Unique => {
                if solve.first_solution != Some(candidate.target) {
                    return Err("unique thermo solution differs from the SAT target".into());
                }
                write_checkpoint(&checkpoint, next_checkpoint)?;
                println!("status=unique");
                println!("proof_certified=false");
                println!("positive_witness_recheckable=true");
                println!("global_19c_conclusion=true");
                println!("conclusion=at-most-19c-existence-witness");
                println!("search_scope={}", topology_scope.search_scope());
                print!("{}", format_candidate_body(&candidate, &edges));
                println!(
                    "iterations={}\npairs={}\ncheckpoint={}\noracle_nodes={}\nsymmetry_break={}\ntopology_scope={}\n",
                    iteration + 1,
                    checkpoint.pairs.len(),
                    next_checkpoint.display(),
                    solve.stats.nodes,
                    symmetry_break.as_str(),
                    topology_scope.as_str()
                );
                println!("checkpoint_fnv1a64={:016x}", checkpoint.checksum);
                return Ok(());
            }
            Multiplicity::Multiple => {
                let first = solve
                    .first_solution
                    .ok_or_else(|| "multiple result has no first solution".to_string())?;
                let second = solve
                    .second_solution
                    .ok_or_else(|| "multiple result has no second solution".to_string())?;
                let pair = GridPair::new(first, second)?;
                let cut = pair_cut(&pair, &edges);
                if pair_clause(cut).iter().any(|&variable| {
                    let edge_id = (variable - EDGE_BASE) as usize;
                    candidate.selected.binary_search(&edge_id).is_ok()
                }) {
                    return Err("thermo alternatives generated a cut hit by the candidate".into());
                }
                if checkpoint.pair_index.contains(&checkpoint.pairs, &pair) {
                    return Err("thermo alternatives generated a duplicate learned pair".into());
                }
                if checkpoint.cut_index.contains(&checkpoint.cuts, &cut) {
                    return Err("thermo alternatives generated a duplicate learned pair cut".into());
                }
                let before_pairs = checkpoint.pairs.len();
                let before_cuts = checkpoint.cuts.len();
                let before_checksum = checkpoint.checksum;
                checkpoint.reserve_for_append(1, 1)?;
                if !checkpoint.insert_pair(pair)? || !checkpoint.insert_cut(cut, before_pairs)? {
                    return Err("learned pair indexes changed during insertion".into());
                }
                checkpoint.checksum = extend_pairs_checksum(before_checksum, &[pair]);
                append_pair_to_cnf(
                    cnf,
                    before_pairs,
                    before_cuts,
                    before_checksum,
                    &checkpoint,
                    &pair,
                    symmetry_break,
                    topology_scope,
                )?;
                clauses += 1;
                write_checkpoint(&checkpoint, next_checkpoint)?;
                eprintln!(
                    "topology-loop iteration={iteration} learned_pair={}|{} oracle_nodes={}",
                    format_packed_grid(pair.first),
                    format_packed_grid(pair.second),
                    solve.stats.nodes
                );
            }
        }
    }
    println!(
        "status=iteration-limit\nproof_certified=false\nglobal_19c_conclusion=false\niterations={max_iterations}\npairs={}\ncheckpoint={}\nsymmetry_break={}\ntopology_scope={}\n",
        checkpoint.pairs.len(),
        next_checkpoint.display(),
        symmetry_break.as_str(),
        topology_scope.as_str()
    );
    println!("checkpoint_fnv1a64={:016x}", checkpoint.checksum);
    Ok(())
}

fn milliseconds(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

#[derive(Clone, Copy, Debug, Default)]
struct CheckpointWriteMetrics {
    writes: usize,
    total_ms: f64,
}

impl CheckpointWriteMetrics {
    fn record(&mut self, elapsed_ms: Option<f64>) {
        if let Some(elapsed_ms) = elapsed_ms {
            self.writes += 1;
            self.total_ms += elapsed_ms;
        }
    }
}

fn checkpoint_due(dirty_refinement_batches: usize, checkpoint_every: usize) -> bool {
    dirty_refinement_batches >= checkpoint_every
}

#[allow(clippy::too_many_arguments)]
fn persist_incremental_state(
    checkpoint: &Checkpoint,
    next_checkpoint: &Path,
    lazy: Option<&LazyCutRuntime>,
    cnf: &Path,
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
    checkpoint_dirty: bool,
    rewrite_lazy_cnf: bool,
) -> Result<Option<f64>, String> {
    // The pair checkpoint is authoritative and is replaced first. Therefore a
    // crash can leave the active manifest behind, never ahead of its cut pool.
    let checkpoint_write_ms = if checkpoint_dirty {
        let started = Instant::now();
        write_checkpoint(checkpoint, next_checkpoint)?;
        Some(milliseconds(started))
    } else {
        None
    };
    if let Some(lazy) = lazy {
        write_active_cuts_manifest(
            checkpoint,
            &lazy.active,
            &lazy.options.manifest,
            symmetry_break,
            topology_scope,
        )?;
        if rewrite_lazy_cnf {
            write_lazy_cnf_for_scope(
                checkpoint,
                &lazy.active,
                cnf,
                symmetry_break,
                topology_scope,
            )?;
        }
    }
    Ok(checkpoint_write_ms)
}

#[allow(clippy::too_many_arguments)]
fn preserve_incremental_progress_error(
    checkpoint: &Checkpoint,
    next_checkpoint: &Path,
    dirty: bool,
    lazy: Option<&LazyCutRuntime>,
    cnf: &Path,
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
    error: String,
) -> String {
    if !dirty && lazy.is_none() {
        return error;
    }
    let saved = if dirty {
        persist_incremental_state(
            checkpoint,
            next_checkpoint,
            lazy,
            cnf,
            symmetry_break,
            topology_scope,
            true,
            false,
        )
        .map(|_| ())
    } else if let Some(lazy) = lazy {
        write_active_cuts_manifest(
            checkpoint,
            &lazy.active,
            &lazy.options.manifest,
            symmetry_break,
            topology_scope,
        )
    } else {
        Ok(())
    };
    match saved {
        Ok(()) => format!("{error}; in-memory incremental state was saved"),
        Err(save_error) => {
            format!("{error}; additionally failed to save incremental state: {save_error}")
        }
    }
}

fn candidate_hits_cut(candidate: &DecodedCandidate, cut: PairCut) -> bool {
    candidate
        .selected
        .iter()
        .any(|&edge| cut.0[edge / 64] & (1u64 << (edge % 64)) != 0)
}

fn validate_oracle_solution(
    candidate: &DecodedCandidate,
    grid: &Grid,
    edges: &[DirectedEdge],
) -> Result<(), String> {
    if !validate_sudoku(grid) {
        return Err("thermo batch oracle returned a non-Sudoku grid".into());
    }
    if candidate.selected.iter().any(|&edge_id| {
        let edge = edges[edge_id];
        grid[edge.lower as usize] >= grid[edge.upper as usize]
    }) {
        return Err("thermo batch oracle returned a comparison-violating grid".into());
    }
    Ok(())
}

fn validate_oracle_batch(
    candidate: &DecodedCandidate,
    solutions: &[Grid],
    edges: &[DirectedEdge],
) -> Result<(), String> {
    let mut distinct = HashSet::new();
    for solution in solutions {
        validate_oracle_solution(candidate, solution, edges)?;
        if !distinct.insert(*solution) {
            return Err("thermo batch oracle returned a duplicate solution".into());
        }
    }
    Ok(())
}

fn select_oracle_alternatives(
    target: &Grid,
    solutions: &[Grid],
    exhausted: bool,
    limit: usize,
) -> Result<Vec<Grid>, String> {
    if exhausted && !solutions.contains(target) {
        return Err("exhausted thermo batch does not contain the SAT target".into());
    }
    let alternatives = solutions
        .iter()
        .copied()
        .filter(|grid| grid != target)
        .take(limit)
        .collect::<Vec<_>>();
    if alternatives.is_empty() && !exhausted {
        return Err("capped thermo batch contains no alternative solution".into());
    }
    Ok(alternatives)
}

#[derive(Debug, Default, Eq, PartialEq)]
struct Refinement {
    pairs: Vec<GridPair>,
    cuts: Vec<PairCut>,
    /// Offsets into `pairs`, parallel to `cuts`.
    cut_witness_offsets: Vec<usize>,
}

fn collect_refinement(
    candidate: &DecodedCandidate,
    alternatives: &[Grid],
    edges: &[DirectedEdge],
    pair_mode: PairMode,
    checkpoint: &Checkpoint,
) -> Result<Refinement, String> {
    let pool = std::iter::once(candidate.target)
        .chain(alternatives.iter().copied())
        .collect::<Vec<_>>();
    let mut refinement = Refinement::default();
    let mut batch_pairs = HashSet::new();
    let mut batch_cuts = HashSet::new();
    let mut learn_pair = |left: Grid, right: Grid| -> Result<(), String> {
        let pair = GridPair::new(left, right)?;
        let cut = pair_cut(&pair, edges);
        if candidate_hits_cut(candidate, cut) {
            return Err("oracle pair cut is hit by the checked topology".into());
        }
        if !checkpoint.pair_index.contains(&checkpoint.pairs, &pair) && batch_pairs.insert(pair) {
            let pair_offset = refinement.pairs.len();
            refinement.pairs.push(pair);
            if !checkpoint.cut_index.contains(&checkpoint.cuts, &cut) && batch_cuts.insert(cut) {
                refinement.cuts.push(cut);
                refinement.cut_witness_offsets.push(pair_offset);
            }
        }
        Ok(())
    };
    match pair_mode {
        PairMode::Anchor => {
            for &alternative in alternatives {
                learn_pair(candidate.target, alternative)?;
            }
        }
        PairMode::All => {
            for left in 0..pool.len() {
                for right in left + 1..pool.len() {
                    learn_pair(pool[left], pool[right])?;
                }
            }
        }
    }
    Ok(refinement)
}

#[allow(clippy::too_many_arguments)]
fn print_incremental_metadata(
    bridge: &IncrementalBridge,
    checkpoint: &Checkpoint,
    checkpoint_lock: &RunLock,
    cnf_lock: &RunLock,
    max_iterations: usize,
    conflicts: Option<u64>,
    oracle_batch: usize,
    pair_mode: PairMode,
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
    checkpoint_every: usize,
    lazy: Option<&LazyCutRuntime>,
    sat_solves: usize,
    full_pool_scans: usize,
    lazy_cuts_activated: usize,
    checkpoint_write_metrics: CheckpointWriteMetrics,
    elapsed_seconds: f64,
) {
    println!("cnf_schema={}", topology_scope.cnf_schema());
    println!("bridge_protocol={BRIDGE_PROTOCOL}");
    println!("bridge_executable={}", bridge.executable.display());
    println!("cadical_signature={}", bridge.metadata.cadical);
    println!("cadical_revision={}", bridge.metadata.revision);
    println!("cadical_library_sha256={}", bridge.metadata.library_sha256);
    println!("prefer_selected={}", bridge.metadata.prefer_selected);
    println!("configured_max_iterations={max_iterations}");
    match conflicts {
        Some(limit) => println!("conflicts_per_solve={limit}"),
        None => println!("conflicts_per_solve=unlimited"),
    }
    println!("oracle_batch={oracle_batch}");
    println!("pair_mode={}", pair_mode.as_str());
    println!("symmetry_break={}", symmetry_break.as_str());
    println!("topology_scope={}", topology_scope.as_str());
    println!("checkpoint_atomic=true");
    println!(
        "checkpoint_writer_lock={}",
        checkpoint_lock.path().display()
    );
    println!("cnf_writer_lock={}", cnf_lock.path().display());
    println!("checkpoint_every={checkpoint_every}");
    println!("max_uncheckpointed_refinement_batches={checkpoint_every}");
    println!("checkpoint_writes={}", checkpoint_write_metrics.writes);
    println!(
        "total_checkpoint_write_ms={:.3}",
        checkpoint_write_metrics.total_ms
    );
    println!("sat_solves={sat_solves}");
    println!("full_pool_scans={full_pool_scans}");
    println!("lazy_cuts_activated={lazy_cuts_activated}");
    if let Some(lazy) = lazy {
        println!("cut_pool_mode=lazy-active-v1");
        println!("active_pair_cuts={}", lazy.active.indices.len());
        println!(
            "active_fnv1a64={:016x}",
            active_cuts_checksum(checkpoint, &lazy.active.indices)
                .expect("terminal active pool was already validated")
        );
        println!("active_cuts_manifest={}", lazy.options.manifest.display());
        println!("lazy_active_seed={}", lazy.options.active_seed);
        match lazy.options.violation_batch {
            Some(limit) => println!("lazy_violation_batch={limit}"),
            None => println!("lazy_violation_batch=all"),
        }
        println!("cnf_mutable=false");
        println!("cnf_snapshot=terminal-active");
        println!("cnf_reconstruction=emit-active");
    } else {
        println!("cut_pool_mode=full");
        println!("cnf_mutable=true");
        println!("cnf_snapshot=current-checkpoint");
    }
    println!("bridge_total_clauses={}", bridge.total_clauses());
    println!("bridge_clause_count_identity_verified=true");
    println!("elapsed_seconds={elapsed_seconds:.6}");
}

fn verify_bridge_clause_count(
    bridge: &IncrementalBridge,
    base_clause_count: usize,
    checkpoint: &Checkpoint,
    lazy: Option<&LazyCutRuntime>,
) -> Result<(), String> {
    let pair_clauses = lazy.map_or(checkpoint.cuts.len(), |state| state.active.indices.len());
    let expected = base_clause_count + pair_clauses;
    if bridge.total_clauses() != expected {
        return Err(format!(
            "bridge clause count {} disagrees with reconstructed formula count {expected}",
            bridge.total_clauses()
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_incremental_loop(
    checkpoint_path: &Path,
    next_checkpoint: &Path,
    bridge_exe: &Path,
    cnf: &Path,
    max_iterations: usize,
    conflicts: Option<u64>,
    oracle_batch: usize,
    pair_mode: PairMode,
    prefer_selected: bool,
    checkpoint_every: usize,
    symmetry_break: SymmetryBreak,
    topology_scope: TopologyScope,
    lazy_options: Option<LazyCutOptions>,
) -> Result<(), String> {
    let run_started = Instant::now();
    let checkpoint_lock = RunLock::acquire(next_checkpoint, "checkpoint")?;
    let cnf_lock = RunLock::acquire(cnf, "CNF")?;
    let load_started = Instant::now();
    let reserve_pairs = eager_refinement_reserve(max_iterations, oracle_batch, pair_mode)?;
    let mut checkpoint = load_checkpoint_with_reserve(checkpoint_path, reserve_pairs)?;
    let load_ms = milliseconds(load_started);
    let edges = directed_edges();
    let base = base_clauses_for_scope(&edges, symmetry_break, topology_scope);

    let mut lazy = if let Some(options) = lazy_options {
        let manifest_lock = RunLock::acquire(&options.manifest, "active-cut manifest")?;
        let mut active = if options.manifest.exists() {
            load_active_cuts_manifest(
                &options.manifest,
                &checkpoint,
                &edges,
                symmetry_break,
                topology_scope,
            )?
        } else {
            ActiveCutPool::from_indices(checkpoint.cuts.len(), Vec::new())?
        };
        for index in evenly_spaced_cut_indices(checkpoint.cuts.len(), options.active_seed) {
            if !active.mask[index] {
                active.activate(index)?;
            }
        }
        Some(LazyCutRuntime {
            options,
            active,
            _manifest_lock: manifest_lock,
        })
    } else {
        None
    };

    let initial_checkpoint_started = Instant::now();
    write_checkpoint(&checkpoint, next_checkpoint)?;
    let initial_checkpoint_ms = milliseconds(initial_checkpoint_started);
    let mut checkpoint_write_metrics = CheckpointWriteMetrics::default();
    checkpoint_write_metrics.record(Some(initial_checkpoint_ms));
    if let Some(lazy) = &lazy {
        write_active_cuts_manifest(
            &checkpoint,
            &lazy.active,
            &lazy.options.manifest,
            symmetry_break,
            topology_scope,
        )?;
    }
    let cnf_started = Instant::now();
    let (_, clauses) = if let Some(lazy) = &lazy {
        write_lazy_cnf_for_scope(
            &checkpoint,
            &lazy.active,
            cnf,
            symmetry_break,
            topology_scope,
        )?
    } else {
        write_cnf_for_scope(&checkpoint, cnf, symmetry_break, topology_scope)?
    };
    let cnf_ms = milliseconds(cnf_started);
    let bridge_started = Instant::now();
    let mut bridge = IncrementalBridge::spawn(
        bridge_exe,
        cnf,
        topology_scope.variable_count() as usize,
        clauses,
        prefer_selected,
    )?;
    let bridge_ready_ms = milliseconds(bridge_started);
    eprintln!(
        "incremental-topology ready pairs={} unique_cuts={} active_cuts={} cut_pool_mode={} clauses={} load_ms={load_ms:.3} cnf_ms={cnf_ms:.3} initial_checkpoint_ms={initial_checkpoint_ms:.3} bridge_ready_ms={bridge_ready_ms:.3} cadical={} revision={} library_sha256={} prefer_selected={} symmetry_break={} topology_scope={}",
        checkpoint.pairs.len(),
        checkpoint.cuts.len(),
        lazy.as_ref()
            .map_or(checkpoint.cuts.len(), |state| state.active.indices.len()),
        if lazy.is_some() {
            "lazy-active-v1"
        } else {
            "full"
        },
        bridge.total_clauses(),
        bridge.metadata.cadical,
        bridge.metadata.revision,
        bridge.metadata.library_sha256,
        bridge.metadata.prefer_selected,
        symmetry_break.as_str(),
        topology_scope.as_str()
    );

    let mut dirty_refinement_batches = 0usize;
    let mut total_sat_ms = 0.0;
    let mut total_decode_validation_ms = 0.0;
    let mut total_oracle_ms = 0.0;
    let mut total_refinement_ms = 0.0;
    let mut total_pool_scan_ms = 0.0;
    let mut total_oracle_nodes = 0u64;
    let mut sat_solves = 0usize;
    let mut full_pool_scans = 0usize;
    let mut lazy_cuts_activated = 0usize;
    let initial_pairs = checkpoint.pairs.len();
    let initial_cuts = checkpoint.cuts.len();

    for iteration in 0..max_iterations {
        let iteration_started = Instant::now();
        let mut iteration_sat_ms = 0.0;
        let (candidate, selected_mask) = loop {
            let sat_started = Instant::now();
            let sat = bridge.solve(conflicts).map_err(|error| {
                preserve_incremental_progress_error(
                    &checkpoint,
                    next_checkpoint,
                    dirty_refinement_batches != 0,
                    lazy.as_ref(),
                    cnf,
                    symmetry_break,
                    topology_scope,
                    error,
                )
            })?;
            sat_solves += 1;
            let sat_ms = milliseconds(sat_started);
            iteration_sat_ms += sat_ms;
            total_sat_ms += sat_ms;
            match sat.status {
                SatStatus::Unknown => {
                    verify_bridge_clause_count(&bridge, base.len(), &checkpoint, lazy.as_ref())?;
                    let checkpoint_ms = persist_incremental_state(
                        &checkpoint,
                        next_checkpoint,
                        lazy.as_ref(),
                        cnf,
                        symmetry_break,
                        topology_scope,
                        dirty_refinement_batches != 0,
                        true,
                    )?;
                    checkpoint_write_metrics.record(checkpoint_ms);
                    bridge.shutdown()?;
                    println!("status=inconclusive-sat-limit");
                    println!("proof_certified=false");
                    println!("global_19c_conclusion=false");
                    println!("iterations={}", iteration + 1);
                    println!("pairs={}", checkpoint.pairs.len());
                    println!("unique_pair_cuts={}", checkpoint.cuts.len());
                    println!("checkpoint={}", next_checkpoint.display());
                    println!("checkpoint_fnv1a64={:016x}", checkpoint.checksum);
                    println!("cnf={}", cnf.display());
                    println!("total_sat_ms={total_sat_ms:.3}");
                    println!("total_pool_scan_ms={total_pool_scan_ms:.3}");
                    println!("total_decode_validation_ms={total_decode_validation_ms:.3}");
                    println!("total_oracle_ms={total_oracle_ms:.3}");
                    println!("total_refinement_ms={total_refinement_ms:.3}");
                    println!("total_oracle_nodes={total_oracle_nodes}");
                    print_incremental_metadata(
                        &bridge,
                        &checkpoint,
                        &checkpoint_lock,
                        &cnf_lock,
                        max_iterations,
                        conflicts,
                        oracle_batch,
                        pair_mode,
                        symmetry_break,
                        topology_scope,
                        checkpoint_every,
                        lazy.as_ref(),
                        sat_solves,
                        full_pool_scans,
                        lazy_cuts_activated,
                        checkpoint_write_metrics,
                        run_started.elapsed().as_secs_f64(),
                    );
                    return Ok(());
                }
                SatStatus::Unsatisfiable => {
                    verify_bridge_clause_count(&bridge, base.len(), &checkpoint, lazy.as_ref())?;
                    let checkpoint_ms = persist_incremental_state(
                        &checkpoint,
                        next_checkpoint,
                        lazy.as_ref(),
                        cnf,
                        symmetry_break,
                        topology_scope,
                        dirty_refinement_batches != 0,
                        true,
                    )?;
                    checkpoint_write_metrics.record(checkpoint_ms);
                    bridge.shutdown()?;
                    println!("status=incremental-topology-unsat-provisional");
                    println!("proof_certified=false");
                    println!("global_19c_conclusion=false");
                    println!(
                        "negative_exclusion_requires_symmetry_orbit_lemma={}",
                        symmetry_break != SymmetryBreak::None
                    );
                    println!("next_action=fresh-static-cnf-proof-rerun");
                    if lazy.is_some() {
                        println!("proof_formula=terminal-base-plus-active-cnf");
                    }
                    println!("iterations={}", iteration + 1);
                    println!("pairs={}", checkpoint.pairs.len());
                    println!("unique_pair_cuts={}", checkpoint.cuts.len());
                    println!("checkpoint={}", next_checkpoint.display());
                    println!("checkpoint_fnv1a64={:016x}", checkpoint.checksum);
                    println!("cnf={}", cnf.display());
                    println!("total_sat_ms={total_sat_ms:.3}");
                    println!("total_pool_scan_ms={total_pool_scan_ms:.3}");
                    println!("total_decode_validation_ms={total_decode_validation_ms:.3}");
                    println!("total_oracle_ms={total_oracle_ms:.3}");
                    println!("total_refinement_ms={total_refinement_ms:.3}");
                    println!("total_oracle_nodes={total_oracle_nodes}");
                    print_incremental_metadata(
                        &bridge,
                        &checkpoint,
                        &checkpoint_lock,
                        &cnf_lock,
                        max_iterations,
                        conflicts,
                        oracle_batch,
                        pair_mode,
                        symmetry_break,
                        topology_scope,
                        checkpoint_every,
                        lazy.as_ref(),
                        sat_solves,
                        full_pool_scans,
                        lazy_cuts_activated,
                        checkpoint_write_metrics,
                        run_started.elapsed().as_secs_f64(),
                    );
                    return Ok(());
                }
                SatStatus::Satisfiable => {}
            }

            let assignment = sat.assignment.as_deref().ok_or_else(|| {
                preserve_incremental_progress_error(
                    &checkpoint,
                    next_checkpoint,
                    dirty_refinement_batches != 0,
                    lazy.as_ref(),
                    cnf,
                    symmetry_break,
                    topology_scope,
                    "bridge SAT result has no assignment".into(),
                )
            })?;
            let active_cuts = lazy.as_ref().map(|state| {
                state
                    .active
                    .indices
                    .iter()
                    .map(|&index| checkpoint.cuts[index])
                    .collect::<Vec<_>>()
            });
            let required_cuts = active_cuts.as_deref().unwrap_or(&checkpoint.cuts);
            let decode_validation_started = Instant::now();
            let candidate = decode_candidate_with_scope_and_base(
                required_cuts,
                assignment,
                &edges,
                &base,
                topology_scope,
            )
            .map_err(|error| {
                preserve_incremental_progress_error(
                    &checkpoint,
                    next_checkpoint,
                    dirty_refinement_batches != 0,
                    lazy.as_ref(),
                    cnf,
                    symmetry_break,
                    topology_scope,
                    error,
                )
            })?;
            let decode_validation_ms = milliseconds(decode_validation_started);
            total_decode_validation_ms += decode_validation_ms;
            let selected_mask = selected_edge_mask(assignment);

            if let Some(lazy_state) = lazy.as_ref() {
                let scan_started = Instant::now();
                let (violated, violated_total) = violated_inactive_cut_indices(
                    &checkpoint.cuts,
                    &lazy_state.active,
                    selected_mask,
                    lazy_state.options.violation_batch,
                )?;
                let scan_ms = milliseconds(scan_started);
                total_pool_scan_ms += scan_ms;
                full_pool_scans += 1;
                if violated_total != 0 {
                    if violated.is_empty() {
                        return Err("lazy pool found violations but selected an empty batch".into());
                    }
                    for index in violated.iter().copied() {
                        bridge
                            .add_clause(&pair_clause(checkpoint.cuts[index]))
                            .map_err(|error| {
                                preserve_incremental_progress_error(
                                    &checkpoint,
                                    next_checkpoint,
                                    dirty_refinement_batches != 0,
                                    lazy.as_ref(),
                                    cnf,
                                    symmetry_break,
                                    topology_scope,
                                    error,
                                )
                            })?;
                        // An index becomes active only after the bridge ACK.
                        lazy.as_mut()
                            .expect("checked above")
                            .active
                            .activate(index)?;
                        lazy_cuts_activated += 1;
                    }
                    // Keep bridge-only activations in memory until the next
                    // scheduled, terminal, or error-path persistence. This
                    // avoids rewriting a large dirty checkpoint merely to
                    // advance its small active subset. After a hard crash the
                    // last durable manifest is still a valid prefix of the
                    // last durable checkpoint; the disposable CNF and bridge
                    // are regenerated from that mutually consistent state.
                    eprintln!(
                        "incremental-topology lazy-filter iteration={iteration} sat_solve={sat_solves} sat_ms={sat_ms:.3} scan_ms={scan_ms:.3} selected={} violated_total={violated_total} activated={} active_cuts={} full_pool_cuts={} bridge_clauses={}",
                        candidate.selected.len(),
                        violated.len(),
                        lazy.as_ref().expect("checked above").active.indices.len(),
                        checkpoint.cuts.len(),
                        bridge.total_clauses()
                    );
                    continue;
                }
            }
            break (candidate, selected_mask);
        };

        let oracle_started = Instant::now();
        let solver = Solver::blank(&candidate.paths).map_err(|error| {
            preserve_incremental_progress_error(
                &checkpoint,
                next_checkpoint,
                dirty_refinement_batches != 0,
                lazy.as_ref(),
                cnf,
                symmetry_break,
                topology_scope,
                format!("cannot build exact thermo batch oracle: {error}"),
            )
        })?;
        let oracle = solver.enumerate_up_to(oracle_batch + 1);
        let oracle_ms = milliseconds(oracle_started);
        total_oracle_ms += oracle_ms;
        total_oracle_nodes += oracle.stats.nodes;
        validate_oracle_batch(&candidate, &oracle.solutions, &edges).map_err(|error| {
            preserve_incremental_progress_error(
                &checkpoint,
                next_checkpoint,
                dirty_refinement_batches != 0,
                lazy.as_ref(),
                cnf,
                symmetry_break,
                topology_scope,
                error,
            )
        })?;
        let alternatives = select_oracle_alternatives(
            &candidate.target,
            &oracle.solutions,
            oracle.exhausted,
            oracle_batch,
        )
        .map_err(|error| {
            preserve_incremental_progress_error(
                &checkpoint,
                next_checkpoint,
                dirty_refinement_batches != 0,
                lazy.as_ref(),
                cnf,
                symmetry_break,
                topology_scope,
                error,
            )
        })?;

        if alternatives.is_empty() {
            debug_assert!(oracle.exhausted);
            verify_bridge_clause_count(&bridge, base.len(), &checkpoint, lazy.as_ref())?;
            let checkpoint_ms = persist_incremental_state(
                &checkpoint,
                next_checkpoint,
                lazy.as_ref(),
                cnf,
                symmetry_break,
                topology_scope,
                dirty_refinement_batches != 0,
                true,
            )?;
            checkpoint_write_metrics.record(checkpoint_ms);
            bridge.shutdown()?;
            println!("status=unique");
            println!("proof_certified=false");
            println!("positive_witness_recheckable=true");
            println!("global_19c_conclusion=true");
            println!("conclusion=at-most-19c-existence-witness");
            println!("search_scope={}", topology_scope.search_scope());
            print!("{}", format_candidate_body(&candidate, &edges));
            println!("classification=exact-unique");
            println!("iterations={}", iteration + 1);
            println!("pairs={}", checkpoint.pairs.len());
            println!("unique_pair_cuts={}", checkpoint.cuts.len());
            println!("checkpoint={}", next_checkpoint.display());
            println!("checkpoint_fnv1a64={:016x}", checkpoint.checksum);
            println!("cnf={}", cnf.display());
            println!("oracle_nodes={}", oracle.stats.nodes);
            println!("total_sat_ms={total_sat_ms:.3}");
            println!("total_pool_scan_ms={total_pool_scan_ms:.3}");
            println!("total_decode_validation_ms={total_decode_validation_ms:.3}");
            println!("total_oracle_ms={total_oracle_ms:.3}");
            println!("total_refinement_ms={total_refinement_ms:.3}");
            println!("total_oracle_nodes={total_oracle_nodes}");
            print_incremental_metadata(
                &bridge,
                &checkpoint,
                &checkpoint_lock,
                &cnf_lock,
                max_iterations,
                conflicts,
                oracle_batch,
                pair_mode,
                symmetry_break,
                topology_scope,
                checkpoint_every,
                lazy.as_ref(),
                sat_solves,
                full_pool_scans,
                lazy_cuts_activated,
                checkpoint_write_metrics,
                run_started.elapsed().as_secs_f64(),
            );
            return Ok(());
        }

        let refinement_started = Instant::now();
        let before_pairs = checkpoint.pairs.len();
        let before_cuts = checkpoint.cuts.len();
        let before_checksum = checkpoint.checksum;
        let refinement =
            collect_refinement(&candidate, &alternatives, &edges, pair_mode, &checkpoint).map_err(
                |error| {
                    preserve_incremental_progress_error(
                        &checkpoint,
                        next_checkpoint,
                        dirty_refinement_batches != 0,
                        lazy.as_ref(),
                        cnf,
                        symmetry_break,
                        topology_scope,
                        error,
                    )
                },
            )?;
        if refinement.pairs.is_empty() || refinement.cuts.is_empty() {
            return Err(preserve_incremental_progress_error(
                &checkpoint,
                next_checkpoint,
                dirty_refinement_batches != 0,
                lazy.as_ref(),
                cnf,
                symmetry_break,
                topology_scope,
                "thermo alternatives produced no new unique pair cut".into(),
            ));
        }
        checkpoint.reserve_for_append(refinement.pairs.len(), refinement.cuts.len())?;
        for &pair in &refinement.pairs {
            if !checkpoint.insert_pair(pair)? {
                return Err("refinement pair became duplicate during commit".into());
            }
        }
        for (&cut, &offset) in refinement.cuts.iter().zip(&refinement.cut_witness_offsets) {
            if !checkpoint.insert_cut(cut, before_pairs + offset)? {
                return Err("refinement cut became duplicate during commit".into());
            }
        }
        checkpoint.checksum = extend_pairs_checksum(before_checksum, &refinement.pairs);
        let mut refinement_active_cuts = refinement.cuts.len();
        if let Some(lazy_state) = lazy.as_mut() {
            lazy_state.active.extend_pool(checkpoint.cuts.len())?;
            let mut violated = (before_cuts..checkpoint.cuts.len())
                .map(|index| {
                    let cut = checkpoint.cuts[index];
                    (
                        cut.0.iter().map(|word| word.count_ones()).sum::<u32>(),
                        index,
                    )
                })
                .collect::<Vec<_>>();
            if violated.iter().any(|&(_, index)| {
                lazy_state.active.mask[index]
                    || pair_cut_satisfied(checkpoint.cuts[index], selected_mask)
            }) || violated.len() != refinement.cuts.len()
            {
                return Err(preserve_incremental_progress_error(
                    &checkpoint,
                    next_checkpoint,
                    true,
                    lazy.as_ref(),
                    cnf,
                    symmetry_break,
                    topology_scope,
                    "new oracle cuts were not all violated by the checked topology".into(),
                ));
            }
            violated.sort_unstable();
            if let Some(limit) = lazy_state.options.violation_batch {
                violated.truncate(limit);
            }
            if violated.is_empty() {
                return Err("lazy oracle refinement selected an empty active batch".into());
            }
            refinement_active_cuts = violated.len();
            for (_, index) in violated {
                bridge
                    .add_clause(&pair_clause(checkpoint.cuts[index]))
                    .map_err(|error| {
                        preserve_incremental_progress_error(
                            &checkpoint,
                            next_checkpoint,
                            true,
                            lazy.as_ref(),
                            cnf,
                            symmetry_break,
                            topology_scope,
                            error,
                        )
                    })?;
                lazy.as_mut()
                    .expect("checked above")
                    .active
                    .activate(index)?;
                lazy_cuts_activated += 1;
            }
        } else {
            if let Err(error) = append_refinement_to_cnf(
                cnf,
                before_pairs,
                before_cuts,
                before_checksum,
                &checkpoint,
                &refinement.cuts,
                symmetry_break,
                topology_scope,
            ) {
                return Err(preserve_incremental_progress_error(
                    &checkpoint,
                    next_checkpoint,
                    true,
                    lazy.as_ref(),
                    cnf,
                    symmetry_break,
                    topology_scope,
                    error,
                ));
            }
            for &cut in &refinement.cuts {
                if let Err(error) = bridge.add_clause(&pair_clause(cut)) {
                    return Err(preserve_incremental_progress_error(
                        &checkpoint,
                        next_checkpoint,
                        true,
                        lazy.as_ref(),
                        cnf,
                        symmetry_break,
                        topology_scope,
                        error,
                    ));
                }
            }
        }
        dirty_refinement_batches += 1;
        if checkpoint_due(dirty_refinement_batches, checkpoint_every) {
            let checkpoint_ms = persist_incremental_state(
                &checkpoint,
                next_checkpoint,
                lazy.as_ref(),
                cnf,
                symmetry_break,
                topology_scope,
                true,
                false,
            )?;
            checkpoint_write_metrics.record(checkpoint_ms);
            dirty_refinement_batches = 0;
        }
        let refinement_ms = milliseconds(refinement_started);
        total_refinement_ms += refinement_ms;
        eprintln!(
            "incremental-topology iteration={iteration} sat_ms={iteration_sat_ms:.3} oracle_ms={oracle_ms:.3} refinement_ms={refinement_ms:.3} iteration_ms={:.3} selected={} covered={} thermometers={} oracle_nodes={} oracle_solutions={} oracle_exhausted={} alternatives={} new_pairs={} new_unique_cuts={} newly_activated_cuts={} pairs={} unique_cuts={} active_cuts={} bridge_clauses={}",
            milliseconds(iteration_started),
            candidate.selected.len(),
            candidate.covered_cells,
            candidate.paths.len(),
            oracle.stats.nodes,
            oracle.solutions.len(),
            oracle.exhausted,
            alternatives.len(),
            refinement.pairs.len(),
            refinement.cuts.len(),
            refinement_active_cuts,
            checkpoint.pairs.len(),
            checkpoint.cuts.len(),
            lazy.as_ref()
                .map_or(checkpoint.cuts.len(), |state| state.active.indices.len()),
            bridge.total_clauses()
        );
    }

    verify_bridge_clause_count(&bridge, base.len(), &checkpoint, lazy.as_ref())?;
    let checkpoint_ms = persist_incremental_state(
        &checkpoint,
        next_checkpoint,
        lazy.as_ref(),
        cnf,
        symmetry_break,
        topology_scope,
        dirty_refinement_batches != 0,
        true,
    )?;
    checkpoint_write_metrics.record(checkpoint_ms);
    bridge.shutdown()?;
    println!("status=iteration-limit");
    println!("proof_certified=false");
    println!("global_19c_conclusion=false");
    println!("iterations={max_iterations}");
    println!("initial_pairs={initial_pairs}");
    println!("pairs={}", checkpoint.pairs.len());
    println!("pairs_added={}", checkpoint.pairs.len() - initial_pairs);
    println!("initial_unique_pair_cuts={initial_cuts}");
    println!("unique_pair_cuts={}", checkpoint.cuts.len());
    println!(
        "unique_pair_cuts_added={}",
        checkpoint.cuts.len() - initial_cuts
    );
    println!("checkpoint={}", next_checkpoint.display());
    println!("checkpoint_fnv1a64={:016x}", checkpoint.checksum);
    println!("cnf={}", cnf.display());
    println!("total_sat_ms={total_sat_ms:.3}");
    println!("total_pool_scan_ms={total_pool_scan_ms:.3}");
    println!("total_decode_validation_ms={total_decode_validation_ms:.3}");
    println!("total_oracle_ms={total_oracle_ms:.3}");
    println!("total_refinement_ms={total_refinement_ms:.3}");
    println!("total_oracle_nodes={total_oracle_nodes}");
    print_incremental_metadata(
        &bridge,
        &checkpoint,
        &checkpoint_lock,
        &cnf_lock,
        max_iterations,
        conflicts,
        oracle_batch,
        pair_mode,
        symmetry_break,
        topology_scope,
        checkpoint_every,
        lazy.as_ref(),
        sat_solves,
        full_pool_scans,
        lazy_cuts_activated,
        checkpoint_write_metrics,
        run_started.elapsed().as_secs_f64(),
    );
    Ok(())
}

fn run() -> Result<(), String> {
    match parse_options()? {
        Mode::Stats { checkpoint } => run_stats(&checkpoint),
        Mode::MergeCheckpoints {
            checkpoint,
            merge_checkpoints,
            output,
        } => run_merge_checkpoints(&checkpoint, &merge_checkpoints, &output),
        Mode::Emit {
            checkpoint,
            output,
            symmetry_break,
            topology_scope,
        } => run_emit(&checkpoint, &output, symmetry_break, topology_scope),
        Mode::EmitActive {
            checkpoint,
            active_cuts,
            output,
            symmetry_break,
            topology_scope,
        } => run_emit_active(
            &checkpoint,
            &active_cuts,
            &output,
            symmetry_break,
            topology_scope,
        ),
        Mode::Decode {
            checkpoint,
            model,
            output,
            symmetry_break,
            topology_scope,
        } => run_decode(
            &checkpoint,
            &model,
            output.as_deref(),
            symmetry_break,
            topology_scope,
        ),
        Mode::Loop {
            checkpoint,
            next_checkpoint,
            sat_exe,
            cnf,
            model,
            proof,
            max_iterations,
            conflicts,
            symmetry_break,
            topology_scope,
        } => run_loop(
            &checkpoint,
            &next_checkpoint,
            &sat_exe,
            &cnf,
            &model,
            proof.as_deref(),
            max_iterations,
            conflicts,
            symmetry_break,
            topology_scope,
        ),
        Mode::IncrementalLoop {
            checkpoint,
            next_checkpoint,
            bridge_exe,
            cnf,
            max_iterations,
            conflicts,
            oracle_batch,
            pair_mode,
            prefer_selected,
            checkpoint_every,
            symmetry_break,
            topology_scope,
            lazy_cuts,
        } => run_incremental_loop(
            &checkpoint,
            &next_checkpoint,
            &bridge_exe,
            &cnf,
            max_iterations,
            conflicts,
            oracle_batch,
            pair_mode,
            prefer_selected,
            checkpoint_every,
            symmetry_break,
            topology_scope,
            lazy_cuts,
        ),
    }
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

    const CANONICAL: &str =
        "123456789456789123789123456214365897365897214897214365531642978642978531978531642";
    const BLUE_20: &[&[u8]] = &[
        &[18, 27, 28, 19, 20, 11, 12, 13, 4],
        &[57, 48, 49],
        &[59, 68, 69, 60, 61, 52, 53, 44],
    ];
    const KNOWN_THREE_19: &[&[u8]] = &[
        &[19, 29, 28, 20, 11, 12, 13, 3, 4],
        &[77, 69, 78, 70, 62, 53, 44, 52],
        &[41, 51],
    ];

    fn edge_id(edges: &[DirectedEdge], lower: usize, upper: usize) -> usize {
        edges
            .iter()
            .position(|edge| edge.lower as usize == lower && edge.upper as usize == upper)
            .expect("directed king edge")
    }

    fn assignment_for_row_thermo(edges: &[DirectedEdge]) -> Vec<bool> {
        let grid = parse_grid(CANONICAL).unwrap();
        let mut assignment = vec![false; VARIABLE_COUNT as usize + 1];
        for (cell, &digit) in grid.iter().enumerate() {
            assignment[digit_var(cell, digit as usize - 1) as usize] = true;
        }
        for cell in 0..9 {
            assignment[occupied_var(cell) as usize] = true;
        }
        for cell in 0..8 {
            let id = edge_id(edges, cell, cell + 1);
            assignment[edge_var(id) as usize] = true;
            assignment[swap_var(cell, id) as usize] = true;
        }
        let mut occupied = 0usize;
        for prefix in 0..CELLS - 1 {
            if prefix < 9 {
                occupied += 1;
            }
            for count in 0..COVER_LIMIT {
                assignment[sequential_var(prefix, count) as usize] = occupied > count;
            }
        }
        assignment
    }

    fn assign_exact_counter(
        assignment: &mut [bool],
        variables: &[i32],
        count: usize,
        base: i32,
    ) -> i32 {
        let width = count + 1;
        let mut true_inputs = 0usize;
        for (prefix, &variable) in variables.iter().enumerate() {
            true_inputs += usize::from(assignment[variable as usize]);
            for threshold in 1..=width {
                assignment[exact_counter_var(base, prefix, threshold, width) as usize] =
                    true_inputs >= threshold;
            }
        }
        base + (variables.len() * width) as i32
    }

    fn assignment_for_exact_982(paths: &[Vec<u8>], edges: &[DirectedEdge]) -> Vec<bool> {
        let grid = Solver::blank(paths).unwrap().enumerate_up_to(1).solutions[0];
        let mut assignment = vec![false; EXACT_982_VARIABLE_COUNT as usize + 1];
        for (cell, &digit) in grid.iter().enumerate() {
            assignment[digit_var(cell, digit as usize - 1) as usize] = true;
        }
        for path in paths {
            for &cell in path {
                assignment[occupied_var(cell as usize) as usize] = true;
            }
            for step in path.windows(2) {
                let id = edge_id(edges, step[0] as usize, step[1] as usize);
                assignment[edge_var(id) as usize] = true;
                let lower_digit = grid[step[0] as usize];
                let upper_digit = grid[step[1] as usize];
                if upper_digit == lower_digit + 1 {
                    assignment[swap_var(lower_digit as usize - 1, id) as usize] = true;
                }
            }
            assignment[exact_982_source_var(path[0] as usize) as usize] = true;
            let label = match path.len() {
                9 => 0,
                8 => 1,
                2 => 2,
                length => panic!("unexpected exact-982 test path length {length}"),
            };
            for &cell in path {
                assignment[exact_982_label_var(label, cell as usize) as usize] = true;
            }
        }

        let mut occupied = 0usize;
        for prefix in 0..CELLS - 1 {
            occupied += usize::from(assignment[occupied_var(prefix) as usize]);
            for count in 0..COVER_LIMIT {
                assignment[sequential_var(prefix, count) as usize] = occupied > count;
            }
        }

        let mut counter_base = EXACT_982_COUNTER_BASE;
        for (label, count) in [(0, 9), (1, 8), (2, 2)] {
            let variables = (0..CELLS)
                .map(|cell| exact_982_label_var(label, cell))
                .collect::<Vec<_>>();
            counter_base = assign_exact_counter(&mut assignment, &variables, count, counter_base);
        }
        let sources = (0..CELLS).map(exact_982_source_var).collect::<Vec<_>>();
        counter_base = assign_exact_counter(&mut assignment, &sources, 3, counter_base);
        assert_eq!(counter_base - 1, EXACT_982_VARIABLE_COUNT);
        assignment
    }

    fn clause_true(clause: &[i32], assignment: &[bool]) -> bool {
        clause.iter().any(|&literal| {
            let value = assignment[literal.unsigned_abs() as usize];
            if literal > 0 { value } else { !value }
        })
    }

    fn d4_cell(cell: usize, transform: usize) -> usize {
        let row = cell / 9;
        let column = cell % 9;
        let (new_row, new_column) = match transform {
            0 => (row, column),
            1 => (column, 8 - row),
            2 => (8 - row, 8 - column),
            3 => (8 - column, row),
            4 => (row, 8 - column),
            5 => (8 - row, column),
            6 => (column, row),
            7 => (8 - column, 8 - row),
            _ => panic!("D4 transform out of range"),
        };
        new_row * 9 + new_column
    }

    fn transform_candidate(
        grid: &Grid,
        arcs: &[(usize, usize)],
        transform: usize,
        complement: bool,
    ) -> (Grid, Vec<(usize, usize)>) {
        let mut transformed_grid = [0u8; CELLS];
        for cell in 0..CELLS {
            let digit = if complement {
                10 - grid[cell]
            } else {
                grid[cell]
            };
            transformed_grid[d4_cell(cell, transform)] = digit;
        }
        let transformed_arcs = arcs
            .iter()
            .map(|&(lower, upper)| {
                let lower = d4_cell(lower, transform);
                let upper = d4_cell(upper, transform);
                if complement {
                    (upper, lower)
                } else {
                    (lower, upper)
                }
            })
            .collect();
        (transformed_grid, transformed_arcs)
    }

    fn transform_paths(paths: &[Vec<u8>], transform: usize, complement: bool) -> Vec<Vec<u8>> {
        paths
            .iter()
            .map(|path| {
                let mut transformed = path
                    .iter()
                    .map(|&cell| d4_cell(cell as usize, transform) as u8)
                    .collect::<Vec<_>>();
                if complement {
                    transformed.reverse();
                }
                transformed
            })
            .collect()
    }

    fn empty_checkpoint() -> Checkpoint {
        Checkpoint {
            budget: 16,
            checksum: FNV_OFFSET,
            pairs: Vec::new(),
            cuts: Vec::new(),
            cut_witnesses: Vec::new(),
            pair_index: FlatIndex::new(),
            cut_index: FlatIndex::new(),
        }
    }

    fn swap_symbols(mut grid: Grid, left: u8, right: u8) -> Grid {
        for digit in &mut grid {
            *digit = match *digit {
                value if value == left => right,
                value if value == right => left,
                value => value,
            };
        }
        grid
    }

    fn checkpoint_from_pairs(pairs: Vec<GridPair>) -> Checkpoint {
        let edges = directed_edges();
        let checksum = pairs_checksum(&pairs);
        let mut checkpoint = empty_checkpoint();
        checkpoint.reserve_records(pairs.len()).unwrap();
        for pair in pairs {
            let pair_index = checkpoint.pairs.len();
            assert!(checkpoint.insert_pair(pair).unwrap());
            checkpoint
                .insert_cut(pair_cut(&pair, &edges), pair_index)
                .unwrap();
        }
        checkpoint.checksum = checksum;
        checkpoint
    }

    fn format_complete_model(assignment: &[bool]) -> String {
        let literals = (1..assignment.len())
            .map(|variable| {
                if assignment[variable] {
                    variable.to_string()
                } else {
                    format!("-{variable}")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("s SATISFIABLE\nv {literals} 0\n")
    }

    fn format_bridge_model(assignment: &[bool]) -> String {
        let literals = (1..assignment.len())
            .map(|variable| {
                if assignment[variable] {
                    variable.to_string()
                } else {
                    format!("-{variable}")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("MODEL {literals} 0")
    }

    fn temporary_path(suffix: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!(
            "thermo-topology-cnf-test-{}-{nonce}.{suffix}",
            std::process::id()
        ))
    }

    #[test]
    fn variable_ranges_are_contiguous_and_stable() {
        assert_eq!(digit_var(0, 0), 1);
        assert_eq!(digit_var(80, 8), 729);
        assert_eq!(edge_var(0), 730);
        assert_eq!(edge_var(543), 1273);
        assert_eq!(occupied_var(0), 1274);
        assert_eq!(occupied_var(80), 1354);
        assert_eq!(sequential_var(0, 0), 1355);
        assert_eq!(sequential_var(79, 18), 2874);
        assert_eq!(swap_var(0, 0), 2875);
        assert_eq!(swap_var(7, 543), VARIABLE_COUNT);
        assert_eq!(VARIABLE_COUNT, 7226);
        let edges = directed_edges();
        assert_eq!(edges_checksum(&edges), 0xf12501e5f1df08d5);
        assert_eq!(
            base_clauses(&edges, SymmetryBreak::None).len(),
            BASE_CLAUSE_COUNT
        );
    }

    #[test]
    fn exact_982_variable_ranges_and_clause_count_are_stable() {
        assert_eq!(EXACT_982_LABEL_BASE, 7227);
        assert_eq!(exact_982_label_var(2, 80), 7469);
        assert_eq!(EXACT_982_SOURCE_BASE, 7470);
        assert_eq!(exact_982_source_var(80), 7550);
        assert_eq!(EXACT_982_COUNTER_BASE, 7551);
        assert_eq!(EXACT_982_VARIABLE_COUNT, 9656);
        let edges = directed_edges();
        assert_eq!(
            base_clauses_for_scope(&edges, SymmetryBreak::None, TopologyScope::Exact982).len(),
            BASE_CLAUSE_COUNT + EXACT_982_EXTRA_CLAUSE_COUNT
        );
        assert_eq!(
            base_clauses_for_scope(
                &edges,
                SymmetryBreak::D4ComplementV1,
                TopologyScope::Exact982,
            )
            .len(),
            BASE_CLAUSE_COUNT + EXACT_982_EXTRA_CLAUSE_COUNT + 148
        );
    }

    #[test]
    fn exact_cardinality_counter_truth_table_is_exact() {
        let variables = [1, 2, 3, 4];
        let base = 5;
        let mut clauses = Vec::new();
        let next = exact_cardinality_clauses(&mut clauses, &variables, 2, base);
        assert_eq!(next, 17);
        let mut satisfiable_inputs = [false; 16];
        for bits in 0u32..(1 << 16) {
            let assignment = (0..=16)
                .map(|variable| variable != 0 && bits & (1 << (variable - 1)) != 0)
                .collect::<Vec<_>>();
            if clauses
                .iter()
                .all(|clause| clause_satisfied(clause, &assignment))
            {
                satisfiable_inputs[(bits & 0x0f) as usize] = true;
            }
        }
        for (inputs, &satisfiable) in satisfiable_inputs.iter().enumerate() {
            assert_eq!(satisfiable, inputs.count_ones() == 2, "inputs={inputs:04b}");
        }
    }

    #[test]
    fn known_three_solution_982_layout_satisfies_and_decodes_exact_master() {
        let paths = KNOWN_THREE_19
            .iter()
            .map(|path| path.to_vec())
            .collect::<Vec<_>>();
        let edges = directed_edges();
        let base = base_clauses_for_scope(&edges, SymmetryBreak::None, TopologyScope::Exact982);
        let assignment = assignment_for_exact_982(&paths, &edges);
        assert!(
            base.iter()
                .all(|clause| clause_satisfied(clause, &assignment))
        );
        let parsed = parse_sat_result(
            &format_complete_model(&assignment),
            EXACT_982_VARIABLE_COUNT,
        )
        .unwrap();
        assert_eq!(parsed.assignment.as_deref(), Some(assignment.as_slice()));
        assert_eq!(
            parse_bridge_model(
                &format_bridge_model(&assignment),
                EXACT_982_VARIABLE_COUNT as usize,
            )
            .unwrap(),
            assignment
        );
        let decoded = decode_candidate_with_scope_and_base(
            &[],
            &assignment,
            &edges,
            &base,
            TopologyScope::Exact982,
        )
        .unwrap();
        let mut lengths = decoded.paths.iter().map(Vec::len).collect::<Vec<_>>();
        lengths.sort_unstable();
        assert_eq!(lengths, [2, 8, 9]);
        assert_eq!(decoded.covered_cells, 19);
        assert_eq!(decoded.selected.len(), 16);
    }

    #[test]
    fn selected_mask_validation_matches_materialized_pair_clauses() {
        let mut state = 0xd1b5_4a32_d192_ed03u64;
        for _ in 0..10_000 {
            let mut assignment = vec![false; VARIABLE_COUNT as usize + 1];
            let mut cut = PairCut([0; DIRECTED_EDGES.div_ceil(64)]);
            for edge_id in 0..DIRECTED_EDGES {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                if state & 1 != 0 {
                    assignment[edge_var(edge_id) as usize] = true;
                }
                if state & 2 != 0 {
                    cut.0[edge_id / 64] |= 1u64 << (edge_id % 64);
                }
            }
            assert_eq!(
                pair_cut_satisfied(cut, selected_edge_mask(&assignment)),
                clause_satisfied(&pair_clause(cut), &assignment)
            );
        }
    }

    #[test]
    fn lazy_pool_scan_is_complete_bounded_and_deterministic() {
        let mut selected = PairCut([0; DIRECTED_EDGES.div_ceil(64)]);
        selected.0[0] |= 1;
        let cut = |edges: &[usize]| {
            let mut cut = PairCut([0; DIRECTED_EDGES.div_ceil(64)]);
            for &edge in edges {
                cut.0[edge / 64] |= 1u64 << (edge % 64);
            }
            cut
        };
        let cuts = vec![
            cut(&[0, 8]), // active and satisfied
            cut(&[0]),    // inactive but satisfied
            cut(&[1, 2]),
            cut(&[]), // empty clauses remain visible and rank first
            cut(&[3]),
            cut(&[3]), // duplicate fixture: pool IDs still tie-break stably
        ];
        let active = ActiveCutPool::from_indices(cuts.len(), vec![0]).unwrap();
        let (bounded, total) =
            violated_inactive_cut_indices(&cuts, &active, selected, Some(1)).unwrap();
        assert_eq!(total, 4);
        assert_eq!(bounded, vec![3]);
        let (all, total) = violated_inactive_cut_indices(&cuts, &active, selected, None).unwrap();
        assert_eq!(total, 4);
        assert_eq!(all, vec![3, 4, 5, 2]);
        assert!(pair_clause(cuts[3]).is_empty());
    }

    #[test]
    fn active_pool_extension_preserves_ids_and_rejects_mask_drift() {
        let mut active = ActiveCutPool::from_indices(2, vec![1]).unwrap();
        active.extend_pool(5).unwrap();
        assert_eq!(active.indices, vec![1]);
        assert_eq!(active.mask, vec![false, true, false, false, false]);
        active.activate(4).unwrap();
        assert!(active.validate(5).is_ok());
        active.mask[2] = true;
        assert!(active.validate(5).is_err());
    }

    #[test]
    fn structural_decoder_fails_hard_on_a_violated_active_cut() {
        let edges = directed_edges();
        let assignment = assignment_for_row_thermo(&edges);
        let base = base_clauses(&edges, SymmetryBreak::None);
        let empty = PairCut([0; DIRECTED_EDGES.div_ceil(64)]);
        let error = decode_candidate_with_base(&[empty], &assignment, &edges, &base).unwrap_err();
        assert!(error.contains("violates checkpoint pair-cut clause"));
    }

    #[test]
    fn active_manifest_round_trips_accepts_descendants_and_reemits_exact_cnf() {
        let first = parse_grid(CANONICAL).unwrap();
        let pair_one = GridPair::new(first, swap_symbols(first, 1, 2)).unwrap();
        let pair_two = GridPair::new(first, swap_symbols(first, 3, 4)).unwrap();
        let checkpoint = checkpoint_from_pairs(vec![pair_one]);
        let descendant = checkpoint_from_pairs(vec![pair_one, pair_two]);
        assert_eq!(checkpoint.cuts.len(), 1);
        assert_eq!(descendant.cuts.len(), 2);
        let active = ActiveCutPool::from_indices(checkpoint.cuts.len(), vec![0]).unwrap();
        let edges = directed_edges();

        for symmetry in [SymmetryBreak::None, SymmetryBreak::D4ComplementV1] {
            let manifest = temporary_path(&format!("active-{symmetry:?}"));
            let first_cnf = temporary_path(&format!("active-first-{symmetry:?}.cnf"));
            let second_cnf = temporary_path(&format!("active-second-{symmetry:?}.cnf"));
            write_active_cuts_manifest(
                &checkpoint,
                &active,
                &manifest,
                symmetry,
                TopologyScope::AtMost19,
            )
            .unwrap();
            let loaded = load_active_cuts_manifest(
                &manifest,
                &checkpoint,
                &edges,
                symmetry,
                TopologyScope::AtMost19,
            )
            .unwrap();
            assert_eq!(loaded, active);
            let loaded_from_descendant = load_active_cuts_manifest(
                &manifest,
                &descendant,
                &edges,
                symmetry,
                TopologyScope::AtMost19,
            )
            .unwrap();
            assert_eq!(loaded_from_descendant.indices, vec![0]);
            assert_eq!(loaded_from_descendant.mask, vec![true, false]);
            let (_, first_clauses) =
                write_lazy_cnf(&checkpoint, &loaded, &first_cnf, symmetry).unwrap();
            let (_, second_clauses) =
                write_lazy_cnf(&checkpoint, &loaded, &second_cnf, symmetry).unwrap();
            assert_eq!(
                first_clauses,
                BASE_CLAUSE_COUNT + symmetry.extra_clauses() + 1
            );
            assert_eq!(first_clauses, second_clauses);
            assert_eq!(
                fs::read(&first_cnf).unwrap(),
                fs::read(&second_cnf).unwrap()
            );
            assert!(
                load_active_cuts_manifest(
                    &manifest,
                    &checkpoint,
                    &edges,
                    if symmetry == SymmetryBreak::None {
                        SymmetryBreak::D4ComplementV1
                    } else {
                        SymmetryBreak::None
                    },
                    TopologyScope::AtMost19,
                )
                .is_err()
            );
            fs::remove_file(manifest).unwrap();
            fs::remove_file(first_cnf).unwrap();
            fs::remove_file(second_cnf).unwrap();
        }

        let ahead_manifest = temporary_path("active-ahead");
        let ahead = ActiveCutPool::from_indices(descendant.cuts.len(), vec![0, 1]).unwrap();
        write_active_cuts_manifest(
            &descendant,
            &ahead,
            &ahead_manifest,
            SymmetryBreak::None,
            TopologyScope::AtMost19,
        )
        .unwrap();
        assert!(
            load_active_cuts_manifest(
                &ahead_manifest,
                &checkpoint,
                &edges,
                SymmetryBreak::None,
                TopologyScope::AtMost19,
            )
            .is_err()
        );
        fs::remove_file(ahead_manifest).unwrap();
    }

    #[test]
    fn exact_scope_artifacts_are_bound_and_generic_cnf_bytes_are_unchanged() {
        let checkpoint = empty_checkpoint();
        let generic = temporary_path("generic-direct.cnf");
        let dispatched = temporary_path("generic-dispatched.cnf");
        let exact = temporary_path("exact-982.cnf");
        write_cnf(&checkpoint, &generic, SymmetryBreak::None).unwrap();
        write_cnf_for_scope(
            &checkpoint,
            &dispatched,
            SymmetryBreak::None,
            TopologyScope::AtMost19,
        )
        .unwrap();
        assert_eq!(fs::read(&generic).unwrap(), fs::read(&dispatched).unwrap());
        let (variables, clauses) = write_cnf_for_scope(
            &checkpoint,
            &exact,
            SymmetryBreak::None,
            TopologyScope::Exact982,
        )
        .unwrap();
        assert_eq!(variables, EXACT_982_VARIABLE_COUNT as usize);
        assert_eq!(clauses, BASE_CLAUSE_COUNT + EXACT_982_EXTRA_CLAUSE_COUNT);
        let exact_text = fs::read_to_string(&exact).unwrap();
        assert!(exact_text.starts_with(&format!("c {EXACT_982_CNF_SCHEMA}\n")));
        assert!(exact_text.contains("c topology_scope exact-9+8+2\n"));
        assert!(exact_text.contains("p cnf 9656 69959\n"));
        fs::remove_file(generic).unwrap();
        fs::remove_file(dispatched).unwrap();
        fs::remove_file(exact).unwrap();

        let first = parse_grid(CANONICAL).unwrap();
        let pair = GridPair::new(first, swap_symbols(first, 1, 2)).unwrap();
        let checkpoint = checkpoint_from_pairs(vec![pair]);
        let active = ActiveCutPool::from_indices(1, vec![0]).unwrap();
        let generic_manifest = temporary_path("generic.active");
        let exact_manifest = temporary_path("exact.active");
        write_active_cuts_manifest(
            &checkpoint,
            &active,
            &generic_manifest,
            SymmetryBreak::None,
            TopologyScope::AtMost19,
        )
        .unwrap();
        write_active_cuts_manifest(
            &checkpoint,
            &active,
            &exact_manifest,
            SymmetryBreak::None,
            TopologyScope::Exact982,
        )
        .unwrap();
        let generic_text = fs::read_to_string(&generic_manifest).unwrap();
        let exact_text = fs::read_to_string(&exact_manifest).unwrap();
        assert!(generic_text.starts_with(&format!("{ACTIVE_CUTS_HEADER}\n")));
        assert!(!generic_text.contains("# topology_scope="));
        assert!(exact_text.starts_with(&format!("{ACTIVE_CUTS_HEADER_V2}\n")));
        assert!(exact_text.contains("# topology_scope=exact-9+8+2\n"));
        let edges = directed_edges();
        assert!(
            load_active_cuts_manifest(
                &generic_manifest,
                &checkpoint,
                &edges,
                SymmetryBreak::None,
                TopologyScope::Exact982,
            )
            .is_err()
        );
        assert!(
            load_active_cuts_manifest(
                &exact_manifest,
                &checkpoint,
                &edges,
                SymmetryBreak::None,
                TopologyScope::AtMost19,
            )
            .is_err()
        );
        fs::remove_file(generic_manifest).unwrap();
        fs::remove_file(exact_manifest).unwrap();
    }

    #[test]
    fn deferred_lazy_persistence_has_only_consistent_restart_points() {
        let first = parse_grid(CANONICAL).unwrap();
        let pair_one = GridPair::new(first, swap_symbols(first, 1, 2)).unwrap();
        let pair_two = GridPair::new(first, swap_symbols(first, 3, 4)).unwrap();
        let durable = checkpoint_from_pairs(vec![pair_one]);
        let in_memory = checkpoint_from_pairs(vec![pair_one, pair_two]);
        assert_eq!(durable.cuts.len(), 1);
        assert_eq!(in_memory.cuts.len(), 2);

        let checkpoint_path = temporary_path("deferred.checkpoint");
        let manifest_path = temporary_path("deferred.active");
        let cnf_path = temporary_path("deferred.cnf");
        let expected_cnf_path = temporary_path("deferred-expected.cnf");
        let durable_active = ActiveCutPool::from_indices(durable.cuts.len(), vec![0]).unwrap();
        let in_memory_active =
            ActiveCutPool::from_indices(in_memory.cuts.len(), vec![0, 1]).unwrap();
        let edges = directed_edges();
        write_checkpoint(&durable, &checkpoint_path).unwrap();
        write_active_cuts_manifest(
            &durable,
            &durable_active,
            &manifest_path,
            SymmetryBreak::None,
            TopologyScope::AtMost19,
        )
        .unwrap();

        // A crash before the scheduled checkpoint sees the old checkpoint and
        // old manifest; the in-memory descendant and activations are simply
        // recomputed after restart.
        let restarted = load_checkpoint(&checkpoint_path).unwrap();
        assert_eq!(restarted.pairs, durable.pairs);
        assert_eq!(
            load_active_cuts_manifest(
                &manifest_path,
                &restarted,
                &edges,
                SymmetryBreak::None,
                TopologyScope::AtMost19,
            )
            .unwrap(),
            durable_active
        );

        // A crash between the ordered checkpoint and manifest replacement leaves
        // the old manifest behind. It remains a validated append-only prefix
        // of the newly durable checkpoint, never an ahead reference.
        write_checkpoint(&in_memory, &checkpoint_path).unwrap();
        let restarted = load_checkpoint(&checkpoint_path).unwrap();
        let prefix_active = load_active_cuts_manifest(
            &manifest_path,
            &restarted,
            &edges,
            SymmetryBreak::None,
            TopologyScope::AtMost19,
        )
        .unwrap();
        assert_eq!(prefix_active.indices, vec![0]);
        assert_eq!(prefix_active.mask, vec![true, false]);

        let manifest_lock = RunLock::acquire(&manifest_path, "test active manifest").unwrap();
        let lock_path = manifest_lock.path().to_path_buf();
        let lazy = LazyCutRuntime {
            options: LazyCutOptions {
                manifest: manifest_path.clone(),
                active_seed: 0,
                violation_batch: Some(1),
            },
            active: in_memory_active.clone(),
            _manifest_lock: manifest_lock,
        };
        let checkpoint_ms = persist_incremental_state(
            &in_memory,
            &checkpoint_path,
            Some(&lazy),
            &cnf_path,
            SymmetryBreak::None,
            TopologyScope::AtMost19,
            true,
            false,
        )
        .unwrap();
        assert!(checkpoint_ms.is_some());
        let restarted = load_checkpoint(&checkpoint_path).unwrap();
        assert_eq!(restarted.pairs, in_memory.pairs);
        assert_eq!(
            load_active_cuts_manifest(
                &manifest_path,
                &restarted,
                &edges,
                SymmetryBreak::None,
                TopologyScope::AtMost19,
            )
            .unwrap(),
            in_memory_active
        );

        // A terminal persistence immediately after a cadence-aligned write
        // still refreshes the manifest/CNF as requested but skips the already
        // durable, potentially very large checkpoint.
        let checkpoint_bytes = fs::read(&checkpoint_path).unwrap();
        let checkpoint_ms = persist_incremental_state(
            &in_memory,
            &checkpoint_path,
            Some(&lazy),
            &cnf_path,
            SymmetryBreak::None,
            TopologyScope::AtMost19,
            false,
            true,
        )
        .unwrap();
        assert!(checkpoint_ms.is_none());
        assert_eq!(fs::read(&checkpoint_path).unwrap(), checkpoint_bytes);
        write_lazy_cnf(
            &in_memory,
            &in_memory_active,
            &expected_cnf_path,
            SymmetryBreak::None,
        )
        .unwrap();
        assert_eq!(
            fs::read(&cnf_path).unwrap(),
            fs::read(&expected_cnf_path).unwrap()
        );

        drop(lazy);
        fs::remove_file(checkpoint_path).unwrap();
        fs::remove_file(manifest_path).unwrap();
        fs::remove_file(cnf_path).unwrap();
        fs::remove_file(expected_cnf_path).unwrap();
        fs::remove_file(lock_path).unwrap();
    }

    #[test]
    fn lazy_oracle_refinement_keeps_full_pool_but_activates_only_the_cap() {
        let paths = vec![(0u8..=8).collect::<Vec<_>>()];
        let solutions = Solver::blank(&paths).unwrap().enumerate_up_to(4).solutions;
        let edges = directed_edges();
        let selected = paths[0]
            .windows(2)
            .map(|step| edge_id(&edges, step[0] as usize, step[1] as usize))
            .collect::<Vec<_>>();
        let candidate = DecodedCandidate {
            target: solutions[0],
            selected,
            paths,
            covered_cells: 9,
        };
        let refinement = collect_refinement(
            &candidate,
            &solutions[1..],
            &edges,
            PairMode::All,
            &empty_checkpoint(),
        )
        .unwrap();
        assert!(refinement.cuts.len() > 1);
        let checkpoint = checkpoint_from_pairs(refinement.pairs.clone());
        assert_eq!(checkpoint.cuts, refinement.cuts);
        let mut active = ActiveCutPool::from_indices(checkpoint.cuts.len(), Vec::new()).unwrap();
        let mut selected_mask = PairCut([0; DIRECTED_EDGES.div_ceil(64)]);
        for &edge in &candidate.selected {
            selected_mask.0[edge / 64] |= 1u64 << (edge % 64);
        }
        let (batch, total) =
            violated_inactive_cut_indices(&checkpoint.cuts, &active, selected_mask, Some(1))
                .unwrap();
        assert_eq!(total, checkpoint.cuts.len());
        assert_eq!(batch.len(), 1);
        active.activate(batch[0]).unwrap();
        assert_eq!(active.indices.len(), 1);
        assert_eq!(checkpoint.cuts.len(), refinement.cuts.len());
        assert!(active.mask.iter().filter(|&&value| !value).count() > 0);
    }

    #[test]
    fn d4_complement_v1_is_orbit_complete_and_reverses_complemented_arcs() {
        let grid = parse_grid(CANONICAL).unwrap();
        let arcs = (0..8).map(|cell| (cell, cell + 1)).collect::<Vec<_>>();
        let edges = directed_edges();
        let symmetry_clauses = {
            let mut clauses = Vec::new();
            d4_complement_symmetry_clauses(&mut clauses);
            clauses
        };
        assert_eq!(symmetry_clauses.len(), 148);
        assert_eq!(
            base_clauses(&edges, SymmetryBreak::D4ComplementV1).len(),
            BASE_CLAUSE_COUNT + 148
        );

        let mut representatives = 0;
        for transform in 0..8 {
            for complement in [false, true] {
                let (transformed_grid, transformed_arcs) =
                    transform_candidate(&grid, &arcs, transform, complement);
                assert!(validate_sudoku(&transformed_grid));
                for &(lower, upper) in &transformed_arcs {
                    assert!(transformed_grid[lower] < transformed_grid[upper]);
                    assert!(
                        edges.iter().any(
                            |edge| edge.lower as usize == lower && edge.upper as usize == upper
                        )
                    );
                }
                let mut assignment = vec![false; VARIABLE_COUNT as usize + 1];
                for (cell, &digit) in transformed_grid.iter().enumerate() {
                    assignment[digit_var(cell, digit as usize - 1) as usize] = true;
                }
                if symmetry_clauses
                    .iter()
                    .all(|clause| clause_satisfied(clause, &assignment))
                {
                    representatives += 1;
                }
            }
        }
        assert!(representatives >= 1);
    }

    #[test]
    fn d4_complement_preserves_multi_path_geometry_and_multiplicity() {
        for (raw, covered, expected) in [
            (BLUE_20, 20, Multiplicity::Unique),
            (KNOWN_THREE_19, 19, Multiplicity::Multiple),
        ] {
            let paths = raw.iter().map(|path| path.to_vec()).collect::<Vec<_>>();
            for transform in 0..8 {
                for complement in [false, true] {
                    let transformed = transform_paths(&paths, transform, complement);
                    let solver = Solver::blank(&transformed).unwrap();
                    assert_eq!(solver.layout().covered_cells(), covered);
                    assert_eq!(solver.classify().multiplicity(), expected);
                }
            }
        }
    }

    #[test]
    fn valid_nine_cell_thermometer_satisfies_the_base_master() {
        let edges = directed_edges();
        let clauses = base_clauses(&edges, SymmetryBreak::None);
        let assignment = assignment_for_row_thermo(&edges);
        assert!(
            clauses
                .iter()
                .all(|clause| clause_true(clause, &assignment))
        );
    }

    #[test]
    fn complete_model_decodes_to_stable_target_and_path() {
        let edges = directed_edges();
        let assignment = assignment_for_row_thermo(&edges);
        let parsed = parse_sat_result(&format_complete_model(&assignment), VARIABLE_COUNT).unwrap();
        assert_eq!(parsed.status, SatStatus::Satisfiable);
        let decoded =
            decode_candidate(&empty_checkpoint(), parsed.assignment.as_deref().unwrap()).unwrap();
        assert_eq!(format_grid(&decoded.target), CANONICAL);
        assert_eq!(decoded.paths, vec![(0u8..=8).collect::<Vec<_>>()]);
        assert_eq!(decoded.covered_cells, 9);
        assert_eq!(decoded.selected.len(), 8);
    }

    #[test]
    fn model_parser_rejects_partial_and_conflicting_assignments() {
        assert!(parse_sat_result("s SATISFIABLE\nv 1 -2 0\n", VARIABLE_COUNT).is_err());
        assert!(parse_sat_result("s SATISFIABLE\nv 1 -1 0\n", VARIABLE_COUNT).is_err());
        assert!(parse_sat_result("s UNSATISFIABLE\nv 1 0\n", VARIABLE_COUNT).is_err());
    }

    #[test]
    fn bridge_ready_and_full_model_parsers_are_strict() {
        let ready = "READY thermo-cadical-bridge-v1 variables=7226 clauses=57400 cadical=cadical-2.1.3-f13d744 revision=f13d74439a5b5c963ac5b02d05ce93a8098018b8 library_sha256=6b97694f2c909a9de81eb7c130eccb9f7c41d57b3d66bf2cce5e851dea0518ed prefer_selected=1";
        let metadata = parse_bridge_ready(ready, VARIABLE_COUNT as usize, 57_400, true).unwrap();
        assert_eq!(metadata.cadical, "cadical-2.1.3-f13d744");
        assert_eq!(
            metadata.revision,
            "f13d74439a5b5c963ac5b02d05ce93a8098018b8"
        );
        assert!(metadata.prefer_selected);
        assert!(parse_bridge_ready(ready, VARIABLE_COUNT as usize, 57_401, true).is_err());
        assert!(parse_bridge_ready(ready, VARIABLE_COUNT as usize, 57_400, false).is_err());

        let assignment = assignment_for_row_thermo(&directed_edges());
        assert_eq!(
            parse_bridge_model(&format_bridge_model(&assignment), VARIABLE_COUNT as usize).unwrap(),
            assignment
        );
        assert!(parse_bridge_model("MODEL 1 -2 0", VARIABLE_COUNT as usize).is_err());
        let mut wrong_order = format_bridge_model(&assignment);
        wrong_order = wrong_order.replacen("MODEL 1 ", "MODEL 2 ", 1);
        assert!(parse_bridge_model(&wrong_order, VARIABLE_COUNT as usize).is_err());
    }

    #[test]
    fn incremental_pair_append_matches_a_fresh_cnf_byte_for_byte() {
        let first = parse_grid(CANONICAL).unwrap();
        let mut second = first;
        for digit in &mut second {
            *digit = match *digit {
                1 => 2,
                2 => 1,
                value => value,
            };
        }
        let pair = GridPair::new(first, second).unwrap();
        let before = empty_checkpoint();
        let mut after = empty_checkpoint();
        after.pairs.push(pair);
        after.cuts.push(pair_cut(&pair, &directed_edges()));
        after.checksum = pairs_checksum(&after.pairs);
        let appended = temporary_path("appended.cnf");
        let fresh = temporary_path("fresh.cnf");
        write_cnf(&before, &appended, SymmetryBreak::None).unwrap();
        append_pair_to_cnf(
            &appended,
            before.pairs.len(),
            before.cuts.len(),
            before.checksum,
            &after,
            &pair,
            SymmetryBreak::None,
            TopologyScope::AtMost19,
        )
        .unwrap();
        write_cnf(&after, &fresh, SymmetryBreak::None).unwrap();
        assert_eq!(fs::read(&appended).unwrap(), fs::read(&fresh).unwrap());
        fs::remove_file(appended).unwrap();
        fs::remove_file(fresh).unwrap();
    }

    #[test]
    fn batched_incremental_append_matches_a_fresh_cnf_byte_for_byte() {
        let first = parse_grid(CANONICAL).unwrap();
        let mut second = first;
        let mut third = first;
        for digit in &mut second {
            *digit = match *digit {
                1 => 2,
                2 => 1,
                value => value,
            };
        }
        for digit in &mut third {
            *digit = match *digit {
                3 => 4,
                4 => 3,
                value => value,
            };
        }
        let pairs = [
            GridPair::new(first, second).unwrap(),
            GridPair::new(first, third).unwrap(),
        ];
        let before = empty_checkpoint();
        let mut after = empty_checkpoint();
        after.pairs.extend(pairs);
        let edges = directed_edges();
        after
            .cuts
            .extend(pairs.iter().map(|pair| pair_cut(pair, &edges)));
        assert_ne!(after.cuts[0], after.cuts[1]);
        after.checksum = pairs_checksum(&after.pairs);
        let appended = temporary_path("batch-appended.cnf");
        let fresh = temporary_path("batch-fresh.cnf");
        write_cnf(&before, &appended, SymmetryBreak::None).unwrap();
        append_refinement_to_cnf(
            &appended,
            before.pairs.len(),
            before.cuts.len(),
            before.checksum,
            &after,
            &after.cuts,
            SymmetryBreak::None,
            TopologyScope::AtMost19,
        )
        .unwrap();
        write_cnf(&after, &fresh, SymmetryBreak::None).unwrap();
        assert_eq!(fs::read(&appended).unwrap(), fs::read(&fresh).unwrap());
        fs::remove_file(appended).unwrap();
        fs::remove_file(fresh).unwrap();
    }

    #[test]
    fn exact_982_incremental_append_matches_a_fresh_cnf_byte_for_byte() {
        let first = parse_grid(CANONICAL).unwrap();
        let pair = GridPair::new(first, swap_symbols(first, 1, 2)).unwrap();
        let before = empty_checkpoint();
        let mut after = empty_checkpoint();
        after.pairs.push(pair);
        after.cuts.push(pair_cut(&pair, &directed_edges()));
        after.checksum = pairs_checksum(&after.pairs);
        let appended = temporary_path("exact-982-appended.cnf");
        let fresh = temporary_path("exact-982-fresh.cnf");
        write_cnf_for_scope(
            &before,
            &appended,
            SymmetryBreak::None,
            TopologyScope::Exact982,
        )
        .unwrap();
        append_refinement_to_cnf(
            &appended,
            0,
            0,
            before.checksum,
            &after,
            &after.cuts,
            SymmetryBreak::None,
            TopologyScope::Exact982,
        )
        .unwrap();
        write_cnf_for_scope(&after, &fresh, SymmetryBreak::None, TopologyScope::Exact982).unwrap();
        assert_eq!(fs::read(&appended).unwrap(), fs::read(&fresh).unwrap());
        fs::remove_file(appended).unwrap();
        fs::remove_file(fresh).unwrap();
    }

    #[test]
    fn symmetry_broken_incremental_append_matches_fresh_cnf_byte_for_byte() {
        let first = parse_grid(CANONICAL).unwrap();
        let mut second = first;
        for digit in &mut second {
            *digit = match *digit {
                1 => 2,
                2 => 1,
                value => value,
            };
        }
        let pair = GridPair::new(first, second).unwrap();
        let before = empty_checkpoint();
        let mut after = empty_checkpoint();
        after.pairs.push(pair);
        after.cuts.push(pair_cut(&pair, &directed_edges()));
        after.checksum = pairs_checksum(&after.pairs);
        let appended = temporary_path("symmetry-appended.cnf");
        let fresh = temporary_path("symmetry-fresh.cnf");
        write_cnf(&before, &appended, SymmetryBreak::D4ComplementV1).unwrap();
        append_refinement_to_cnf(
            &appended,
            0,
            0,
            before.checksum,
            &after,
            &after.cuts,
            SymmetryBreak::D4ComplementV1,
            TopologyScope::AtMost19,
        )
        .unwrap();
        write_cnf(&after, &fresh, SymmetryBreak::D4ComplementV1).unwrap();
        let appended_bytes = fs::read(&appended).unwrap();
        assert!(String::from_utf8_lossy(&appended_bytes).starts_with("c thermo-topology-cnf-v2\n"));
        assert!(
            String::from_utf8_lossy(&appended_bytes)
                .contains("c symmetry_break d4-complement-v1\n")
        );
        assert_eq!(appended_bytes, fs::read(&fresh).unwrap());
        fs::remove_file(appended).unwrap();
        fs::remove_file(fresh).unwrap();
    }

    #[test]
    fn checkpoint_replace_is_atomic_and_reloadable() {
        let path = temporary_path("checkpoint");
        let before = empty_checkpoint();
        write_checkpoint(&before, &path).unwrap();
        assert_eq!(load_checkpoint(&path).unwrap().pairs.len(), 0);

        let first = parse_grid(CANONICAL).unwrap();
        let mut second = first;
        for digit in &mut second {
            *digit = match *digit {
                1 => 2,
                2 => 1,
                value => value,
            };
        }
        let pair = GridPair::new(first, second).unwrap();
        let mut after = empty_checkpoint();
        after.pairs.push(pair);
        after.cuts.push(pair_cut(&pair, &directed_edges()));
        after.checksum = pairs_checksum(&after.pairs);
        write_checkpoint(&after, &path).unwrap();
        let loaded = load_checkpoint(&path).unwrap();
        assert_eq!(loaded.pairs, after.pairs);
        assert_eq!(loaded.checksum, after.checksum);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn packed_grid_round_trips_preserves_order_and_has_expected_size() {
        let first = parse_grid(CANONICAL).unwrap();
        let second = swap_symbols(first, 1, 2);
        let packed_first = PackedGrid::new(first);
        let packed_second = PackedGrid::new(second);
        assert_eq!(packed_first.unpack(), first);
        assert_eq!(packed_second.unpack(), second);
        assert_eq!(packed_first.cmp(&packed_second), first.cmp(&second));
        assert_eq!(std::mem::size_of::<PackedGrid>(), 41);
        assert_eq!(std::mem::size_of::<GridPair>(), 82);
        assert_eq!(std::mem::size_of::<PairCut>(), 72);
    }

    #[test]
    fn flat_index_resolves_hash_bucket_collisions_by_full_equality() {
        let mut index = FlatIndex::new();
        let mut values = Vec::<u64>::new();
        index.reserve(&values, 4).unwrap();
        let bucket = index.hash(&0u64) as usize & (index.slots.len() - 1);
        let colliding = (0u64..10_000)
            .filter(|value| index.hash(value) as usize & (index.slots.len() - 1) == bucket)
            .take(4)
            .collect::<Vec<_>>();
        assert_eq!(colliding.len(), 4);
        for value in colliding.iter().copied() {
            assert!(indexed_insert(&mut values, &mut index, value).unwrap());
        }
        for value in &colliding {
            assert!(index.contains(&values, value));
            assert!(!indexed_insert(&mut values, &mut index, *value).unwrap());
        }
        assert_eq!(values, colliding);
    }

    #[test]
    fn eager_refinement_reserve_is_exact_for_standard_run_and_capped_for_huge_run() {
        assert_eq!(
            eager_refinement_reserve(1_000, 64, PairMode::All).unwrap(),
            2_080_000
        );
        assert_eq!(
            eager_refinement_reserve(100, 1_024, PairMode::All).unwrap(),
            MAX_EAGER_REFINEMENT_RESERVE
        );
        assert_eq!(
            eager_refinement_reserve(1_000, 64, PairMode::Anchor).unwrap(),
            64_000
        );
    }

    #[test]
    fn packed_streaming_checkpoint_reemits_identical_bytes_and_first_witnesses() {
        let first = parse_grid(CANONICAL).unwrap();
        let pair_one = GridPair::new(first, swap_symbols(first, 1, 2)).unwrap();
        let pair_two = GridPair::new(first, swap_symbols(first, 3, 4)).unwrap();
        let checkpoint = checkpoint_from_pairs(vec![pair_one, pair_two]);
        let first_path = temporary_path("packed-stream-first.checkpoint");
        let second_path = temporary_path("packed-stream-second.checkpoint");
        write_checkpoint(&checkpoint, &first_path).unwrap();
        let bytes = fs::read(&first_path).unwrap();
        let loaded = load_checkpoint_with_reserve(&first_path, 17).unwrap();
        assert_eq!(loaded.checksum, checkpoint.checksum);
        assert_eq!(loaded.pairs, checkpoint.pairs);
        assert_eq!(loaded.cuts, checkpoint.cuts);
        assert_eq!(loaded.cut_witnesses, checkpoint.cut_witnesses);
        write_checkpoint(&loaded, &second_path).unwrap();
        assert_eq!(fs::read(&second_path).unwrap(), bytes);
        fs::remove_file(first_path).unwrap();
        fs::remove_file(second_path).unwrap();

        let mut witnesses = empty_checkpoint();
        witnesses.reserve_records(2).unwrap();
        assert!(witnesses.insert_pair(pair_one).unwrap());
        assert!(witnesses.insert_pair(pair_two).unwrap());
        let cut = pair_cut(&pair_one, &directed_edges());
        assert!(witnesses.insert_cut(cut, 0).unwrap());
        assert!(!witnesses.insert_cut(cut, 1).unwrap());
        assert_eq!(witnesses.cut_witnesses, vec![0]);
        let distinct_cut = pair_cut(&pair_two, &directed_edges());
        assert_ne!(cut, distinct_cut);
        assert!(witnesses.insert_cut(distinct_cut, 0).is_err());
    }

    #[test]
    fn checkpoint_merge_preserves_base_prefix_and_first_witnesses() {
        let first = parse_grid(CANONICAL).unwrap();
        let pair_one = GridPair::new(first, swap_symbols(first, 1, 2)).unwrap();
        let pair_two = GridPair::new(first, swap_symbols(first, 3, 4)).unwrap();
        let pair_three = GridPair::new(first, swap_symbols(first, 5, 6)).unwrap();
        let mut destination = checkpoint_from_pairs(vec![pair_one]);
        let source = checkpoint_from_pairs(vec![pair_one, pair_two, pair_three]);
        let prefix_pairs = destination.pairs.clone();
        let prefix_cuts = destination.cuts.clone();
        let prefix_witnesses = destination.cut_witnesses.clone();
        let stats = merge_checkpoint_data(&mut destination, &source, &directed_edges()).unwrap();

        assert_eq!(stats.input_pairs, 3);
        assert_eq!(stats.added_pairs, 2);
        assert_eq!(stats.duplicate_pairs, 1);
        assert_eq!(&destination.pairs[..prefix_pairs.len()], &prefix_pairs);
        assert_eq!(&destination.cuts[..prefix_cuts.len()], &prefix_cuts);
        assert_eq!(
            &destination.cut_witnesses[..prefix_witnesses.len()],
            &prefix_witnesses
        );
        assert_eq!(destination.pairs, vec![pair_one, pair_two, pair_three]);
        assert_eq!(destination.checksum, pairs_checksum(&destination.pairs));
        assert!(
            destination
                .cut_witnesses
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );

        let mut wrong_budget = source;
        wrong_budget.budget = 15;
        assert!(merge_checkpoint_data(&mut destination, &wrong_budget, &directed_edges()).is_err());
    }

    #[test]
    fn checkpoint_loader_rejects_duplicate_pair_records() {
        let path = temporary_path("duplicate-pair.checkpoint");
        let first = parse_grid(CANONICAL).unwrap();
        let mut second = first;
        for digit in &mut second {
            *digit = match *digit {
                1 => 2,
                2 => 1,
                value => value,
            };
        }
        let pair = GridPair::new(first, second).unwrap();
        let mut checkpoint = empty_checkpoint();
        checkpoint.pairs.extend([pair, pair]);
        checkpoint.cuts.push(pair_cut(&pair, &directed_edges()));
        checkpoint.checksum = pairs_checksum(&checkpoint.pairs);
        write_checkpoint(&checkpoint, &path).unwrap();
        assert!(load_checkpoint(&path).unwrap_err().contains("duplicate"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn checkpoint_loader_rejects_impossible_declared_count_before_reserving() {
        let path = temporary_path("impossible-count.checkpoint");
        let first = parse_grid(CANONICAL).unwrap();
        let pair = GridPair::new(first, swap_symbols(first, 1, 2)).unwrap();
        write_checkpoint(&checkpoint_from_pairs(vec![pair]), &path).unwrap();
        let text =
            fs::read_to_string(&path)
                .unwrap()
                .replacen("# pairs=1\n", "# pairs=4000000000\n", 1);
        fs::write(&path, text).unwrap();
        let error = load_checkpoint_with_reserve(&path, usize::MAX).unwrap_err();
        assert!(error.contains("incompatible with checkpoint file size"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn all_pair_refinement_is_complete_and_deduplicated() {
        let paths = vec![(0u8..=8).collect::<Vec<_>>()];
        let solutions = Solver::blank(&paths).unwrap().enumerate_up_to(3).solutions;
        assert_eq!(solutions.len(), 3);
        let edges = directed_edges();
        let selected = paths[0]
            .windows(2)
            .map(|step| edge_id(&edges, step[0] as usize, step[1] as usize))
            .collect::<Vec<_>>();
        let candidate = DecodedCandidate {
            target: solutions[0],
            selected,
            paths,
            covered_cells: 9,
        };
        let empty = empty_checkpoint();
        let all =
            collect_refinement(&candidate, &solutions[1..], &edges, PairMode::All, &empty).unwrap();
        assert_eq!(all.pairs.len(), 3);
        assert_eq!(all.cuts.len(), 3);

        let seen = checkpoint_from_pairs(all.pairs.clone());
        let duplicate =
            collect_refinement(&candidate, &solutions[1..], &edges, PairMode::All, &seen).unwrap();
        assert!(duplicate.pairs.is_empty());
        assert!(duplicate.cuts.is_empty());

        let anchor = collect_refinement(
            &candidate,
            &solutions[1..],
            &edges,
            PairMode::Anchor,
            &empty_checkpoint(),
        )
        .unwrap();
        assert_eq!(anchor.pairs.len(), 2);
        assert_eq!(anchor.cuts.len(), 2);
    }

    #[test]
    fn batch_selection_handles_target_position_cap_and_duplicates() {
        let paths = vec![(0u8..=8).collect::<Vec<_>>()];
        let solutions = Solver::blank(&paths).unwrap().enumerate_up_to(4).solutions;
        assert_eq!(solutions.len(), 4);

        let target_in_middle = solutions[1];
        assert_eq!(
            select_oracle_alternatives(&solutions[0], &solutions[..3], true, 2).unwrap(),
            solutions[1..3]
        );
        let alternatives =
            select_oracle_alternatives(&target_in_middle, &solutions[..3], true, 2).unwrap();
        assert_eq!(alternatives, vec![solutions[0], solutions[2]]);

        let target_after_capped_prefix = solutions[3];
        let alternatives =
            select_oracle_alternatives(&target_after_capped_prefix, &solutions[..3], false, 2)
                .unwrap();
        assert_eq!(alternatives, solutions[..2]);
        assert!(
            select_oracle_alternatives(&target_after_capped_prefix, &solutions[..3], true, 2,)
                .is_err()
        );
        assert!(
            select_oracle_alternatives(&target_in_middle, &[target_in_middle], false, 2).is_err()
        );

        let edges = directed_edges();
        let selected = paths[0]
            .windows(2)
            .map(|step| edge_id(&edges, step[0] as usize, step[1] as usize))
            .collect::<Vec<_>>();
        let candidate = DecodedCandidate {
            target: target_in_middle,
            selected,
            paths,
            covered_cells: 9,
        };
        assert!(validate_oracle_batch(&candidate, &solutions[..3], &edges).is_ok());
        assert!(
            validate_oracle_batch(&candidate, &[solutions[0], solutions[0]], &edges)
                .unwrap_err()
                .contains("duplicate")
        );
    }

    #[test]
    fn error_path_flushes_dirty_checkpoint() {
        let path = temporary_path("error-flush.checkpoint");
        let first = parse_grid(CANONICAL).unwrap();
        let mut second = first;
        for digit in &mut second {
            *digit = match *digit {
                1 => 2,
                2 => 1,
                value => value,
            };
        }
        let pair = GridPair::new(first, second).unwrap();
        let mut checkpoint = empty_checkpoint();
        checkpoint.pairs.push(pair);
        checkpoint.cuts.push(pair_cut(&pair, &directed_edges()));
        checkpoint.checksum = pairs_checksum(&checkpoint.pairs);
        let cnf = temporary_path("error-flush.cnf");
        let message = preserve_incremental_progress_error(
            &checkpoint,
            &path,
            true,
            None,
            &cnf,
            SymmetryBreak::None,
            TopologyScope::AtMost19,
            "bridge failed".into(),
        );
        assert!(message.contains("state was saved"));
        let loaded = load_checkpoint(&path).unwrap();
        assert_eq!(loaded.pairs, checkpoint.pairs);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn checkpoint_cadence_is_counted_in_refinement_batches() {
        assert!(!checkpoint_due(0, 3));
        assert!(!checkpoint_due(2, 3));
        assert!(checkpoint_due(3, 3));
        assert!(checkpoint_due(4, 3));
        assert!(checkpoint_due(1, 1));
    }

    #[test]
    fn run_lock_rejects_a_second_writer_and_releases_on_drop() {
        let target = temporary_path("locked.checkpoint");
        let first = RunLock::acquire(&target, "test").unwrap();
        let lock_path = first.path().to_path_buf();
        assert!(RunLock::acquire(&target, "test").is_err());
        drop(first);
        let second = RunLock::acquire(&target, "test").unwrap();
        drop(second);
        fs::remove_file(lock_path).unwrap();
    }

    #[test]
    fn topology_rejects_a_branch() {
        let edges = directed_edges();
        let mut clauses = Vec::new();
        topology_clauses(&mut clauses, &edges);
        let mut assignment = assignment_for_row_thermo(&edges);
        let branch = edge_id(&edges, 0, 9);
        assignment[edge_var(branch) as usize] = true;
        assignment[occupied_var(9) as usize] = true;
        assert!(
            clauses
                .iter()
                .any(|clause| !clause_true(clause, &assignment))
        );
    }

    #[test]
    fn coverage_counter_rejects_twenty_occupied_cells() {
        let mut clauses = Vec::new();
        coverage_clauses(&mut clauses);
        let mut assignment = vec![false; VARIABLE_COUNT as usize + 1];
        for cell in 0..20 {
            assignment[occupied_var(cell) as usize] = true;
        }
        for prefix in 0..CELLS - 1 {
            let occupied = (prefix + 1).min(20);
            for count in 0..COVER_LIMIT {
                assignment[sequential_var(prefix, count) as usize] = occupied > count;
            }
        }
        assert!(
            clauses
                .iter()
                .any(|clause| !clause_true(clause, &assignment))
        );
    }

    #[test]
    fn swap_pair_cut_requires_the_corresponding_consecutive_symbols() {
        let edges = directed_edges();
        let first = parse_grid(CANONICAL).unwrap();
        let mut second = first;
        for digit in &mut second {
            *digit = match *digit {
                1 => 2,
                2 => 1,
                value => value,
            };
        }
        let pair = GridPair::new(first, second).unwrap();
        let cut = pair_clause(pair_cut(&pair, &edges));
        let under_first = cut
            .into_iter()
            .map(|variable| (variable - EDGE_BASE) as usize)
            .filter(|&edge_id| {
                let edge = edges[edge_id];
                first[edge.lower as usize] < first[edge.upper as usize]
            })
            .collect::<Vec<_>>();
        assert!(!under_first.is_empty());
        assert!(under_first.iter().all(|&edge_id| {
            let edge = edges[edge_id];
            first[edge.lower as usize] == 1 && first[edge.upper as usize] == 2
        }));
    }
}
