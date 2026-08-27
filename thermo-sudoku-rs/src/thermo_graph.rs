//! Deterministic graph normal forms used by the constructive 18-cell searches.
//!
//! A state is a Sudoku solution target plus an 18-cell footprint. Saturation
//! adds every target-true unequal king-neighbour comparison. The comparison DAG
//! is represented by its unique transitive reduction and canonicalized under
//! the eight square symmetries and optional digit complement/global edge
//! reversal.

pub const CELLS: usize = 81;

pub type Grid = [u8; CELLS];
pub type Edge = (u8, u8);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Transform {
    pub spatial: u8,
    pub complement: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalState {
    pub full_edges: Vec<Edge>,
    pub hasse_edges: Vec<Edge>,
    pub target: Grid,
    pub cells: Vec<u8>,
    pub transform: Transform,
}

/// Validate an 18-cell footprint and complete Sudoku target, saturate the
/// footprint, prove exact incident coverage and Hasse-closure preservation,
/// and return the canonical D4/complement representative.
///
/// This is the trust boundary for constructive search inputs. The lower-level
/// transform and saturation helpers remain public for verification and tests,
/// but assume their cell and grid inputs are already valid.
pub fn canonical_saturated_state(
    footprint: &[u8],
    target: &Grid,
) -> Result<CanonicalState, String> {
    if footprint.len() != 18 {
        return Err(format!(
            "footprint has {} cells; expected exactly 18",
            footprint.len()
        ));
    }
    let mut cells = footprint.to_vec();
    if let Some(&cell) = cells.iter().find(|&&cell| cell >= CELLS as u8) {
        return Err(format!("footprint cell {cell} is outside 0..=80"));
    }
    cells.sort_unstable();
    if let Some(pair) = cells.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(format!("footprint repeats cell {}", pair[0]));
    }
    validate_complete_sudoku(target)?;

    let full_edges = saturate_target(&cells, target);
    if incident_cells(&full_edges) != cells {
        return Err("saturated graph does not cover every footprint cell".to_owned());
    }
    let hasse_edges = transitive_reduction(&full_edges)?;
    if incident_cells(&hasse_edges) != cells {
        return Err("Hasse reduction lost an incident footprint cell".to_owned());
    }
    if transitive_closure_edges(&hasse_edges)? != transitive_closure_edges(&full_edges)? {
        return Err("Hasse reduction changed the comparison closure".to_owned());
    }
    Ok(canonicalize_state(
        &full_edges,
        &hasse_edges,
        target,
        &cells,
    ))
}

pub fn validate_complete_sudoku(grid: &Grid) -> Result<(), String> {
    let mut houses = [[false; 10]; 27];
    for (cell, &digit) in grid.iter().enumerate() {
        if !(1..=9).contains(&digit) {
            return Err(format!(
                "target cell {cell} has digit {digit}; expected 1..=9"
            ));
        }
        let row = cell / 9;
        let column = cell % 9;
        let box_index = (row / 3) * 3 + column / 3;
        for house in [row, 9 + column, 18 + box_index] {
            if houses[house][digit as usize] {
                return Err(format!(
                    "target repeats digit {digit} in Sudoku house {house}"
                ));
            }
            houses[house][digit as usize] = true;
        }
    }
    Ok(())
}

pub fn transform_cell(cell: u8, spatial: u8) -> u8 {
    let row = cell / 9;
    let column = cell % 9;
    let (next_row, next_column) = match spatial {
        0 => (row, column),
        1 => (column, 8 - row),
        2 => (8 - row, 8 - column),
        3 => (8 - column, row),
        4 => (row, 8 - column),
        5 => (8 - column, 8 - row),
        6 => (8 - row, column),
        7 => (column, row),
        _ => unreachable!("D4 spatial transform is in 0..8"),
    };
    next_row * 9 + next_column
}

pub fn transform_grid(grid: &Grid, transform: Transform) -> Grid {
    let mut result = [0u8; CELLS];
    for (cell, &digit) in grid.iter().enumerate() {
        result[transform_cell(cell as u8, transform.spatial) as usize] = if transform.complement {
            10 - digit
        } else {
            digit
        };
    }
    result
}

pub fn transform_edges(edges: &[Edge], transform: Transform) -> Vec<Edge> {
    let mut result = edges
        .iter()
        .map(|&(lower, upper)| {
            let lower = transform_cell(lower, transform.spatial);
            let upper = transform_cell(upper, transform.spatial);
            if transform.complement {
                (upper, lower)
            } else {
                (lower, upper)
            }
        })
        .collect::<Vec<_>>();
    result.sort_unstable();
    result.dedup();
    result
}

