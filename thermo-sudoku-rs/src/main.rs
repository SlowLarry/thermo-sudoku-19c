use std::env;
use std::process::ExitCode;
use std::time::Instant;

use thermo_sudoku::{Multiplicity, Solver};

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
    let mut givens = [0u8; 81];
    let mut paths: Option<Vec<Vec<u8>>> = None;
    let mut show_solution = false;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--limit" => {
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
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }

    let paths = paths.unwrap_or_default();
    let solver = Solver::new(givens, &paths).map_err(|error| error.to_string())?;
    let started = Instant::now();
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
         \n\
         PATHS uses zero-based row-major cells, commas within a thermometer,\n\
         and | between thermometers. Example:\n\
         --thermos \"19,29,28,20,11,12,13,3,4|77,69,78,70,62,53,44,52|41,51\""
    );
}
