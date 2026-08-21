//! Deterministic gradient-guided search for disjoint 9+8+2 thermometer layouts.
//!
//! This is a bounded construction heuristic, not an exclusion procedure.  It
//! uses every valid distinct layout in the legacy low-count corpus as a scored
//! anchor, then explores one-cell mutations of the length-nine and length-eight
//! base paths.  An opt-in move can also reroute two consecutive cells at once.
//! For each base, all legal disjoint two-cell extensions are scored collectively
//! by the specialized library routine.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::time::Duration;

use thermo_sudoku::{Solver, score_nine_eight_extensions};

const CELLS: usize = 81;
const DEFAULT_GRADIENT_CAPS: &[u64] = &[8, 32, 128];
const DEFAULT_ANCHOR_CAP: u64 = 1_025;
const CHECKPOINT_HEADER: &str = "# thermo-9x8-guided-v1";
const GLOBAL_CHECKPOINT_HEADER: &str = "# thermo-global-cegis-v1";
const GLOBAL_CHECKPOINT_BUDGET: usize = 16;
const DIRECTED_EDGES: usize = 544;
const DEFAULT_PAIR_SEED_SOLUTION_CUTOFF: usize = 65;
const DEFAULT_PAIR_SEED_PAIRS_PER_ANCHOR: usize = 64;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct BaseLayout {
    path9: Vec<u8>,
    path8: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct FullLayout {
    path9: Vec<u8>,
    path8: Vec<u8>,
    path2: Vec<u8>,
}

impl FullLayout {
    fn paths(&self) -> Vec<Vec<u8>> {
        vec![self.path9.clone(), self.path8.clone(), self.path2.clone()]
    }

    fn base(&self) -> BaseLayout {
        canonical_base(BaseLayout {
            path9: self.path9.clone(),
            path8: self.path8.clone(),
        })
    }
}

#[derive(Clone, Debug)]
struct AnchorSource {
    declared_counts: BTreeSet<u64>,
    lines: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
struct CorpusStats {
    lines: usize,
    parsed: usize,
    invalid: usize,
    geometry_valid: usize,
    duplicate_layouts: usize,
    distinct_layouts: usize,
    distinct_bases: usize,
    declaration_mismatches: usize,
    zero_solution_anchors: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScoredLayout {
    layout: FullLayout,
    count: u64,
    exact: bool,
    cap: u64,
    first_solution: Option<[u8; CELLS]>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct GridPair {
    first: [u8; CELLS],
    second: [u8; CELLS],
}

impl GridPair {
    fn new(first: [u8; CELLS], second: [u8; CELLS]) -> Result<Self, String> {
        match first.cmp(&second) {
            std::cmp::Ordering::Less => Ok(Self { first, second }),
            std::cmp::Ordering::Greater => Ok(Self {
                first: second,
                second: first,
            }),
            std::cmp::Ordering::Equal => {
                Err("a pair seed requires two distinct Sudoku solutions".into())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RankedPair {
    cut_length: u16,
    pair: GridPair,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PairSeedOrigin {
    Corpus,
    Guided,
}

#[derive(Debug, Default)]
struct PairSeedBuilder {
    pairs: BTreeSet<GridPair>,
    layouts: BTreeSet<FullLayout>,
    corpus_layouts: usize,
    guided_layouts: usize,
    duplicate_layouts_skipped: usize,
    exact_eligible_layouts: usize,
    layouts_above_cutoff: usize,
    layouts_with_fewer_than_two_solutions: usize,
    candidate_pairs: u64,
    selected_pairs_before_dedup: usize,
    duplicate_selected_pairs: usize,
}

impl PairSeedBuilder {
    fn register_layout(&mut self, layout: &FullLayout, origin: PairSeedOrigin) -> bool {
        if !self.layouts.insert(layout.clone()) {
            self.duplicate_layouts_skipped += 1;
            return false;
        }
        match origin {
            PairSeedOrigin::Corpus => self.corpus_layouts += 1,
            PairSeedOrigin::Guided => self.guided_layouts += 1,
        }
        true
    }

    fn add_proven_above_cutoff(&mut self, layout: &FullLayout, origin: PairSeedOrigin) -> bool {
        if !self.register_layout(layout, origin) {
            return false;
        }
        self.layouts_above_cutoff += 1;
        true
    }

    fn add_layout(
        &mut self,
        layout: &FullLayout,
        origin: PairSeedOrigin,
        solution_cutoff: usize,
        pairs_per_anchor: usize,
    ) -> Result<bool, String> {
        if !self.register_layout(layout, origin) {
            return Ok(false);
        }
        let solver = Solver::blank(&layout.paths()).map_err(|error| error.to_string())?;
        let batch = solver.enumerate_up_to(solution_cutoff);
        if !batch.exhausted {
            debug_assert!(batch.capped);
            self.layouts_above_cutoff += 1;
            return Ok(true);
        }
        self.exact_eligible_layouts += 1;
        for solution in &batch.solutions {
            validate_solution_for_layout(solution, layout)?;
        }
        if batch.solutions.len() < 2 {
            self.layouts_with_fewer_than_two_solutions += 1;
            return Ok(true);
        }

        let candidate_count = batch
            .solutions
            .len()
            .checked_mul(batch.solutions.len() - 1)
            .and_then(|value| value.checked_div(2))
            .ok_or("pair seed candidate count overflow")?;
        self.candidate_pairs = self
            .candidate_pairs
            .checked_add(candidate_count as u64)
            .ok_or("pair seed candidate count overflow")?;

        let mut strongest = BTreeSet::new();
        for left in 0..batch.solutions.len() {
            for right in left + 1..batch.solutions.len() {
                let pair = GridPair::new(batch.solutions[left], batch.solutions[right])?;
                strongest.insert(RankedPair {
                    cut_length: pair_cut_length(&pair),
                    pair,
                });
                if strongest.len() > pairs_per_anchor {
                    strongest.pop_last();
                }
            }
        }
        self.selected_pairs_before_dedup += strongest.len();
        for ranked in strongest {
            if !self.pairs.insert(ranked.pair) {
                self.duplicate_selected_pairs += 1;
            }
        }
        Ok(true)
    }
}

impl ScoredLayout {
    fn is_unique(&self) -> bool {
        self.exact && self.count == 1
    }

    fn rank_key(&self) -> (u64, bool, &FullLayout) {
        (self.count, !self.exact, &self.layout)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EvaluatedBase {
    base: BaseLayout,
    best: Option<ScoredLayout>,
    positive_extensions: usize,
    zero_extensions: usize,
    capped_extensions: usize,
    stages: usize,
}

impl EvaluatedBase {
    fn rank_key(&self) -> (u64, bool, &BaseLayout) {
        match &self.best {
            Some(best) => (best.count, !best.exact, &self.base),
            None => (u64::MAX, true, &self.base),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutationKind {
    OneCell,
    TwoCellReroute,
}

impl MutationKind {
    fn origin(self) -> &'static str {
        match self {
            Self::OneCell => "one-cell-mutation",
            Self::TwoCellReroute => "two-cell-reroute",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MutationCandidate {
    base: BaseLayout,
    kind: MutationKind,
}

#[derive(Debug)]
struct Options {
    input: PathBuf,
    output: Option<PathBuf>,
    checkpoint: Option<PathBuf>,
    resume: bool,
    anchor_cap: u64,
    gradient_caps: Vec<u64>,
    collective_prefix: u64,
    beam_width: usize,
    anchor_batch: usize,
    rounds: usize,
    max_base_evaluations: usize,
    candidates_per_round: usize,
    report_below: u64,
    solution_preserving: bool,
    two_cell_reroutes: bool,
    dry_run: bool,
    pair_seed_checkpoint: Option<PathBuf>,
    pair_seed_solution_cutoff: usize,
    pair_seed_pairs_per_anchor: usize,
}

#[derive(Clone, Debug)]
struct Checkpoint {
    input_fingerprint: u64,
    anchor_cap: u64,
    gradient_caps: Vec<u64>,
    collective_prefix: u64,
    beam_width: usize,
    anchor_batch: usize,
    candidates_per_round: usize,
    solution_preserving: bool,
    two_cell_reroutes: bool,
    next_round: usize,
    anchor_cursor: usize,
    base_evaluations: usize,
    evaluated: BTreeMap<BaseLayout, EvaluatedBase>,
    beam: Vec<BaseLayout>,
}

#[derive(Default)]
struct RunCounters {
    anchor_solver_calls: usize,
    base_solver_calls: usize,
    generated_mutations: usize,
    generated_one_cell_mutations: usize,
    generated_two_cell_reroutes: usize,
    duplicate_mutations: usize,
    rounds_completed: usize,
    pair_seed_solver_calls: usize,
}

fn add_scored_pair_seed(
    seed: &mut Option<PairSeedBuilder>,
    counters: &mut RunCounters,
    options: &Options,
    scored: Option<&ScoredLayout>,
) -> Result<(), String> {
    let Some(scored) = scored.filter(|scored| {
        scored.exact
            && usize::try_from(scored.count)
                .is_ok_and(|count| count <= options.pair_seed_solution_cutoff)
    }) else {
        return Ok(());
    };
    if let Some(seed) = seed {
        counters.pair_seed_solver_calls += usize::from(seed.add_layout(
            &scored.layout,
            PairSeedOrigin::Guided,
            options.pair_seed_solution_cutoff,
            options.pair_seed_pairs_per_anchor,
        )?);
    }
    Ok(())
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
    let input_bytes = fs::read(&options.input)
        .map_err(|error| format!("cannot read {}: {error}", options.input.display()))?;
    let input_fingerprint = fnv1a64(&input_bytes);
    let input_text = std::str::from_utf8(&input_bytes)
        .map_err(|error| format!("{} is not UTF-8: {error}", options.input.display()))?;

    let (anchors, mut corpus_stats, invalid_details) = load_corpus(input_text);
    corpus_stats.distinct_layouts = anchors.len();
    let anchor_bases = anchors
        .keys()
        .map(FullLayout::base)
        .collect::<BTreeSet<_>>();
    corpus_stats.distinct_bases = anchor_bases.len();
    if anchors.is_empty() {
        return Err("the corpus contains no valid 9+8+2 layouts".into());
    }

    let mut output = open_output(&options)?;
    write_header(
        &mut output,
        &options,
        input_fingerprint,
        &corpus_stats,
        &invalid_details,
    )?;

    let mut scored_anchors = Vec::with_capacity(anchors.len());
    let mut counters = RunCounters::default();
    let mut pair_seed = options
        .pair_seed_checkpoint
        .as_ref()
        .map(|_| PairSeedBuilder::default());
    for (layout, source) in &anchors {
        let scored = score_full_layout(layout, options.anchor_cap)?;
        counters.anchor_solver_calls += 1;
        if let Some(seed) = &mut pair_seed {
            let cutoff = u64::try_from(options.pair_seed_solution_cutoff).unwrap_or(u64::MAX);
            if scored.count > cutoff {
                seed.add_proven_above_cutoff(layout, PairSeedOrigin::Corpus);
            } else {
                counters.pair_seed_solver_calls += usize::from(seed.add_layout(
                    layout,
                    PairSeedOrigin::Corpus,
                    options.pair_seed_solution_cutoff,
                    options.pair_seed_pairs_per_anchor,
                )?);
            }
        }
        let declaration_matches = scored.exact
            && source.declared_counts.len() == 1
            && source.declared_counts.contains(&scored.count);
        if !declaration_matches {
            corpus_stats.declaration_mismatches += 1;
        }
        if scored.exact && scored.count == 0 {
            corpus_stats.zero_solution_anchors += 1;
        }
        write_anchor(&mut output, &scored, source, declaration_matches)?;
        if scored.count != 0 {
            scored_anchors.push(scored);
        }
    }
    scored_anchors.sort_by(|left, right| left.rank_key().cmp(&right.rank_key()));
    if let Some(unique) = scored_anchors.iter().find(|scored| scored.is_unique()) {
        write_pair_seed_if_requested(&mut output, &options, pair_seed.as_ref())?;
        write_unique(&mut output, "corpus-anchor", 0, unique)?;
        write_summary(
            &mut output,
            "unique",
            &corpus_stats,
            &counters,
            Some(unique),
            started.elapsed().as_secs_f64(),
        )?;
        return Ok(());
    }

    if options.dry_run {
        write_pair_seed_if_requested(&mut output, &options, pair_seed.as_ref())?;
        write_summary(
            &mut output,
            "dry-run",
            &corpus_stats,
            &counters,
            scored_anchors.first(),
            started.elapsed().as_secs_f64(),
        )?;
        return Ok(());
    }

    let best_anchor_by_base = best_anchors_by_base(&scored_anchors);
    let mut evaluated = BTreeMap::new();
    let anchor_schedule = anchor_schedule(&best_anchor_by_base);
    let mut next_round = 0usize;
    let mut anchor_cursor = 0usize;
    let mut base_evaluations = 0usize;
    let mut beam = Vec::new();

    if options.resume {
        let checkpoint_path = options
            .checkpoint
            .as_ref()
            .ok_or("--resume requires --checkpoint FILE")?;
        let checkpoint = load_checkpoint(checkpoint_path)?;
        validate_checkpoint(&checkpoint, &options, input_fingerprint)?;
        next_round = checkpoint.next_round;
        anchor_cursor = checkpoint.anchor_cursor;
        base_evaluations = checkpoint.base_evaluations;
        evaluated = checkpoint.evaluated;
        beam = checkpoint.beam;
        if anchor_cursor > anchor_schedule.len() {
            return Err("checkpoint anchor cursor exceeds the current schedule".into());
        }
        writeln!(
            output,
            "{{\"type\":\"resume\",\"next_round\":{next_round},\"anchor_cursor\":{anchor_cursor},\"base_evaluations\":{base_evaluations},\"evaluated_bases\":{}}}",
            evaluated.len()
        )
        .map_err(|error| error.to_string())?;
        for evaluated_base in evaluated.values() {
            add_scored_pair_seed(
                &mut pair_seed,
                &mut counters,
                &options,
                evaluated_base.best.as_ref(),
            )?;
        }
    }

    let mut global_best = scored_anchors.first().cloned();
    for evaluated_base in evaluated.values() {
        update_best(&mut global_best, evaluated_base.best.as_ref());
    }

    for round in next_round..options.rounds {
        if base_evaluations >= options.max_base_evaluations
            || (beam.is_empty() && anchor_cursor >= anchor_schedule.len())
        {
            break;
        }

        // Force a deterministic slice of previously unused corpus bases into
        // every generation.  Consequently every canonical corpus base becomes
        // a real local-search start after finitely many resumed rounds; the
        // gradient-selected beam cannot permanently starve higher-count seeds.
        let injection = if beam.is_empty() {
            options.beam_width
        } else {
            options.anchor_batch.min(options.beam_width)
        };
        let end = (anchor_cursor + injection).min(anchor_schedule.len());
        let mut parents = anchor_schedule[anchor_cursor..end].to_vec();
        for base in &beam {
            if parents.len() >= options.beam_width {
                break;
            }
            if !parents.contains(base) {
                parents.push(base.clone());
            }
        }

        // Score unvisited beam bases first.  The initial beam comes from every
        // re-scored corpus anchor; later beams contain the best mutations from
        // the preceding generation.
        let mut scored_generation = Vec::new();
        for base in &parents {
            if base_evaluations >= options.max_base_evaluations {
                break;
            }
            if let Some(existing) = evaluated.get(base) {
                scored_generation.push(existing.clone());
                if anchor_cursor < end {
                    debug_assert_eq!(*base, anchor_schedule[anchor_cursor]);
                    anchor_cursor += 1;
                }
                continue;
            }
            let candidate = evaluate_base(base, &options.gradient_caps, options.collective_prefix)?;
            base_evaluations += 1;
            counters.base_solver_calls += candidate.stages;
            write_candidate(
                &mut output,
                "beam",
                round,
                base_evaluations,
                &candidate,
                options.report_below,
            )?;
            update_best(&mut global_best, candidate.best.as_ref());
            add_scored_pair_seed(
                &mut pair_seed,
                &mut counters,
                &options,
                candidate.best.as_ref(),
            )?;
            if anchor_cursor < end {
                debug_assert_eq!(*base, anchor_schedule[anchor_cursor]);
                anchor_cursor += 1;
            }
            if let Some(unique) = candidate.best.as_ref().filter(|best| best.is_unique()) {
                evaluated.insert(base.clone(), candidate.clone());
                write_unique(&mut output, "beam", round, unique)?;
                counters.rounds_completed = round;
                checkpoint_if_requested(
                    &options,
                    input_fingerprint,
                    round,
                    anchor_cursor,
                    base_evaluations,
                    &evaluated,
                    &[],
                )?;
                write_pair_seed_if_requested(&mut output, &options, pair_seed.as_ref())?;
                write_summary(
                    &mut output,
                    "unique",
                    &corpus_stats,
                    &counters,
                    global_best.as_ref(),
                    started.elapsed().as_secs_f64(),
                )?;
                return Ok(());
            }
            evaluated.insert(base.clone(), candidate.clone());
            scored_generation.push(candidate);
        }

        if base_evaluations >= options.max_base_evaluations {
            counters.rounds_completed = round;
            beam = select_evaluated_beam(&scored_generation, options.beam_width);
            checkpoint_if_requested(
                &options,
                input_fingerprint,
                round + 1,
                anchor_cursor,
                base_evaluations,
                &evaluated,
                &beam,
            )?;
            break;
        }

        let remaining = options.max_base_evaluations - base_evaluations;
        // Reserve enough of the hard evaluation budget for every corpus base
        // that has not yet reached its forced-start slot.  Mutation work can
        // never consume the only budget with which those anchors could enter.
        let unscheduled_anchors = anchor_schedule.len() - anchor_cursor;
        let generation_limit =
            mutation_allowance(remaining, unscheduled_anchors, options.candidates_per_round);
        let mutation_candidates = round_robin_mutations(
            &scored_generation,
            evaluated.keys().cloned().collect(),
            generation_limit,
            options.solution_preserving,
            options.two_cell_reroutes,
            &mut counters,
        );
        let mut next_generation = Vec::with_capacity(mutation_candidates.len());
        for mutation in mutation_candidates {
            let origin = mutation.kind.origin();
            let base = mutation.base;
            let candidate =
                evaluate_base(&base, &options.gradient_caps, options.collective_prefix)?;
            base_evaluations += 1;
            counters.base_solver_calls += candidate.stages;
            write_candidate(
                &mut output,
                origin,
                round,
                base_evaluations,
                &candidate,
                options.report_below,
            )?;
            update_best(&mut global_best, candidate.best.as_ref());
            add_scored_pair_seed(
                &mut pair_seed,
                &mut counters,
                &options,
                candidate.best.as_ref(),
            )?;
            let unique = candidate
                .best
                .as_ref()
                .filter(|best| best.is_unique())
                .cloned();
            evaluated.insert(base, candidate.clone());
            next_generation.push(candidate);
            if let Some(unique) = unique.as_ref() {
                write_unique(&mut output, origin, round, unique)?;
                counters.rounds_completed = round + 1;
                let next_beam =
                    select_elitist_beam(&scored_generation, &next_generation, options.beam_width);
                checkpoint_if_requested(
                    &options,
                    input_fingerprint,
                    round + 1,
                    anchor_cursor,
                    base_evaluations,
                    &evaluated,
                    &next_beam,
                )?;
                write_pair_seed_if_requested(&mut output, &options, pair_seed.as_ref())?;
                write_summary(
                    &mut output,
                    "unique",
                    &corpus_stats,
                    &counters,
                    global_best.as_ref(),
                    started.elapsed().as_secs_f64(),
                )?;
                return Ok(());
            }
        }
        beam = select_elitist_beam(&scored_generation, &next_generation, options.beam_width);
        counters.rounds_completed = round + 1;
        checkpoint_if_requested(
            &options,
            input_fingerprint,
            round + 1,
            anchor_cursor,
            base_evaluations,
            &evaluated,
            &beam,
        )?;
        writeln!(
            output,
            "{{\"type\":\"round\",\"round\":{round},\"anchor_cursor\":{anchor_cursor},\"anchor_bases\":{},\"base_evaluations\":{base_evaluations},\"next_beam\":{},\"best_count\":{},\"best_exact\":{}}}",
            anchor_schedule.len(),
            beam.len(),
            global_best.as_ref().map_or(u64::MAX, |best| best.count),
            global_best.as_ref().is_some_and(|best| best.exact),
        )
        .map_err(|error| error.to_string())?;
        output.flush().map_err(|error| error.to_string())?;
    }

    let status = if base_evaluations >= options.max_base_evaluations {
        "base-evaluation-limit"
    } else if next_round >= options.rounds || counters.rounds_completed >= options.rounds {
        "round-limit"
    } else if anchor_cursor < anchor_schedule.len() {
        "anchor-schedule-incomplete"
    } else {
        "frontier-exhausted"
    };
    write_pair_seed_if_requested(&mut output, &options, pair_seed.as_ref())?;
    write_summary(
        &mut output,
        status,
        &corpus_stats,
        &counters,
        global_best.as_ref(),
        started.elapsed().as_secs_f64(),
    )?;
    Ok(())
}

fn load_corpus(
    text: &str,
) -> (
    BTreeMap<FullLayout, AnchorSource>,
    CorpusStats,
    Vec<(usize, String)>,
) {
    let mut anchors: BTreeMap<FullLayout, AnchorSource> = BTreeMap::new();
    let mut stats = CorpusStats::default();
    let mut invalid = Vec::new();
    for (line_index, raw_line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        stats.lines += 1;
        let parsed = (|| -> Result<(u64, FullLayout), String> {
            let (declared, layout_text) = line
                .split_once(';')
                .ok_or("missing semicolon between count and layout")?;
            let declared = declared
                .trim()
                .parse::<u64>()
                .map_err(|_| "invalid declared solution count")?;
            let paths = parse_nested_paths(layout_text)?;
            stats.parsed += 1;
            let layout = normalize_982(paths)?;
            Solver::blank(&layout.paths()).map_err(|error| error.to_string())?;
            Ok((declared, canonical_full(layout)))
        })();
        match parsed {
            Ok((declared, layout)) => {
                stats.geometry_valid += 1;
                match anchors.entry(layout) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        let mut declared_counts = BTreeSet::new();
                        declared_counts.insert(declared);
                        entry.insert(AnchorSource {
                            declared_counts,
                            lines: vec![line_number],
                        });
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        stats.duplicate_layouts += 1;
                        entry.get_mut().declared_counts.insert(declared);
                        entry.get_mut().lines.push(line_number);
                    }
                }
            }
            Err(error) => {
                stats.invalid += 1;
                invalid.push((line_number, error));
            }
        }
    }
    (anchors, stats, invalid)
}

fn normalize_982(mut paths: Vec<Vec<u8>>) -> Result<FullLayout, String> {
    if paths.len() != 3 {
        return Err(format!(
            "expected exactly three thermometers, found {}",
            paths.len()
        ));
    }
    paths.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    let lengths = [paths[0].len(), paths[1].len(), paths[2].len()];
    if lengths != [9, 8, 2] {
        return Err(format!(
            "expected thermometer lengths 9+8+2, found {}+{}+{}",
            lengths[0], lengths[1], lengths[2]
        ));
    }
    Ok(FullLayout {
        path9: paths.remove(0),
        path8: paths.remove(0),
        path2: paths.remove(0),
    })
}

struct NestedPathParser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> NestedPathParser<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            position: 0,
        }
    }

    fn parse(mut self) -> Result<Vec<Vec<u8>>, String> {
        self.skip_space();
        self.expect(b'[', "layout must start with '['")?;
        let mut paths = Vec::new();
        loop {
            self.skip_space();
            if self.consume(b']') {
                break;
            }
            if !paths.is_empty() {
                self.expect(b',', "expected ',' between thermometers")?;
                self.skip_space();
            }
            paths.push(self.parse_path()?);
        }
        self.skip_space();
        if self.position != self.bytes.len() {
            return Err(format!(
                "unexpected trailing input at byte {}",
                self.position
            ));
        }
        Ok(paths)
    }

    fn parse_path(&mut self) -> Result<Vec<u8>, String> {
        let open = self
            .peek()
            .ok_or("unexpected end while reading thermometer")?;
        let close = match open {
            b'(' => b')',
            b'[' => b']',
            _ => return Err(format!("expected '(' or '[' at byte {}", self.position)),
        };
        self.position += 1;
        let mut cells = Vec::new();
        loop {
            self.skip_space();
            if self.consume(close) {
                break;
            }
            if !cells.is_empty() {
                self.expect(b',', "expected ',' between cells")?;
                self.skip_space();
                if self.consume(close) {
                    break;
                }
            }
            let start = self.position;
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.position += 1;
            }
            if start == self.position {
                return Err(format!("expected cell number at byte {}", self.position));
            }
            let value = std::str::from_utf8(&self.bytes[start..self.position])
                .expect("ASCII digits are UTF-8")
                .parse::<u16>()
                .map_err(|_| format!("invalid cell number at byte {start}"))?;
            if value >= CELLS as u16 {
                return Err(format!("cell {value} is outside 0..=80"));
            }
            cells.push(value as u8);
        }
        Ok(cells)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: u8, message: &str) -> Result<(), String> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(format!("{message} at byte {}", self.position))
        }
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.position += 1;
        }
    }
}

fn parse_nested_paths(text: &str) -> Result<Vec<Vec<u8>>, String> {
    NestedPathParser::new(text).parse()
}

fn transform_cell(cell: u8, spatial: u8) -> u8 {
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
        _ => unreachable!(),
    };
    next_row * 9 + next_column
}

fn transform_path(path: &[u8], spatial: u8, reverse: bool) -> Vec<u8> {
    let mut transformed = path
        .iter()
        .map(|&cell| transform_cell(cell, spatial))
        .collect::<Vec<_>>();
    if reverse {
        transformed.reverse();
    }
    transformed
}

fn canonical_full(layout: FullLayout) -> FullLayout {
    canonical_full_with_solution(layout, None).0
}

fn canonical_full_with_solution(
    layout: FullLayout,
    solution: Option<[u8; CELLS]>,
) -> (FullLayout, Option<[u8; CELLS]>) {
    let mut best = layout.clone();
    let mut best_solution = solution;
    for reverse in [false, true] {
        for spatial in 0..8 {
            let candidate = FullLayout {
                path9: transform_path(&layout.path9, spatial, reverse),
                path8: transform_path(&layout.path8, spatial, reverse),
                path2: transform_path(&layout.path2, spatial, reverse),
            };
            if candidate < best {
                best = candidate;
                best_solution = solution.map(|grid| transform_grid(grid, spatial, reverse));
            }
        }
    }
    (best, best_solution)
}

fn transform_grid(grid: [u8; CELLS], spatial: u8, reverse: bool) -> [u8; CELLS] {
    let mut transformed = [0u8; CELLS];
    for (cell, digit) in grid.into_iter().enumerate() {
        transformed[transform_cell(cell as u8, spatial) as usize] =
            if reverse { 10 - digit } else { digit };
    }
    transformed
}

fn canonical_base(base: BaseLayout) -> BaseLayout {
    canonical_base_with_solution(base, None).0
}

fn canonical_base_with_solution(
    base: BaseLayout,
    solution: Option<[u8; CELLS]>,
) -> (BaseLayout, Option<[u8; CELLS]>) {
    let mut best = base.clone();
    let mut best_solution = solution;
    for reverse in [false, true] {
        for spatial in 0..8 {
            let candidate = BaseLayout {
                path9: transform_path(&base.path9, spatial, reverse),
                path8: transform_path(&base.path8, spatial, reverse),
            };
            if candidate < best {
                best = candidate;
                best_solution = solution.map(|grid| transform_grid(grid, spatial, reverse));
            }
        }
    }
    (best, best_solution)
}

fn king_adjacent(left: u8, right: u8) -> bool {
    if left == right {
        return false;
    }
    let left_row = left / 9;
    let left_column = left % 9;
    let right_row = right / 9;
    let right_column = right % 9;
    left_row.abs_diff(right_row) <= 1 && left_column.abs_diff(right_column) <= 1
}

fn legal_base_mutations(base: &BaseLayout, target: Option<&[u8; CELLS]>) -> Vec<BaseLayout> {
    let paths = [&base.path9, &base.path8];
    let occupied = base
        .path9
        .iter()
        .chain(&base.path8)
        .copied()
        .collect::<HashSet<_>>();
    let mut result = BTreeSet::new();
    for (path_index, path) in paths.into_iter().enumerate() {
        for position in 0..path.len() {
            for replacement in 0u8..CELLS as u8 {
                if replacement == path[position]
                    || occupied.contains(&replacement)
                    || (position != 0 && !king_adjacent(path[position - 1], replacement))
                    || (position + 1 != path.len()
                        && !king_adjacent(replacement, path[position + 1]))
                {
                    continue;
                }
                let mut candidate = base.clone();
                if path_index == 0 {
                    candidate.path9[position] = replacement;
                } else {
                    candidate.path8[position] = replacement;
                }
                if target.is_some_and(|grid| {
                    !path_is_increasing(&candidate.path9, grid)
                        || !path_is_increasing(&candidate.path8, grid)
                }) {
                    continue;
                }
                let (candidate, preserved_solution) =
                    canonical_base_with_solution(candidate, target.copied());
                debug_assert!(preserved_solution.as_ref().is_none_or(|grid| {
                    path_is_increasing(&candidate.path9, grid)
                        && path_is_increasing(&candidate.path8, grid)
                }));
                result.insert(candidate);
            }
        }
    }
    result.into_iter().collect()
}

fn legal_two_cell_reroutes(base: &BaseLayout, target: Option<&[u8; CELLS]>) -> Vec<BaseLayout> {
    let paths = [&base.path9, &base.path8];
    let occupied = base
        .path9
        .iter()
        .chain(&base.path8)
        .copied()
        .collect::<HashSet<_>>();
    let one_cell = legal_base_mutations(base, target)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut result = BTreeSet::new();
    for (path_index, path) in paths.into_iter().enumerate() {
        for position in 0..path.len() - 1 {
            let old_left = path[position];
            let old_right = path[position + 1];
            let mut occupied_outside_segment = occupied.clone();
            occupied_outside_segment.remove(&old_left);
            occupied_outside_segment.remove(&old_right);
            for replacement_left in 0u8..CELLS as u8 {
                if replacement_left == old_left
                    || occupied_outside_segment.contains(&replacement_left)
                    || (position != 0 && !king_adjacent(path[position - 1], replacement_left))
                {
                    continue;
                }
                for replacement_right in 0u8..CELLS as u8 {
                    if replacement_right == old_right
                        || replacement_right == replacement_left
                        || occupied_outside_segment.contains(&replacement_right)
                        || !king_adjacent(replacement_left, replacement_right)
                        || (position + 2 != path.len()
                            && !king_adjacent(replacement_right, path[position + 2]))
                    {
                        continue;
                    }
                    let mut candidate = base.clone();
                    let candidate_path = if path_index == 0 {
                        &mut candidate.path9
                    } else {
                        &mut candidate.path8
                    };
                    candidate_path[position] = replacement_left;
                    candidate_path[position + 1] = replacement_right;
                    if target.is_some_and(|grid| {
                        !path_is_increasing(&candidate.path9, grid)
                            || !path_is_increasing(&candidate.path8, grid)
                    }) {
                        continue;
                    }
                    let (candidate, preserved_solution) =
                        canonical_base_with_solution(candidate, target.copied());
                    debug_assert!(preserved_solution.as_ref().is_none_or(|grid| {
                        path_is_increasing(&candidate.path9, grid)
                            && path_is_increasing(&candidate.path8, grid)
                    }));
                    if !one_cell.contains(&candidate) {
                        result.insert(candidate);
                    }
                }
            }
        }
    }
    result.into_iter().collect()
}

fn path_is_increasing(path: &[u8], grid: &[u8; CELLS]) -> bool {
    path.windows(2)
        .all(|edge| grid[edge[0] as usize] < grid[edge[1] as usize])
}

fn validate_solution_for_layout(grid: &[u8; CELLS], layout: &FullLayout) -> Result<(), String> {
    for house in 0..27 {
        let mut seen = 0u16;
        for position in 0..9 {
            let cell = match house {
                0..=8 => house * 9 + position,
                9..=17 => position * 9 + house - 9,
                _ => {
                    let box_index = house - 18;
                    ((box_index / 3) * 3 + position / 3) * 9 + (box_index % 3) * 3 + position % 3
                }
            };
            let digit = grid[cell];
            if !(1..=9).contains(&digit) {
                return Err(format!("solution cell {} has digit {digit}", cell + 1));
            }
            let bit = 1u16 << digit;
            if seen & bit != 0 {
                return Err(format!(
                    "solution repeats digit {digit} in Sudoku house {}",
                    house + 1
                ));
            }
            seen |= bit;
        }
    }
    for path in [&layout.path9, &layout.path8, &layout.path2] {
        if !path_is_increasing(path, grid) {
            return Err("enumerated solution violates its source thermometer layout".into());
        }
    }
    Ok(())
}

fn pair_cut_length(pair: &GridPair) -> u16 {
    let mut length = 0u16;
    let mut edges = 0usize;
    for left in 0..CELLS {
        for right in left + 1..CELLS {
            let row_distance = (left / 9).abs_diff(right / 9);
            let column_distance = (left % 9).abs_diff(right % 9);
            if row_distance > 1 || column_distance > 1 {
                continue;
            }
            for (lower, upper) in [(left, right), (right, left)] {
                edges += 1;
                if !(pair.first[lower] < pair.first[upper]
                    && pair.second[lower] < pair.second[upper])
                {
                    length += 1;
                }
            }
        }
    }
    debug_assert_eq!(edges, DIRECTED_EDGES);
    length
}

fn score_full_layout(layout: &FullLayout, cap: u64) -> Result<ScoredLayout, String> {
    let solver = Solver::blank(&layout.paths()).map_err(|error| error.to_string())?;
    let result = solver.count_up_to(cap);
    Ok(ScoredLayout {
        layout: layout.clone(),
        count: result.count,
        exact: !result.capped,
        cap,
        first_solution: result.first_solution,
    })
}

fn evaluate_base(
    base: &BaseLayout,
    caps: &[u64],
    collective_prefix: u64,
) -> Result<EvaluatedBase, String> {
    for (stage_index, &cap) in caps.iter().enumerate() {
        let stages = stage_index + 1;
        let result = score_nine_eight_extensions(&base.path9, &base.path8, cap, collective_prefix)
            .map_err(|error| error.to_string())?;
        let mut positive_extensions = 0usize;
        let mut zero_extensions = 0usize;
        let mut capped_extensions = 0usize;
        let mut best: Option<ScoredLayout> = None;
        for extension in &result.extensions {
            if extension.count == 0 {
                zero_extensions += 1;
                continue;
            }
            positive_extensions += 1;
            capped_extensions += usize::from(extension.capped());
            let raw_layout = FullLayout {
                path9: base.path9.clone(),
                path8: base.path8.clone(),
                path2: vec![extension.bulb, extension.tip],
            };
            let raw_solution = extension
                .first_witness
                .map(|index| result.witness_solutions[index as usize]);
            let (layout, first_solution) = canonical_full_with_solution(raw_layout, raw_solution);
            let candidate = ScoredLayout {
                layout,
                count: extension.count,
                exact: extension.exact,
                cap,
                first_solution,
            };
            debug_assert_eq!(candidate.layout.base(), *base);
            if best
                .as_ref()
                .is_none_or(|current| candidate.rank_key() < current.rank_key())
            {
                best = Some(candidate);
            }
        }
        let best_is_exact = best.as_ref().is_none_or(|candidate| candidate.exact);
        if best_is_exact || cap == *caps.last().expect("caps are non-empty") {
            return Ok(EvaluatedBase {
                base: base.clone(),
                best,
                positive_extensions,
                zero_extensions,
                capped_extensions,
                stages,
            });
        }
    }
    unreachable!("validated caps are non-empty")
}

fn best_anchors_by_base(scored: &[ScoredLayout]) -> BTreeMap<BaseLayout, ScoredLayout> {
    let mut result = BTreeMap::new();
    for anchor in scored {
        let base = anchor.layout.base();
        match result.entry(base) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(anchor.clone());
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if anchor.rank_key() < entry.get().rank_key() {
                    entry.insert(anchor.clone());
                }
            }
        }
    }
    result
}

fn anchor_schedule(anchors: &BTreeMap<BaseLayout, ScoredLayout>) -> Vec<BaseLayout> {
    let mut ranked = anchors
        .iter()
        .map(|(base, score)| (score.rank_key(), base.clone()))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.0.cmp(&right.0));
    ranked.into_iter().map(|(_, base)| base).collect()
}

fn select_evaluated_beam(candidates: &[EvaluatedBase], beam_width: usize) -> Vec<BaseLayout> {
    let mut ranked = candidates
        .iter()
        .filter(|candidate| candidate.best.is_some())
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.rank_key().cmp(&right.rank_key()));
    ranked
        .into_iter()
        .take(beam_width)
        .map(|candidate| candidate.base.clone())
        .collect()
}

fn select_elitist_beam(
    parents: &[EvaluatedBase],
    children: &[EvaluatedBase],
    beam_width: usize,
) -> Vec<BaseLayout> {
    let mut distinct = BTreeMap::<BaseLayout, &EvaluatedBase>::new();
    for candidate in parents.iter().chain(children) {
        if candidate.best.is_none() {
            continue;
        }
        match distinct.entry(candidate.base.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if candidate.rank_key() < entry.get().rank_key() {
                    entry.insert(candidate);
                }
            }
        }
    }
    let mut ranked = distinct.into_values().collect::<Vec<_>>();
    ranked.sort_by(|left, right| left.rank_key().cmp(&right.rank_key()));
    ranked
        .into_iter()
        .take(beam_width)
        .map(|candidate| candidate.base.clone())
        .collect()
}

fn round_robin_mutations(
    parents: &[EvaluatedBase],
    mut seen: HashSet<BaseLayout>,
    limit: usize,
    solution_preserving: bool,
    two_cell_reroutes: bool,
    counters: &mut RunCounters,
) -> Vec<MutationCandidate> {
    struct Neighborhood {
        one_cell: Vec<BaseLayout>,
        reroutes: Vec<BaseLayout>,
    }

    let neighborhoods = parents
        .iter()
        .filter(|parent| parent.best.is_some())
        .map(|parent| {
            let target = solution_preserving
                .then(|| parent.best.as_ref()?.first_solution.as_ref())
                .flatten();
            Neighborhood {
                one_cell: legal_base_mutations(&parent.base, target),
                reroutes: if two_cell_reroutes {
                    legal_two_cell_reroutes(&parent.base, target)
                } else {
                    Vec::new()
                },
            }
        })
        .collect::<Vec<_>>();
    let generated_one_cell = neighborhoods
        .iter()
        .map(|neighborhood| neighborhood.one_cell.len())
        .sum::<usize>();
    let generated_reroutes = neighborhoods
        .iter()
        .map(|neighborhood| neighborhood.reroutes.len())
        .sum::<usize>();
    counters.generated_mutations += generated_one_cell + generated_reroutes;
    counters.generated_one_cell_mutations += generated_one_cell;
    counters.generated_two_cell_reroutes += generated_reroutes;
    let mut result = Vec::with_capacity(limit);
    let mut depth = 0usize;
    while result.len() < limit {
        let mut any = false;
        for neighborhood in &neighborhoods {
            for (kind, candidate) in [
                (MutationKind::OneCell, neighborhood.one_cell.get(depth)),
                (
                    MutationKind::TwoCellReroute,
                    neighborhood.reroutes.get(depth),
                ),
            ] {
                let Some(candidate) = candidate else {
                    continue;
                };
                any = true;
                if seen.insert(candidate.clone()) {
                    result.push(MutationCandidate {
                        base: candidate.clone(),
                        kind,
                    });
                    if result.len() == limit {
                        break;
                    }
                } else {
                    counters.duplicate_mutations += 1;
                }
            }
            if result.len() == limit {
                break;
            }
        }
        if !any {
            break;
        }
        depth += 1;
    }
    result
}

fn mutation_allowance(
    remaining_evaluations: usize,
    unscheduled_anchors: usize,
    candidates_per_round: usize,
) -> usize {
    remaining_evaluations
        .saturating_sub(unscheduled_anchors)
        .min(candidates_per_round)
}

fn update_best(best: &mut Option<ScoredLayout>, candidate: Option<&ScoredLayout>) {
    let Some(candidate) = candidate else {
        return;
    };
    if best
        .as_ref()
        .is_none_or(|current| candidate.rank_key() < current.rank_key())
    {
        *best = Some(candidate.clone());
    }
}

fn parse_options() -> Result<Options, String> {
    let default_input = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate has a parent")
        .join("sources")
        .join("min_thermos_9_8_2.txt");
    let mut options = Options {
        input: default_input,
        output: None,
        checkpoint: None,
        resume: false,
        anchor_cap: DEFAULT_ANCHOR_CAP,
        gradient_caps: DEFAULT_GRADIENT_CAPS.to_vec(),
        collective_prefix: 128,
        beam_width: 64,
        anchor_batch: 32,
        rounds: 32,
        max_base_evaluations: 10_000,
        candidates_per_round: 256,
        report_below: 65,
        solution_preserving: false,
        two_cell_reroutes: false,
        dry_run: false,
        pair_seed_checkpoint: None,
        pair_seed_solution_cutoff: DEFAULT_PAIR_SEED_SOLUTION_CUTOFF,
        pair_seed_pairs_per_anchor: DEFAULT_PAIR_SEED_PAIRS_PER_ANCHOR,
    };
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--input" => options.input = PathBuf::from(next_value(&mut args, "--input")?),
            "--output" => {
                options.output = Some(PathBuf::from(next_value(&mut args, "--output")?));
            }
            "--checkpoint" => {
                options.checkpoint = Some(PathBuf::from(next_value(&mut args, "--checkpoint")?));
            }
            "--resume" => options.resume = true,
            "--anchor-cap" => options.anchor_cap = next_u64(&mut args, "--anchor-cap")?,
            "--gradient-caps" | "--caps" => {
                options.gradient_caps = parse_caps(&next_value(&mut args, &argument)?)?;
            }
            "--collective-prefix" => {
                options.collective_prefix = next_u64(&mut args, "--collective-prefix")?;
            }
            "--beam-width" => options.beam_width = next_usize(&mut args, "--beam-width")?,
            "--anchor-batch" => options.anchor_batch = next_usize(&mut args, "--anchor-batch")?,
            "--rounds" => options.rounds = next_usize(&mut args, "--rounds")?,
            "--max-base-evaluations" => {
                options.max_base_evaluations = next_usize(&mut args, "--max-base-evaluations")?;
            }
            "--candidates-per-round" => {
                options.candidates_per_round = next_usize(&mut args, "--candidates-per-round")?;
            }
            "--report-below" => {
                options.report_below = next_u64(&mut args, "--report-below")?;
            }
            "--pair-seed-checkpoint" => {
                options.pair_seed_checkpoint = Some(PathBuf::from(next_value(
                    &mut args,
                    "--pair-seed-checkpoint",
                )?));
            }
            "--pair-seed-solution-cutoff" => {
                options.pair_seed_solution_cutoff =
                    next_usize(&mut args, "--pair-seed-solution-cutoff")?;
            }
            "--pair-seed-pairs-per-anchor" => {
                options.pair_seed_pairs_per_anchor =
                    next_usize(&mut args, "--pair-seed-pairs-per-anchor")?;
            }
            "--solution-preserving-moves" => options.solution_preserving = true,
            "--unconstrained-moves" => options.solution_preserving = false,
            "--two-cell-reroutes" => options.two_cell_reroutes = true,
            "--dry-run" => options.dry_run = true,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    if options.anchor_cap < 2 {
        return Err("--anchor-cap must be at least two".into());
    }
    if options.gradient_caps.is_empty() {
        return Err("--gradient-caps must contain at least one cap".into());
    }
    if options.gradient_caps.iter().any(|&cap| cap < 2) {
        return Err("every --gradient-caps value must be at least two".into());
    }
    if !options
        .gradient_caps
        .windows(2)
        .all(|pair| pair[0] < pair[1])
    {
        return Err("--gradient-caps values must be strictly increasing".into());
    }
    if options.beam_width < 2 {
        return Err("--beam-width must be at least two".into());
    }
    if options.anchor_batch == 0 || options.anchor_batch >= options.beam_width {
        return Err("require 1 <= --anchor-batch < --beam-width".into());
    }
    if options.candidates_per_round == 0 && options.rounds != 0 {
        return Err("--candidates-per-round must be positive when --rounds is nonzero".into());
    }
    if options.resume && options.checkpoint.is_none() {
        return Err("--resume requires --checkpoint FILE".into());
    }
    if let Some(pair_seed) = &options.pair_seed_checkpoint {
        if options.pair_seed_solution_cutoff < 2 {
            return Err("--pair-seed-solution-cutoff must be at least two".into());
        }
        if options.pair_seed_pairs_per_anchor == 0 {
            return Err("--pair-seed-pairs-per-anchor must be positive".into());
        }
        for (other, name) in [
            (Some(&options.input), "--input"),
            (options.output.as_ref(), "--output"),
            (options.checkpoint.as_ref(), "--checkpoint"),
        ] {
            if let Some(other) = other
                && destinations_equal(pair_seed, other)?
            {
                return Err(format!(
                    "--pair-seed-checkpoint and {name} must name different files"
                ));
            }
        }
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
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
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

fn next_value(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn next_u64(args: &mut impl Iterator<Item = String>, name: &str) -> Result<u64, String> {
    next_value(args, name)?
        .parse()
        .map_err(|_| format!("invalid integer for {name}"))
}

fn next_usize(args: &mut impl Iterator<Item = String>, name: &str) -> Result<usize, String> {
    next_value(args, name)?
        .parse()
        .map_err(|_| format!("invalid integer for {name}"))
}

fn parse_caps(text: &str) -> Result<Vec<u64>, String> {
    text.split(',')
        .map(|part| {
            part.trim()
                .parse::<u64>()
                .map_err(|_| format!("invalid cap: {part}"))
        })
        .collect()
}

fn print_help() {
    println!(
        "thermo-9x8-guided [options]\n\
         \n\
         Deterministic bounded construction search for disjoint 9+8+2 thermos.\n\
         It is heuristic and cannot prove that no 19-cell puzzle exists.\n\
         \n\
         --input FILE                 legacy count;layout corpus\n\
         --output FILE                machine-readable JSONL event log\n\
         --checkpoint FILE            atomically replaced resumable state\n\
         --resume                     resume the supplied checkpoint\n\
         --anchor-cap N               one common cap for corpus verification (1025)\n\
         --gradient-caps 8,32,128     cheap common staged search caps\n\
         --collective-prefix N        shared base solutions per score stage\n\
         --beam-width N               bases retained per generation\n\
         --anchor-batch N             fresh corpus bases forced per generation\n\
         --rounds N                   maximum mutation generations\n\
         --max-base-evaluations N     maximum collectively scored bases\n\
         --candidates-per-round N     bound on mutation neighbors per round\n\
         --report-below N             emit detailed low-count candidates\n\
         --pair-seed-checkpoint FILE  export globally valid pairs for exact low-count layouts\n\
         --pair-seed-solution-cutoff N\n\
                                      include layouts with at most N solutions (65)\n\
         --pair-seed-pairs-per-anchor N\n\
                                      retain N shortest pair cuts per layout (64)\n\
         --solution-preserving-moves  restrict moves to a parent-solution symmetry orbit\n\
         --unconstrained-moves        use unrestricted moves (default; compatibility flag)\n\
         --two-cell-reroutes          also reroute consecutive cell pairs (default off)\n\
         --dry-run                    only validate and re-score all anchors"
    );
}

fn open_output(options: &Options) -> Result<Box<dyn Write>, String> {
    let Some(path) = &options.output else {
        return Ok(Box::new(BufWriter::new(std::io::stdout())));
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let file = if options.resume {
        OpenOptions::new().create(true).append(true).open(path)
    } else {
        OpenOptions::new().write(true).create_new(true).open(path)
    }
    .map_err(|error| format!("cannot open output {}: {error}", path.display()))?;
    Ok(Box::new(BufWriter::new(file)))
}

fn json_string(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + 2);
    output.push('"');
    for character in text.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                write!(output, "\\u{:04x}", character as u32).unwrap();
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn json_numbers(values: impl IntoIterator<Item = impl std::fmt::Display>) -> String {
    let values = values
        .into_iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",");
    format!("[{values}]")
}

fn json_full(layout: &FullLayout) -> String {
    format!(
        "[{},{},{}]",
        json_numbers(layout.path9.iter()),
        json_numbers(layout.path8.iter()),
        json_numbers(layout.path2.iter())
    )
}

fn json_base(base: &BaseLayout) -> String {
    format!(
        "[{},{}]",
        json_numbers(base.path9.iter()),
        json_numbers(base.path8.iter())
    )
}

fn json_solution(solution: Option<&[u8; CELLS]>) -> String {
    solution.map_or_else(
        || "null".into(),
        |grid| {
            json_string(
                &grid
                    .iter()
                    .map(|digit| char::from(b'0' + *digit))
                    .collect::<String>(),
            )
        },
    )
}

fn write_header(
    output: &mut impl Write,
    options: &Options,
    fingerprint: u64,
    stats: &CorpusStats,
    invalid: &[(usize, String)],
) -> Result<(), String> {
    writeln!(
        output,
        "{{\"type\":\"header\",\"schema\":1,\"search\":\"deterministic-gradient-guided-9+8+2\",\"exhaustive\":false,\"input\":{},\"input_fnv1a64\":\"{fingerprint:016x}\",\"anchor_cap\":{},\"gradient_caps\":{},\"collective_prefix\":{},\"beam_width\":{},\"anchor_batch\":{},\"rounds\":{},\"max_base_evaluations\":{},\"candidates_per_round\":{},\"solution_preserving_up_to_symmetry\":{},\"two_cell_reroutes\":{},\"symmetry\":\"D4+global-path-reversal\"}}",
        json_string(&options.input.display().to_string()),
        options.anchor_cap,
        json_numbers(&options.gradient_caps),
        options.collective_prefix,
        options.beam_width,
        options.anchor_batch,
        options.rounds,
        options.max_base_evaluations,
        options.candidates_per_round,
        options.solution_preserving,
        options.two_cell_reroutes,
    )
    .map_err(|error| error.to_string())?;
    writeln!(
        output,
        "{{\"type\":\"corpus\",\"lines\":{},\"parsed\":{},\"geometry_valid\":{},\"invalid\":{},\"duplicate_layouts\":{},\"distinct_layouts\":{},\"distinct_bases\":{}}}",
        stats.lines,
        stats.parsed,
        stats.geometry_valid,
        stats.invalid,
        stats.duplicate_layouts,
        stats.distinct_layouts,
        stats.distinct_bases,
    )
    .map_err(|error| error.to_string())?;
    for (line, error) in invalid {
        writeln!(
            output,
            "{{\"type\":\"invalid-corpus-line\",\"line\":{line},\"error\":{}}}",
            json_string(error)
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn write_anchor(
    output: &mut impl Write,
    scored: &ScoredLayout,
    source: &AnchorSource,
    declaration_matches: bool,
) -> Result<(), String> {
    writeln!(
        output,
        "{{\"type\":\"anchor\",\"paths\":{},\"source_lines\":{},\"declared_counts\":{},\"count\":{},\"exact\":{},\"cap\":{},\"declaration_matches\":{},\"solution\":{}}}",
        json_full(&scored.layout),
        json_numbers(&source.lines),
        json_numbers(&source.declared_counts),
        scored.count,
        scored.exact,
        scored.cap,
        declaration_matches,
        json_solution(scored.first_solution.as_ref()),
    )
    .map_err(|error| error.to_string())
}

fn write_candidate(
    output: &mut impl Write,
    origin: &str,
    round: usize,
    evaluation: usize,
    candidate: &EvaluatedBase,
    report_below: u64,
) -> Result<(), String> {
    let Some(best) = &candidate.best else {
        return writeln!(
            output,
            "{{\"type\":\"candidate\",\"origin\":{},\"round\":{round},\"evaluation\":{evaluation},\"base\":{},\"best\":null,\"positive_extensions\":0,\"zero_extensions\":{},\"stages\":{}}}",
            json_string(origin),
            json_base(&candidate.base),
            candidate.zero_extensions,
            candidate.stages,
        )
        .map_err(|error| error.to_string());
    };
    // All candidates are machine-readable at the threshold; higher scores get
    // a compact base-only record to keep long searches manageable.
    if best.count <= report_below || best.is_unique() {
        writeln!(
            output,
            "{{\"type\":\"candidate\",\"origin\":{},\"round\":{round},\"evaluation\":{evaluation},\"base\":{},\"paths\":{},\"count\":{},\"exact\":{},\"cap\":{},\"positive_extensions\":{},\"zero_extensions\":{},\"capped_extensions\":{},\"stages\":{},\"solution\":{}}}",
            json_string(origin),
            json_base(&candidate.base),
            json_full(&best.layout),
            best.count,
            best.exact,
            best.cap,
            candidate.positive_extensions,
            candidate.zero_extensions,
            candidate.capped_extensions,
            candidate.stages,
            json_solution(best.first_solution.as_ref()),
        )
    } else {
        writeln!(
            output,
            "{{\"type\":\"candidate\",\"origin\":{},\"round\":{round},\"evaluation\":{evaluation},\"base\":{},\"count\":{},\"exact\":{},\"cap\":{},\"stages\":{}}}",
            json_string(origin),
            json_base(&candidate.base),
            best.count,
            best.exact,
            best.cap,
            candidate.stages,
        )
    }
    .map_err(|error| error.to_string())
}

fn write_unique(
    output: &mut impl Write,
    origin: &str,
    round: usize,
    unique: &ScoredLayout,
) -> Result<(), String> {
    writeln!(
        output,
        "{{\"type\":\"unique\",\"origin\":{},\"round\":{round},\"paths\":{},\"count\":1,\"exact\":true,\"solution\":{}}}",
        json_string(origin),
        json_full(&unique.layout),
        json_solution(unique.first_solution.as_ref()),
    )
    .map_err(|error| error.to_string())
}

fn write_summary(
    output: &mut impl Write,
    status: &str,
    corpus: &CorpusStats,
    counters: &RunCounters,
    best: Option<&ScoredLayout>,
    elapsed_seconds: f64,
) -> Result<(), String> {
    let (best_layout, best_count, best_exact) = best.map_or_else(
        || ("null".into(), "null".into(), "null".into()),
        |best| {
            (
                json_full(&best.layout),
                best.count.to_string(),
                best.exact.to_string(),
            )
        },
    );
    writeln!(
        output,
        "{{\"type\":\"summary\",\"status\":{},\"exhaustive\":false,\"distinct_anchors\":{},\"declaration_mismatches\":{},\"zero_solution_anchors\":{},\"anchor_solver_calls\":{},\"pair_seed_solver_calls\":{},\"base_score_calls\":{},\"generated_mutations\":{},\"generated_one_cell_mutations\":{},\"generated_two_cell_reroutes\":{},\"duplicate_mutations\":{},\"rounds_completed\":{},\"best_paths\":{best_layout},\"best_count\":{best_count},\"best_exact\":{best_exact},\"elapsed_seconds\":{elapsed_seconds:.6}}}",
        json_string(status),
        corpus.distinct_layouts,
        corpus.declaration_mismatches,
        corpus.zero_solution_anchors,
        counters.anchor_solver_calls,
        counters.pair_seed_solver_calls,
        counters.base_solver_calls,
        counters.generated_mutations,
        counters.generated_one_cell_mutations,
        counters.generated_two_cell_reroutes,
        counters.duplicate_mutations,
        counters.rounds_completed,
    )
    .and_then(|_| output.flush())
    .map_err(|error| error.to_string())
}

fn write_pair_seed_if_requested(
    output: &mut impl Write,
    options: &Options,
    seed: Option<&PairSeedBuilder>,
) -> Result<(), String> {
    let Some(path) = &options.pair_seed_checkpoint else {
        debug_assert!(seed.is_none());
        return Ok(());
    };
    let seed = seed.ok_or("internal pair-seed state is missing")?;
    write_pair_seed_checkpoint(path, &seed.pairs)?;
    write_pair_seed_event(
        output,
        path,
        options.pair_seed_solution_cutoff,
        options.pair_seed_pairs_per_anchor,
        seed,
    )
}

fn write_pair_seed_event(
    output: &mut impl Write,
    path: &Path,
    solution_cutoff: usize,
    pairs_per_anchor: usize,
    seed: &PairSeedBuilder,
) -> Result<(), String> {
    writeln!(
        output,
        "{{\"type\":\"pair-seed-checkpoint\",\"path\":{},\"schema\":\"thermo-global-cegis-v1\",\"proof_basis\":\"fully-enumerated-classic-sudoku-solution-pairs\",\"gradient_used_as_proof\":false,\"solution_cutoff\":{solution_cutoff},\"pairs_per_layout\":{pairs_per_anchor},\"distinct_layouts_examined\":{},\"corpus_layouts\":{},\"guided_layouts\":{},\"duplicate_layouts_skipped\":{},\"exact_eligible_layouts\":{},\"layouts_above_cutoff\":{},\"layouts_with_fewer_than_two_solutions\":{},\"candidate_pairs\":{},\"selected_pairs_before_global_dedup\":{},\"duplicate_selected_pairs\":{},\"unique_pairs\":{},\"fnv1a64\":\"{:016x}\"}}",
        json_string(&path.display().to_string()),
        seed.layouts.len(),
        seed.corpus_layouts,
        seed.guided_layouts,
        seed.duplicate_layouts_skipped,
        seed.exact_eligible_layouts,
        seed.layouts_above_cutoff,
        seed.layouts_with_fewer_than_two_solutions,
        seed.candidate_pairs,
        seed.selected_pairs_before_dedup,
        seed.duplicate_selected_pairs,
        seed.pairs.len(),
        pair_seed_checksum(&seed.pairs),
    )
    .and_then(|_| output.flush())
    .map_err(|error| error.to_string())
}

fn pair_seed_checksum(pairs: &BTreeSet<GridPair>) -> u64 {
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

fn write_pair_seed_checkpoint(path: &Path, pairs: &BTreeSet<GridPair>) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("checkpoint")
    ));
    let file = File::create(&temporary)
        .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
    let mut writer = BufWriter::new(&file);
    let checksum = pair_seed_checksum(pairs);
    writeln!(writer, "{GLOBAL_CHECKPOINT_HEADER}")
        .and_then(|_| writeln!(writer, "# budget={GLOBAL_CHECKPOINT_BUDGET}"))
        .and_then(|_| writeln!(writer, "# directed_edges={DIRECTED_EDGES}"))
        .and_then(|_| writeln!(writer, "# pairs={}", pairs.len()))
        .and_then(|_| writeln!(writer, "# fnv1a64={checksum:016x}"))
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    let mut line = [b'0'; CELLS * 2 + 1];
    line[CELLS] = b'|';
    for pair in pairs {
        if pair.first >= pair.second {
            return Err("internal pair seed is not canonically ordered".into());
        }
        for (target, digit) in line[..CELLS].iter_mut().zip(pair.first) {
            *target = b'0' + digit;
        }
        for (target, digit) in line[CELLS + 1..].iter_mut().zip(pair.second) {
            *target = b'0' + digit;
        }
        writer
            .write_all(&line)
            .and_then(|_| writer.write_all(b"\n"))
            .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    }
    writeln!(
        writer,
        "# end pairs={} fnv1a64={checksum:016x}",
        pairs.len()
    )
    .and_then(|_| writer.flush())
    .map_err(|error| format!("cannot finish {}: {error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", temporary.display()))?;
    drop(writer);
    drop(file);
    replace_file(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

fn checkpoint_if_requested(
    options: &Options,
    input_fingerprint: u64,
    next_round: usize,
    anchor_cursor: usize,
    base_evaluations: usize,
    evaluated: &BTreeMap<BaseLayout, EvaluatedBase>,
    beam: &[BaseLayout],
) -> Result<(), String> {
    let Some(path) = &options.checkpoint else {
        return Ok(());
    };
    let checkpoint = Checkpoint {
        input_fingerprint,
        anchor_cap: options.anchor_cap,
        gradient_caps: options.gradient_caps.clone(),
        collective_prefix: options.collective_prefix,
        beam_width: options.beam_width,
        anchor_batch: options.anchor_batch,
        candidates_per_round: options.candidates_per_round,
        solution_preserving: options.solution_preserving,
        two_cell_reroutes: options.two_cell_reroutes,
        next_round,
        anchor_cursor,
        base_evaluations,
        evaluated: evaluated.clone(),
        beam: beam.to_vec(),
    };
    write_checkpoint(path, &checkpoint)
}

fn write_checkpoint(path: &Path, checkpoint: &Checkpoint) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("checkpoint")
    ));
    let file = File::create(&temporary)
        .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
    let mut output = BufWriter::new(&file);
    writeln!(output, "{CHECKPOINT_HEADER}").map_err(|error| error.to_string())?;
    writeln!(
        output,
        "config\t{:016x}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        checkpoint.input_fingerprint,
        checkpoint.anchor_cap,
        encode_u64s(&checkpoint.gradient_caps),
        checkpoint.collective_prefix,
        checkpoint.beam_width,
        checkpoint.anchor_batch,
        checkpoint.candidates_per_round,
        u8::from(checkpoint.solution_preserving),
        u8::from(checkpoint.two_cell_reroutes),
    )
    .map_err(|error| error.to_string())?;
    writeln!(
        output,
        "state\t{}\t{}\t{}",
        checkpoint.next_round, checkpoint.anchor_cursor, checkpoint.base_evaluations
    )
    .map_err(|error| error.to_string())?;
    for candidate in checkpoint.evaluated.values() {
        if let Some(best) = &candidate.best {
            writeln!(
                output,
                "E\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                encode_base(&candidate.base),
                best.count,
                u8::from(best.exact),
                best.cap,
                encode_full(&best.layout),
                encode_solution(best.first_solution.as_ref()),
                candidate.positive_extensions,
                candidate.zero_extensions,
                candidate.capped_extensions,
                candidate.stages,
            )
        } else {
            writeln!(
                output,
                "E\t{}\t-\t-\t-\t-\t-\t{}\t{}\t{}\t{}",
                encode_base(&candidate.base),
                candidate.positive_extensions,
                candidate.zero_extensions,
                candidate.capped_extensions,
                candidate.stages,
            )
        }
        .map_err(|error| error.to_string())?;
    }
    for base in &checkpoint.beam {
        writeln!(output, "Q\t{}", encode_base(base)).map_err(|error| error.to_string())?;
    }
    writeln!(
        output,
        "# end evaluated={} beam={}",
        checkpoint.evaluated.len(),
        checkpoint.beam.len()
    )
    .and_then(|_| output.flush())
    .map_err(|error| format!("cannot finish {}: {error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("cannot sync {}: {error}", temporary.display()))?;
    drop(output);
    drop(file);
    replace_file(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

fn load_checkpoint(path: &Path) -> Result<Checkpoint, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read checkpoint {}: {error}", path.display()))?;
    let mut lines = text.lines();
    if lines.next() != Some(CHECKPOINT_HEADER) {
        return Err(format!(
            "{} has the wrong checkpoint header",
            path.display()
        ));
    }
    let config = lines
        .next()
        .ok_or("checkpoint is missing config")?
        .split('\t')
        .collect::<Vec<_>>();
    if !matches!(config.len(), 9 | 10) || config[0] != "config" {
        return Err("malformed checkpoint config".into());
    }
    let input_fingerprint =
        u64::from_str_radix(config[1], 16).map_err(|_| "invalid checkpoint input fingerprint")?;
    let anchor_cap = parse_field(config[2], "anchor cap")?;
    let gradient_caps = decode_u64s(config[3])?;
    let collective_prefix = parse_field(config[4], "collective prefix")?;
    let beam_width = parse_field(config[5], "beam width")?;
    let anchor_batch = parse_field(config[6], "anchor batch")?;
    let candidates_per_round = parse_field(config[7], "candidates per round")?;
    let solution_preserving = parse_bool_field(config[8], "solution preserving")?;
    // The ninth field predates reroutes; old checkpoints remain resumable when
    // the new opt-in move is left disabled.
    let two_cell_reroutes = if config.len() == 10 {
        parse_bool_field(config[9], "two-cell reroutes")?
    } else {
        false
    };

    let state = lines
        .next()
        .ok_or("checkpoint is missing state")?
        .split('\t')
        .collect::<Vec<_>>();
    if state.len() != 4 || state[0] != "state" {
        return Err("malformed checkpoint state".into());
    }
    let next_round = parse_field(state[1], "next round")?;
    let anchor_cursor = parse_field(state[2], "anchor cursor")?;
    let base_evaluations = parse_field(state[3], "base evaluations")?;
    let mut evaluated = BTreeMap::new();
    let mut beam = Vec::new();
    let mut saw_footer = false;
    for line in lines {
        if line.starts_with("# end ") {
            saw_footer = true;
            break;
        }
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.first().copied() {
            Some("E") if fields.len() == 11 => {
                let base = decode_base(fields[1])?;
                let positive_extensions = parse_field(fields[7], "positive extensions")?;
                let zero_extensions = parse_field(fields[8], "zero extensions")?;
                let capped_extensions = parse_field(fields[9], "capped extensions")?;
                let stages = parse_field(fields[10], "stages")?;
                let best = if fields[2] == "-" {
                    if fields[3..7].iter().any(|field| *field != "-") {
                        return Err("malformed empty checkpoint evaluation".into());
                    }
                    None
                } else {
                    let count = parse_field(fields[2], "count")?;
                    let exact = parse_bool_field(fields[3], "exact")?;
                    let cap = parse_field(fields[4], "cap")?;
                    let layout = decode_full(fields[5])?;
                    if layout.base() != base {
                        return Err("checkpoint best layout does not match its base".into());
                    }
                    Some(ScoredLayout {
                        layout,
                        count,
                        exact,
                        cap,
                        first_solution: decode_solution(fields[6])?,
                    })
                };
                let candidate = EvaluatedBase {
                    base: base.clone(),
                    best,
                    positive_extensions,
                    zero_extensions,
                    capped_extensions,
                    stages,
                };
                if evaluated.insert(base, candidate).is_some() {
                    return Err("checkpoint repeats an evaluated base".into());
                }
            }
            Some("Q") if fields.len() == 2 => beam.push(decode_base(fields[1])?),
            _ => return Err(format!("malformed checkpoint line: {line}")),
        }
    }
    if !saw_footer {
        return Err("checkpoint is incomplete (missing footer)".into());
    }
    if base_evaluations != evaluated.len() {
        return Err(format!(
            "checkpoint says {base_evaluations} evaluations but stores {}",
            evaluated.len()
        ));
    }
    if beam.iter().any(|base| !evaluated.contains_key(base)) {
        return Err("checkpoint beam contains an unevaluated base".into());
    }
    Ok(Checkpoint {
        input_fingerprint,
        anchor_cap,
        gradient_caps,
        collective_prefix,
        beam_width,
        anchor_batch,
        candidates_per_round,
        solution_preserving,
        two_cell_reroutes,
        next_round,
        anchor_cursor,
        base_evaluations,
        evaluated,
        beam,
    })
}

fn validate_checkpoint(
    checkpoint: &Checkpoint,
    options: &Options,
    input_fingerprint: u64,
) -> Result<(), String> {
    if checkpoint.input_fingerprint != input_fingerprint {
        return Err("checkpoint was created from a different corpus".into());
    }
    if checkpoint.anchor_cap != options.anchor_cap
        || checkpoint.gradient_caps != options.gradient_caps
        || checkpoint.collective_prefix != options.collective_prefix
        || checkpoint.beam_width != options.beam_width
        || checkpoint.anchor_batch != options.anchor_batch
        || checkpoint.candidates_per_round != options.candidates_per_round
        || checkpoint.solution_preserving != options.solution_preserving
        || checkpoint.two_cell_reroutes != options.two_cell_reroutes
    {
        return Err("checkpoint search configuration does not match command line".into());
    }
    if options.rounds < checkpoint.next_round {
        return Err("--rounds is below the checkpoint's next round".into());
    }
    if options.max_base_evaluations < checkpoint.base_evaluations {
        return Err("--max-base-evaluations is below the checkpoint total".into());
    }
    Ok(())
}

fn encode_path(path: &[u8]) -> String {
    path.iter().map(u8::to_string).collect::<Vec<_>>().join(",")
}

fn decode_path(text: &str, length: usize) -> Result<Vec<u8>, String> {
    let path = if text.is_empty() {
        Vec::new()
    } else {
        text.split(',')
            .map(|cell| {
                cell.parse::<u8>()
                    .map_err(|_| format!("invalid checkpoint cell: {cell}"))
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    if path.len() != length || path.iter().any(|&cell| cell >= CELLS as u8) {
        return Err(format!(
            "invalid checkpoint path of expected length {length}"
        ));
    }
    Ok(path)
}

fn encode_base(base: &BaseLayout) -> String {
    format!("{}/{}", encode_path(&base.path9), encode_path(&base.path8))
}

fn decode_base(text: &str) -> Result<BaseLayout, String> {
    let (path9, path8) = text
        .split_once('/')
        .ok_or("invalid checkpoint base encoding")?;
    let base = BaseLayout {
        path9: decode_path(path9, 9)?,
        path8: decode_path(path8, 8)?,
    };
    if canonical_base(base.clone()) != base {
        return Err("checkpoint base is not canonical".into());
    }
    Solver::blank(&[base.path9.clone(), base.path8.clone()])
        .map_err(|error| format!("invalid checkpoint base: {error}"))?;
    Ok(base)
}

fn encode_full(layout: &FullLayout) -> String {
    format!(
        "{}/{}/{}",
        encode_path(&layout.path9),
        encode_path(&layout.path8),
        encode_path(&layout.path2)
    )
}

fn decode_full(text: &str) -> Result<FullLayout, String> {
    let parts = text.split('/').collect::<Vec<_>>();
    if parts.len() != 3 {
        return Err("invalid checkpoint full-layout encoding".into());
    }
    let layout = FullLayout {
        path9: decode_path(parts[0], 9)?,
        path8: decode_path(parts[1], 8)?,
        path2: decode_path(parts[2], 2)?,
    };
    if canonical_full(layout.clone()) != layout {
        return Err("checkpoint full layout is not canonical".into());
    }
    Solver::blank(&layout.paths())
        .map_err(|error| format!("invalid checkpoint layout: {error}"))?;
    Ok(layout)
}

fn encode_solution(solution: Option<&[u8; CELLS]>) -> String {
    solution.map_or_else(
        || "-".into(),
        |grid| grid.iter().map(|digit| char::from(b'0' + *digit)).collect(),
    )
}

fn decode_solution(text: &str) -> Result<Option<[u8; CELLS]>, String> {
    if text == "-" {
        return Ok(None);
    }
    if text.len() != CELLS || !text.bytes().all(|byte| (b'1'..=b'9').contains(&byte)) {
        return Err("invalid checkpoint solution".into());
    }
    let mut result = [0u8; CELLS];
    for (target, byte) in result.iter_mut().zip(text.bytes()) {
        *target = byte - b'0';
    }
    Ok(Some(result))
}

fn encode_u64s(values: &[u64]) -> String {
    values
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn decode_u64s(text: &str) -> Result<Vec<u64>, String> {
    text.split(',')
        .map(|value| parse_field(value, "cap"))
        .collect()
}

fn parse_field<T: std::str::FromStr>(text: &str, name: &str) -> Result<T, String> {
    text.parse()
        .map_err(|_| format!("invalid checkpoint {name}"))
}

fn parse_bool_field(text: &str, name: &str) -> Result<bool, String> {
    match text {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("invalid checkpoint {name}")),
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(not(windows))]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination).map_err(|error| {
        format!(
            "cannot atomically replace {} with {}: {error}",
            destination.display(),
            source.display()
        )
    })
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    const REPLACE_ATTEMPTS: usize = 50;
    const RETRY_DELAY: Duration = Duration::from_millis(50);
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
        // SAFETY: both pointers refer to live NUL-terminated UTF-16 buffers.
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
        "cannot atomically replace {} with {}: {}",
        destination.display(),
        source.display(),
        last_error.expect("at least one replace attempt")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KNOWN_THREE: &str = "[(19,29,28,20,11,12,13,3,4),(77,69,78,70,62,53,44,52),(41,51)]";

    fn known_layout() -> FullLayout {
        normalize_982(parse_nested_paths(KNOWN_THREE).unwrap()).unwrap()
    }

    #[test]
    fn legacy_parser_accepts_python_tuples_and_json_lists() {
        let tuples = parse_nested_paths(KNOWN_THREE).unwrap();
        let lists =
            parse_nested_paths("[[19,29,28,20,11,12,13,3,4], [77,69,78,70,62,53,44,52], [41,51]]")
                .unwrap();
        assert_eq!(tuples, lists);
        assert_eq!(normalize_982(tuples).unwrap(), known_layout());
        assert!(parse_nested_paths("[(1,2),]").is_err());
        assert!(parse_nested_paths("[(1,99)]").is_err());
    }

    #[test]
    fn corpus_validation_deduplicates_symmetry_and_rejects_overlap() {
        let layout = known_layout();
        let transformed = FullLayout {
            path9: transform_path(&layout.path9, 1, true),
            path8: transform_path(&layout.path8, 1, true),
            path2: transform_path(&layout.path2, 1, true),
        };
        let text = format!(
            "03;{KNOWN_THREE}\n03;{}\n03;[(19,29,28,20,11,12,13,3,4),(77,69,78,70,62,53,44,52),(44,53)]\n",
            json_full(&transformed)
        );
        let (anchors, stats, invalid) = load_corpus(&text);
        assert_eq!(stats.lines, 3);
        assert_eq!(stats.geometry_valid, 2);
        assert_eq!(stats.duplicate_layouts, 1);
        assert_eq!(stats.invalid, 1);
        assert_eq!(anchors.len(), 1);
        assert_eq!(invalid[0].0, 3);
        assert!(invalid[0].1.contains("multiple thermometers"));
    }

    #[test]
    fn canonicalization_moves_the_solution_with_the_layout() {
        let layout = known_layout();
        let solved = Solver::blank(&layout.paths())
            .unwrap()
            .count_up_to(2)
            .first_solution
            .unwrap();
        let spatial = 3;
        let reverse = true;
        let transformed_layout = FullLayout {
            path9: transform_path(&layout.path9, spatial, reverse),
            path8: transform_path(&layout.path8, spatial, reverse),
            path2: transform_path(&layout.path2, spatial, reverse),
        };
        let transformed_solution = transform_grid(solved, spatial, reverse);
        let (canonical, canonical_solution) =
            canonical_full_with_solution(transformed_layout, Some(transformed_solution));
        let canonical_solution = canonical_solution.unwrap();
        assert_eq!(canonical, canonical_full(layout));
        for path in [&canonical.path9, &canonical.path8, &canonical.path2] {
            assert!(path_is_increasing(path, &canonical_solution));
        }
        assert!(valid_sudoku(&canonical_solution));
    }

    #[test]
    fn solution_preserving_mutations_retain_a_symmetry_moved_parent_grid() {
        let canonical = canonical_full(known_layout());
        let solution = Solver::blank(&canonical.paths())
            .unwrap()
            .count_up_to(2)
            .first_solution
            .unwrap();
        let base = canonical.base();
        let mutations = legal_base_mutations(&base, Some(&solution));
        assert!(!mutations.is_empty());
        for mutation in mutations {
            let transformed_witness = [false, true].into_iter().any(|reverse| {
                (0..8).any(|spatial| {
                    let transformed = transform_grid(solution, spatial, reverse);
                    path_is_increasing(&mutation.path9, &transformed)
                        && path_is_increasing(&mutation.path8, &transformed)
                })
            });
            assert!(transformed_witness);
            Solver::blank(&[mutation.path9, mutation.path8]).unwrap();
        }
    }

    #[test]
    fn two_cell_reroutes_are_legal_and_escape_the_two_hop_neighborhood() {
        let base = canonical_full(known_layout()).base();
        let one_cell = legal_base_mutations(&base, None)
            .into_iter()
            .collect::<HashSet<_>>();
        let reroutes = legal_two_cell_reroutes(&base, None);
        assert!(!reroutes.is_empty());
        assert!(
            reroutes
                .iter()
                .all(|candidate| !one_cell.contains(candidate))
        );
        for candidate in &reroutes {
            Solver::blank(&[candidate.path9.clone(), candidate.path8.clone()]).unwrap();
        }

        let two_hop = one_cell
            .iter()
            .flat_map(|intermediate| legal_base_mutations(intermediate, None))
            .collect::<HashSet<_>>();
        assert!(
            reroutes
                .iter()
                .any(|candidate| !two_hop.contains(candidate))
        );
    }

    #[test]
    fn mutation_classes_are_deterministically_interleaved() {
        let layout = canonical_full(known_layout());
        let parent = EvaluatedBase {
            base: layout.base(),
            best: Some(ScoredLayout {
                layout,
                count: 3,
                exact: true,
                cap: 4,
                first_solution: None,
            }),
            positive_extensions: 1,
            zero_extensions: 0,
            capped_extensions: 0,
            stages: 1,
        };
        let generate = |two_cell_reroutes| {
            let mut counters = RunCounters::default();
            let candidates = round_robin_mutations(
                std::slice::from_ref(&parent),
                HashSet::new(),
                6,
                false,
                two_cell_reroutes,
                &mut counters,
            );
            (candidates, counters)
        };
        let (first, counters) = generate(true);
        let (second, _) = generate(true);
        assert_eq!(first, second);
        assert_eq!(
            first
                .iter()
                .map(|candidate| candidate.kind)
                .collect::<Vec<_>>(),
            vec![
                MutationKind::OneCell,
                MutationKind::TwoCellReroute,
                MutationKind::OneCell,
                MutationKind::TwoCellReroute,
                MutationKind::OneCell,
                MutationKind::TwoCellReroute,
            ]
        );
        assert!(counters.generated_one_cell_mutations > 0);
        assert!(counters.generated_two_cell_reroutes > 0);

        let (one_cell_only, counters) = generate(false);
        assert!(
            one_cell_only
                .iter()
                .all(|candidate| candidate.kind == MutationKind::OneCell)
        );
        assert_eq!(counters.generated_two_cell_reroutes, 0);
    }

    #[test]
    fn staged_base_score_finds_an_extension_no_worse_than_the_anchor() {
        let base = canonical_full(known_layout()).base();
        let scored = evaluate_base(&base, &[4], 8).unwrap();
        let best = scored.best.unwrap();
        assert!(best.count <= 3);
        assert!(best.exact);
        assert!(best.first_solution.is_some());
        assert_eq!(best.layout.base(), base);
    }

    #[test]
    fn elitist_beam_never_discards_a_better_parent() {
        fn evaluated(base: BaseLayout, layout: FullLayout, count: u64) -> EvaluatedBase {
            EvaluatedBase {
                base,
                best: Some(ScoredLayout {
                    layout,
                    count,
                    exact: true,
                    cap: 128,
                    first_solution: None,
                }),
                positive_extensions: 1,
                zero_extensions: 0,
                capped_extensions: 0,
                stages: 1,
            }
        }

        let layout = canonical_full(known_layout());
        let parent_base = layout.base();
        let child_base = legal_base_mutations(&parent_base, None)
            .into_iter()
            .next()
            .unwrap();
        let parent = evaluated(parent_base.clone(), layout.clone(), 4);
        let worse_child = evaluated(child_base.clone(), layout.clone(), 7);
        assert_eq!(
            select_elitist_beam(std::slice::from_ref(&parent), &[worse_child], 1),
            vec![parent_base.clone()]
        );

        let stale_parent = evaluated(parent_base.clone(), layout.clone(), 9);
        let improved_duplicate = evaluated(parent_base.clone(), layout.clone(), 3);
        let other_child = evaluated(child_base, layout, 5);
        assert_eq!(
            select_elitist_beam(&[stale_parent], &[other_child, improved_duplicate], 2)[0],
            parent_base
        );
    }

    #[test]
    fn reserved_budget_prevents_anchor_starvation_with_default_scale() {
        let anchor_count = 749usize;
        let max_evaluations = 10_000usize;
        let beam_width = 64usize;
        let anchor_batch = 32usize;
        let candidates_per_round = 256usize;
        let rounds = 32usize;
        let mut cursor = 0usize;
        let mut used = 0usize;
        let mut beam_empty = true;
        for _ in 0..rounds {
            let injection = if beam_empty { beam_width } else { anchor_batch };
            let injected = injection.min(anchor_count - cursor);
            cursor += injected;
            used += injected;
            let allowance = mutation_allowance(
                max_evaluations - used,
                anchor_count - cursor,
                candidates_per_round,
            );
            used += allowance;
            beam_empty = allowance == 0;
            if cursor == anchor_count {
                break;
            }
        }
        assert_eq!(cursor, anchor_count);
        assert!(used <= max_evaluations);
    }

    #[test]
    fn checkpoint_round_trip_preserves_target_witnesses_and_cursor() {
        let layout = canonical_full(known_layout());
        let scored = score_full_layout(&layout, 4).unwrap();
        let base = layout.base();
        let evaluated_base = EvaluatedBase {
            base: base.clone(),
            best: Some(scored),
            positive_extensions: 10,
            zero_extensions: 2,
            capped_extensions: 3,
            stages: 1,
        };
        let mut evaluated = BTreeMap::new();
        evaluated.insert(base.clone(), evaluated_base);
        let checkpoint = Checkpoint {
            input_fingerprint: 7,
            anchor_cap: 4,
            gradient_caps: vec![4, 8],
            collective_prefix: 8,
            beam_width: 2,
            anchor_batch: 1,
            candidates_per_round: 3,
            solution_preserving: true,
            two_cell_reroutes: true,
            next_round: 5,
            anchor_cursor: 6,
            base_evaluations: 1,
            evaluated,
            beam: vec![base],
        };
        let path = std::env::temp_dir().join(format!(
            "thermo-9x8-guided-test-{}.checkpoint",
            std::process::id()
        ));
        write_checkpoint(&path, &checkpoint).unwrap();
        let loaded = load_checkpoint(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(loaded.input_fingerprint, checkpoint.input_fingerprint);
        assert_eq!(loaded.anchor_cursor, checkpoint.anchor_cursor);
        assert_eq!(loaded.two_cell_reroutes, checkpoint.two_cell_reroutes);
        assert_eq!(loaded.evaluated, checkpoint.evaluated);
        assert_eq!(loaded.beam, checkpoint.beam);
    }

    #[test]
    fn pair_seed_retains_the_shortest_cut_and_deduplicates_globally() {
        let layout = canonical_full(known_layout());
        let batch = Solver::blank(&layout.paths()).unwrap().enumerate_up_to(3);
        assert!(batch.exhausted);
        assert_eq!(batch.solutions.len(), 3);
        let mut ranked = BTreeSet::new();
        for left in 0..batch.solutions.len() {
            for right in left + 1..batch.solutions.len() {
                let pair = GridPair::new(batch.solutions[left], batch.solutions[right]).unwrap();
                ranked.insert(RankedPair {
                    cut_length: pair_cut_length(&pair),
                    pair,
                });
            }
        }
        let expected = ranked.first().unwrap().pair;

        let mut seed = PairSeedBuilder::default();
        seed.add_layout(&layout, PairSeedOrigin::Corpus, 3, 1)
            .unwrap();
        assert_eq!(seed.pairs, BTreeSet::from([expected]));
        assert_eq!(seed.candidate_pairs, 3);
        assert_eq!(seed.selected_pairs_before_dedup, 1);
        assert_eq!(seed.duplicate_selected_pairs, 0);

        seed.add_layout(&layout, PairSeedOrigin::Guided, 3, 1)
            .unwrap();
        assert_eq!(seed.pairs, BTreeSet::from([expected]));
        assert_eq!(seed.candidate_pairs, 3);
        assert_eq!(seed.selected_pairs_before_dedup, 1);
        assert_eq!(seed.duplicate_selected_pairs, 0);
        assert_eq!(seed.duplicate_layouts_skipped, 1);

        let mut below_exact_count = PairSeedBuilder::default();
        below_exact_count
            .add_layout(&layout, PairSeedOrigin::Corpus, 2, 3)
            .unwrap();
        assert!(below_exact_count.pairs.is_empty());
        assert_eq!(below_exact_count.layouts_above_cutoff, 1);
        assert_eq!(below_exact_count.exact_eligible_layouts, 0);
    }

    #[test]
    fn pair_seed_checkpoint_matches_the_global_schema() {
        let layout = canonical_full(known_layout());
        let mut seed = PairSeedBuilder::default();
        seed.add_layout(&layout, PairSeedOrigin::Corpus, 3, 2)
            .unwrap();
        assert_eq!(seed.pairs.len(), 2);
        let path = std::env::temp_dir().join(format!(
            "thermo-9x8-guided-pair-seed-test-{}.checkpoint",
            std::process::id()
        ));
        write_pair_seed_checkpoint(&path, &seed.pairs).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        assert_eq!(lines[0], GLOBAL_CHECKPOINT_HEADER);
        assert_eq!(lines[1], "# budget=16");
        assert_eq!(lines[2], "# directed_edges=544");
        assert_eq!(lines[3], "# pairs=2");
        assert_eq!(
            lines[4],
            format!("# fnv1a64={:016x}", pair_seed_checksum(&seed.pairs))
        );
        assert_eq!(lines.len(), 8);
        assert!(lines[5..7].iter().all(|line| {
            line.len() == CELLS * 2 + 1
                && line.as_bytes()[CELLS] == b'|'
                && line
                    .bytes()
                    .enumerate()
                    .all(|(index, byte)| index == CELLS || (b'1'..=b'9').contains(&byte))
        }));
        assert_eq!(
            lines[7],
            format!(
                "# end pairs=2 fnv1a64={:016x}",
                pair_seed_checksum(&seed.pairs)
            )
        );
    }

    fn valid_sudoku(grid: &[u8; CELLS]) -> bool {
        for house in 0..27 {
            let mut seen = 0u16;
            for position in 0..9 {
                let cell = match house {
                    0..=8 => house * 9 + position,
                    9..=17 => position * 9 + house - 9,
                    _ => {
                        let box_index = house - 18;
                        ((box_index / 3) * 3 + position / 3) * 9
                            + (box_index % 3) * 3
                            + position % 3
                    }
                };
                let digit = grid[cell];
                if !(1..=9).contains(&digit) || seen & (1 << digit) != 0 {
                    return false;
                }
                seen |= 1 << digit;
            }
        }
        true
    }
}
