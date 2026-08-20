use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use thermo_sudoku::{Multiplicity, Solver, screen_nine_eight_extensions};

const BOARD_SIDE: u8 = 9;
const PILOT_P9_START: u64 = 0;
const PILOT_P9_END: u64 = 64;
const PILOT_P8_START: u64 = 16_414_504;
const PILOT_P8_END: u64 = 16_418_600;
const NO_CELL: u8 = u8::MAX;

const fn king_adjacent_const(left: u8, right: u8) -> bool {
    if left == right {
        return false;
    }
    let left_row = left / BOARD_SIDE;
    let left_col = left % BOARD_SIDE;
    let right_row = right / BOARD_SIDE;
    let right_col = right % BOARD_SIDE;
    left_row.abs_diff(right_row) <= 1 && left_col.abs_diff(right_col) <= 1
}

const fn make_king_neighbors() -> [[u8; 8]; 81] {
    let mut result = [[NO_CELL; 8]; 81];
    let mut cell = 0u8;
    while cell < 81 {
        let mut candidate = 0u8;
        let mut count = 0usize;
        while candidate < 81 {
            if king_adjacent_const(cell, candidate) {
                result[cell as usize][count] = candidate;
                count += 1;
            }
            candidate += 1;
        }
        cell += 1;
    }
    result
}

const KING_NEIGHBORS: [[u8; 8]; 81] = make_king_neighbors();

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct GridPath {
    cells: [u8; 9],
    length: u8,
    footprint_lo: u64,
    footprint_hi: u32,
}

impl GridPath {
    fn from_slice(cells: &[u8]) -> Self {
        let mut stored = [u8::MAX; 9];
        stored[..cells.len()].copy_from_slice(cells);
        let mut footprint_lo = 0u64;
        let mut footprint_hi = 0u32;
        for &cell in cells {
            if cell < 64 {
                footprint_lo |= 1u64 << cell;
            } else {
                footprint_hi |= 1u32 << (cell - 64);
            }
        }
        Self {
            cells: stored,
            length: cells.len() as u8,
            footprint_lo,
            footprint_hi,
        }
    }

    fn cells(&self) -> &[u8] {
        &self.cells[..self.length as usize]
    }

    fn disjoint(&self, other: &Self) -> bool {
        self.footprint_lo & other.footprint_lo == 0 && self.footprint_hi & other.footprint_hi == 0
    }