pub fn canonicalize_state(
    full_edges: &[Edge],
    hasse_edges: &[Edge],
    target: &Grid,
    cells: &[u8],
) -> CanonicalState {
    let mut best: Option<CanonicalState> = None;
    for complement in [false, true] {
        for spatial in 0..8 {
            let transform = Transform {
                spatial,
                complement,
            };
            let mut transformed_cells = cells
                .iter()
                .map(|&cell| transform_cell(cell, spatial))
                .collect::<Vec<_>>();
            transformed_cells.sort_unstable();
            let candidate = CanonicalState {
                full_edges: transform_edges(full_edges, transform),
                hasse_edges: transform_edges(hasse_edges, transform),
                target: transform_grid(target, transform),
                cells: transformed_cells,
                transform,
            };
            let replace = best.as_ref().is_none_or(|current| {
                (
                    &candidate.hasse_edges,
                    &candidate.target,
                    &candidate.full_edges,
                    &candidate.cells,
                ) < (
                    &current.hasse_edges,
                    &current.target,
                    &current.full_edges,
                    &current.cells,
                )
            });
            if replace {
                best = Some(candidate);
            }
        }
    }
    best.expect("the D4/complement orbit is nonempty")
}

pub fn king_adjacent(left: u8, right: u8) -> bool {
    if left == right {
        return false;
    }
    let left_row = left / 9;
    let left_column = left % 9;
    let right_row = right / 9;
    let right_column = right % 9;
    left_row.abs_diff(right_row) <= 1 && left_column.abs_diff(right_column) <= 1
}

pub fn saturate_target(footprint: &[u8], target: &Grid) -> Vec<Edge> {
    let mut edges = Vec::new();
    for (left_index, &left) in footprint.iter().enumerate() {
        for &right in &footprint[left_index + 1..] {
            if !king_adjacent(left, right) {
                continue;
            }
            let left_digit = target[left as usize];
            let right_digit = target[right as usize];
            if left_digit < right_digit {
                edges.push((left, right));
            } else if right_digit < left_digit {
                edges.push((right, left));
            }
        }
    }
    edges.sort_unstable();
    edges.dedup();
    edges
}

pub fn incident_cells(edges: &[Edge]) -> Vec<u8> {
    let mut incident = [false; CELLS];
    for &(lower, upper) in edges {
        incident[lower as usize] = true;
        incident[upper as usize] = true;
    }
    incident
        .into_iter()
        .enumerate()
        .filter_map(|(cell, value)| value.then_some(cell as u8))
        .collect()
}

pub fn grid_satisfies_edges(grid: &Grid, edges: &[Edge]) -> bool {
    edges
        .iter()
        .all(|&(lower, upper)| grid[lower as usize] < grid[upper as usize])
}

fn dag_reach(edges: &[Edge]) -> Result<([u128; CELLS], Vec<u8>), String> {
    let mut direct = [0u128; CELLS];
    let mut indegree = [0u8; CELLS];
    let mut active = 0u128;
    for &(lower, upper) in edges {
        if lower >= CELLS as u8 || upper >= CELLS as u8 {
            return Err(format!(
                "comparison {lower}->{upper} has an out-of-range cell"
            ));
        }
        let bit = 1u128 << upper;
        if direct[lower as usize] & bit == 0 {
            direct[lower as usize] |= bit;
            indegree[upper as usize] = indegree[upper as usize]
                .checked_add(1)
                .ok_or("comparison indegree overflow")?;
        }
        active |= (1u128 << lower) | (1u128 << upper);
    }
    let mut ready = active;
    for (cell, &degree) in indegree.iter().enumerate() {
        if degree != 0 {
            ready &= !(1u128 << cell);
        }
    }
    let mut topological = Vec::with_capacity(active.count_ones() as usize);
    while ready != 0 {
        let lower = ready.trailing_zeros() as usize;
        ready &= ready - 1;
        topological.push(lower as u8);
        let mut uppers = direct[lower];
        while uppers != 0 {
            let upper = uppers.trailing_zeros() as usize;
            uppers &= uppers - 1;
            indegree[upper] -= 1;
            if indegree[upper] == 0 {
                ready |= 1u128 << upper;
            }
        }
    }
    if topological.len() != active.count_ones() as usize {
        return Err("comparison graph contains a directed cycle".to_owned());
    }
    let mut reach = [0u128; CELLS];
    for &lower in topological.iter().rev() {
        let mut uppers = direct[lower as usize];
        while uppers != 0 {
            let upper = uppers.trailing_zeros() as usize;
            uppers &= uppers - 1;
            reach[lower as usize] |= (1u128 << upper) | reach[upper];
        }
    }
    Ok((reach, topological))
}

