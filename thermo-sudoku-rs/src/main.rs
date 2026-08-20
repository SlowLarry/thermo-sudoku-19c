use std::env;
use std::process::ExitCode;
use std::time::Instant;

use thermo_sudoku::{Multiplicity, Solver, screen_nine_eight_extensions};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let mut limit = 2u64;
    let mut limit_set = false;
    let mut givens = [0u8; 81];
    let mut paths: Option<Vec<Vec<u8>>> = None;
    let mut show_solution = false;
    let mut screen_two_cell = false;
    let mut collective_only = false;
    let mut collective_prefix = 128u64;
    let mut collective_prefix_set = false;
    let mut emit_certificate = false;
    let mut nine_eight_templates = false;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--limit" => {
                limit_set = true;
                limit = args
                    .next()
                    .ok_or("--limit requires an integer of at least two")?
                    .parse()
                    .map_err(|_| "invalid --limit")?;
                if limit < 2 {
                    return Err("--limit must be at least two".into());
                }
            }
            "--givens" => {
                givens = parse_givens(&args.next().ok_or("--givens requires 81 characters")?)?;
            }
            "--thermos" => {
                paths = Some(parse_paths(
                    &args
                        .next()
                        .ok_or("--thermos requires a compact path list")?,
                )?);
            }
            "--show-solution" => show_solution = true,
            "--screen-two-cell" => screen_two_cell = true,
            "--collective-only" => collective_only = true,
            "--collective-prefix" => {
                collective_prefix_set = true;
                collective_prefix = args
                    .next()
                    .ok_or("--collective-prefix requires a non-negative integer")?
                    .parse()
                    .map_err(|_| "invalid --collective-prefix")?;
            }
            "--emit-certificate" => emit_certificate = true,
            "--nine-eight-templates" => nine_eight_templates = true,
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }

    let paths = paths.unwrap_or_default();
    let solver = Solver::new(givens, &paths).map_err(|error| error.to_string())?;
    if emit_certificate && !screen_two_cell {
        return Err("--emit-certificate requires --screen-two-cell".into());
    }
    if collective_only && !screen_two_cell {
        return Err("--collective-only requires --screen-two-cell".into());
    }
    if collective_only && collective_prefix_set {
        return Err("--collective-only cannot be combined with --collective-prefix".into());
    }
    if screen_two_cell && limit_set {
        return Err("--limit is not used with --screen-two-cell".into());
    }
    if screen_two_cell && show_solution {
        return Err("--show-solution is not used with --screen-two-cell".into());
    }
    if nine_eight_templates && !screen_two_cell {
        return Err("--nine-eight-templates requires --screen-two-cell".into());
    }
    if nine_eight_templates && collective_only {
        return Err("--nine-eight-templates does not support --collective-only".into());
    }
    if nine_eight_templates && givens.iter().any(|&digit| digit != 0) {
        return Err("--nine-eight-templates requires a blank base grid".into());
    }
    let started = Instant::now();
    if screen_two_cell {
        let (result, compatible_templates) = if nine_eight_templates {
            if paths.len() != 2 || paths[0].len() != 9 || paths[1].len() != 8 {
                return Err(
                    "--nine-eight-templates requires one length-9 then one length-8 path".into(),
                );
            }
            let specialized = screen_nine_eight_extensions(&paths[0], &paths[1], collective_prefix)
                .map_err(|error| error.to_string())?;
            (specialized.screen, Some(specialized.compatible_templates))
        } else if collective_only {
            (solver.screen_two_cell_extensions_collective(), None)
        } else {
            (
                solver.screen_two_cell_extensions_hybrid(collective_prefix),
                None,
            )
        };
        let elapsed = started.elapsed();
        println!("mode=screen-two-cell");
        println!("candidate_edges={}", result.extensions.len());
        if let Some(compatible_templates) = compatible_templates {
            println!("compatible_templates={compatible_templates}");
        }
        println!(
            "collective_solution_limit={}",
            result
                .collective_solution_limit
                .map_or_else(|| "all".to_owned(), |value| value.to_string())
        );
        println!(
            "collective_solutions_visited={}",
            result.base_solutions_visited
        );
        println!("base_exhausted={}", result.base_exhausted);
        println!("fallback_searches={}", result.fallback_searches);
        println!("zero_extensions={}", result.zero_count());
        println!("unique_extensions={}", result.unique_count());
        println!("multiple_extensions={}", result.multiple_count());
        println!("witness_solutions={}", result.witness_solutions.len());
        println!("elapsed_us={}", elapsed.as_micros());
        println!("nodes={}", result.stats.nodes);
        println!("branches={}", result.stats.branches);

        if emit_certificate {
            println!("certificate_version=thermo-two-cell-v1");
            println!("base_givens={}", format_givens(&givens));
            println!("base_thermos={}", format_paths(&paths));
            println!(
                "witness_complete={}",
                result.zero_count() == 0 && result.unique_count() == 0
            );
            for (index, solution) in result.witness_solutions.iter().enumerate() {
                println!("witness={index},{}", format_givens(solution));
            }
            for extension in &result.extensions {
                println!(
                    "extension={},{},{},{},{}",
                    extension.bulb,
                    extension.tip,
                    extension_label(extension.count, extension.exact),
                    format_witness(extension.first_witness),
                    format_witness(extension.second_witness)
                );
            }
        } else {
            for extension in &result.extensions {
                if extension.count < 2 {
                    println!(
                        "extension={},{},{}",
                        extension.bulb,
                        extension.tip,
                        extension_label(extension.count, extension.exact)
                    );
                }
            }
        }
        return Ok(());
    }

    let result = solver.count_up_to(limit);
    let elapsed = started.elapsed();

    let label = match result.multiplicity() {
        Multiplicity::Zero => "0",
        Multiplicity::Unique => "1",
        Multiplicity::Multiple if result.capped => "2+",
        Multiplicity::Multiple => "multiple",
    };
    println!("classification={label}");
    println!(
        "count={}{}",
        result.count,
        if result.capped { "+" } else { "" }
    );
    println!("elapsed_us={}", elapsed.as_micros());
    println!("nodes={}", result.stats.nodes);
    println!("branches={}", result.stats.branches);
    println!("propagation_rounds={}", result.stats.propagation_rounds);
    println!("thermo_revisions={}", result.stats.thermo_revisions);
    if show_solution && let Some(solution) = result.first_solution {
        let text: String = solution
            .iter()
            .map(|digit| char::from(b'0' + *digit))
            .collect();
        println!("solution={text}");
    }
    Ok(())
}