    fn compact(&self) -> String {
        self.cells()
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[derive(Clone, Copy, Debug)]
struct GroupElement {
    spatial: u8,
    reverse: bool,
}

#[derive(Debug)]
struct Options {
    p9_start: u64,
    p9_end: u64,
    p8_start: u64,
    p8_end: u64,
    prefix_solutions: u64,
    base_offset: u64,
    max_bases: Option<u64>,
    output: Option<PathBuf>,
    dry_run: bool,
    template_solver: bool,
    progress_every: u64,
}

#[derive(Default)]
struct Counters {
    raw_pairs: u64,
    overlap_rejected: u64,
    symmetry_rejected: u64,
    canonical_bases: u64,
    skipped_by_offset: u64,
    processed_bases: u64,
    compatible_templates: u64,
    template_incompatible_bases: u64,
    candidate_edges: u64,
    zero_solution_bases: u64,
    unique_extensions: u64,
    collective_solutions: u64,
    fallback_searches: u64,
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
    if options.p9_start >= options.p9_end || options.p8_start >= options.p8_end {
        return Err("path rank ranges must be non-empty".into());
    }
    let started = Instant::now();
    let p9_paths = collect_path_range(9, options.p9_start, options.p9_end)?;
    let p8_paths = collect_path_range(8, options.p8_start, options.p8_end)?;
    eprintln!(
        "loaded {} length-9 and {} length-8 paths in {:.3}s",
        p9_paths.len(),
        p8_paths.len(),
        started.elapsed().as_secs_f64()
    );

    let mut output = options
        .output
        .as_ref()
        .map(File::create)
        .transpose()
        .map_err(|error| format!("cannot create output: {error}"))?
        .map(BufWriter::new);
    if let Some(writer) = output.as_mut() {
        writeln!(
            writer,
            "{{\"type\":\"header\",\"schema\":2,\"crate_version\":\"{}\",\"geometry\":\"disjoint-simple-king-paths\",\"symmetry\":\"d4+global-reversal\",\"path_order\":\"dfs-start-then-neighbor-ascending\",\"p9_start\":{},\"p9_end\":{},\"p8_start\":{},\"p8_end\":{},\"prefix_solutions\":{},\"base_offset\":{},\"max_bases\":{},\"dry_run\":{},\"solver\":\"{}\"}}",
            env!("CARGO_PKG_VERSION"),
            options.p9_start,
            options.p9_end,
            options.p8_start,
            options.p8_end,
            options.prefix_solutions,
            options.base_offset,
            options
                .max_bases
                .map_or_else(|| "null".to_owned(), |value| value.to_string()),
            options.dry_run,
            if options.template_solver {
                "nine-eight-templates"
            } else {
                "generic-thermo"
            }
        )
        .map_err(|error| error.to_string())?;
    }

    let group = all_group_elements();
    let mut counters = Counters::default();
    'p9: for (p9_offset, p9) in p9_paths.iter().enumerate() {
        let p9_rank = options.p9_start + p9_offset as u64;
        if !is_canonical(p9, &group) {
            counters.raw_pairs += p8_paths.len() as u64;
            counters.symmetry_rejected += p8_paths.len() as u64;
            continue;
        }
        let stabilizer9: Vec<_> = group
            .iter()
            .copied()
            .filter(|&element| transform_path(p9, element) == *p9)
            .collect();

        for (p8_offset, p8) in p8_paths.iter().enumerate() {
            let p8_rank = options.p8_start + p8_offset as u64;
            counters.raw_pairs += 1;
            if !p9.disjoint(p8) {
                counters.overlap_rejected += 1;
                continue;
            }
            if !is_canonical(p8, &stabilizer9) {
                counters.symmetry_rejected += 1;
                continue;
            }
            if options
                .max_bases
                .is_some_and(|limit| counters.processed_bases >= limit)
            {
                break 'p9;
            }
            counters.canonical_bases += 1;
            if counters.canonical_bases <= options.base_offset {
                counters.skipped_by_offset += 1;
                continue;
            }

            let compatible = compatible_template_count(p9, p8);
            counters.compatible_templates += u64::from(compatible);
            if compatible == 0 {
                counters.template_incompatible_bases += 1;
                counters.zero_solution_bases += 1;
            }
            counters.processed_bases += 1;

            if !options.dry_run && compatible != 0 {
                let screen = if options.template_solver {
                    let specialized = screen_nine_eight_extensions(
                        p9.cells(),
                        p8.cells(),
                        options.prefix_solutions,
                    )
                    .map_err(|error| error.to_string())?;
                    debug_assert_eq!(specialized.compatible_templates, compatible);
                    specialized.screen
                } else {
                    let paths = vec![p9.cells().to_vec(), p8.cells().to_vec()];
                    Solver::blank(&paths)
                        .map_err(|error| error.to_string())?
                        .screen_two_cell_extensions_hybrid(options.prefix_solutions)
                };
                counters.candidate_edges += screen.extensions.len() as u64;
                counters.collective_solutions += screen.base_solutions_visited;
                counters.fallback_searches += u64::from(screen.fallback_searches);
                debug_assert!(
                    screen
                        .extensions
                        .iter()
                        .all(|edge| edge.multiplicity().is_some())
                );
                if screen.base_exhausted && screen.base_solutions_visited == 0 {
                    counters.zero_solution_bases += 1;
                }
                for extension in screen
                    .extensions
                    .iter()
                    .filter(|edge| edge.multiplicity() == Some(Multiplicity::Unique))
                {
                    counters.unique_extensions += 1;
                    if let Some(writer) = output.as_mut() {
                        let witness = extension
                            .first_witness
                            .map(|index| format_solution(&screen.witness_solutions[index as usize]))
                            .unwrap_or_default();
                        writeln!(
                            writer,
                            "{{\"type\":\"unique\",\"p9_rank\":{p9_rank},\"p8_rank\":{p8_rank},\"p9\":\"{}\",\"p8\":\"{}\",\"edge\":[{},{}],\"solution\":\"{witness}\"}}",
                            p9.compact(),
                            p8.compact(),
                            extension.bulb,
                            extension.tip
                        )
                        .map_err(|error| error.to_string())?;
                    }
                }
            } else {
                counters.candidate_edges += legal_extension_count(p9, p8);
            }

            if options.progress_every != 0
                && counters
                    .processed_bases
                    .is_multiple_of(options.progress_every)
            {
                if let Some(writer) = output.as_mut() {
                    writeln!(
                        writer,
                        "{{\"type\":\"checkpoint\",\"p9_rank\":{p9_rank},\"p8_rank\":{p8_rank},\"resume_base_offset\":{},\"processed_bases\":{},\"unique_extensions\":{},\"elapsed_seconds\":{:.6}}}",
                        counters.canonical_bases,
                        counters.processed_bases,
                        counters.unique_extensions,
                        started.elapsed().as_secs_f64()
                    )
                    .map_err(|error| error.to_string())?;
                    writer.flush().map_err(|error| error.to_string())?;
                }
                eprintln!(
                    "processed={} canonical={} unique={} elapsed={:.1}s",
                    counters.processed_bases,
                    counters.canonical_bases,
                    counters.unique_extensions,
                    started.elapsed().as_secs_f64()
                );
            }
        }
    }

    let elapsed = started.elapsed();
    let summary = format!(
        "{{\"type\":\"summary\",\"raw_pairs\":{},\"overlap_rejected\":{},\"symmetry_rejected\":{},\"canonical_bases\":{},\"skipped_by_offset\":{},\"processed_bases\":{},\"compatible_templates\":{},\"template_incompatible_bases\":{},\"candidate_edges\":{},\"zero_solution_bases\":{},\"unique_extensions\":{},\"collective_solutions\":{},\"fallback_searches\":{},\"elapsed_seconds\":{:.6}}}",
        counters.raw_pairs,
        counters.overlap_rejected,
        counters.symmetry_rejected,
        counters.canonical_bases,
        counters.skipped_by_offset,
        counters.processed_bases,
        counters.compatible_templates,
        counters.template_incompatible_bases,
        counters.candidate_edges,
        counters.zero_solution_bases,
        counters.unique_extensions,
        counters.collective_solutions,
        counters.fallback_searches,
        elapsed.as_secs_f64()
    );
    println!("{summary}");
    if let Some(writer) = output.as_mut() {
        writeln!(writer, "{summary}").map_err(|error| error.to_string())?;
        writer.flush().map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn parse_options() -> Result<Options, String> {
    let mut options = Options {
        p9_start: PILOT_P9_START,
        p9_end: PILOT_P9_END,
        p8_start: PILOT_P8_START,
        p8_end: PILOT_P8_END,
        prefix_solutions: 128,
        base_offset: 0,
        max_bases: None,
        output: None,
        dry_run: false,
        template_solver: false,
        progress_every: 10_000,
    };
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--p9-start" => options.p9_start = next_u64(&mut args, "--p9-start")?,
            "--p9-end" => options.p9_end = next_u64(&mut args, "--p9-end")?,
            "--p8-start" => options.p8_start = next_u64(&mut args, "--p8-start")?,
            "--p8-end" => options.p8_end = next_u64(&mut args, "--p8-end")?,
            "--prefix-solutions" => {
                options.prefix_solutions = next_u64(&mut args, "--prefix-solutions")?;
            }
            "--base-offset" => options.base_offset = next_u64(&mut args, "--base-offset")?,
            "--max-bases" => {
                options.max_bases = Some(next_u64(&mut args, "--max-bases")?);
            }
            "--output" => {
                options.output = Some(PathBuf::from(
                    args.next().ok_or("--output requires a path")?,
                ));
            }
            "--dry-run" => options.dry_run = true,
            "--template-solver" => options.template_solver = true,
            "--progress-every" => {
                options.progress_every = next_u64(&mut args, "--progress-every")?;
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(options)
}

fn next_u64(args: &mut impl Iterator<Item = String>, name: &str) -> Result<u64, String> {
    args.next()
        .ok_or_else(|| format!("{name} requires an integer"))?
        .parse()
        .map_err(|_| format!("invalid {name}"))
}

fn print_help() {
    println!(
        "thermo-9x8-pilot [rank ranges] [--dry-run] [--max-bases N]\n\
         \n\
         Defaults to the reproducible pilot shard:\n\
         --p9-start 0 --p9-end 64\n\
         --p8-start 16414504 --p8-end 16418600\n\
         \n\
         Other options: --prefix-solutions N --base-offset N --output FILE\n\
         --template-solver (benchmark the 17-given template specialization)\n\
         --progress-every N"
    );
}

fn collect_path_range(length: usize, start: u64, end: u64) -> Result<Vec<GridPath>, String> {
    let mut output = Vec::with_capacity((end - start) as usize);
    let mut rank = 0u64;
    let mut path = [u8::MAX; 9];
    for first in 0u8..81 {
        path[0] = first;
        let (visited_lo, visited_hi) = add_cell(0, 0, first);
        enumerate_paths(
            length,
            1,
            &mut path,
            visited_lo,
            visited_hi,
            start,
            end,
            &mut rank,
            &mut output,
        );
        if rank >= end {
            break;
        }
    }
    if rank < end {
        return Err(format!(
            "path range ends at {end}, but length {length} has only {rank} paths"
        ));
    }
    Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn enumerate_paths(
    target_length: usize,
    depth: usize,
    path: &mut [u8; 9],
    visited_lo: u64,
    visited_hi: u32,
    start: u64,
    end: u64,
    rank: &mut u64,
    output: &mut Vec<GridPath>,
) {
    if *rank >= end {
        return;
    }
    if depth == target_length {
        if *rank >= start {
            output.push(GridPath::from_slice(&path[..target_length]));
        }
        *rank += 1;
        return;
    }
    let current = path[depth - 1];
    for &next in &KING_NEIGHBORS[current as usize] {
        if next == NO_CELL {
            break;
        }
        if contains_cell(visited_lo, visited_hi, next) {
            continue;
        }
        path[depth] = next;
        let (next_lo, next_hi) = add_cell(visited_lo, visited_hi, next);
        enumerate_paths(
            target_length,
            depth + 1,
            path,
            next_lo,
            next_hi,
            start,
            end,
            rank,
            output,
        );
        if *rank >= end {
            return;
        }
    }
}

fn contains_cell(lo: u64, hi: u32, cell: u8) -> bool {
    if cell < 64 {
        lo & (1u64 << cell) != 0
    } else {
        hi & (1u32 << (cell - 64)) != 0
    }
}

fn add_cell(mut lo: u64, mut hi: u32, cell: u8) -> (u64, u32) {
    if cell < 64 {
        lo |= 1u64 << cell;
    } else {
        hi |= 1u32 << (cell - 64);
    }
    (lo, hi)
}

fn king_adjacent(left: u8, right: u8) -> bool {
    king_adjacent_const(left, right)
}

fn all_group_elements() -> Vec<GroupElement> {
    let mut elements = Vec::with_capacity(16);
    for reverse in [false, true] {
        for spatial in 0..8 {
            elements.push(GroupElement { spatial, reverse });
        }
    }
    elements
}

fn transform_cell(cell: u8, spatial: u8) -> u8 {
    let row = cell / 9;
    let col = cell % 9;
    let (next_row, next_col) = match spatial {
        0 => (row, col),
        1 => (col, 8 - row),
        2 => (8 - row, 8 - col),
        3 => (8 - col, row),
        4 => (row, 8 - col),
        5 => (8 - col, 8 - row),
        6 => (8 - row, col),
        7 => (col, row),
        _ => unreachable!(),
    };
    next_row * 9 + next_col
}

fn transform_path(path: &GridPath, element: GroupElement) -> GridPath {
    let mut cells: Vec<u8> = path
        .cells()
        .iter()
        .map(|&cell| transform_cell(cell, element.spatial))
        .collect();
    if element.reverse {
        cells.reverse();
    }
    GridPath::from_slice(&cells)
}

fn is_canonical(path: &GridPath, group: &[GroupElement]) -> bool {
    group
        .iter()
        .all(|&element| *path <= transform_path(path, element))
}

fn compatible_template_count(p9: &GridPath, p8: &GridPath) -> u8 {
    let mut count = 0u8;
    for omitted in 1u8..=9 {
        let mut rows = [0u16; 9];
        let mut columns = [0u16; 9];
        let mut boxes = [0u16; 9];
        let mut valid = true;
        for (position, &cell) in p9.cells().iter().enumerate() {
            valid &= add_given(
                &mut rows,
                &mut columns,
                &mut boxes,
                cell,
                position as u8 + 1,
            );
        }
        for (position, &cell) in p8.cells().iter().enumerate() {
            let ordinal = position as u8 + 1;
            let digit = if ordinal < omitted {
                ordinal
            } else {
                ordinal + 1
            };
            valid &= add_given(&mut rows, &mut columns, &mut boxes, cell, digit);
        }
        count += u8::from(valid);
    }
    count
}

fn add_given(
    rows: &mut [u16; 9],
    columns: &mut [u16; 9],
    boxes: &mut [u16; 9],
    cell: u8,
    digit: u8,
) -> bool {
    let row = (cell / 9) as usize;
    let col = (cell % 9) as usize;
    let box_index = (row / 3) * 3 + col / 3;
    let bit = 1u16 << (digit - 1);
    if rows[row] & bit != 0 || columns[col] & bit != 0 || boxes[box_index] & bit != 0 {
        return false;
    }
    rows[row] |= bit;
    columns[col] |= bit;
    boxes[box_index] |= bit;
    true
}

fn legal_extension_count(p9: &GridPath, p8: &GridPath) -> u64 {
    let occupied_lo = p9.footprint_lo | p8.footprint_lo;
    let occupied_hi = p9.footprint_hi | p8.footprint_hi;
    let mut count = 0u64;
    for bulb in 0u8..81 {
        if contains_cell(occupied_lo, occupied_hi, bulb) {
            continue;
        }
        for tip in 0u8..81 {
            if !contains_cell(occupied_lo, occupied_hi, tip) && king_adjacent(bulb, tip) {
                count += 1;
            }
        }
    }
    count
}

fn format_solution(solution: &[u8; 81]) -> String {
    solution
        .iter()
        .map(|digit| char::from(b'0' + *digit))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directed_two_cell_path_count_is_544() {
        assert_eq!(collect_path_range(2, 0, 544).unwrap().len(), 544);
        assert!(collect_path_range(2, 0, 545).is_err());
    }

    #[test]
    fn d4_and_global_reversal_form_sixteen_distinct_images_generically() {
        let path = GridPath::from_slice(&[0, 1, 11, 21, 31, 41, 51, 61, 71]);
        let mut images: Vec<_> = all_group_elements()
            .into_iter()
            .map(|element| transform_path(&path, element))
            .collect();
        images.sort_unstable();
        images.dedup();
        assert_eq!(images.len(), 16);
    }

    #[test]
    fn stabilizer_augmentation_matches_full_pair_canonicalization() {
        let group = all_group_elements();
        let first_paths = collect_path_range(4, 0, 64).unwrap();
        let second_paths = collect_path_range(3, 0, 64).unwrap();
        for first in &first_paths {
            let stabilizer: Vec<_> = group
                .iter()
                .copied()
                .filter(|&element| transform_path(first, element) == *first)
                .collect();
            for second in &second_paths {
                let augmented = is_canonical(first, &group) && is_canonical(second, &stabilizer);
                let full = group.iter().all(|&element| {
                    (*first, *second)
                        <= (
                            transform_path(first, element),
                            transform_path(second, element),
                        )
                });
                assert_eq!(augmented, full);
            }
        }
    }

    #[test]
    fn length_nine_and_eight_templates_are_checked_as_givens() {
        let p9 = GridPath::from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8]);
        let p8 = GridPath::from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);
        assert_eq!(compatible_template_count(&p9, &p8), 0);
    }
}