pub fn transitive_reduction(edges: &[Edge]) -> Result<Vec<Edge>, String> {
    let (reach, _) = dag_reach(edges)?;
    let mut direct = [0u128; CELLS];
    for &(lower, upper) in edges {
        direct[lower as usize] |= 1u128 << upper;
    }
    let mut reduced = Vec::new();
    for &(lower, upper) in edges {
        let upper_bit = 1u128 << upper;
        let mut alternatives = direct[lower as usize] & !upper_bit;
        let mut redundant = false;
        while alternatives != 0 {
            let alternative = alternatives.trailing_zeros() as usize;
            alternatives &= alternatives - 1;
            if reach[alternative] & upper_bit != 0 {
                redundant = true;
                break;
            }
        }
        if !redundant {
            reduced.push((lower, upper));
        }
    }
    reduced.sort_unstable();
    reduced.dedup();
    Ok(reduced)
}

pub fn transitive_closure_edges(edges: &[Edge]) -> Result<Vec<Edge>, String> {
    let (reach, _) = dag_reach(edges)?;
    let mut result = Vec::new();
    for (lower, &uppers) in reach.iter().enumerate() {
        let mut remaining = uppers;
        while remaining != 0 {
            let upper = remaining.trailing_zeros() as u8;
            remaining &= remaining - 1;
            result.push((lower as u8, upper));
        }
    }
    Ok(result)
}

pub fn encode_edges(prefix: &[u8], edges: &[Edge]) -> Vec<u8> {
    let mut bytes = prefix.to_vec();
    bytes.extend_from_slice(&(edges.len() as u16).to_be_bytes());
    for &(lower, upper) in edges {
        bytes.push(lower);
        bytes.push(upper);
    }
    bytes
}

pub fn network_sha256(edges: &[Edge]) -> String {
    sha256_hex(&encode_edges(b"thermo-18c-network-v1\0", edges))
}

pub fn state_sha256(edges: &[Edge], target: &Grid) -> String {
    let mut bytes = encode_edges(b"thermo-18c-state-v1\0", edges);
    bytes.extend_from_slice(target);
    sha256_hex(&bytes)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    sha256(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let bit_length = (bytes.len() as u64).wrapping_mul(8);
    let padded_length = (bytes.len() + 1 + 8).div_ceil(64) * 64;
    let mut padded = Vec::with_capacity(padded_length);
    padded.extend_from_slice(bytes);
    padded.push(0x80);
    padded.resize(padded_length - 8, 0);
    padded.extend_from_slice(&bit_length.to_be_bytes());

    let mut state = INITIAL;
    for chunk in padded.chunks_exact(64) {
        let mut words = [0u32; 64];
        for (index, word) in words[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes([
                chunk[index * 4],
                chunk[index * 4 + 1],
                chunk[index * 4 + 2],
                chunk[index * 4 + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for index in 0..64 {
            let big1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(big1)
                .wrapping_add(choice)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let big0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = big0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut output = [0u8; 32];
    for (index, value) in state.into_iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solved_grid() -> Grid {
        std::array::from_fn(|cell| {
            let row = cell / 9;
            let column = cell % 9;
            ((row * 3 + row / 3 + column) % 9 + 1) as u8
        })
    }

    #[test]
    fn canonical_state_is_invariant_under_all_transforms() {
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
    fn reduction_preserves_closure_and_hashes_are_stable() {
        let full = vec![(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)];
        let reduced = transitive_reduction(&full).unwrap();
        assert_eq!(reduced, vec![(0, 1), (1, 2), (2, 3)]);
        assert_eq!(
            transitive_closure_edges(&full).unwrap(),
            transitive_closure_edges(&reduced).unwrap()
        );
        assert!(transitive_reduction(&[(0, 1), (1, 0)]).is_err());
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn safe_constructor_rejects_invalid_grids_cells_and_coverage() {
        let solved = solved_grid();
        let valid_cells = (0u8..18).collect::<Vec<_>>();
        assert!(canonical_saturated_state(&valid_cells, &solved).is_ok());

        let mut invalid_grid = solved;
        invalid_grid[0] = invalid_grid[1];
        assert!(canonical_saturated_state(&valid_cells, &invalid_grid).is_err());

        let mut duplicate = valid_cells.clone();
        duplicate[17] = duplicate[16];
        assert!(canonical_saturated_state(&duplicate, &solved).is_err());

        let mut out_of_range = valid_cells.clone();
        out_of_range[17] = 81;
        assert!(canonical_saturated_state(&out_of_range, &solved).is_err());

        let nonincident = (0u8..17).chain(std::iter::once(80)).collect::<Vec<_>>();
        assert_eq!(nonincident.len(), 18);
        assert!(canonical_saturated_state(&nonincident, &solved).is_err());
    }
}