fn extension_label(count: u8, exact: bool) -> &'static str {
    match (count, exact) {
        (0, true) => "0",
        (1, true) => "1",
        _ => "2+",
    }
}

fn format_witness(witness: Option<u32>) -> String {
    witness.map_or_else(|| "-".to_owned(), |index| index.to_string())
}

fn format_givens(givens: &[u8; 81]) -> String {
    givens
        .iter()
        .map(|digit| char::from(b'0' + *digit))
        .collect()
}

fn format_paths(paths: &[Vec<u8>]) -> String {
    paths
        .iter()
        .map(|path| path.iter().map(u8::to_string).collect::<Vec<_>>().join(","))
        .collect::<Vec<_>>()
        .join("|")
}

fn parse_givens(text: &str) -> Result<[u8; 81], String> {
    if text.len() != 81 {
        return Err(format!(
            "givens must contain 81 characters, got {}",
            text.len()
        ));
    }
    let mut givens = [0u8; 81];
    for (index, byte) in text.bytes().enumerate() {
        givens[index] = match byte {
            b'.' | b'0' => 0,
            b'1'..=b'9' => byte - b'0',
            _ => return Err(format!("invalid given character at position {index}")),
        };
    }
    Ok(givens)
}

fn parse_paths(text: &str) -> Result<Vec<Vec<u8>>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    text.split('|')
        .enumerate()
        .map(|(thermo, path)| {
            path.split(',')
                .enumerate()
                .map(|(position, cell)| {
                    cell.trim().parse::<u8>().map_err(|_| {
                        format!("invalid cell in thermometer {thermo}, position {position}")
                    })
                })
                .collect()
        })
        .collect()
}

fn print_help() {
    println!(
        "thermo-sudoku [--limit N] [--givens GRID] [--thermos PATHS] [--show-solution]\n\
         thermo-sudoku --thermos PATHS --screen-two-cell\n\
           [--collective-prefix N | --collective-only] [--emit-certificate]\n\
           [--nine-eight-templates]\n\
         \n\
         PATHS uses zero-based row-major cells, commas within a thermometer,\n\
         and | between thermometers. Example:\n\
         --thermos \"19,29,28,20,11,12,13,3,4|77,69,78,70,62,53,44,52|41,51\""
    );
}
