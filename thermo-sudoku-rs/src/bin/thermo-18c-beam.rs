//! Deterministic, bounded beam rounds over the independent generalized
//! 18-cell branching seed corpus.
//!
//! The frozen 71-seed artifact is strictly parsed and replayed. All seed
//! networks enter the exact count cache and visited archive, while a
//! diversity-aware 16-state frontier is expanded with four deterministic
//! target witnesses per network. An explicit continuation mode strictly loads
//! the complete round-one artifact, chooses a parent/footprint/topology-balanced
//! frontier, and expands eight farthest-first comparison-signature witnesses.
//! Moves retarget the same footprint or replace one cell by a cell in the
//! radius-one neighbourhood. Every resulting state crosses the validated
//! saturation/Hasse/canonicalization trust boundary before exact-vector
//! deduplication and capped counting.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use thermo_sudoku::thermo_graph::{
    CELLS, CanonicalState, Edge, Grid, Transform, canonical_saturated_state, grid_satisfies_edges,
    king_adjacent, network_sha256, sha256_hex, state_sha256, transform_cell, transform_edges,
    transform_grid, validate_complete_sudoku,
};
use thermo_sudoku::{SolveStats, Solver};

const SCHEMA: &str = "thermo-18c-beam-v1";
const CONTINUATION_SCHEMA: &str = "thermo-18c-beam-v2";
const ROOT_NEIGHBORHOOD_SCHEMA: &str = "thermo-18c-root-neighborhood-v1";
const EXTERNAL_ROOT_NEIGHBORHOOD_SCHEMA: &str = "thermo-18c-root-neighborhood-v2";
const ROOT_TWO_CELL_NEIGHBORHOOD_SCHEMA: &str = "thermo-18c-root-two-cell-neighborhood-v1";
const ALGORITHM_REVISION: &str =
    "strict-seed-replay-w16-q4-retarget-radius1-hasse-cache-configurable-probe-v1";
const CONTINUATION_ALGORITHM_REVISION: &str =
    "strict-round1-continuation-w16-q8-signature-diverse-balanced-retarget-radius1-v1";
const ROOT_NEIGHBORHOOD_ALGORITHM_REVISION: &str =
    "strict-seed-replay-exact-root-all-solutions-radius1-saturated-hasse-dedupe-v1";
const EXTERNAL_ROOT_NEIGHBORHOOD_ALGORITHM_REVISION: &str =
    "pinned-external-exact-record-replay-all-solutions-radius1-saturated-hasse-dedupe-v1";
const ROOT_TWO_CELL_NEIGHBORHOOD_ALGORITHM_REVISION: &str =
    "strict-seed42-replay-all-targets-removed-pair-shell2-shard-hasse-dedupe-cap129-v1";
const SEED_SCHEMA: &str = "thermo-18c-seed-harvest-v1";
const SEED_ALGORITHM_REVISION: &str =
    "verified-982-target-saturation-hasse-d4-complement-global-dedupe-v1";
const SEED_ARTIFACT_BYTES: usize = 124_417;
const SEED_ARTIFACT_SHA256: &str =
    "8e6a11d7d7e7f51dd497e64ca6d9ed7ae492b42fea60d46bb2aeded51f2bf99e";
const SEED_SOURCE_SHA256: &str = "79bec9ad12bf7c3c6cb28948e1c54cd98809929d5fe5a3003a8c6215367046a7";
const SEED_COUNT: usize = 71;
const SEED_SOLUTION_CAP: u64 = 1_024;

const ROUND1_ARTIFACT_BYTES: usize = 16_269_145;
const ROUND1_ARTIFACT_SHA256: &str =
    "d3b7571679a63c4e50085433b05d082000f8683d32baac0fa0564f5e6e1cdc6b";
const ROUND1_RECORDS_SHA256: &str =
    "a0e80b9224714df4279670e4a6de0ea1612578df41bf6dda68fd5e7f56ff0577";
const ROUND1_NETWORKS: usize = 9_311;
const ROUND1_EXACT_NETWORKS: usize = 143;
const ROUND1_LOWER_BOUND_NETWORKS: usize = 9_168;
const ROUND1_EXPANDED_TARGET_PAIRS: usize = 64;

const ROUNDS: usize = 1;
const BEAM_WIDTH: usize = 16;
const EXPLORATION_FRONTIER_SLOTS: usize = 4;
const ROUND1_TARGETS_PER_NETWORK: usize = 4;
const ROUND2_TARGETS_PER_NETWORK: usize = 8;
const ROUND2_TARGET_POOL_CAP: usize = 4_096;
const ROUND2_EXPLOIT_SLOTS: usize = 8;
const ROUND2_SEED_ANCHOR_SLOTS: usize = 4;
const ROUND2_BARRIER_SLOTS: usize = 4;
const NORMAL_CAP: u64 = 129;
const DEFAULT_EXPLORATION_PROBES: usize = 128;
const DEFAULT_EXPLORATION_CAP: u64 = 512;
const MAX_EXPLORATION_CAP: u64 = 4_096;
const RAW_MOVE_HARD_MAX: u64 = 72_640;
const ROUND2_RAW_MOVE_HARD_MAX: u64 = 145_280;
const DEFAULT_MAX_NEW_COUNTS: u64 = 75_000;
const DEFAULT_MAX_TOTAL_SOLVER_CALLS: u64 = 80_000;
const DEFAULT_ROOT_SEED_ORDINAL: usize = 42;
const ROOT_TWO_CELL_SEED_SHA256: &str =
    "000686520eb98f01cfee9ef0be013e1d3758bbb258add2b09866094ae31fd7ae";
const DEFAULT_ROOT_COUNT_CAP: u64 = 4_096;
const MAX_ROOT_COUNT_CAP: u64 = 65_536;
const ROOT_MOVES_PER_TARGET: u64 = 1 + 18 * 63;
const ROOT_TWO_CELL_REMOVAL_PAIRS: u64 = 18 * 17 / 2;
const ROOT_TWO_CELL_ADDITION_PAIRS: u64 = 63 * 62 / 2;
const ROOT_TWO_CELL_MOVES_PER_TARGET: u64 =
    ROOT_TWO_CELL_REMOVAL_PAIRS * ROOT_TWO_CELL_ADDITION_PAIRS;
const MAX_ROOT_TWO_CELL_REMOVAL_PAIRS_PER_SHARD: usize = 4;
const ROUND2_LANDSCAPE_ARTIFACT_BYTES: usize = 64_925_329;
const ROUND2_LANDSCAPE_ARTIFACT_SHA256: &str =
    "d1b080f52243775fba5446c0a84c0dfd6a14b705f16b471e57820f433922763a";
const FIRST_EXTERNAL_ROOT_SHA256: &str =
    "9c7213a6b17d41ad74b5c3ef33b92af58f9da754f770a4e20277c1d9661996aa";
const FIRST_EXTERNAL_ROOT_COUNT: u64 = 560;

#[derive(Clone, Debug)]
struct Options {
    seeds: PathBuf,
    continuation: Option<PathBuf>,
    output: PathBuf,
    progress_every: u64,
    exploration_probes: usize,
    exploration_cap: u64,
    max_new_counts: u64,
    max_total_solver_calls: u64,
    root_neighborhood: Option<RootNeighborhoodOptions>,
    root_two_cell_neighborhood: Option<RootTwoCellNeighborhoodOptions>,
}

#[derive(Clone, Debug)]
struct RootNeighborhoodOptions {
    seed_ordinal: Option<usize>,
    network_sha256: Option<String>,
    record_input: Option<PathBuf>,
    record_sha256: Option<String>,
    count_cap: u64,
}

#[derive(Clone, Debug)]
struct RootTwoCellNeighborhoodOptions {
    seed_ordinal: Option<usize>,
    network_sha256: Option<String>,
    first_removed_pair_ordinal: usize,
    last_removed_pair_ordinal: usize,
    count_cap: u64,
}

impl Options {
    fn target_limit(&self) -> usize {
        if self.continuation.is_some() {
            ROUND2_TARGETS_PER_NETWORK
        } else {
            ROUND1_TARGETS_PER_NETWORK
        }
    }

    fn raw_move_hard_max(&self) -> u64 {
        if self.continuation.is_some() {
            ROUND2_RAW_MOVE_HARD_MAX
        } else {
            RAW_MOVE_HARD_MAX
        }
    }
}

#[derive(Clone, Debug)]
struct BinaryProvenance {
    path: PathBuf,
    bytes: usize,
    sha256: String,
}

#[derive(Clone, Debug)]
struct ContinuationProvenance {
    path: PathBuf,
    bytes: usize,
    sha256: String,
    records_sha256: String,
    historical_whole_file_match: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SeedInput {
    ordinal: usize,
    network_sha256: String,
    state_sha256: String,
    solution_count: u64,
    cells: Vec<u8>,
    full_edges: Vec<Edge>,
    hasse_edges: Vec<Edge>,
    target: Grid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Score {
    count: u64,
    exact: bool,
    cap: u64,
}

impl Score {
    fn exact(count: u64, cap: u64) -> Self {
        Self {
            count,
            exact: true,
            cap,
        }
    }

    fn from_result(count: u64, capped: bool, cap: u64) -> Self {
        Self {
            count,
            exact: !capped,
            cap,
        }
    }

    fn relation(self) -> &'static str {
        if self.exact { "exact" } else { "at-least" }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TargetReservoir {
    targets: Vec<Grid>,
}

impl TargetReservoir {
    fn new() -> Self {
        Self {
            targets: Vec::new(),
        }
    }

    fn from_sorted(mut targets: Vec<Grid>, limit: usize) -> Self {
        targets.sort_unstable();
        targets.dedup();
        targets.truncate(limit);
        Self { targets }
    }

    fn insert(&mut self, target: Grid, limit: usize) {
        match self.targets.binary_search(&target) {
            Ok(_) => return,
            Err(index) => self.targets.insert(index, target),
        }
        self.targets.truncate(limit);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Origin {
    Seed {
        ordinal: usize,
    },
    Generated {
        parent_network_sha256: String,
        target_ordinal: usize,
        move_kind: MoveKind,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum MoveKind {
    Retarget,
    Swap { removed: u8, added: u8 },
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SourceStratum {
    parent_hasse_edges: Vec<Edge>,
    target_ordinal: usize,
}

#[derive(Clone, Debug)]
struct GeneratedProvenance {
    origin: Origin,
    source: SourceStratum,
}

#[derive(Clone, Debug)]
struct ArchiveEntry {
    hasse_edges: Vec<Edge>,
    representative_full_edges: Vec<Edge>,
    representative_target: Grid,
    cells: Vec<u8>,
    targets: TargetReservoir,
    score: Option<Score>,
    origin: Origin,
    occurrences: u64,
    expanded_in_round: bool,
    expanded_targets: BTreeSet<Grid>,
    high_cap_probed: bool,
    normal_stats: Option<SolveStats>,
    probe_stats: Option<SolveStats>,
}

impl ArchiveEntry {
    fn expansion_targets(&self, limit: usize) -> Vec<Grid> {
        let mut result = Vec::with_capacity(limit);
        result.push(self.representative_target);
        result.extend(
            self.targets
                .targets
                .iter()
                .copied()
                .filter(|target| target != &self.representative_target)
                .take(limit - 1),
        );
        result
    }

    fn has_unexpanded_target(&self, limit: usize) -> bool {
        self.expansion_targets(limit)
            .iter()
            .any(|target| !self.expanded_targets.contains(target))
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    state: CanonicalState,
    targets: TargetReservoir,
    origin: Origin,
    occurrences: u64,
}

type SeedReplay = (
    BTreeMap<Vec<Edge>, ArchiveEntry>,
    BTreeMap<Vec<Edge>, Score>,
    SolverTotals,
);

impl Candidate {
    fn new(state: CanonicalState, origin: Origin, target_limit: usize) -> Self {
        let mut targets = TargetReservoir::new();
        targets.insert(state.target, target_limit);
        Self {
            state,
            targets,
            origin,
            occurrences: 1,
        }
    }

    fn observe(&mut self, state: CanonicalState, origin: Origin, target_limit: usize) {
        self.occurrences += 1;
        self.targets.insert(state.target, target_limit);
        let candidate_rank = (&state.target, &state.full_edges, &state.cells, &origin);
        let current_rank = (
            &self.state.target,
            &self.state.full_edges,
            &self.state.cells,
            &self.origin,
        );
        if candidate_rank < current_rank {
            self.state = state;
            self.origin = origin;
        }
    }
}

impl Ord for Origin {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        origin_rank(self).cmp(&origin_rank(other))
    }
}

impl PartialOrd for Origin {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn origin_rank(origin: &Origin) -> (u8, usize, &str, usize, MoveKind) {
    match origin {
        Origin::Seed { ordinal } => (0, *ordinal, "", 0, MoveKind::Retarget),
        Origin::Generated {
            parent_network_sha256,
            target_ordinal,
            move_kind,
        } => (1, 0, parent_network_sha256, *target_ordinal, *move_kind),
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SolverTotals {
    calls: u64,
    nodes: u64,
    branches: u64,
    propagation_rounds: u64,
    comparison_revisions: u64,
    max_depth: u8,
}

impl SolverTotals {
    fn add(&mut self, stats: SolveStats) {
        self.calls += 1;
        self.nodes = self.nodes.saturating_add(stats.nodes);
        self.branches = self.branches.saturating_add(stats.branches);
        self.propagation_rounds = self
            .propagation_rounds
            .saturating_add(stats.propagation_rounds);
        self.comparison_revisions = self
            .comparison_revisions
            .saturating_add(stats.thermo_revisions);
        self.max_depth = self.max_depth.max(stats.max_depth);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RoundAccounting {
    seed_archive: u64,
    seed_frontier: u64,
    seed_pending: u64,
    seed_replay_calls: u64,
    target_pool_calls: u64,
    target_pool_solutions: u64,
    target_witnesses_selected: u64,
    previously_expanded_targets_skipped: u64,
    target_pool_exact_upgrades: u64,
    raw_move_attempts: u64,
    radius_rejections: u64,
    coverage_rejections: u64,
    accepted_observations: u64,
    visited_observations: u64,
    duplicate_new_observations: u64,
    new_canonical_networks: u64,
    normal_count_calls: u64,
    normal_exact: u64,
    normal_lower_bounds: u64,
    high_cap_probes_requested: u64,
    high_cap_probes_eligible: u64,
    high_cap_probe_cap: u64,
    high_cap_probe_calls: u64,
    high_cap_exact: u64,
    high_cap_lower_bounds: u64,
    count_cache_hits: u64,
    best_new_exact_count: Option<u64>,
    unique_found: bool,
    raw_ceiling_hit: bool,
    new_count_ceiling_hit: bool,
    total_call_ceiling_hit: bool,
}

impl RoundAccounting {
    fn round_complete(&self) -> bool {
        !self.raw_ceiling_hit
            && !self.new_count_ceiling_hit
            && !self.total_call_ceiling_hit
            && (self.unique_found
                || self.normal_count_calls + self.count_cache_hits == self.new_canonical_networks)
    }

    fn ceiling_hit(&self) -> bool {
        self.raw_ceiling_hit || self.new_count_ceiling_hit || self.total_call_ceiling_hit
    }
}

#[derive(Clone, Debug)]
struct RunOutcome {
    archive: BTreeMap<Vec<Edge>, ArchiveEntry>,
    initial_frontier: Vec<Vec<Edge>>,
    next_frontier: Vec<Vec<Edge>>,
    pending_seed_ordinals: Vec<usize>,
    frontier_roles: BTreeMap<Vec<Edge>, FrontierRole>,
    frontier_scores_at_selection: BTreeMap<Vec<Edge>, Score>,
    next_frontier_roles: BTreeMap<Vec<Edge>, FrontierRole>,
    target_audits: BTreeMap<Vec<Edge>, TargetSelectionAudit>,
    current_sources: BTreeMap<Vec<Edge>, BTreeSet<SourceStratum>>,
    continuation: Option<ContinuationProvenance>,
    count_cache: BTreeMap<Vec<Edge>, Score>,
    accounting: RoundAccounting,
    solver_totals: SolverTotals,
    status: &'static str,
    unique_network: Option<Vec<Edge>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TargetSelectionAudit {
    enumerated_solutions: usize,
    enumeration_exhausted: bool,
    enumeration_capped: bool,
    normalized_candidates: usize,
    unique_signatures: usize,
    signature_pairs: usize,
    selected_witnesses: usize,
    historical_signatures_excluded: usize,
    ordered_targets: Vec<Grid>,
    solver_stats: SolveStats,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RootMoveOrigin {
    target_ordinal: usize,
    move_kind: MoveKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExternalRootProvenance {
    path: PathBuf,
    bytes: usize,
    sha256: String,
    line_number: usize,
    schema: String,
    algorithm_revision: String,
    authentication: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExternalRootArtifactKind {
    PinnedRound2Landscape,
    CompletedRootNeighborhoodV1,
    CompletedRootNeighborhoodV2,
}

impl ExternalRootArtifactKind {
    fn schema(self) -> &'static str {
        match self {
            Self::PinnedRound2Landscape => CONTINUATION_SCHEMA,
            Self::CompletedRootNeighborhoodV1 => ROOT_NEIGHBORHOOD_SCHEMA,
            Self::CompletedRootNeighborhoodV2 => EXTERNAL_ROOT_NEIGHBORHOOD_SCHEMA,
        }
    }

    fn algorithm_revision(self) -> &'static str {
        match self {
            Self::PinnedRound2Landscape => CONTINUATION_ALGORITHM_REVISION,
            Self::CompletedRootNeighborhoodV1 => ROOT_NEIGHBORHOOD_ALGORITHM_REVISION,
            Self::CompletedRootNeighborhoodV2 => EXTERNAL_ROOT_NEIGHBORHOOD_ALGORITHM_REVISION,
        }
    }

    fn is_root_neighborhood(self) -> bool {
        !matches!(self, Self::PinnedRound2Landscape)
    }

    fn authentication(self) -> &'static str {
        match self {
            Self::PinnedRound2Landscape => "compiled-round2-size-and-sha256-pin",
            Self::CompletedRootNeighborhoodV1 | Self::CompletedRootNeighborhoodV2 => {
                "explicit-whole-file-sha256-pin"
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RootSource {
    FrozenSeed { ordinal: usize },
    ExternalRecord(ExternalRootProvenance),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExactRoot {
    network_sha256: String,
    state_sha256: String,
    solution_count: u64,
    cells: Vec<u8>,
    full_edges: Vec<Edge>,
    hasse_edges: Vec<Edge>,
    target: Grid,
    source: RootSource,
}

impl ExactRoot {
    fn from_seed(seed: &SeedInput) -> Self {
        Self {
            network_sha256: seed.network_sha256.clone(),
            state_sha256: seed.state_sha256.clone(),
            solution_count: seed.solution_count,
            cells: seed.cells.clone(),
            full_edges: seed.full_edges.clone(),
            hasse_edges: seed.hasse_edges.clone(),
            target: seed.target,
            source: RootSource::FrozenSeed {
                ordinal: seed.ordinal,
            },
        }
    }

    fn seed_ordinal(&self) -> Option<usize> {
        match self.source {
            RootSource::FrozenSeed { ordinal } => Some(ordinal),
            RootSource::ExternalRecord(_) => None,
        }
    }

    fn is_external(&self) -> bool {
        matches!(self.source, RootSource::ExternalRecord(_))
    }
}

#[derive(Clone, Debug)]
struct RootNetworkResult {
    state: CanonicalState,
    representative_origin: RootMoveOrigin,
    observed_target_ordinals: BTreeSet<usize>,
    occurrences: u64,
    score: Option<Score>,
    solver_stats: Option<SolveStats>,
    preexisting_seed_ordinal: Option<usize>,
    external_root_replay_cache: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RootNeighborhoodAccounting {
    seed_replay_calls: u64,
    root_exact_replay_calls: u64,
    root_enumeration_calls: u64,
    raw_move_attempts: u64,
    radius_rejections: u64,
    coverage_rejections: u64,
    accepted_observations: u64,
    distinct_observed_networks: u64,
    duplicate_observations: u64,
    preexisting_seed_networks_observed: u64,
    new_canonical_networks: u64,
    count_cache_hits: u64,
    frozen_seed_cache_hits: u64,
    external_root_cache_hits: u64,
    network_count_calls: u64,
    exact_networks: u64,
    lower_bound_networks: u64,
    unclassified_networks: u64,
}

#[derive(Clone, Debug)]
struct RootNeighborhoodOutcome {
    root: ExactRoot,
    root_exact_replay_stats: Option<SolveStats>,
    root_solutions: Vec<Grid>,
    root_enumeration_stats: SolveStats,
    networks: BTreeMap<Vec<Edge>, RootNetworkResult>,
    accounting: RootNeighborhoodAccounting,
    solver_totals: SolverTotals,
    status: &'static str,
    unique_network: Option<Vec<Edge>>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RootTwoCellMoveOrigin {
    target_ordinal: usize,
    removed_pair_ordinal: usize,
    removed: [u8; 2],
    added: [u8; 2],
}

#[derive(Clone, Debug)]
struct RootTwoCellNetworkResult {
    representative_origin: RootTwoCellMoveOrigin,
    representative_canonical_target: Grid,
    observed_target_mask: [u64; 2],
    occurrences: u64,
    score: Option<Score>,
    solver_stats: Option<SolveStats>,
    preexisting_seed_ordinal: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct RootTwoCellFootprintMove {
    removed_pair_ordinal: usize,
    removed: [u8; 2],
    added: [u8; 2],
    footprint: [u8; 18],
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RootTwoCellAccounting {
    seed_replay_calls: u64,
    root_enumeration_calls: u64,
    raw_move_attempts: u64,
    geometric_incidence_rejections: u64,
    coverage_rejections: u64,
    accepted_observations: u64,
    distinct_observed_networks: u64,
    duplicate_observations: u64,
    preexisting_seed_networks_observed: u64,
    new_canonical_networks: u64,
    count_cache_hits: u64,
    network_count_calls: u64,
    exact_networks: u64,
    lower_bound_networks: u64,
    unclassified_networks: u64,
}

#[derive(Clone, Debug)]
struct RootTwoCellNeighborhoodOutcome {
    root: ExactRoot,
    root_solutions: Vec<Grid>,
    root_enumeration_stats: SolveStats,
    first_removed_pair_ordinal: usize,
    last_removed_pair_ordinal: usize,
    networks: BTreeMap<Vec<Edge>, RootTwoCellNetworkResult>,
    accounting: RootTwoCellAccounting,
    solver_totals: SolverTotals,
    status: &'static str,
    unique_network: Option<Vec<Edge>>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum FrontierRole {
    ExactExploit,
    ExactExploitUntouchedSeed,
    BarrierLe4x,
    Barrier4To8x,
    Barrier8To16x,
    BarrierGt16x,
    HotNovelty,
}

impl FrontierRole {
    fn name(self) -> &'static str {
        match self {
            Self::ExactExploit => "exact-exploit",
            Self::ExactExploitUntouchedSeed => "exact-exploit-untouched-seed",
            Self::BarrierLe4x => "barrier-le-4x",
            Self::Barrier4To8x => "barrier-4x-to-8x",
            Self::Barrier8To16x => "barrier-8x-to-16x",
            Self::BarrierGt16x => "barrier-gt-16x",
            Self::HotNovelty => "hot-novelty-censored",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TopologySignature {
    components: Vec<(usize, usize)>,
    degrees: Vec<(u8, u8)>,
}

type FrontierSelection = (Vec<Vec<Edge>>, BTreeMap<Vec<Edge>, FrontierRole>);

struct DiverseTargetSelection {
    reservoir: TargetReservoir,
    ordered_targets: Vec<Grid>,
    normalized_candidates: usize,
    unique_signatures: usize,
    signature_pairs: usize,
    historical_signatures_excluded: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> JsonParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn parse(mut self) -> Result<Json, String> {
        self.skip_space();
        let value = self.parse_value()?;
        self.skip_space();
        if self.position != self.bytes.len() {
            return Err(format!("trailing JSON data at byte {}", self.position));
        }
        Ok(value)
    }

    fn parse_value(&mut self) -> Result<Json, String> {
        self.skip_space();
        match self.peek() {
            Some(b'n') => self.literal(b"null", Json::Null),
            Some(b't') => self.literal(b"true", Json::Bool(true)),
            Some(b'f') => self.literal(b"false", Json::Bool(false)),
            Some(b'\"') => self.parse_string().map(Json::String),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(byte) => Err(format!(
                "unexpected JSON byte 0x{byte:02x} at {}",
                self.position
            )),
            None => Err("unexpected end of JSON".to_owned()),
        }
    }

    fn literal(&mut self, expected: &[u8], value: Json) -> Result<Json, String> {
        if self
            .bytes
            .get(self.position..self.position + expected.len())
            == Some(expected)
        {
            self.position += expected.len();
            Ok(value)
        } else {
            Err(format!("invalid JSON literal at byte {}", self.position))
        }
    }

    fn parse_array(&mut self) -> Result<Json, String> {
        self.expect(b'[')?;
        self.skip_space();
        let mut values = Vec::new();
        if self.consume(b']') {
            return Ok(Json::Array(values));
        }
        loop {
            values.push(self.parse_value()?);
            self.skip_space();
            if self.consume(b']') {
                break;
            }
            self.expect(b',')?;
        }
        Ok(Json::Array(values))
    }

    fn parse_object(&mut self) -> Result<Json, String> {
        self.expect(b'{')?;
        self.skip_space();
        let mut fields = BTreeMap::new();
        if self.consume(b'}') {
            return Ok(Json::Object(fields));
        }
        loop {
            self.skip_space();
            let key = self.parse_string()?;
            self.skip_space();
            self.expect(b':')?;
            let value = self.parse_value()?;
            if fields.insert(key.clone(), value).is_some() {
                return Err(format!("duplicate JSON field {key:?}"));
            }
            self.skip_space();
            if self.consume(b'}') {
                break;
            }
            self.expect(b',')?;
        }
        Ok(Json::Object(fields))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect(b'\"')?;
        let mut output = String::new();
        loop {
            let byte = self
                .next()
                .ok_or_else(|| "unterminated JSON string".to_owned())?;
            match byte {
                b'\"' => return Ok(output),
                b'\\' => {
                    let escaped = self
                        .next()
                        .ok_or_else(|| "unterminated JSON escape".to_owned())?;
                    match escaped {
                        b'\"' => output.push('\"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{08}'),
                        b'f' => output.push('\u{0c}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => {
                            let code = self.parse_hex4()?;
                            let character = char::from_u32(code)
                                .ok_or_else(|| format!("invalid Unicode escape {code:04x}"))?;
                            output.push(character);
                        }
                        _ => return Err(format!("invalid JSON escape \\{}", escaped as char)),
                    }
                }
                0x00..=0x1f => return Err("unescaped control byte in JSON string".to_owned()),
                0x20..=0x7f => output.push(byte as char),
                _ => {
                    let start = self.position - 1;
                    let rest = std::str::from_utf8(&self.bytes[start..])
                        .map_err(|_| format!("invalid UTF-8 in JSON string at byte {start}"))?;
                    let character = rest
                        .chars()
                        .next()
                        .ok_or_else(|| "unterminated UTF-8 character".to_owned())?;
                    self.position = start + character.len_utf8();
                    output.push(character);
                }
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, String> {
        let end = self.position + 4;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| "short Unicode escape".to_owned())?;
        self.position = end;
        let text = std::str::from_utf8(bytes).map_err(|_| "invalid Unicode escape".to_owned())?;
        u32::from_str_radix(text, 16).map_err(|_| format!("invalid Unicode escape {text:?}"))
    }

    fn parse_number(&mut self) -> Result<Json, String> {
        let start = self.position;
        if self.consume(b'-') && !self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            return Err("minus sign without JSON number".to_owned());
        }
        if self.consume(b'0') {
            if self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                return Err("leading zero in JSON number".to_owned());
            }
        } else {
            self.consume_digits()?;
        }
        if self.consume(b'.') {
            self.consume_digits()?;
        }
        if self.consume(b'e') || self.consume(b'E') {
            self.consume(b'+');
            self.consume(b'-');
            self.consume_digits()?;
        }
        let number = std::str::from_utf8(&self.bytes[start..self.position])
            .map_err(|_| "number is not UTF-8".to_owned())?;
        Ok(Json::Number(number.to_owned()))
    }

    fn consume_digits(&mut self) -> Result<(), String> {
        let start = self.position;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.position += 1;
        }
        if self.position == start {
            Err(format!("expected JSON digit at byte {}", self.position))
        } else {
            Ok(())
        }
    }

    fn skip_space(&mut self) {
        while self
            .peek()
            .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.position += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(format!(
                "expected JSON byte {:?} at {}",
                expected as char, self.position
            ))
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let value = self.peek()?;
        self.position += 1;
        Some(value)
    }
}

fn json_object(value: &Json) -> Result<&BTreeMap<String, Json>, String> {
    match value {
        Json::Object(fields) => Ok(fields),
        _ => Err("expected JSON object".to_owned()),
    }
}

fn json_array(value: &Json) -> Result<&[Json], String> {
    match value {
        Json::Array(values) => Ok(values),
        _ => Err("expected JSON array".to_owned()),
    }
}

fn json_field<'a>(fields: &'a BTreeMap<String, Json>, name: &str) -> Result<&'a Json, String> {
    fields
        .get(name)
        .ok_or_else(|| format!("missing JSON field {name:?}"))
}

fn json_string(value: &Json) -> Result<&str, String> {
    match value {
        Json::String(value) => Ok(value),
        _ => Err("expected JSON string".to_owned()),
    }
}

fn json_bool(value: &Json) -> Result<bool, String> {
    match value {
        Json::Bool(value) => Ok(*value),
        _ => Err("expected JSON Boolean".to_owned()),
    }
}

fn json_u64(value: &Json) -> Result<u64, String> {
    match value {
        Json::Number(value) => value
            .parse::<u64>()
            .map_err(|_| format!("expected unsigned integer, got {value:?}")),
        _ => Err("expected JSON number".to_owned()),
    }
}

fn json_usize(value: &Json) -> Result<usize, String> {
    usize::try_from(json_u64(value)?).map_err(|_| "JSON integer exceeds usize".to_owned())
}

fn require_string_field(
    fields: &BTreeMap<String, Json>,
    name: &str,
    expected: &str,
) -> Result<(), String> {
    let observed = json_string(json_field(fields, name)?)?;
    if observed == expected {
        Ok(())
    } else {
        Err(format!(
            "JSON field {name:?} is {observed:?}; expected {expected:?}"
        ))
    }
}

fn parse_grid(value: &Json) -> Result<Grid, String> {
    let text = json_string(value)?;
    if text.len() != CELLS || !text.bytes().all(|byte| matches!(byte, b'1'..=b'9')) {
        return Err("grid string must contain exactly 81 digits in 1..=9".to_owned());
    }
    let mut grid = [0u8; CELLS];
    for (slot, byte) in grid.iter_mut().zip(text.bytes()) {
        *slot = byte - b'0';
    }
    Ok(grid)
}

fn parse_cells(value: &Json) -> Result<Vec<u8>, String> {
    json_array(value)?
        .iter()
        .map(|value| {
            let cell = json_u64(value)?;
            u8::try_from(cell).map_err(|_| format!("cell {cell} exceeds u8"))
        })
        .collect()
}

fn parse_edges(value: &Json) -> Result<Vec<Edge>, String> {
    json_array(value)?
        .iter()
        .map(|edge| {
            let values = json_array(edge)?;
            if values.len() != 2 {
                return Err("edge must contain exactly two cells".to_owned());
            }
            let lower = u8::try_from(json_u64(&values[0])?)
                .map_err(|_| "edge lower cell exceeds u8".to_owned())?;
            let upper = u8::try_from(json_u64(&values[1])?)
                .map_err(|_| "edge upper cell exceeds u8".to_owned())?;
            Ok((lower, upper))
        })
        .collect()
}

fn parse_seed_record(fields: &BTreeMap<String, Json>) -> Result<SeedInput, String> {
    require_string_field(fields, "type", "seed")?;
    require_string_field(fields, "schema", SEED_SCHEMA)?;
    if !json_bool(json_field(fields, "exact")?)? || json_bool(json_field(fields, "capped")?)? {
        return Err("seed record must be exact and uncapped".to_owned());
    }
    let solution_cap = json_u64(json_field(fields, "solution_cap")?)?;
    if solution_cap != SEED_SOLUTION_CAP {
        return Err(format!(
            "seed solution_cap is {solution_cap}; expected {SEED_SOLUTION_CAP}"
        ));
    }
    let seed = SeedInput {
        ordinal: json_usize(json_field(fields, "ordinal")?)?,
        network_sha256: json_string(json_field(fields, "network_sha256")?)?.to_owned(),
        state_sha256: json_string(json_field(fields, "state_sha256")?)?.to_owned(),
        solution_count: json_u64(json_field(fields, "solution_count")?)?,
        cells: parse_cells(json_field(fields, "canonical_cells")?)?,
        full_edges: parse_edges(json_field(fields, "full_saturated_edges")?)?,
        hasse_edges: parse_edges(json_field(fields, "hasse_edges")?)?,
        target: parse_grid(json_field(fields, "canonical_target")?)?,
    };
    validate_seed_record(&seed)?;
    Ok(seed)
}

fn validate_seed_record(seed: &SeedInput) -> Result<(), String> {
    if seed.ordinal == 0 || seed.ordinal > SEED_COUNT {
        return Err(format!(
            "seed ordinal {} is outside 1..={SEED_COUNT}",
            seed.ordinal
        ));
    }
    if seed.solution_count == 0 || seed.solution_count >= SEED_SOLUTION_CAP {
        return Err(format!(
            "seed {} count {} is outside 1..{}",
            seed.ordinal,
            seed.solution_count,
            SEED_SOLUTION_CAP - 1
        ));
    }
    if seed.network_sha256 != network_sha256(&seed.hasse_edges) {
        return Err(format!("seed {} network SHA-256 mismatch", seed.ordinal));
    }
    if seed.state_sha256 != state_sha256(&seed.hasse_edges, &seed.target) {
        return Err(format!("seed {} state SHA-256 mismatch", seed.ordinal));
    }
    if !grid_satisfies_edges(&seed.target, &seed.hasse_edges) {
        return Err(format!(
            "seed {} target violates its Hasse DAG",
            seed.ordinal
        ));
    }
    let canonical = canonical_saturated_state(&seed.cells, &seed.target)
        .map_err(|error| format!("seed {} state validation: {error}", seed.ordinal))?;
    if canonical.cells != seed.cells
        || canonical.full_edges != seed.full_edges
        || canonical.hasse_edges != seed.hasse_edges
        || canonical.target != seed.target
    {
        return Err(format!(
            "seed {} is not its declared saturated canonical state",
            seed.ordinal
        ));
    }
    Ok(())
}

fn load_seed_artifact(bytes: &[u8]) -> Result<Vec<SeedInput>, String> {
    if bytes.len() != SEED_ARTIFACT_BYTES {
        return Err(format!(
            "seed artifact has {} bytes; expected {SEED_ARTIFACT_BYTES}",
            bytes.len()
        ));
    }
    let observed_sha256 = sha256_hex(bytes);
    if observed_sha256 != SEED_ARTIFACT_SHA256 {
        return Err(format!(
            "seed artifact SHA-256 is {observed_sha256}; expected {SEED_ARTIFACT_SHA256}"
        ));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("seed artifact is not UTF-8 at byte {}", error.valid_up_to()))?;
    if !text.ends_with('\n') {
        return Err("seed artifact must end with a newline".to_owned());
    }

    let mut seeds = Vec::new();
    let mut saw_header = false;
    let mut saw_summary = false;
    let mut invalid_records = 0usize;
    for (line_index, line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let parsed = JsonParser::new(line.as_bytes())
            .parse()
            .map_err(|error| format!("seed artifact line {line_number}: {error}"))?;
        let fields = json_object(&parsed)
            .map_err(|error| format!("seed artifact line {line_number}: {error}"))?;
        let record_type = json_string(json_field(fields, "type")?)?;
        match record_type {
            "header" => {
                if line_number != 1 || saw_header {
                    return Err("seed artifact header must be the first and only header".to_owned());
                }
                require_string_field(fields, "schema", SEED_SCHEMA)?;
                require_string_field(fields, "algorithm_revision", SEED_ALGORITHM_REVISION)?;
                let input = json_object(json_field(fields, "input")?)?;
                require_string_field(input, "sha256", SEED_SOURCE_SHA256)?;
                let independence = json_object(json_field(fields, "independence")?)?;
                if !json_bool(json_field(independence, "asserted_for_this_input")?)? {
                    return Err("seed artifact independence declaration is not bound".to_owned());
                }
                saw_header = true;
            }
            "invalid" => {
                if !saw_header || saw_summary || !seeds.is_empty() {
                    return Err("invalid record is out of canonical artifact order".to_owned());
                }
                require_string_field(fields, "schema", SEED_SCHEMA)?;
                invalid_records += 1;
            }
            "seed" => {
                if !saw_header || saw_summary {
                    return Err("seed record is outside header/summary boundaries".to_owned());
                }
                let seed = parse_seed_record(fields)
                    .map_err(|error| format!("seed artifact line {line_number}: {error}"))?;
                if seed.ordinal != seeds.len() + 1 {
                    return Err(format!(
                        "seed ordinal {} is out of sequence; expected {}",
                        seed.ordinal,
                        seeds.len() + 1
                    ));
                }
                seeds.push(seed);
            }
            "summary" => {
                if !saw_header || saw_summary {
                    return Err("seed artifact summary is duplicated or precedes header".to_owned());
                }
                require_string_field(fields, "schema", SEED_SCHEMA)?;
                require_string_field(fields, "status", "selected-scope-complete")?;
                require_string_field(fields, "input_sha256", SEED_SOURCE_SHA256)?;
                let accounting = json_object(json_field(fields, "accounting")?)?;
                if json_u64(json_field(accounting, "seed_records")?)? != SEED_COUNT as u64
                    || json_u64(json_field(accounting, "unique_seed_records")?)? != 0
                    || json_u64(json_field(accounting, "invalid_records")?)? != 1
                {
                    return Err(
                        "seed artifact summary accounting is not the frozen v1 scope".to_owned(),
                    );
                }
                if json_u64(json_field(fields, "best_exact_count")?)? != 128
                    || json_u64(json_field(fields, "solution_cap")?)? != SEED_SOLUTION_CAP
                {
                    return Err("seed artifact summary cap/best fields changed".to_owned());
                }
                saw_summary = true;
            }
            other => return Err(format!("unexpected seed artifact record type {other:?}")),
        }
    }
    if !saw_header || !saw_summary || invalid_records != 1 || seeds.len() != SEED_COUNT {
        return Err(format!(
            "seed artifact structure mismatch: header={saw_header}, summary={saw_summary}, invalid={invalid_records}, seeds={}",
            seeds.len()
        ));
    }
    let unique = seeds
        .iter()
        .map(|seed| seed.hasse_edges.clone())
        .collect::<BTreeSet<_>>();
    if unique.len() != seeds.len() {
        return Err("seed artifact repeats a canonical Hasse network".to_owned());
    }
    Ok(seeds)
}

fn parse_solve_stats(value: &Json) -> Result<Option<SolveStats>, String> {
    if matches!(value, Json::Null) {
        return Ok(None);
    }
    let fields = json_object(value)?;
    Ok(Some(SolveStats {
        nodes: json_u64(json_field(fields, "nodes")?)?,
        branches: json_u64(json_field(fields, "branches")?)?,
        propagation_rounds: json_u64(json_field(fields, "propagation_rounds")?)?,
        thermo_revisions: json_u64(json_field(fields, "comparison_revisions")?)?,
        max_depth: u8::try_from(json_u64(json_field(fields, "max_depth")?)?)
            .map_err(|_| "solver max_depth exceeds u8".to_owned())?,
    }))
}

fn parse_score(value: &Json) -> Result<Option<Score>, String> {
    if matches!(value, Json::Null) {
        return Ok(None);
    }
    let fields = json_object(value)?;
    let score = Score {
        count: json_u64(json_field(fields, "count")?)?,
        exact: json_bool(json_field(fields, "exact")?)?,
        cap: json_u64(json_field(fields, "cap")?)?,
    };
    if score.count == 0
        || json_string(json_field(fields, "relation")?)? != score.relation()
        || (score.exact && score.count > score.cap)
        || (!score.exact && score.count != score.cap)
    {
        return Err("invalid score relation/count/cap tuple".to_owned());
    }
    Ok(Some(score))
}

fn parse_origin(value: &Json) -> Result<Origin, String> {
    let fields = json_object(value)?;
    match json_string(json_field(fields, "kind")?)? {
        "seed" => Ok(Origin::Seed {
            ordinal: json_usize(json_field(fields, "ordinal")?)?,
        }),
        "generated" => {
            let move_fields = json_object(json_field(fields, "move")?)?;
            let move_kind = match json_string(json_field(move_fields, "kind")?)? {
                "retarget" => MoveKind::Retarget,
                "swap" => MoveKind::Swap {
                    removed: u8::try_from(json_u64(json_field(move_fields, "removed")?)?)
                        .map_err(|_| "move removed cell exceeds u8".to_owned())?,
                    added: u8::try_from(json_u64(json_field(move_fields, "added")?)?)
                        .map_err(|_| "move added cell exceeds u8".to_owned())?,
                },
                other => return Err(format!("unknown move kind {other:?}")),
            };
            Ok(Origin::Generated {
                parent_network_sha256: json_string(json_field(fields, "parent_network_sha256")?)?
                    .to_owned(),
                target_ordinal: json_usize(json_field(fields, "target_ordinal")?)?,
                move_kind,
            })
        }
        other => Err(format!("unknown origin kind {other:?}")),
    }
}

fn parse_grid_array(value: &Json) -> Result<Vec<Grid>, String> {
    json_array(value)?.iter().map(parse_grid).collect()
}

fn parse_string_array(value: &Json) -> Result<Vec<String>, String> {
    json_array(value)?
        .iter()
        .map(|value| Ok(json_string(value)?.to_owned()))
        .collect()
}

fn require_u64_field(
    fields: &BTreeMap<String, Json>,
    name: &str,
    expected: u64,
) -> Result<(), String> {
    let observed = json_u64(json_field(fields, name)?)?;
    if observed == expected {
        Ok(())
    } else {
        Err(format!(
            "JSON field {name:?} is {observed}; expected {expected}"
        ))
    }
}

fn require_bool_field(
    fields: &BTreeMap<String, Json>,
    name: &str,
    expected: bool,
) -> Result<(), String> {
    let observed = json_bool(json_field(fields, name)?)?;
    if observed == expected {
        Ok(())
    } else {
        Err(format!(
            "JSON field {name:?} is {observed}; expected {expected}"
        ))
    }
}

type Round1Load = (
    BTreeMap<Vec<Edge>, ArchiveEntry>,
    BTreeMap<Vec<Edge>, Score>,
    ContinuationProvenance,
);

fn load_round1_artifact(
    path: &Path,
    bytes: &[u8],
    seeds: &[SeedInput],
) -> Result<Round1Load, String> {
    let first_newline = bytes
        .iter()
        .position(|&byte| byte == b'\n')
        .ok_or_else(|| "round-one artifact has no complete header line".to_owned())?;
    let records = &bytes[first_newline + 1..];
    let records_sha256 = sha256_hex(records);
    if records_sha256 != ROUND1_RECORDS_SHA256 {
        return Err(format!(
            "round-one records SHA-256 is {records_sha256}; expected {ROUND1_RECORDS_SHA256}"
        ));
    }
    let whole_sha256 = sha256_hex(bytes);
    let historical_whole_file_match =
        bytes.len() == ROUND1_ARTIFACT_BYTES && whole_sha256 == ROUND1_ARTIFACT_SHA256;
    let text = std::str::from_utf8(bytes).map_err(|error| {
        format!(
            "round-one artifact is not UTF-8 at byte {}",
            error.valid_up_to()
        )
    })?;
    if !text.ends_with('\n') {
        return Err("round-one artifact must end with a newline".to_owned());
    }

    let mut archive = BTreeMap::new();
    let mut count_cache = BTreeMap::new();
    let mut sha_keys = BTreeMap::<String, Vec<Edge>>::new();
    let mut initial_hashes = BTreeSet::new();
    let mut next_hashes = BTreeSet::new();
    let mut expanded_pairs = 0usize;
    let mut exact = 0usize;
    let mut lower_bounds = 0usize;
    let mut summary_next = None;
    let mut saw_header = false;
    let mut saw_summary = false;

    for (line_index, line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let parsed = JsonParser::new(line.as_bytes())
            .parse()
            .map_err(|error| format!("round-one artifact line {line_number}: {error}"))?;
        let fields = json_object(&parsed)
            .map_err(|error| format!("round-one artifact line {line_number}: {error}"))?;
        match json_string(json_field(fields, "type")?)? {
            "header" => {
                if line_number != 1 || saw_header {
                    return Err("round-one header must be first and unique".to_owned());
                }
                require_string_field(fields, "schema", SCHEMA)?;
                require_string_field(fields, "algorithm_revision", ALGORITHM_REVISION)?;
                require_u64_field(fields, "rounds", 1)?;
                let scope = json_object(json_field(fields, "scope")?)?;
                require_bool_field(scope, "global_18c_exhaustive", false)?;
                require_bool_field(scope, "negative_result_proof", false)?;
                require_bool_field(scope, "complete_only_for_declared_round", true)?;
                let seed_input = json_object(json_field(fields, "seed_input")?)?;
                require_u64_field(seed_input, "bytes", SEED_ARTIFACT_BYTES as u64)?;
                require_string_field(seed_input, "sha256", SEED_ARTIFACT_SHA256)?;
                let configuration = json_object(json_field(fields, "configuration")?)?;
                require_u64_field(configuration, "W", BEAM_WIDTH as u64)?;
                require_u64_field(
                    configuration,
                    "target_witnesses_per_network_max",
                    ROUND1_TARGETS_PER_NETWORK as u64,
                )?;
                require_u64_field(configuration, "normal_cap", NORMAL_CAP)?;
                require_u64_field(configuration, "exploration_cap", MAX_EXPLORATION_CAP)?;
                require_u64_field(configuration, "raw_move_hard_max", RAW_MOVE_HARD_MAX)?;
                saw_header = true;
            }
            "network" => {
                if !saw_header || saw_summary {
                    return Err("round-one network is outside header/summary".to_owned());
                }
                require_string_field(fields, "schema", SCHEMA)?;
                let hasse_edges = parse_edges(json_field(fields, "hasse_edges")?)?;
                let hash = json_string(json_field(fields, "network_sha256")?)?.to_owned();
                if hash != network_sha256(&hasse_edges) {
                    return Err(format!(
                        "round-one network line {line_number} SHA-256 mismatch"
                    ));
                }
                let cells = parse_cells(json_field(fields, "canonical_cells")?)?;
                let representative_full_edges =
                    parse_edges(json_field(fields, "representative_full_saturated_edges")?)?;
                let representative_target =
                    parse_grid(json_field(fields, "representative_target")?)?;
                let canonical = canonical_saturated_state(&cells, &representative_target)
                    .map_err(|error| format!("round-one network {hash} representative: {error}"))?;
                if canonical.cells != cells
                    || canonical.hasse_edges != hasse_edges
                    || canonical.full_edges != representative_full_edges
                    || canonical.target != representative_target
                {
                    return Err(format!(
                        "round-one network {hash} representative is not the declared canonical state"
                    ));
                }
                let targets = parse_grid_array(json_field(fields, "target_reservoir")?)?;
                let expanded_targets = parse_grid_array(json_field(fields, "expanded_targets")?)?;
                if targets.is_empty() || targets.len() > ROUND1_TARGETS_PER_NETWORK {
                    return Err(format!(
                        "round-one network {hash} target reservoir has invalid length {}",
                        targets.len()
                    ));
                }
                let target_set = targets.iter().copied().collect::<BTreeSet<_>>();
                if target_set.len() != targets.len() {
                    return Err(format!(
                        "round-one network {hash} target reservoir contains duplicates"
                    ));
                }
                let expanded_set = expanded_targets.iter().copied().collect::<BTreeSet<_>>();
                if expanded_set.len() != expanded_targets.len() {
                    return Err(format!(
                        "round-one network {hash} expanded target set is invalid"
                    ));
                }
                for target in target_set.iter().chain(expanded_set.iter()) {
                    validate_complete_sudoku(target)
                        .map_err(|error| format!("round-one network {hash} target: {error}"))?;
                    if !grid_satisfies_edges(target, &hasse_edges) {
                        return Err(format!(
                            "round-one network {hash} target violates its exact Hasse network"
                        ));
                    }
                }
                let mut expansion_targets = vec![representative_target];
                expansion_targets.extend(
                    targets
                        .iter()
                        .copied()
                        .filter(|target| target != &representative_target)
                        .take(ROUND1_TARGETS_PER_NETWORK - 1),
                );
                let unexpanded = expansion_targets
                    .iter()
                    .filter(|target| !expanded_set.contains(*target))
                    .count();
                if json_usize(json_field(fields, "unexpanded_target_count")?)? != unexpanded {
                    return Err(format!(
                        "round-one network {hash} unexpanded target count mismatch"
                    ));
                }
                let score = parse_score(json_field(fields, "score")?)?
                    .ok_or_else(|| format!("round-one network {hash} lacks a score"))?;
                if score.exact {
                    exact += 1;
                } else {
                    lower_bounds += 1;
                }
                let origin = parse_origin(json_field(fields, "origin")?)?;
                let entry = ArchiveEntry {
                    hasse_edges: hasse_edges.clone(),
                    representative_full_edges,
                    representative_target,
                    cells,
                    targets: TargetReservoir { targets },
                    score: Some(score),
                    origin,
                    occurrences: json_u64(json_field(fields, "occurrences_in_source_stage")?)?,
                    expanded_in_round: false,
                    expanded_targets: expanded_set,
                    high_cap_probed: json_bool(json_field(fields, "high_cap_probed")?)?,
                    normal_stats: parse_solve_stats(json_field(fields, "normal_solver_stats")?)?,
                    probe_stats: parse_solve_stats(json_field(fields, "probe_solver_stats")?)?,
                };
                if json_bool(json_field(fields, "initial_frontier")?)? {
                    initial_hashes.insert(hash.clone());
                }
                if json_bool(json_field(fields, "next_frontier")?)? {
                    next_hashes.insert(hash.clone());
                }
                expanded_pairs += entry.expanded_targets.len();
                if archive.insert(hasse_edges.clone(), entry).is_some()
                    || count_cache.insert(hasse_edges.clone(), score).is_some()
                    || sha_keys.insert(hash, hasse_edges).is_some()
                {
                    return Err("round-one artifact repeats a network identity".to_owned());
                }
            }
            "summary" => {
                if !saw_header || saw_summary || line_number != ROUND1_NETWORKS + 2 {
                    return Err("round-one summary is misplaced or duplicated".to_owned());
                }
                require_string_field(fields, "schema", SCHEMA)?;
                require_string_field(fields, "status", "round-complete")?;
                require_bool_field(fields, "round_complete", true)?;
                require_bool_field(fields, "terminal_unique", false)?;
                let ceilings = json_object(json_field(fields, "ceilings")?)?;
                require_bool_field(ceilings, "raw_hit", false)?;
                require_bool_field(ceilings, "new_count_hit", false)?;
                require_bool_field(ceilings, "total_call_hit", false)?;
                let moves = json_object(json_field(fields, "moves")?)?;
                require_u64_field(moves, "raw_attempts", RAW_MOVE_HARD_MAX)?;
                require_u64_field(moves, "new_canonical_networks", 9_240)?;
                let classification = json_object(json_field(fields, "classification")?)?;
                require_u64_field(classification, "cache_entries", ROUND1_NETWORKS as u64)?;
                require_u64_field(
                    classification,
                    "archive_exact",
                    ROUND1_EXACT_NETWORKS as u64,
                )?;
                require_u64_field(
                    classification,
                    "archive_lower_bounds",
                    ROUND1_LOWER_BOUND_NETWORKS as u64,
                )?;
                require_u64_field(classification, "archive_unclassified", 0)?;
                require_u64_field(classification, "best_exact_count", 128)?;
                summary_next = Some(parse_string_array(json_field(
                    fields,
                    "next_frontier_network_sha256",
                )?)?);
                if !matches!(json_field(fields, "unique_network_sha256")?, Json::Null) {
                    return Err("round-one summary unexpectedly names a unique network".to_owned());
                }
                saw_summary = true;
            }
            other => return Err(format!("unexpected round-one record type {other:?}")),
        }
    }
    if !saw_header
        || !saw_summary
        || archive.len() != ROUND1_NETWORKS
        || exact != ROUND1_EXACT_NETWORKS
        || lower_bounds != ROUND1_LOWER_BOUND_NETWORKS
        || expanded_pairs != ROUND1_EXPANDED_TARGET_PAIRS
        || initial_hashes.len() != BEAM_WIDTH
        || next_hashes.len() != BEAM_WIDTH
    {
        return Err(format!(
            "round-one artifact structure mismatch: networks={}, exact={exact}, lower_bounds={lower_bounds}, expanded_pairs={expanded_pairs}, initial={}, next={}",
            archive.len(),
            initial_hashes.len(),
            next_hashes.len()
        ));
    }
    let summary_next = summary_next.expect("summary presence checked above");
    if summary_next.len() != BEAM_WIDTH
        || summary_next.iter().cloned().collect::<BTreeSet<_>>() != next_hashes
    {
        return Err("round-one summary/frontier flags disagree".to_owned());
    }
    for entry in archive.values() {
        match &entry.origin {
            Origin::Seed { ordinal } => {
                let seed = seeds
                    .get(ordinal.saturating_sub(1))
                    .ok_or_else(|| format!("round-one seed ordinal {ordinal} is invalid"))?;
                if seed.ordinal != *ordinal
                    || seed.hasse_edges != entry.hasse_edges
                    || seed.cells != entry.cells
                    || seed.full_edges != entry.representative_full_edges
                    || seed.target != entry.representative_target
                    || entry.score != Some(Score::exact(seed.solution_count, SEED_SOLUTION_CAP))
                {
                    return Err(format!(
                        "round-one seed ordinal {ordinal} disagrees with frozen seed artifact"
                    ));
                }
            }
            Origin::Generated {
                parent_network_sha256,
                ..
            } => {
                if !sha_keys.contains_key(parent_network_sha256) {
                    return Err(format!(
                        "round-one generated network names absent representative parent {parent_network_sha256}"
                    ));
                }
            }
        }
    }
    if archive
        .values()
        .filter(|entry| matches!(entry.origin, Origin::Seed { .. }))
        .count()
        != SEED_COUNT
    {
        return Err("round-one archive does not contain exactly 71 frozen seeds".to_owned());
    }

    Ok((
        archive,
        count_cache,
        ContinuationProvenance {
            path: path.to_path_buf(),
            bytes: bytes.len(),
            sha256: whole_sha256,
            records_sha256,
            historical_whole_file_match,
        },
    ))
}

fn seed_rank(seed: &SeedInput) -> (u64, &[Edge]) {
    (seed.solution_count, &seed.hasse_edges)
}

fn select_seed_frontier(seeds: &[SeedInput]) -> Result<(Vec<usize>, Vec<usize>), String> {
    if seeds.len() != SEED_COUNT {
        return Err(format!(
            "frontier selection received {} seeds; expected {SEED_COUNT}",
            seeds.len()
        ));
    }
    let mut global = (0..seeds.len()).collect::<Vec<_>>();
    global.sort_by(|&left, &right| seed_rank(&seeds[left]).cmp(&seed_rank(&seeds[right])));

    let mut selected = global[..8].to_vec();
    let mut selected_set = selected.iter().copied().collect::<BTreeSet<_>>();
    let mut represented = selected
        .iter()
        .map(|&index| seeds[index].cells.clone())
        .collect::<BTreeSet<_>>();

    select_best_new_footprints(
        seeds,
        &mut selected,
        &mut selected_set,
        &mut represented,
        4,
        |seed| seed.hasse_edges.len() == 16,
    );
    select_best_new_footprints(
        seeds,
        &mut selected,
        &mut selected_set,
        &mut represented,
        4,
        |_| true,
    );
    if selected.len() != BEAM_WIDTH {
        return Err(format!(
            "diversity-aware selection produced {} states; expected {BEAM_WIDTH}",
            selected.len()
        ));
    }

    let pending = global
        .into_iter()
        .filter(|index| !selected_set.contains(index))
        .collect::<Vec<_>>();
    Ok((selected, pending))
}

fn select_best_new_footprints<F>(
    seeds: &[SeedInput],
    selected: &mut Vec<usize>,
    selected_set: &mut BTreeSet<usize>,
    represented: &mut BTreeSet<Vec<u8>>,
    count: usize,
    filter: F,
) where
    F: Fn(&SeedInput) -> bool,
{
    let mut best_by_footprint: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for (index, seed) in seeds.iter().enumerate() {
        if selected_set.contains(&index) || represented.contains(&seed.cells) || !filter(seed) {
            continue;
        }
        match best_by_footprint.entry(seed.cells.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(index);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if seed_rank(seed) < seed_rank(&seeds[*entry.get()]) {
                    entry.insert(index);
                }
            }
        }
    }
    let mut candidates = best_by_footprint.into_values().collect::<Vec<_>>();
    candidates.sort_by(|&left, &right| seed_rank(&seeds[left]).cmp(&seed_rank(&seeds[right])));
    for index in candidates.into_iter().take(count) {
        selected.push(index);
        selected_set.insert(index);
        represented.insert(seeds[index].cells.clone());
    }
}

fn footprint_orbit_key(cells: &[u8]) -> Vec<u8> {
    (0..8)
        .map(|spatial| {
            let mut transformed = cells
                .iter()
                .map(|&cell| transform_cell(cell, spatial))
                .collect::<Vec<_>>();
            transformed.sort_unstable();
            transformed
        })
        .min()
        .expect("D4 footprint orbit is nonempty")
}

fn topology_signature(edges: &[Edge]) -> TopologySignature {
    let mut active = BTreeSet::new();
    let mut incoming = [0u8; CELLS];
    let mut outgoing = [0u8; CELLS];
    let mut neighbours = vec![Vec::<u8>::new(); CELLS];
    for &(lower, upper) in edges {
        active.insert(lower);
        active.insert(upper);
        outgoing[lower as usize] += 1;
        incoming[upper as usize] += 1;
        neighbours[lower as usize].push(upper);
        neighbours[upper as usize].push(lower);
    }
    let mut degrees = active
        .iter()
        .map(|&cell| (incoming[cell as usize], outgoing[cell as usize]))
        .collect::<Vec<_>>();
    degrees.sort_unstable();
    let mut reversed_degrees = degrees
        .iter()
        .map(|&(incoming, outgoing)| (outgoing, incoming))
        .collect::<Vec<_>>();
    reversed_degrees.sort_unstable();
    degrees = degrees.min(reversed_degrees);

    let mut unseen = active;
    let mut components = Vec::new();
    while let Some(&start) = unseen.first() {
        unseen.remove(&start);
        let mut stack = vec![start];
        let mut cells = 0usize;
        let mut degree_sum = 0usize;
        while let Some(cell) = stack.pop() {
            cells += 1;
            degree_sum += neighbours[cell as usize].len();
            for &next in &neighbours[cell as usize] {
                if unseen.remove(&next) {
                    stack.push(next);
                }
            }
        }
        components.push((cells, degree_sum / 2));
    }
    components.sort_unstable();
    TopologySignature {
        components,
        degrees,
    }
}

fn exact_historical_parent_keys(
    archive: &BTreeMap<Vec<Edge>, ArchiveEntry>,
) -> Result<BTreeMap<Vec<Edge>, Vec<Edge>>, String> {
    let sha_keys = archive
        .keys()
        .map(|key| (network_sha256(key), key.clone()))
        .collect::<BTreeMap<_, _>>();
    archive
        .iter()
        .map(|(key, entry)| {
            let parent = match &entry.origin {
                Origin::Seed { .. } => key.clone(),
                Origin::Generated {
                    parent_network_sha256,
                    ..
                } => sha_keys
                    .get(parent_network_sha256)
                    .cloned()
                    .ok_or_else(|| {
                        format!(
                            "historical origin {} is absent from exact archive keys",
                            parent_network_sha256
                        )
                    })?,
            };
            Ok((key.clone(), parent))
        })
        .collect()
}

fn round2_selection_parent_keys(
    archive: &BTreeMap<Vec<Edge>, ArchiveEntry>,
    current_sources: &BTreeMap<Vec<Edge>, BTreeSet<SourceStratum>>,
) -> Result<BTreeMap<Vec<Edge>, Vec<Edge>>, String> {
    let historical = exact_historical_parent_keys(archive)?;
    Ok(archive
        .keys()
        .map(|key| {
            let current = current_sources.get(key).and_then(|sources| {
                sources
                    .iter()
                    .map(|source| {
                        let score = archive[&source.parent_hasse_edges]
                            .score
                            .expect("round-two parent archive is classified");
                        (
                            !score.exact,
                            if score.exact { score.count } else { u64::MAX },
                            source.parent_hasse_edges.clone(),
                        )
                    })
                    .min()
                    .map(|(_, _, parent)| parent)
            });
            (
                key.clone(),
                current.unwrap_or_else(|| historical[key].clone()),
            )
        })
        .collect())
}

fn exact_log2_band(count: u64) -> i16 {
    debug_assert!(count > 0);
    count.ilog2() as i16 - 7
}

fn factor_band_role(child: u64, parent: u64) -> FrontierRole {
    if child <= parent.saturating_mul(4) {
        FrontierRole::BarrierLe4x
    } else if child <= parent.saturating_mul(8) {
        FrontierRole::Barrier4To8x
    } else if child <= parent.saturating_mul(16) {
        FrontierRole::Barrier8To16x
    } else {
        FrontierRole::BarrierGt16x
    }
}

fn select_round2_successor_frontier(
    archive: &BTreeMap<Vec<Edge>, ArchiveEntry>,
    current_sources: &BTreeMap<Vec<Edge>, BTreeSet<SourceStratum>>,
) -> Result<FrontierSelection, String> {
    let selection_parents = round2_selection_parent_keys(archive, current_sources)?;
    let mut selected = Vec::with_capacity(BEAM_WIDTH);
    let mut roles = BTreeMap::new();
    let mut footprints = BTreeSet::new();
    let mut topologies = BTreeSet::new();
    let mut parent_counts = BTreeMap::<Vec<Edge>, usize>::new();

    let exact_rank = |left: &Vec<Edge>, right: &Vec<Edge>| {
        let left_score = archive[left]
            .score
            .expect("round-one archive is classified");
        let right_score = archive[right]
            .score
            .expect("round-one archive is classified");
        left_score
            .count
            .cmp(&right_score.count)
            .then_with(|| left.cmp(right))
    };
    let mut exact = archive
        .iter()
        .filter_map(|(key, entry)| entry.score?.exact.then_some(key.clone()))
        .collect::<Vec<_>>();
    exact.sort_by(exact_rank);

    // Successor exploitation keeps raw-count ranking. The first pass adds
    // footprint orbits and each deterministic exact selected/current parent
    // Hasse key is capped at two; exact count remains primary inside each pass.
    for require_new_footprint in [true, false] {
        for key in &exact {
            if selected.len() == ROUND2_EXPLOIT_SLOTS || roles.contains_key(key) {
                continue;
            }
            let entry = &archive[key];
            let parent = selection_parents[key].clone();
            let footprint = footprint_orbit_key(&entry.cells);
            if parent_counts.get(&parent).copied().unwrap_or(0) >= 2
                || (require_new_footprint && footprints.contains(&footprint))
            {
                continue;
            }
            selected.push(key.clone());
            roles.insert(key.clone(), FrontierRole::ExactExploit);
            footprints.insert(footprint);
            topologies.insert(topology_signature(key));
            *parent_counts.entry(parent).or_default() += 1;
        }
    }
    if selected.len() != ROUND2_EXPLOIT_SLOTS {
        return Err(format!(
            "round-two exact exploitation selected {}; expected {ROUND2_EXPLOIT_SLOTS}",
            selected.len()
        ));
    }

    // Four explicit child-to-parent factor bands preserve multiplicative
    // exploration without scalarizing logarithmic energy. The <=4x band also
    // contains improvements and equal-count children. A band that is empty is
    // filled only after every populated band has supplied its lowest-count
    // balanced member.
    let barrier_roles = [
        FrontierRole::BarrierLe4x,
        FrontierRole::Barrier4To8x,
        FrontierRole::Barrier8To16x,
        FrontierRole::BarrierGt16x,
    ];
    let mut barrier_by_role = BTreeMap::<FrontierRole, Vec<Vec<Edge>>>::new();
    for (key, entry) in archive {
        let Some(score) = entry.score.filter(|score| score.exact) else {
            continue;
        };
        let Some(sources) = current_sources.get(key) else {
            continue;
        };
        let Some(parent_score) = sources
            .iter()
            .filter_map(|source| archive.get(&source.parent_hasse_edges)?.score)
            .filter(|score| score.exact)
            .min_by_key(|score| score.count)
        else {
            continue;
        };
        barrier_by_role
            .entry(factor_band_role(score.count, parent_score.count))
            .or_default()
            .push(key.clone());
    }
    for values in barrier_by_role.values_mut() {
        values.sort_by(exact_rank);
    }
    for role in barrier_roles {
        let Some(candidates) = barrier_by_role.get(&role) else {
            continue;
        };
        let best = candidates
            .iter()
            .filter(|key| !roles.contains_key(*key))
            .min_by_key(|key| {
                let entry = &archive[*key];
                let parent = selection_parents[*key].clone();
                let footprint = footprint_orbit_key(&entry.cells);
                let topology = topology_signature(key);
                (
                    parent_counts.get(&parent).copied().unwrap_or(0),
                    footprints.contains(&footprint),
                    topologies.contains(&topology),
                    entry.score.expect("barrier is exact").count,
                    (*key).clone(),
                )
            })
            .cloned();
        if let Some(key) = best {
            let entry = &archive[&key];
            let parent = selection_parents[&key].clone();
            footprints.insert(footprint_orbit_key(&entry.cells));
            topologies.insert(topology_signature(&key));
            *parent_counts.entry(parent).or_default() += 1;
            roles.insert(key.clone(), role);
            selected.push(key);
        }
    }
    if selected.len() < ROUND2_EXPLOIT_SLOTS + ROUND2_BARRIER_SLOTS {
        let mut fallback = barrier_by_role
            .values()
            .flatten()
            .filter(|key| !roles.contains_key(*key))
            .cloned()
            .collect::<Vec<_>>();
        fallback.sort_by(exact_rank);
        for key in fallback {
            if selected.len() == ROUND2_EXPLOIT_SLOTS + ROUND2_BARRIER_SLOTS {
                break;
            }
            let entry = &archive[&key];
            let parent_score = current_sources[&key]
                .iter()
                .filter_map(|source| archive.get(&source.parent_hasse_edges)?.score)
                .filter(|score| score.exact)
                .min_by_key(|score| score.count)
                .expect("barrier candidate has an exact current parent");
            let role = factor_band_role(
                entry.score.expect("barrier is exact").count,
                parent_score.count,
            );
            footprints.insert(footprint_orbit_key(&entry.cells));
            topologies.insert(topology_signature(&key));
            *parent_counts
                .entry(selection_parents[&key].clone())
                .or_default() += 1;
            roles.insert(key.clone(), role);
            selected.push(key);
        }
    }
    if selected.len() != ROUND2_EXPLOIT_SLOTS + ROUND2_BARRIER_SLOTS {
        return Err(format!(
            "round-two barrier selection produced {} total states; expected {}",
            selected.len(),
            ROUND2_EXPLOIT_SLOTS + ROUND2_BARRIER_SLOTS
        ));
    }

    // Censored states are not numerically compared with exact states. These
    // four slots maximize deterministic current-parent, footprint-orbit and
    // coarse topology novelty, then prefer denser Hasse graphs and exact ties.
    let censored = archive
        .iter()
        .filter_map(|(key, entry)| {
            (current_sources.contains_key(key) && entry.score.is_some_and(|score| !score.exact))
                .then_some(key.clone())
        })
        .collect::<Vec<_>>();
    for _ in 0..ROUND2_BARRIER_SLOTS {
        let key = censored
            .iter()
            .filter(|key| !roles.contains_key(*key))
            .min_by_key(|key| {
                let entry = &archive[*key];
                let parent = selection_parents[*key].clone();
                let footprint = footprint_orbit_key(&entry.cells);
                let topology = topology_signature(key);
                (
                    parent_counts.get(&parent).copied().unwrap_or(0),
                    footprints.contains(&footprint),
                    topologies.contains(&topology),
                    std::cmp::Reverse(key.len()),
                    (*key).clone(),
                )
            })
            .cloned()
            .ok_or_else(|| "round-two censored exploration pool is exhausted".to_owned())?;
        let entry = &archive[&key];
        footprints.insert(footprint_orbit_key(&entry.cells));
        topologies.insert(topology_signature(&key));
        *parent_counts
            .entry(selection_parents[&key].clone())
            .or_default() += 1;
        roles.insert(key.clone(), FrontierRole::HotNovelty);
        selected.push(key);
    }
    if selected.len() != BEAM_WIDTH || roles.len() != BEAM_WIDTH {
        return Err("round-two frontier does not contain exactly 16 distinct states".to_owned());
    }
    Ok((selected, roles))
}

fn select_round2_parent_frontier(
    archive: &BTreeMap<Vec<Edge>, ArchiveEntry>,
) -> Result<FrontierSelection, String> {
    let selection_parents = exact_historical_parent_keys(archive)?;
    let mut selected = Vec::with_capacity(BEAM_WIDTH);
    let mut roles = BTreeMap::new();
    let mut footprints = BTreeSet::new();
    let mut topologies = BTreeSet::new();
    let mut parents = BTreeMap::<Vec<Edge>, usize>::new();

    let exact_rank = |left: &Vec<Edge>, right: &Vec<Edge>| {
        archive[left]
            .score
            .expect("round-one archive is classified")
            .count
            .cmp(
                &archive[right]
                    .score
                    .expect("round-one archive is classified")
                    .count,
            )
            .then_with(|| left.cmp(right))
    };
    let mut generated_exact = archive
        .iter()
        .filter_map(|(key, entry)| {
            (matches!(entry.origin, Origin::Generated { .. })
                && entry.score.is_some_and(|score| score.exact))
            .then_some(key.clone())
        })
        .collect::<Vec<_>>();
    generated_exact.sort_by(exact_rank);
    for pass in 0..3 {
        for key in &generated_exact {
            if selected.len() == 8 || roles.contains_key(key) {
                continue;
            }
            let entry = &archive[key];
            let parent = selection_parents[key].clone();
            let footprint = footprint_orbit_key(&entry.cells);
            let topology = topology_signature(key);
            let eligible = match pass {
                0 => {
                    parents.get(&parent).copied().unwrap_or(0) == 0
                        && !footprints.contains(&footprint)
                        && !topologies.contains(&topology)
                }
                1 => {
                    parents.get(&parent).copied().unwrap_or(0) < 2
                        && !footprints.contains(&footprint)
                }
                _ => parents.get(&parent).copied().unwrap_or(0) < 2,
            };
            if !eligible {
                continue;
            }
            selected.push(key.clone());
            roles.insert(key.clone(), FrontierRole::ExactExploit);
            footprints.insert(footprint);
            topologies.insert(topology);
            *parents.entry(parent).or_default() += 1;
        }
    }
    if selected.len() != 8 {
        return Err(format!(
            "round-two parent exact-generated lane selected {}; expected 8",
            selected.len()
        ));
    }

    let mut pending_seeds = archive
        .iter()
        .filter_map(|(key, entry)| {
            (matches!(entry.origin, Origin::Seed { .. }) && entry.expanded_targets.is_empty())
                .then_some(key.clone())
        })
        .collect::<Vec<_>>();
    pending_seeds.sort_by(exact_rank);
    for pass in 0..2 {
        for key in &pending_seeds {
            if selected.len() == 12 || roles.contains_key(key) {
                continue;
            }
            let entry = &archive[key];
            let footprint = footprint_orbit_key(&entry.cells);
            let topology = topology_signature(key);
            if pass == 0 && (footprints.contains(&footprint) || topologies.contains(&topology)) {
                continue;
            }
            selected.push(key.clone());
            roles.insert(key.clone(), FrontierRole::ExactExploitUntouchedSeed);
            footprints.insert(footprint);
            topologies.insert(topology);
            *parents.entry(selection_parents[key].clone()).or_default() += 1;
        }
    }
    if selected.len() != 12 {
        return Err(format!(
            "round-two parent pending-seed lane selected {} total; expected 12",
            selected.len()
        ));
    }

    let censored = archive
        .iter()
        .filter_map(|(key, entry)| {
            entry
                .score
                .is_some_and(|score| !score.exact)
                .then_some(key.clone())
        })
        .collect::<Vec<_>>();
    for _ in 0..4 {
        let key = censored
            .iter()
            .filter(|key| !roles.contains_key(*key))
            .min_by_key(|key| {
                let entry = &archive[*key];
                let parent = selection_parents[*key].clone();
                let footprint = footprint_orbit_key(&entry.cells);
                let topology = topology_signature(key);
                (
                    parents.get(&parent).copied().unwrap_or(0),
                    footprints.contains(&footprint),
                    topologies.contains(&topology),
                    std::cmp::Reverse(key.len()),
                    (*key).clone(),
                )
            })
            .cloned()
            .ok_or_else(|| "round-two parent structural pool is exhausted".to_owned())?;
        let entry = &archive[&key];
        footprints.insert(footprint_orbit_key(&entry.cells));
        topologies.insert(topology_signature(&key));
        *parents.entry(selection_parents[&key].clone()).or_default() += 1;
        roles.insert(key.clone(), FrontierRole::HotNovelty);
        selected.push(key);
    }
    if selected.len() != BEAM_WIDTH || roles.len() != BEAM_WIDTH {
        return Err("round-two parent frontier is not exactly 8+4+4".to_owned());
    }
    Ok((selected, roles))
}

fn target_normal_form(edges: &[Edge], target: &Grid) -> Grid {
    let mut best = *target;
    for complement in [false, true] {
        for spatial in 0..8 {
            let transform = Transform {
                spatial,
                complement,
            };
            if transform_edges(edges, transform) == edges {
                best = best.min(transform_grid(target, transform));
            }
        }
    }
    best
}

fn relevant_comparison_pairs(cells: &[u8]) -> Vec<(u8, u8)> {
    let footprint = cells.iter().copied().collect::<BTreeSet<_>>();
    let mut pairs = Vec::new();
    for left in 0u8..CELLS as u8 {
        for right in left + 1..CELLS as u8 {
            if king_adjacent(left, right)
                && (footprint.contains(&left) || footprint.contains(&right))
            {
                pairs.push((left, right));
            }
        }
    }
    pairs
}

fn signature_symbol(target: &Grid, left: u8, right: u8) -> i8 {
    match target[left as usize].cmp(&target[right as usize]) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

fn target_signature(target: &Grid, pairs: &[(u8, u8)]) -> Vec<i8> {
    pairs
        .iter()
        .map(|&(left, right)| signature_symbol(target, left, right))
        .collect()
}

fn signature_distance(left: &[i8], right: &[i8]) -> usize {
    debug_assert_eq!(left.len(), right.len());
    left.iter()
        .zip(right)
        .filter(|(left, right)| left != right)
        .count()
}

fn choose_signature_diverse_targets(
    edges: &[Edge],
    cells: &[u8],
    representative: Grid,
    solutions: Vec<Grid>,
    expanded_targets: &BTreeSet<Grid>,
    limit: usize,
) -> Result<DiverseTargetSelection, String> {
    let pairs = relevant_comparison_pairs(cells);
    let representative_normal = target_normal_form(edges, &representative);
    let historical_normals = expanded_targets
        .iter()
        .map(|target| target_normal_form(edges, target))
        .collect::<BTreeSet<_>>();
    let mut candidates = solutions
        .into_iter()
        .map(|target| target_normal_form(edges, &target))
        .collect::<BTreeSet<_>>();
    candidates.insert(representative_normal);
    let normalized_candidates = candidates.len();
    let historical_signatures = historical_normals
        .iter()
        .map(|target| target_signature(target, &pairs))
        .collect::<BTreeSet<_>>();
    let representative_signature = target_signature(&representative_normal, &pairs);
    let mut by_signature = BTreeMap::<Vec<i8>, Grid>::new();
    for target in &candidates {
        let signature = target_signature(target, &pairs);
        by_signature
            .entry(signature)
            .and_modify(|current| *current = (*current).min(*target))
            .or_insert(*target);
    }
    by_signature.insert(representative_signature.clone(), representative_normal);
    let unique_signatures = by_signature.len();
    let historical_signatures_excluded = by_signature
        .keys()
        .filter(|signature| historical_signatures.contains(*signature))
        .count();
    by_signature.retain(|signature, _| !historical_signatures.contains(signature));
    if by_signature.len() < limit {
        return Err(format!(
            "only {} unexpanded relevant comparison signatures; need {limit}",
            by_signature.len()
        ));
    }

    let first_signature = if by_signature.contains_key(&representative_signature) {
        representative_signature
    } else {
        by_signature
            .keys()
            .next()
            .expect("at least eight signatures remain")
            .clone()
    };
    let mut chosen_signatures = vec![first_signature.clone()];
    let mut chosen_signature_set = BTreeSet::from([first_signature]);
    while chosen_signatures.len() < limit {
        let mut best: Option<(usize, Vec<i8>)> = None;
        for signature in by_signature
            .keys()
            .filter(|signature| !chosen_signature_set.contains(*signature))
        {
            let minimum = chosen_signatures
                .iter()
                .map(|selected| signature_distance(signature, selected))
                .min()
                .expect("chosen target set is nonempty");
            if best.as_ref().is_none_or(|(distance, current)| {
                minimum > *distance || (minimum == *distance && signature < current)
            }) {
                best = Some((minimum, signature.clone()));
            }
        }
        let (distance, signature) =
            best.ok_or_else(|| "target diversity selection exhausted".to_owned())?;
        if distance == 0 {
            return Err("target diversity selected a duplicate comparison signature".to_owned());
        }
        chosen_signature_set.insert(signature.clone());
        chosen_signatures.push(signature);
    }
    let chosen = chosen_signatures
        .iter()
        .map(|signature| by_signature[signature])
        .collect::<Vec<_>>();
    for target in &chosen {
        validate_complete_sudoku(target)
            .map_err(|error| format!("normalized target witness: {error}"))?;
        if !grid_satisfies_edges(target, edges) {
            return Err(
                "normalized target witness violates its exact parent Hasse network".to_owned(),
            );
        }
    }
    let ordered = chosen.clone();
    let mut reservoir = chosen;
    reservoir.sort_unstable();
    reservoir.dedup();
    if reservoir.len() != limit {
        return Err("pinned representative collapsed a selected target orbit".to_owned());
    }
    Ok(DiverseTargetSelection {
        reservoir: TargetReservoir { targets: reservoir },
        ordered_targets: ordered,
        normalized_candidates,
        unique_signatures,
        signature_pairs: pairs.len(),
        historical_signatures_excluded,
    })
}

fn enrich_round2_targets(
    archive: &mut BTreeMap<Vec<Edge>, ArchiveEntry>,
    count_cache: &mut BTreeMap<Vec<Edge>, Score>,
    frontier: &[Vec<Edge>],
    options: &Options,
    accounting: &mut RoundAccounting,
    totals: &mut SolverTotals,
) -> Result<BTreeMap<Vec<Edge>, TargetSelectionAudit>, String> {
    let mut audits = BTreeMap::new();
    for key in frontier {
        if totals.calls >= options.max_total_solver_calls {
            accounting.total_call_ceiling_hit = true;
            return Err("solver-call ceiling reached during target-pool enumeration".to_owned());
        }
        let entry = archive
            .get(key)
            .ok_or_else(|| "round-two frontier key is absent from archive".to_owned())?
            .clone();
        let score = entry
            .score
            .ok_or_else(|| "round-two frontier entry is unclassified".to_owned())?;
        let enumerate_limit = if score.exact {
            usize::try_from(score.count).map_err(|_| "exact count exceeds usize".to_owned())?
        } else {
            ROUND2_TARGET_POOL_CAP
        };
        let solver = Solver::blank_comparisons(key).map_err(|error| {
            format!(
                "cannot build round-two target-pool solver {}: {error}",
                network_sha256(key)
            )
        })?;
        let batch = solver.enumerate_up_to(enumerate_limit);
        totals.add(batch.stats);
        accounting.target_pool_calls += 1;
        accounting.target_pool_solutions += batch.solutions.len() as u64;
        if batch.exhausted == batch.capped {
            return Err("target-pool exhausted/capped flags are not complements".to_owned());
        }
        if score.exact {
            if !batch.exhausted || batch.capped || batch.solutions.len() != enumerate_limit {
                return Err(format!(
                    "exact target pool {} expected {} exhaustive solutions; got {} exhausted={} capped={}",
                    network_sha256(key),
                    enumerate_limit,
                    batch.solutions.len(),
                    batch.exhausted,
                    batch.capped
                ));
            }
        } else if batch.solutions.len() != ROUND2_TARGET_POOL_CAP {
            return Err(format!(
                "censored target pool {} did not produce the fixed {}-solution prefix",
                network_sha256(key),
                ROUND2_TARGET_POOL_CAP
            ));
        }
        let enumerated_solutions = batch.solutions.len();
        let mut solutions = batch.solutions;
        solutions.sort_unstable();
        let raw_len = solutions.len();
        solutions.dedup();
        if solutions.len() != raw_len {
            return Err(format!(
                "target-pool solver returned duplicate solutions for {}",
                network_sha256(key)
            ));
        }
        if score.exact {
            let mut required = entry.targets.targets.clone();
            required.push(entry.representative_target);
            required.extend(entry.expanded_targets.iter().copied());
            required.sort_unstable();
            required.dedup();
            if let Some(target) = required
                .iter()
                .find(|target| solutions.binary_search(target).is_err())
            {
                return Err(format!(
                    "exact target pool {} omits stored witness {}",
                    network_sha256(key),
                    grid_string(target)
                ));
            }
        } else {
            solutions.extend(entry.targets.targets.iter().copied());
            solutions.push(entry.representative_target);
            solutions.sort_unstable();
            solutions.dedup();
        }
        for target in &solutions {
            validate_complete_sudoku(target).map_err(|error| {
                format!("target-pool solution {}: {error}", network_sha256(key))
            })?;
            if !grid_satisfies_edges(target, key) {
                return Err(format!(
                    "target-pool solution violates network {}",
                    network_sha256(key)
                ));
            }
        }
        let selection = choose_signature_diverse_targets(
            key,
            &entry.cells,
            entry.representative_target,
            solutions,
            &entry.expanded_targets,
            ROUND2_TARGETS_PER_NETWORK,
        )?;
        if !score.exact && batch.exhausted {
            let upgraded =
                Score::exact(ROUND2_TARGET_POOL_CAP as u64, ROUND2_TARGET_POOL_CAP as u64);
            let upgraded = monotonic_score_upgrade(score, upgraded).map_err(|error| {
                format!("target-pool exact upgrade {}: {error}", network_sha256(key))
            })?;
            archive
                .get_mut(key)
                .expect("frontier entry existence checked above")
                .score = Some(upgraded);
            let previous = count_cache.insert(key.clone(), upgraded);
            if previous != Some(score) {
                return Err(
                    "target-pool score upgrade changed an unexpected cache entry".to_owned(),
                );
            }
            accounting.target_pool_exact_upgrades += 1;
        }
        accounting.target_witnesses_selected += selection.reservoir.targets.len() as u64;
        archive
            .get_mut(key)
            .expect("frontier entry existence checked above")
            .targets = selection.reservoir;
        audits.insert(
            key.clone(),
            TargetSelectionAudit {
                enumerated_solutions,
                enumeration_exhausted: batch.exhausted,
                enumeration_capped: batch.capped,
                normalized_candidates: selection.normalized_candidates,
                unique_signatures: selection.unique_signatures,
                signature_pairs: selection.signature_pairs,
                selected_witnesses: archive[key].targets.targets.len(),
                historical_signatures_excluded: selection.historical_signatures_excluded,
                ordered_targets: selection.ordered_targets,
                solver_stats: batch.stats,
            },
        );
    }
    Ok(audits)
}

fn replay_seed_archive(
    seeds: &[SeedInput],
    max_total_solver_calls: u64,
) -> Result<SeedReplay, String> {
    if max_total_solver_calls < seeds.len() as u64 {
        return Err(format!(
            "--max-total-solver-calls {max_total_solver_calls} cannot replay all {} seeds",
            seeds.len()
        ));
    }
    let mut archive = BTreeMap::new();
    let mut cache = BTreeMap::new();
    let mut totals = SolverTotals::default();
    for seed in seeds {
        let solver = Solver::blank_comparisons(&seed.hasse_edges)
            .map_err(|error| format!("seed {} solver construction: {error}", seed.ordinal))?;
        let batch = solver.enumerate_up_to(seed.solution_count as usize);
        totals.add(batch.stats);
        if !batch.exhausted || batch.capped || batch.solutions.len() != seed.solution_count as usize
        {
            let observed = if batch.capped {
                format!("at least {}", seed.solution_count.saturating_add(1))
            } else {
                batch.solutions.len().to_string()
            };
            return Err(format!(
                "seed {} declares {} exact solutions; replay found {observed}",
                seed.ordinal, seed.solution_count
            ));
        }
        let mut solutions = batch.solutions;
        solutions.sort_unstable();
        solutions.dedup();
        if solutions.len() != seed.solution_count as usize {
            return Err(format!(
                "seed {} replay returned duplicate solutions",
                seed.ordinal
            ));
        }
        if solutions.binary_search(&seed.target).is_err() {
            return Err(format!(
                "seed {} canonical target is absent from exact replay",
                seed.ordinal
            ));
        }
        let targets = TargetReservoir::from_sorted(solutions, ROUND1_TARGETS_PER_NETWORK);
        if targets.targets.len() != ROUND1_TARGETS_PER_NETWORK {
            return Err(format!(
                "seed {} supplied only {} target witnesses; expected {ROUND1_TARGETS_PER_NETWORK}",
                seed.ordinal,
                targets.targets.len()
            ));
        }
        let score = Score::exact(seed.solution_count, SEED_SOLUTION_CAP);
        let key = seed.hasse_edges.clone();
        let entry = ArchiveEntry {
            hasse_edges: key.clone(),
            representative_full_edges: seed.full_edges.clone(),
            representative_target: seed.target,
            cells: seed.cells.clone(),
            targets,
            score: Some(score),
            origin: Origin::Seed {
                ordinal: seed.ordinal,
            },
            occurrences: 1,
            expanded_in_round: false,
            expanded_targets: BTreeSet::new(),
            high_cap_probed: false,
            normal_stats: None,
            probe_stats: None,
        };
        if archive.insert(key.clone(), entry).is_some() || cache.insert(key, score).is_some() {
            return Err(format!(
                "seed {} duplicates a Hasse cache key",
                seed.ordinal
            ));
        }
    }
    Ok((archive, cache, totals))
}

fn parse_external_root_artifact_kind(bytes: &[u8]) -> Result<ExternalRootArtifactKind, String> {
    let first_newline = bytes
        .iter()
        .position(|&byte| byte == b'\n')
        .ok_or_else(|| "external root artifact has no complete header line".to_owned())?;
    let header = &bytes[..first_newline];
    let header = header.strip_suffix(b"\r").unwrap_or(header);
    let parsed = JsonParser::new(header)
        .parse()
        .map_err(|error| format!("external root artifact line 1: {error}"))?;
    let fields =
        json_object(&parsed).map_err(|error| format!("external root artifact line 1: {error}"))?;
    require_string_field(fields, "type", "header")?;
    let kind = match json_string(json_field(fields, "schema")?)? {
        CONTINUATION_SCHEMA => ExternalRootArtifactKind::PinnedRound2Landscape,
        ROOT_NEIGHBORHOOD_SCHEMA => ExternalRootArtifactKind::CompletedRootNeighborhoodV1,
        EXTERNAL_ROOT_NEIGHBORHOOD_SCHEMA => ExternalRootArtifactKind::CompletedRootNeighborhoodV2,
        schema => {
            return Err(format!(
                "unsupported external root artifact schema {schema:?}"
            ));
        }
    };
    require_string_field(fields, "algorithm_revision", kind.algorithm_revision())?;
    Ok(kind)
}

fn parse_external_root_network(
    fields: &BTreeMap<String, Json>,
    requested_sha256: &str,
    artifact_kind: ExternalRootArtifactKind,
    provenance: ExternalRootProvenance,
) -> Result<ExactRoot, String> {
    require_string_field(fields, "type", "network")?;
    require_string_field(fields, "schema", artifact_kind.schema())?;
    require_string_field(fields, "network_sha256", requested_sha256)?;
    let hasse_edges = parse_edges(json_field(fields, "hasse_edges")?)?;
    if network_sha256(&hasse_edges) != requested_sha256 {
        return Err(
            "external root network SHA-256 does not match its exact Hasse vector".to_owned(),
        );
    }
    let cells = parse_cells(json_field(fields, "canonical_cells")?)?;
    let full_edges = parse_edges(json_field(fields, "representative_full_saturated_edges")?)?;
    let target = parse_grid(json_field(fields, "representative_target")?)?;
    validate_complete_sudoku(&target)
        .map_err(|error| format!("external root representative target: {error}"))?;
    let canonical = canonical_saturated_state(&cells, &target)
        .map_err(|error| format!("external root canonical state: {error}"))?;
    if canonical.cells != cells
        || canonical.full_edges != full_edges
        || canonical.hasse_edges != hasse_edges
        || canonical.target != target
    {
        return Err("external root record is not its declared exact canonical state".to_owned());
    }
    let canonical_state_sha256 = state_sha256(&hasse_edges, &target);
    if artifact_kind.is_root_neighborhood() {
        require_string_field(
            fields,
            "representative_state_sha256",
            &canonical_state_sha256,
        )?;
    }
    let score = parse_score(json_field(fields, "score")?)?
        .ok_or_else(|| "external root record has no score".to_owned())?;
    if !score.exact {
        return Err("external root record score is only a lower bound".to_owned());
    }
    if artifact_kind == ExternalRootArtifactKind::PinnedRound2Landscape
        && requested_sha256 == FIRST_EXTERNAL_ROOT_SHA256
        && (score.count != FIRST_EXTERNAL_ROOT_COUNT || score.cap != MAX_EXPLORATION_CAP)
    {
        return Err(format!(
            "pinned external root declares exact {}/cap {}; expected exact {FIRST_EXTERNAL_ROOT_COUNT}/cap {MAX_EXPLORATION_CAP}",
            score.count, score.cap
        ));
    }
    Ok(ExactRoot {
        network_sha256: requested_sha256.to_owned(),
        state_sha256: canonical_state_sha256,
        solution_count: score.count,
        cells,
        full_edges,
        hasse_edges,
        target,
        source: RootSource::ExternalRecord(provenance),
    })
}

fn extract_external_root_record<R: BufRead>(
    mut reader: R,
    provenance: ExternalRootProvenance,
    requested_sha256: &str,
    artifact_kind: ExternalRootArtifactKind,
) -> Result<ExactRoot, String> {
    if provenance.schema != artifact_kind.schema()
        || provenance.algorithm_revision != artifact_kind.algorithm_revision()
        || provenance.authentication != artifact_kind.authentication()
    {
        return Err(
            "external root provenance disagrees with authenticated artifact kind".to_owned(),
        );
    }
    let mut line = String::new();
    let mut line_number = 0usize;
    let mut saw_header = false;
    let mut saw_summary = false;
    let mut saw_network = false;
    let mut root_solution_records = 0usize;
    let mut source_root_sha256 = None::<String>;
    let mut expected_root_solution_records = None::<usize>;
    let mut source_root_solutions = BTreeSet::<Grid>::new();
    let mut root = None;
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| format!("cannot stream external root artifact: {error}"))?;
        if read == 0 {
            break;
        }
        line_number += 1;
        if saw_summary {
            return Err(
                "external root artifact has a record after its terminal summary".to_owned(),
            );
        }
        if !line.ends_with('\n') {
            return Err("external root artifact must end every record with LF".to_owned());
        }
        let json = line.strip_suffix('\n').expect("checked LF");
        let json = json.strip_suffix('\r').unwrap_or(json);
        let parsed = JsonParser::new(json.as_bytes())
            .parse()
            .map_err(|error| format!("external root artifact line {line_number}: {error}"))?;
        let fields = json_object(&parsed)
            .map_err(|error| format!("external root artifact line {line_number}: {error}"))?;
        let record_type = json_string(json_field(fields, "type")?)?;
        require_string_field(fields, "schema", artifact_kind.schema())?;
        match record_type {
            "header" => {
                if line_number != 1 || saw_header {
                    return Err("external root artifact header must be first and unique".to_owned());
                }
                require_string_field(
                    fields,
                    "algorithm_revision",
                    artifact_kind.algorithm_revision(),
                )?;
                if artifact_kind.is_root_neighborhood() {
                    let scope = json_object(json_field(fields, "scope")?)?;
                    require_bool_field(scope, "complete_root_solution_set", true)?;
                    require_bool_field(
                        scope,
                        "complete_saturated_radius_one_generation_for_declared_root",
                        true,
                    )?;
                    require_bool_field(scope, "classification_complete", true)?;
                    let configuration = json_object(json_field(fields, "configuration")?)?;
                    let target_count = json_usize(json_field(configuration, "target_count")?)?;
                    if target_count == 0 {
                        return Err(
                            "completed root-neighborhood header has zero targets".to_owned()
                        );
                    }
                    let declared_root = json_object(json_field(fields, "root")?)?;
                    let declared_root_sha256 =
                        json_string(json_field(declared_root, "network_sha256")?)?.to_owned();
                    let declared_count_field = match artifact_kind {
                        ExternalRootArtifactKind::CompletedRootNeighborhoodV1 => {
                            "exact_solution_count"
                        }
                        ExternalRootArtifactKind::CompletedRootNeighborhoodV2 => {
                            "stored_exact_solution_count"
                        }
                        ExternalRootArtifactKind::PinnedRound2Landscape => unreachable!(),
                    };
                    if json_usize(json_field(declared_root, declared_count_field)?)? != target_count
                    {
                        return Err(
                            "root-neighborhood header root count disagrees with target_count"
                                .to_owned(),
                        );
                    }
                    source_root_sha256 = Some(declared_root_sha256);
                    expected_root_solution_records = Some(target_count);
                }
                saw_header = true;
            }
            "root_solution" => {
                if !artifact_kind.is_root_neighborhood() {
                    return Err(
                        "round-two root artifact contains a root_solution record".to_owned()
                    );
                }
                if !saw_header || saw_network {
                    return Err(
                        "external root solution is outside header/network boundaries".to_owned(),
                    );
                }
                let expected_ordinal = root_solution_records + 1;
                if json_usize(json_field(fields, "ordinal")?)? != expected_ordinal {
                    return Err(format!(
                        "external root solution ordinal is not sequential at {expected_ordinal}"
                    ));
                }
                require_string_field(
                    fields,
                    "root_network_sha256",
                    source_root_sha256
                        .as_deref()
                        .expect("root-neighborhood header established root identity"),
                )?;
                let grid = parse_grid(json_field(fields, "grid")?)?;
                validate_complete_sudoku(&grid)
                    .map_err(|error| format!("external root solution: {error}"))?;
                if !source_root_solutions.insert(grid) {
                    return Err("external root artifact repeats a root solution".to_owned());
                }
                root_solution_records += 1;
            }
            "network" => {
                if !saw_header || saw_summary {
                    return Err("external root network is outside header/summary".to_owned());
                }
                saw_network = true;
                if json_string(json_field(fields, "network_sha256")?)? == requested_sha256 {
                    if root.is_some() {
                        return Err(format!(
                            "external root artifact repeats network SHA-256 {requested_sha256}"
                        ));
                    }
                    let mut record_provenance = provenance.clone();
                    record_provenance.line_number = line_number;
                    root = Some(parse_external_root_network(
                        fields,
                        requested_sha256,
                        artifact_kind,
                        record_provenance,
                    )?);
                }
            }
            "summary" => {
                if !saw_header || saw_summary {
                    return Err(
                        "external root artifact summary is misplaced or repeated".to_owned()
                    );
                }
                match artifact_kind {
                    ExternalRootArtifactKind::PinnedRound2Landscape => {
                        require_string_field(fields, "status", "round-complete")?;
                        require_bool_field(fields, "round_complete", true)?;
                        require_bool_field(fields, "terminal_unique", false)?;
                    }
                    ExternalRootArtifactKind::CompletedRootNeighborhoodV1
                    | ExternalRootArtifactKind::CompletedRootNeighborhoodV2 => {
                        require_string_field(fields, "status", "root-neighborhood-complete")?;
                        require_bool_field(fields, "generation_complete", true)?;
                        require_bool_field(fields, "classification_complete", true)?;
                        require_bool_field(fields, "terminal_unique", false)?;
                        let expected_solutions = expected_root_solution_records
                            .expect("root-neighborhood header established target count");
                        if root_solution_records != expected_solutions
                            || source_root_solutions.len() != expected_solutions
                        {
                            return Err(format!(
                                "root-neighborhood source has {root_solution_records} distinct root_solution records; expected {expected_solutions}"
                            ));
                        }
                        let summary_root = json_object(json_field(fields, "root")?)?;
                        require_string_field(
                            summary_root,
                            "network_sha256",
                            source_root_sha256
                                .as_deref()
                                .expect("root-neighborhood header established root identity"),
                        )?;
                        require_u64_field(
                            summary_root,
                            "exact_solution_count",
                            expected_solutions as u64,
                        )?;
                        let target_enumeration =
                            json_object(json_field(fields, "target_enumeration")?)?;
                        require_u64_field(target_enumeration, "calls", 1)?;
                        require_u64_field(
                            target_enumeration,
                            "solutions",
                            expected_solutions as u64,
                        )?;
                        require_bool_field(target_enumeration, "exhausted", true)?;
                        require_bool_field(target_enumeration, "capped", false)?;
                        let classification = json_object(json_field(fields, "classification")?)?;
                        require_u64_field(classification, "unclassified_after_terminal_unique", 0)?;
                        let selected = root.as_ref().ok_or_else(|| {
                            format!(
                                "external root artifact lacks network SHA-256 {requested_sha256}"
                            )
                        })?;
                        let mut selected_summary_matches = 0usize;
                        for exact_state in
                            json_array(json_field(classification, "all_exact_states")?)?
                        {
                            let exact_state = json_object(exact_state)?;
                            if json_string(json_field(exact_state, "network_sha256")?)?
                                == requested_sha256
                            {
                                require_u64_field(exact_state, "count", selected.solution_count)?;
                                selected_summary_matches += 1;
                            }
                        }
                        if selected_summary_matches != 1 {
                            return Err(format!(
                                "selected external root has {selected_summary_matches} matching all_exact_states entries; expected 1"
                            ));
                        }
                    }
                }
                saw_summary = true;
            }
            other => return Err(format!("unknown external root record type {other:?}")),
        }
    }
    if !saw_header || !saw_summary {
        return Err("external root artifact lacks a header or terminal summary".to_owned());
    }
    root.ok_or_else(|| format!("external root artifact lacks network SHA-256 {requested_sha256}"))
}

fn load_external_root_record(
    path: &Path,
    requested_sha256: &str,
    expected_artifact_sha256: Option<&str>,
) -> Result<ExactRoot, String> {
    let canonical_path = fs::canonicalize(path)
        .map_err(|error| format!("cannot canonicalize {}: {error}", path.display()))?;
    let bytes = fs::read(&canonical_path).map_err(|error| {
        format!(
            "cannot read external root artifact {}: {error}",
            path.display()
        )
    })?;
    let observed_sha256 = sha256_hex(&bytes);
    if let Some(expected) = expected_artifact_sha256
        && observed_sha256 != expected
    {
        return Err(format!(
            "external root artifact SHA-256 is {observed_sha256}; expected explicit pin {expected}"
        ));
    }
    let artifact_kind = parse_external_root_artifact_kind(&bytes)?;
    match artifact_kind {
        ExternalRootArtifactKind::PinnedRound2Landscape => {
            if bytes.len() != ROUND2_LANDSCAPE_ARTIFACT_BYTES
                || observed_sha256 != ROUND2_LANDSCAPE_ARTIFACT_SHA256
            {
                return Err(format!(
                    "round-two external root artifact is {} bytes with SHA-256 {observed_sha256}; expected {ROUND2_LANDSCAPE_ARTIFACT_BYTES} bytes and {ROUND2_LANDSCAPE_ARTIFACT_SHA256}",
                    bytes.len()
                ));
            }
        }
        ExternalRootArtifactKind::CompletedRootNeighborhoodV1
        | ExternalRootArtifactKind::CompletedRootNeighborhoodV2 => {
            if expected_artifact_sha256.is_none() {
                return Err(
                    "root-neighborhood record input requires --root-record-sha256 authentication"
                        .to_owned(),
                );
            }
        }
    }
    let provenance = ExternalRootProvenance {
        path: canonical_path,
        bytes: bytes.len(),
        sha256: observed_sha256,
        line_number: 0,
        schema: artifact_kind.schema().to_owned(),
        algorithm_revision: artifact_kind.algorithm_revision().to_owned(),
        authentication: artifact_kind.authentication().to_owned(),
    };
    extract_external_root_record(
        BufReader::new(Cursor::new(bytes)),
        provenance,
        requested_sha256,
        artifact_kind,
    )
}

fn select_root_seed(
    seeds: &[SeedInput],
    ordinal: Option<usize>,
    requested_sha256: Option<&str>,
) -> Result<ExactRoot, String> {
    let selected = match (ordinal, requested_sha256) {
        (Some(ordinal), _) => seeds
            .iter()
            .find(|seed| seed.ordinal == ordinal)
            .ok_or_else(|| format!("root seed ordinal {ordinal} is absent"))?,
        (None, Some(hash)) => seeds
            .iter()
            .find(|seed| seed.network_sha256 == hash)
            .ok_or_else(|| format!("root seed SHA-256 {hash} is absent"))?,
        (None, None) => seeds
            .iter()
            .find(|seed| seed.ordinal == DEFAULT_ROOT_SEED_ORDINAL)
            .ok_or_else(|| {
                format!("default root seed ordinal {DEFAULT_ROOT_SEED_ORDINAL} is absent")
            })?,
    };
    if let Some(hash) = requested_sha256
        && selected.network_sha256 != hash
    {
        return Err(format!(
            "root selectors disagree: ordinal {} has SHA-256 {}, not {hash}",
            selected.ordinal, selected.network_sha256
        ));
    }
    Ok(ExactRoot::from_seed(selected))
}

fn observe_root_network(
    state: CanonicalState,
    origin: RootMoveOrigin,
    networks: &mut BTreeMap<Vec<Edge>, RootNetworkResult>,
    accounting: &mut RootNeighborhoodAccounting,
) {
    accounting.accepted_observations += 1;
    let key = state.hasse_edges.clone();
    match networks.entry(key) {
        std::collections::btree_map::Entry::Vacant(vacant) => {
            vacant.insert(RootNetworkResult {
                state,
                representative_origin: origin.clone(),
                observed_target_ordinals: BTreeSet::from([origin.target_ordinal]),
                occurrences: 1,
                score: None,
                solver_stats: None,
                preexisting_seed_ordinal: None,
                external_root_replay_cache: false,
            });
        }
        std::collections::btree_map::Entry::Occupied(mut occupied) => {
            let current = occupied.get_mut();
            current.occurrences += 1;
            current
                .observed_target_ordinals
                .insert(origin.target_ordinal);
            let observed_rank = (&origin, &state.target, &state.full_edges, &state.cells);
            let current_rank = (
                &current.representative_origin,
                &current.state.target,
                &current.state.full_edges,
                &current.state.cells,
            );
            if observed_rank < current_rank {
                current.state = state;
                current.representative_origin = origin;
            }
        }
    }
}

fn generate_root_neighborhood(
    root: &ExactRoot,
    solutions: &[Grid],
    accounting: &mut RootNeighborhoodAccounting,
) -> Result<BTreeMap<Vec<Edge>, RootNetworkResult>, String> {
    let expected_raw = u64::try_from(solutions.len())
        .map_err(|_| "root solution count exceeds u64".to_owned())?
        .checked_mul(ROOT_MOVES_PER_TARGET)
        .ok_or_else(|| "root raw-move count overflows u64".to_owned())?;
    let occupied = root.cells.iter().copied().collect::<BTreeSet<_>>();
    if occupied.len() != 18 {
        return Err(format!(
            "root network {} footprint has {} cells; expected 18",
            root.network_sha256,
            occupied.len()
        ));
    }
    let mut networks = BTreeMap::new();
    for (index, &target) in solutions.iter().enumerate() {
        let target_ordinal = index + 1;
        accounting.raw_move_attempts += 1;
        match construct_move_state(&root.cells, &target).map_err(|error| {
            format!("root target {target_ordinal} same-footprint saturation: {error}")
        })? {
            Some(state) => observe_root_network(
                state,
                RootMoveOrigin {
                    target_ordinal,
                    move_kind: MoveKind::Retarget,
                },
                &mut networks,
                accounting,
            ),
            None => accounting.coverage_rejections += 1,
        }

        for &removed in &root.cells {
            let retained = root
                .cells
                .iter()
                .copied()
                .filter(|&cell| cell != removed)
                .collect::<Vec<_>>();
            if retained.len() != 17 {
                return Err("root footprint is not 18 unique cells".to_owned());
            }
            for added in 0u8..CELLS as u8 {
                if occupied.contains(&added) {
                    continue;
                }
                accounting.raw_move_attempts += 1;
                if !retained.iter().any(|&cell| king_adjacent(cell, added)) {
                    // With no king-neighbour in the retained footprint, the
                    // added cell cannot be incident to a saturated comparison.
                    accounting.radius_rejections += 1;
                    continue;
                }
                let mut footprint = retained.clone();
                footprint.push(added);
                footprint.sort_unstable();
                match construct_move_state(&footprint, &target).map_err(|error| {
                    format!("root target {target_ordinal} swap {removed}->{added}: {error}")
                })? {
                    Some(state) => observe_root_network(
                        state,
                        RootMoveOrigin {
                            target_ordinal,
                            move_kind: MoveKind::Swap { removed, added },
                        },
                        &mut networks,
                        accounting,
                    ),
                    None => accounting.coverage_rejections += 1,
                }
            }
        }
    }
    if accounting.raw_move_attempts != expected_raw {
        return Err(format!(
            "root raw-move accounting is {}; expected {expected_raw}",
            accounting.raw_move_attempts
        ));
    }
    if accounting.radius_rejections
        + accounting.coverage_rejections
        + accounting.accepted_observations
        != accounting.raw_move_attempts
    {
        return Err("root raw moves do not partition exactly".to_owned());
    }
    accounting.distinct_observed_networks = networks.len() as u64;
    accounting.duplicate_observations = accounting
        .accepted_observations
        .checked_sub(accounting.distinct_observed_networks)
        .ok_or_else(|| "root distinct networks exceed accepted observations".to_owned())?;
    Ok(networks)
}

fn root_two_cell_footprint_plan(
    root: &ExactRoot,
) -> Result<(Vec<RootTwoCellFootprintMove>, u64), String> {
    let occupied = root.cells.iter().copied().collect::<BTreeSet<_>>();
    if occupied.len() != 18 {
        return Err(format!(
            "root network {} footprint has {} cells; expected 18",
            root.network_sha256,
            occupied.len()
        ));
    }
    let outside = (0u8..CELLS as u8)
        .filter(|cell| !occupied.contains(cell))
        .collect::<Vec<_>>();
    if outside.len() != 63 {
        return Err("root two-cell move space does not have 63 outside cells".to_owned());
    }
    let mut adjacency_masks = [0u128; CELLS];
    for left in 0u8..CELLS as u8 {
        for right in 0u8..CELLS as u8 {
            if king_adjacent(left, right) {
                adjacency_masks[left as usize] |= 1u128 << right;
            }
        }
    }

    let mut plan = Vec::new();
    let mut geometric_rejections = 0u64;
    let mut removed_pair_ordinal = 0usize;
    for first_removed_index in 0..root.cells.len() {
        for second_removed_index in first_removed_index + 1..root.cells.len() {
            removed_pair_ordinal += 1;
            let removed = [
                root.cells[first_removed_index],
                root.cells[second_removed_index],
            ];
            let retained = root
                .cells
                .iter()
                .copied()
                .filter(|cell| !removed.contains(cell))
                .collect::<Vec<_>>();
            if retained.len() != 16 {
                return Err("root footprint is not 18 unique cells".to_owned());
            }
            let retained_mask = retained
                .iter()
                .fold(0u128, |mask, &cell| mask | (1u128 << cell));
            let isolated_retained = retained
                .iter()
                .copied()
                .filter(|&cell| adjacency_masks[cell as usize] & retained_mask == 0)
                .collect::<Vec<_>>();
            for first_added_index in 0..outside.len() {
                for second_added_index in first_added_index + 1..outside.len() {
                    let added = [outside[first_added_index], outside[second_added_index]];
                    let final_mask = retained_mask | (1u128 << added[0]) | (1u128 << added[1]);
                    let all_incident = adjacency_masks[added[0] as usize] & final_mask != 0
                        && adjacency_masks[added[1] as usize] & final_mask != 0
                        && isolated_retained
                            .iter()
                            .all(|&cell| adjacency_masks[cell as usize] & final_mask != 0);
                    if !all_incident {
                        geometric_rejections += 1;
                        continue;
                    }
                    let mut footprint = [0u8; 18];
                    footprint[..16].copy_from_slice(&retained);
                    footprint[16..].copy_from_slice(&added);
                    footprint.sort_unstable();
                    plan.push(RootTwoCellFootprintMove {
                        removed_pair_ordinal,
                        removed,
                        added,
                        footprint,
                    });
                }
            }
        }
    }
    if plan.len() as u64 + geometric_rejections != ROOT_TWO_CELL_MOVES_PER_TARGET {
        return Err(format!(
            "root two-cell geometric plan partitions {} moves; expected {ROOT_TWO_CELL_MOVES_PER_TARGET}",
            plan.len() as u64 + geometric_rejections
        ));
    }
    Ok((plan, geometric_rejections))
}

fn observe_root_two_cell_network(
    state: CanonicalState,
    origin: RootTwoCellMoveOrigin,
    networks: &mut BTreeMap<Vec<Edge>, RootTwoCellNetworkResult>,
    accounting: &mut RootTwoCellAccounting,
) -> Result<(), String> {
    if !(1..=128).contains(&origin.target_ordinal) {
        return Err(format!(
            "root two-cell target ordinal {} is outside 1..=128",
            origin.target_ordinal
        ));
    }
    accounting.accepted_observations += 1;
    let key = state.hasse_edges.clone();
    let mask_index = (origin.target_ordinal - 1) / 64;
    let mask_bit = 1u64 << ((origin.target_ordinal - 1) % 64);
    match networks.entry(key) {
        std::collections::btree_map::Entry::Vacant(vacant) => {
            let mut observed_target_mask = [0u64; 2];
            observed_target_mask[mask_index] |= mask_bit;
            vacant.insert(RootTwoCellNetworkResult {
                representative_origin: origin,
                representative_canonical_target: state.target,
                observed_target_mask,
                occurrences: 1,
                score: None,
                solver_stats: None,
                preexisting_seed_ordinal: None,
            });
        }
        std::collections::btree_map::Entry::Occupied(mut occupied) => {
            let current = occupied.get_mut();
            current.occurrences += 1;
            current.observed_target_mask[mask_index] |= mask_bit;
            if origin < current.representative_origin {
                current.representative_origin = origin;
                current.representative_canonical_target = state.target;
            }
        }
    }
    Ok(())
}

fn generate_root_two_cell_neighborhood(
    root: &ExactRoot,
    solutions: &[Grid],
    first_removed_pair_ordinal: usize,
    last_removed_pair_ordinal: usize,
    accounting: &mut RootTwoCellAccounting,
) -> Result<BTreeMap<Vec<Edge>, RootTwoCellNetworkResult>, String> {
    if solutions.len() != 128 {
        return Err(format!(
            "root two-cell removed-pair shard requires all 128 solutions; got {}",
            solutions.len()
        ));
    }
    if first_removed_pair_ordinal == 0
        || first_removed_pair_ordinal > last_removed_pair_ordinal
        || last_removed_pair_ordinal > ROOT_TWO_CELL_REMOVAL_PAIRS as usize
    {
        return Err(format!(
            "root two-cell removed-pair range {first_removed_pair_ordinal}..={last_removed_pair_ordinal} is outside 1..={ROOT_TWO_CELL_REMOVAL_PAIRS}"
        ));
    }
    if last_removed_pair_ordinal - first_removed_pair_ordinal + 1
        > MAX_ROOT_TWO_CELL_REMOVAL_PAIRS_PER_SHARD
    {
        return Err(format!(
            "root two-cell generator accepts at most {MAX_ROOT_TWO_CELL_REMOVAL_PAIRS_PER_SHARD} removed pairs per shard"
        ));
    }
    let removed_pair_count =
        u64::try_from(last_removed_pair_ordinal - first_removed_pair_ordinal + 1)
            .map_err(|_| "root two-cell removed-pair range exceeds u64".to_owned())?;
    let raw_moves_per_target = removed_pair_count
        .checked_mul(ROOT_TWO_CELL_ADDITION_PAIRS)
        .ok_or_else(|| "root two-cell per-target move count overflows u64".to_owned())?;
    let expected_raw = (solutions.len() as u64)
        .checked_mul(raw_moves_per_target)
        .ok_or_else(|| "root two-cell raw-move count overflows u64".to_owned())?;
    let (mut plan, _) = root_two_cell_footprint_plan(root)?;
    plan.retain(|footprint_move| {
        (first_removed_pair_ordinal..=last_removed_pair_ordinal)
            .contains(&footprint_move.removed_pair_ordinal)
    });
    let geometric_rejections_per_target = raw_moves_per_target
        .checked_sub(plan.len() as u64)
        .ok_or_else(|| "two-cell geometric plan exceeds selected raw moves".to_owned())?;
    let mut networks = BTreeMap::new();
    for target_ordinal in 1..=solutions.len() {
        let target = &solutions[target_ordinal - 1];
        accounting.raw_move_attempts += raw_moves_per_target;
        accounting.geometric_incidence_rejections += geometric_rejections_per_target;
        for footprint_move in &plan {
            match construct_move_state(&footprint_move.footprint, target).map_err(|error| {
                format!(
                    "root target {target_ordinal} two-cell swap {:?}->{:?}: {error}",
                    footprint_move.removed, footprint_move.added
                )
            })? {
                Some(state) => observe_root_two_cell_network(
                    state,
                    RootTwoCellMoveOrigin {
                        target_ordinal,
                        removed_pair_ordinal: footprint_move.removed_pair_ordinal,
                        removed: footprint_move.removed,
                        added: footprint_move.added,
                    },
                    &mut networks,
                    accounting,
                )?,
                None => accounting.coverage_rejections += 1,
            }
        }
    }
    if accounting.raw_move_attempts != expected_raw
        || accounting.geometric_incidence_rejections
            + accounting.coverage_rejections
            + accounting.accepted_observations
            != accounting.raw_move_attempts
    {
        return Err("root two-cell raw moves do not partition exactly".to_owned());
    }
    accounting.distinct_observed_networks = networks.len() as u64;
    accounting.duplicate_observations = accounting
        .accepted_observations
        .checked_sub(accounting.distinct_observed_networks)
        .ok_or_else(|| "root two-cell distinct networks exceed observations".to_owned())?;
    Ok(networks)
}

fn first_exact_unique(networks: &BTreeMap<Vec<Edge>, RootNetworkResult>) -> Option<Vec<Edge>> {
    networks.iter().find_map(|(key, network)| {
        network
            .score
            .is_some_and(|score| score.exact && score.count == 1)
            .then(|| key.clone())
    })
}

fn execute_root_neighborhood(
    seeds: &[SeedInput],
    root: ExactRoot,
    count_cap: u64,
    progress_every: u64,
) -> Result<RootNeighborhoodOutcome, String> {
    let (_seed_archive, count_cache, mut solver_totals) = replay_seed_archive(seeds, u64::MAX)?;
    let expected_root_solutions = usize::try_from(root.solution_count)
        .map_err(|_| "root exact count exceeds usize".to_owned())?;
    let root_solver = Solver::blank_comparisons(&root.hasse_edges)
        .map_err(|error| format!("root solver construction: {error}"))?;
    let root_exact_replay_stats = if root.is_external() {
        let replay_cap = root
            .solution_count
            .checked_add(1)
            .ok_or_else(|| "external root exact count overflows replay cap".to_owned())?;
        let replay = root_solver.count_up_to(replay_cap);
        solver_totals.add(replay.stats);
        if replay.capped || replay.count != root.solution_count {
            return Err(format!(
                "external root declares exact {}; independent cap-{replay_cap} replay returned {} capped={}",
                root.solution_count, replay.count, replay.capped
            ));
        }
        Some(replay.stats)
    } else {
        None
    };
    let batch = root_solver.enumerate_up_to(expected_root_solutions);
    solver_totals.add(batch.stats);
    if !batch.exhausted || batch.capped || batch.solutions.len() != expected_root_solutions {
        return Err(format!(
            "root {} declares {} exact solutions; enumeration returned {} exhausted={} capped={}",
            root.network_sha256,
            root.solution_count,
            batch.solutions.len(),
            batch.exhausted,
            batch.capped
        ));
    }
    let root_enumeration_stats = batch.stats;
    let mut root_solutions = batch.solutions;
    root_solutions.sort_unstable();
    let raw_solution_count = root_solutions.len();
    root_solutions.dedup();
    if root_solutions.len() != raw_solution_count || root_solutions.len() != expected_root_solutions
    {
        return Err("root enumeration contains duplicate solutions".to_owned());
    }
    if root_solutions.binary_search(&root.target).is_err() {
        return Err("root canonical target is absent from exact enumeration".to_owned());
    }
    for target in &root_solutions {
        validate_complete_sudoku(target)
            .map_err(|error| format!("root enumerated solution: {error}"))?;
        if !grid_satisfies_edges(target, &root.hasse_edges) {
            return Err("root enumerated solution violates the root Hasse network".to_owned());
        }
    }

    let mut accounting = RootNeighborhoodAccounting {
        seed_replay_calls: seeds.len() as u64,
        root_exact_replay_calls: u64::from(root.is_external()),
        root_enumeration_calls: 1,
        ..RootNeighborhoodAccounting::default()
    };
    let mut networks = generate_root_neighborhood(&root, &root_solutions, &mut accounting)?;
    let seed_ordinals = seeds
        .iter()
        .map(|seed| (seed.hasse_edges.clone(), seed.ordinal))
        .collect::<BTreeMap<_, _>>();
    for (key, network) in &mut networks {
        network.preexisting_seed_ordinal = seed_ordinals.get(key).copied();
        if root.is_external() && key == &root.hasse_edges {
            if let Some(seed_score) = count_cache.get(key).copied()
                && (!seed_score.exact || seed_score.count != root.solution_count)
            {
                return Err(
                    "external root exact replay disagrees with frozen seed cache".to_owned(),
                );
            }
            network.score = Some(Score::exact(
                root.solution_count,
                root.solution_count.saturating_add(1),
            ));
            network.external_root_replay_cache = true;
            accounting.count_cache_hits += 1;
            accounting.external_root_cache_hits += 1;
        } else if let Some(score) = count_cache.get(key).copied() {
            network.score = Some(score);
            accounting.count_cache_hits += 1;
            accounting.frozen_seed_cache_hits += 1;
        }
    }
    if root.is_external() && accounting.external_root_cache_hits != 1 {
        return Err(
            "external root was not regenerated exactly once as a cache identity".to_owned(),
        );
    }
    accounting.preexisting_seed_networks_observed = networks
        .values()
        .filter(|network| network.preexisting_seed_ordinal.is_some())
        .count() as u64;
    accounting.new_canonical_networks = accounting
        .distinct_observed_networks
        .checked_sub(accounting.preexisting_seed_networks_observed)
        .ok_or_else(|| "preexisting root networks exceed distinct observations".to_owned())?;

    let keys = networks.keys().cloned().collect::<Vec<_>>();
    let mut unique_network = first_exact_unique(&networks);
    for key in keys {
        if networks[&key].score.is_some() {
            continue;
        }
        if unique_network.is_some() {
            break;
        }
        let solver = Solver::blank_comparisons(&key).map_err(|error| {
            format!(
                "root-neighborhood network {} solver construction: {error}",
                network_sha256(&key)
            )
        })?;
        let result = solver.count_up_to(count_cap);
        solver_totals.add(result.stats);
        accounting.network_count_calls += 1;
        if result.count == 0 {
            return Err(format!(
                "root-neighborhood network {} has zero solutions despite its generating target",
                network_sha256(&key)
            ));
        }
        let score = Score::from_result(result.count, result.capped, count_cap);
        let network = networks
            .get_mut(&key)
            .expect("root network key collected above");
        network.score = Some(score);
        network.solver_stats = Some(result.stats);
        if progress_every != 0
            && accounting
                .network_count_calls
                .is_multiple_of(progress_every)
        {
            eprintln!(
                "root count progress: classified={} of {} current={}{}",
                accounting.count_cache_hits + accounting.network_count_calls,
                accounting.distinct_observed_networks,
                score.relation(),
                score.count
            );
        }
        if score.exact && score.count == 1 {
            unique_network = Some(key);
        }
    }

    accounting.exact_networks = networks
        .values()
        .filter(|network| network.score.is_some_and(|score| score.exact))
        .count() as u64;
    accounting.lower_bound_networks = networks
        .values()
        .filter(|network| network.score.is_some_and(|score| !score.exact))
        .count() as u64;
    accounting.unclassified_networks = networks
        .values()
        .filter(|network| network.score.is_none())
        .count() as u64;
    if accounting.exact_networks
        + accounting.lower_bound_networks
        + accounting.unclassified_networks
        != accounting.distinct_observed_networks
        || accounting.count_cache_hits
            != accounting.frozen_seed_cache_hits + accounting.external_root_cache_hits
        || accounting.count_cache_hits
            + accounting.network_count_calls
            + accounting.unclassified_networks
            != accounting.distinct_observed_networks
        || networks
            .values()
            .map(|network| network.occurrences)
            .sum::<u64>()
            != accounting.accepted_observations
        || accounting.seed_replay_calls
            + accounting.root_exact_replay_calls
            + accounting.root_enumeration_calls
            + accounting.network_count_calls
            != solver_totals.calls
    {
        return Err("root-neighborhood accounting does not partition exactly".to_owned());
    }
    if unique_network.is_none() && accounting.unclassified_networks != 0 {
        return Err("root-neighborhood completed without classifying every network".to_owned());
    }
    let status = if unique_network.is_some() {
        "unique-found"
    } else {
        "root-neighborhood-complete"
    };
    Ok(RootNeighborhoodOutcome {
        root,
        root_exact_replay_stats,
        root_solutions,
        root_enumeration_stats,
        networks,
        accounting,
        solver_totals,
        status,
        unique_network,
    })
}

fn execute_root_two_cell_neighborhood(
    seeds: &[SeedInput],
    root: ExactRoot,
    first_removed_pair_ordinal: usize,
    last_removed_pair_ordinal: usize,
    count_cap: u64,
    progress_every: u64,
) -> Result<RootTwoCellNeighborhoodOutcome, String> {
    if count_cap < NORMAL_CAP {
        return Err(format!(
            "two-cell root execution requires count cap at least {NORMAL_CAP}"
        ));
    }
    if first_removed_pair_ordinal == 0
        || first_removed_pair_ordinal > last_removed_pair_ordinal
        || last_removed_pair_ordinal > ROOT_TWO_CELL_REMOVAL_PAIRS as usize
        || last_removed_pair_ordinal - first_removed_pair_ordinal + 1
            > MAX_ROOT_TWO_CELL_REMOVAL_PAIRS_PER_SHARD
    {
        return Err("two-cell root execution received an invalid removed-pair shard".to_owned());
    }
    if root.seed_ordinal() != Some(DEFAULT_ROOT_SEED_ORDINAL)
        || root.network_sha256 != ROOT_TWO_CELL_SEED_SHA256
        || root.solution_count != 128
    {
        return Err("two-cell root mode is pinned to frozen seed 42/count 128".to_owned());
    }
    let (_seed_archive, count_cache, mut solver_totals) = replay_seed_archive(seeds, u64::MAX)?;
    let expected_root_solutions = usize::try_from(root.solution_count)
        .map_err(|_| "root exact count exceeds usize".to_owned())?;
    let root_solver = Solver::blank_comparisons(&root.hasse_edges)
        .map_err(|error| format!("two-cell root solver construction: {error}"))?;
    let batch = root_solver.enumerate_up_to(expected_root_solutions);
    solver_totals.add(batch.stats);
    if !batch.exhausted || batch.capped || batch.solutions.len() != expected_root_solutions {
        return Err(format!(
            "two-cell root declares {} exact solutions; enumeration returned {} exhausted={} capped={}",
            root.solution_count,
            batch.solutions.len(),
            batch.exhausted,
            batch.capped
        ));
    }
    let root_enumeration_stats = batch.stats;
    let mut root_solutions = batch.solutions;
    root_solutions.sort_unstable();
    let raw_solution_count = root_solutions.len();
    root_solutions.dedup();
    if root_solutions.len() != raw_solution_count || root_solutions.len() != 128 {
        return Err("two-cell root enumeration is not 128 distinct solutions".to_owned());
    }
    if root_solutions.binary_search(&root.target).is_err() {
        return Err("two-cell root canonical target is absent from exact enumeration".to_owned());
    }
    for target in &root_solutions {
        validate_complete_sudoku(target)
            .map_err(|error| format!("two-cell root enumerated solution: {error}"))?;
        if !grid_satisfies_edges(target, &root.hasse_edges) {
            return Err("two-cell root solution violates the root Hasse network".to_owned());
        }
    }

    let mut accounting = RootTwoCellAccounting {
        seed_replay_calls: seeds.len() as u64,
        root_enumeration_calls: 1,
        ..RootTwoCellAccounting::default()
    };
    let mut networks = generate_root_two_cell_neighborhood(
        &root,
        &root_solutions,
        first_removed_pair_ordinal,
        last_removed_pair_ordinal,
        &mut accounting,
    )?;
    if networks.values().any(|network| {
        !(first_removed_pair_ordinal..=last_removed_pair_ordinal)
            .contains(&network.representative_origin.removed_pair_ordinal)
            || !(1..=128).contains(&network.representative_origin.target_ordinal)
    }) {
        return Err("two-cell representative origin lies outside its declared shard".to_owned());
    }
    let seed_ordinals = seeds
        .iter()
        .map(|seed| (seed.hasse_edges.clone(), seed.ordinal))
        .collect::<BTreeMap<_, _>>();
    let mut unique_network = None;
    for (key, network) in &mut networks {
        network.preexisting_seed_ordinal = seed_ordinals.get(key).copied();
        if let Some(score) = count_cache.get(key).copied() {
            network.score = Some(score);
            accounting.count_cache_hits += 1;
            if score.exact && score.count == 1 {
                unique_network = Some(key.clone());
            }
        }
    }
    accounting.preexisting_seed_networks_observed = networks
        .values()
        .filter(|network| network.preexisting_seed_ordinal.is_some())
        .count() as u64;
    accounting.new_canonical_networks = accounting
        .distinct_observed_networks
        .checked_sub(accounting.preexisting_seed_networks_observed)
        .ok_or_else(|| "preexisting two-cell networks exceed distinct observations".to_owned())?;

    let keys = networks.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        if networks[&key].score.is_some() {
            continue;
        }
        if unique_network.is_some() {
            break;
        }
        let solver = Solver::blank_comparisons(&key).map_err(|error| {
            format!(
                "two-cell root network {} solver construction: {error}",
                network_sha256(&key)
            )
        })?;
        let result = solver.count_up_to(count_cap);
        solver_totals.add(result.stats);
        accounting.network_count_calls += 1;
        if result.count == 0 {
            return Err(format!(
                "two-cell root network {} has zero solutions despite its generating witness",
                network_sha256(&key)
            ));
        }
        let score = Score::from_result(result.count, result.capped, count_cap);
        let network = networks
            .get_mut(&key)
            .expect("two-cell root network key collected above");
        network.score = Some(score);
        network.solver_stats = Some(result.stats);
        if progress_every != 0
            && accounting
                .network_count_calls
                .is_multiple_of(progress_every)
        {
            eprintln!(
                "root two-cell count progress: classified={} of {} current={}{}",
                accounting.count_cache_hits + accounting.network_count_calls,
                accounting.distinct_observed_networks,
                score.relation(),
                score.count
            );
        }
        if score.exact && score.count == 1 {
            unique_network = Some(key);
        }
    }

    accounting.exact_networks = networks
        .values()
        .filter(|network| network.score.is_some_and(|score| score.exact))
        .count() as u64;
    accounting.lower_bound_networks = networks
        .values()
        .filter(|network| network.score.is_some_and(|score| !score.exact))
        .count() as u64;
    accounting.unclassified_networks = networks
        .values()
        .filter(|network| network.score.is_none())
        .count() as u64;
    let direct_rows = networks
        .values()
        .filter(|network| network.solver_stats.is_some())
        .count() as u64;
    let classification_sources_valid = networks.values().all(|network| {
        matches!(
            (
                network.preexisting_seed_ordinal,
                network.score,
                network.solver_stats,
            ),
            (Some(_), Some(_), None) | (None, Some(_), Some(_)) | (None, None, None)
        )
    });
    let unique_score_valid = unique_network.as_ref().is_none_or(|key| {
        networks[key]
            .score
            .is_some_and(|score| score.exact && score.count == 1)
    });
    if accounting.exact_networks
        + accounting.lower_bound_networks
        + accounting.unclassified_networks
        != accounting.distinct_observed_networks
        || accounting.count_cache_hits != accounting.preexisting_seed_networks_observed
        || accounting.network_count_calls != direct_rows
        || !classification_sources_valid
        || !unique_score_valid
        || accounting.count_cache_hits
            + accounting.network_count_calls
            + accounting.unclassified_networks
            != accounting.distinct_observed_networks
        || networks
            .values()
            .map(|network| network.occurrences)
            .sum::<u64>()
            != accounting.accepted_observations
        || accounting.seed_replay_calls
            + accounting.root_enumeration_calls
            + accounting.network_count_calls
            != solver_totals.calls
    {
        return Err("two-cell root accounting does not partition exactly".to_owned());
    }
    if unique_network.is_none() && accounting.unclassified_networks != 0 {
        return Err("two-cell root shard completed without classifying every network".to_owned());
    }
    let status = if unique_network.is_some() {
        "unique-found"
    } else {
        "root-two-cell-shard-complete"
    };
    Ok(RootTwoCellNeighborhoodOutcome {
        root,
        root_solutions,
        root_enumeration_stats,
        first_removed_pair_ordinal,
        last_removed_pair_ordinal,
        networks,
        accounting,
        solver_totals,
        status,
        unique_network,
    })
}

fn observe_generated_state(
    state: CanonicalState,
    provenance: GeneratedProvenance,
    archive: &mut BTreeMap<Vec<Edge>, ArchiveEntry>,
    candidates: &mut BTreeMap<Vec<Edge>, Candidate>,
    accounting: &mut RoundAccounting,
    target_limit: usize,
    current_sources: &mut BTreeMap<Vec<Edge>, BTreeSet<SourceStratum>>,
) {
    accounting.accepted_observations += 1;
    let key = state.hasse_edges.clone();
    current_sources
        .entry(key.clone())
        .or_default()
        .insert(provenance.source);
    let origin = provenance.origin;
    if let Some(existing) = archive.get_mut(&key) {
        existing.targets.insert(state.target, target_limit);
        existing.occurrences += 1;
        accounting.visited_observations += 1;
        return;
    }
    match candidates.entry(key) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(Candidate::new(state, origin, target_limit));
        }
        std::collections::btree_map::Entry::Occupied(mut entry) => {
            entry.get_mut().observe(state, origin, target_limit);
            accounting.duplicate_new_observations += 1;
        }
    }
}

fn construct_move_state(footprint: &[u8], target: &Grid) -> Result<Option<CanonicalState>, String> {
    match canonical_saturated_state(footprint, target) {
        Ok(state) => Ok(Some(state)),
        Err(error)
            if error == "saturated graph does not cover every footprint cell"
                || error == "Hasse reduction lost an incident footprint cell" =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn generate_round_candidates(
    archive: &mut BTreeMap<Vec<Edge>, ArchiveEntry>,
    frontier: &[Vec<Edge>],
    accounting: &mut RoundAccounting,
    target_limit: usize,
    raw_move_hard_max: u64,
    target_schedules: Option<&BTreeMap<Vec<Edge>, Vec<Grid>>>,
    current_sources: &mut BTreeMap<Vec<Edge>, BTreeSet<SourceStratum>>,
) -> Result<BTreeMap<Vec<Edge>, Candidate>, String> {
    let mut candidates = BTreeMap::new();
    for key in frontier {
        let parent = archive
            .get(key)
            .ok_or_else(|| {
                format!(
                    "frontier network {} is absent from archive",
                    network_sha256(key)
                )
            })?
            .clone();
        let declared_targets = match target_schedules {
            Some(schedules) => schedules
                .get(key)
                .ok_or_else(|| {
                    format!(
                        "round-two frontier network {} lacks a frozen target schedule",
                        network_sha256(key)
                    )
                })?
                .clone(),
            None => parent.expansion_targets(target_limit),
        };
        if declared_targets.is_empty() || declared_targets.len() > target_limit {
            return Err(format!(
                "frontier network {} has {} targets; expected 1..={target_limit}",
                network_sha256(key),
                declared_targets.len()
            ));
        }
        let historical_normals = parent
            .expanded_targets
            .iter()
            .map(|target| target_normal_form(key, target))
            .collect::<BTreeSet<_>>();
        let signature_pairs = relevant_comparison_pairs(&parent.cells);
        let historical_signatures = historical_normals
            .iter()
            .map(|target| target_signature(target, &signature_pairs))
            .collect::<BTreeSet<_>>();
        let expansion_targets = declared_targets
            .into_iter()
            .enumerate()
            .filter_map(|(index, target)| {
                let historical = parent.expanded_targets.contains(&target)
                    || target_schedules.is_some()
                        && historical_signatures.contains(&target_signature(
                            &target_normal_form(key, &target),
                            &signature_pairs,
                        ));
                if historical {
                    accounting.previously_expanded_targets_skipped += 1;
                    None
                } else {
                    Some((index + 1, target))
                }
            })
            .collect::<Vec<_>>();
        if expansion_targets.is_empty() {
            return Err(format!(
                "frontier network {} has no prospectively unexpanded targets",
                network_sha256(key)
            ));
        }
        let stored_parent = archive
            .get_mut(key)
            .expect("frontier existence checked above");
        stored_parent.expanded_in_round = true;
        stored_parent
            .expanded_targets
            .extend(expansion_targets.iter().map(|(_, target)| *target));
        let parent_sha = network_sha256(key);
        let occupied = parent.cells.iter().copied().collect::<BTreeSet<_>>();
        for &(target_ordinal, target) in &expansion_targets {
            accounting.raw_move_attempts += 1;
            let origin = Origin::Generated {
                parent_network_sha256: parent_sha.clone(),
                target_ordinal,
                move_kind: MoveKind::Retarget,
            };
            match construct_move_state(&parent.cells, &target).map_err(|error| {
                format!(
                    "same-footprint move from {parent_sha}, target {}: {error}",
                    target_ordinal
                )
            })? {
                Some(state) => observe_generated_state(
                    state,
                    GeneratedProvenance {
                        origin,
                        source: SourceStratum {
                            parent_hasse_edges: key.clone(),
                            target_ordinal,
                        },
                    },
                    archive,
                    &mut candidates,
                    accounting,
                    target_limit,
                    current_sources,
                ),
                None => accounting.coverage_rejections += 1,
            }

            for &removed in &parent.cells {
                let retained = parent
                    .cells
                    .iter()
                    .copied()
                    .filter(|&cell| cell != removed)
                    .collect::<Vec<_>>();
                if retained.len() != 17 {
                    return Err(format!(
                        "frontier network {parent_sha} footprint is not 18 unique cells"
                    ));
                }
                for added in 0u8..CELLS as u8 {
                    if occupied.contains(&added) {
                        continue;
                    }
                    accounting.raw_move_attempts += 1;
                    if !retained.iter().any(|&cell| king_adjacent(cell, added)) {
                        accounting.radius_rejections += 1;
                        continue;
                    }
                    let mut footprint = retained.clone();
                    footprint.push(added);
                    footprint.sort_unstable();
                    let origin = Origin::Generated {
                        parent_network_sha256: parent_sha.clone(),
                        target_ordinal,
                        move_kind: MoveKind::Swap { removed, added },
                    };
                    match construct_move_state(&footprint, &target).map_err(|error| {
                        format!(
                            "swap move from {parent_sha}, target {}, {removed}->{added}: {error}",
                            target_ordinal
                        )
                    })? {
                        Some(state) => observe_generated_state(
                            state,
                            GeneratedProvenance {
                                origin,
                                source: SourceStratum {
                                    parent_hasse_edges: key.clone(),
                                    target_ordinal,
                                },
                            },
                            archive,
                            &mut candidates,
                            accounting,
                            target_limit,
                            current_sources,
                        ),
                        None => accounting.coverage_rejections += 1,
                    }
                }
            }
        }
    }
    accounting.new_canonical_networks = candidates.len() as u64;
    if accounting.raw_move_attempts > raw_move_hard_max {
        accounting.raw_ceiling_hit = true;
    }
    let partitioned = accounting.radius_rejections
        + accounting.coverage_rejections
        + accounting.accepted_observations;
    if partitioned != accounting.raw_move_attempts {
        return Err(format!(
            "move accounting mismatch: raw={}, partitioned={partitioned}",
            accounting.raw_move_attempts
        ));
    }
    if accounting.accepted_observations
        != accounting.visited_observations
            + accounting.duplicate_new_observations
            + accounting.new_canonical_networks
    {
        return Err(
            "accepted move observations do not partition by exact Hasse identity".to_owned(),
        );
    }
    Ok(candidates)
}

fn insert_result_witnesses(
    entry: &mut ArchiveEntry,
    first: Option<Grid>,
    second: Option<Grid>,
    target_limit: usize,
) {
    if let Some(target) = first {
        entry.targets.insert(target, target_limit);
    }
    if let Some(target) = second {
        entry.targets.insert(target, target_limit);
    }
}

fn monotonic_score_upgrade(previous: Score, next: Score) -> Result<Score, String> {
    if previous.exact {
        if next.exact && next.count == previous.count {
            return Ok(previous);
        }
        return Err(format!(
            "cannot replace exact score {} with {} {}",
            previous.count,
            next.relation(),
            next.count
        ));
    }
    if next.count < previous.count {
        return Err(format!(
            "score upgrade contradicts lower bound {}: got {} {}",
            previous.count,
            next.relation(),
            next.count
        ));
    }
    if !next.exact && next.cap <= previous.cap {
        return Err(format!(
            "lower-bound upgrade cap {} does not exceed previous cap {}",
            next.cap, previous.cap
        ));
    }
    Ok(next)
}

fn probe_bucket(key: &[Edge]) -> usize {
    let hash = network_sha256(key);
    usize::from(u8::from_str_radix(&hash[..2], 16).expect("network SHA is hexadecimal") & 3)
}

fn select_probe_keys(
    archive: &BTreeMap<Vec<Edge>, ArchiveEntry>,
    candidate_keys: &[Vec<Edge>],
    limit: usize,
) -> Vec<Vec<Edge>> {
    let mut buckets: [Vec<Vec<Edge>>; 4] = std::array::from_fn(|_| Vec::new());
    for key in candidate_keys {
        let entry = &archive[key];
        if entry
            .score
            .is_some_and(|score| !score.exact && score.cap == NORMAL_CAP)
        {
            buckets[probe_bucket(key)].push(key.clone());
        }
    }
    let effort_rank = |left: &Vec<Edge>, right: &Vec<Edge>| {
        archive[right]
            .normal_stats
            .expect("normal lower bound has stats")
            .nodes
            .cmp(
                &archive[left]
                    .normal_stats
                    .expect("normal lower bound has stats")
                    .nodes,
            )
            .then_with(|| left.cmp(right))
    };
    for bucket in &mut buckets {
        bucket.sort_by(effort_rank);
    }
    let quota = limit.div_ceil(buckets.len());
    let mut selected = Vec::with_capacity(limit);
    let mut selected_set = BTreeSet::new();
    for bucket in &buckets {
        for key in bucket.iter().take(quota) {
            selected_set.insert(key.clone());
            selected.push(key.clone());
        }
    }
    if selected.len() < limit {
        let mut remainder = buckets
            .iter()
            .flatten()
            .filter(|key| !selected_set.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        remainder.sort_by(effort_rank);
        selected.extend(remainder.into_iter().take(limit - selected.len()));
    }
    selected.truncate(limit);
    selected
}

fn select_probe_keys_by_source(
    archive: &BTreeMap<Vec<Edge>, ArchiveEntry>,
    candidate_keys: &[Vec<Edge>],
    current_sources: &BTreeMap<Vec<Edge>, BTreeSet<SourceStratum>>,
    limit: usize,
) -> Vec<Vec<Edge>> {
    let eligible = candidate_keys
        .iter()
        .filter(|key| {
            archive[*key]
                .score
                .is_some_and(|score| !score.exact && score.cap == NORMAL_CAP)
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut strata = BTreeMap::<SourceStratum, Vec<Vec<Edge>>>::new();
    for key in &eligible {
        if let Some(sources) = current_sources.get(key) {
            for source in sources {
                strata.entry(source.clone()).or_default().push(key.clone());
            }
        }
    }
    for keys in strata.values_mut() {
        keys.sort();
        keys.dedup();
    }
    let mut offsets = BTreeMap::<SourceStratum, usize>::new();
    let mut selected = Vec::with_capacity(limit.min(eligible.len()));
    let mut selected_set = BTreeSet::new();
    while selected.len() < limit.min(eligible.len()) {
        let mut progress = false;
        for (stratum, keys) in &strata {
            let offset = offsets.entry(stratum.clone()).or_default();
            while *offset < keys.len() && selected_set.contains(&keys[*offset]) {
                *offset += 1;
            }
            if *offset < keys.len() {
                let key = keys[*offset].clone();
                *offset += 1;
                if selected_set.insert(key.clone()) {
                    selected.push(key);
                    progress = true;
                    if selected.len() == limit.min(eligible.len()) {
                        break;
                    }
                }
            }
        }
        if !progress {
            break;
        }
    }
    if selected.len() < limit.min(eligible.len()) {
        selected.extend(
            eligible
                .into_iter()
                .filter(|key| !selected_set.contains(key))
                .take(limit - selected.len()),
        );
    }
    selected
}

fn classify_candidates(
    archive: &mut BTreeMap<Vec<Edge>, ArchiveEntry>,
    count_cache: &mut BTreeMap<Vec<Edge>, Score>,
    candidates: BTreeMap<Vec<Edge>, Candidate>,
    options: &Options,
    accounting: &mut RoundAccounting,
    totals: &mut SolverTotals,
    current_sources: &BTreeMap<Vec<Edge>, BTreeSet<SourceStratum>>,
) -> Result<Option<Vec<Edge>>, String> {
    let candidate_keys = candidates.keys().cloned().collect::<Vec<_>>();
    for (key, candidate) in candidates {
        let entry = ArchiveEntry {
            hasse_edges: key.clone(),
            representative_full_edges: candidate.state.full_edges,
            representative_target: candidate.state.target,
            cells: candidate.state.cells,
            targets: candidate.targets,
            score: None,
            origin: candidate.origin,
            occurrences: candidate.occurrences,
            expanded_in_round: false,
            expanded_targets: BTreeSet::new(),
            high_cap_probed: false,
            normal_stats: None,
            probe_stats: None,
        };
        if archive.insert(key, entry).is_some() {
            return Err("new candidate collided with visited archive during insertion".to_owned());
        }
    }

    let mut unique = None;
    for key in &candidate_keys {
        if let Some(score) = count_cache.get(key).copied() {
            accounting.count_cache_hits += 1;
            if score.exact {
                accounting.best_new_exact_count = Some(
                    accounting
                        .best_new_exact_count
                        .map_or(score.count, |best| best.min(score.count)),
                );
            }
            archive
                .get_mut(key)
                .expect("candidate inserted above")
                .score = Some(score);
            continue;
        }
        if accounting.normal_count_calls >= options.max_new_counts {
            accounting.new_count_ceiling_hit = true;
            break;
        }
        if totals.calls >= options.max_total_solver_calls {
            accounting.total_call_ceiling_hit = true;
            break;
        }
        let solver = Solver::blank_comparisons(key)
            .map_err(|error| format!("cannot build candidate {}: {error}", network_sha256(key)))?;
        let result = solver.count_up_to(NORMAL_CAP);
        totals.add(result.stats);
        accounting.normal_count_calls += 1;
        if result.count == 0 {
            return Err(format!(
                "candidate {} has zero solutions despite a stored target witness",
                network_sha256(key)
            ));
        }
        let score = Score::from_result(result.count, result.capped, NORMAL_CAP);
        if score.exact {
            accounting.normal_exact += 1;
            accounting.best_new_exact_count = Some(
                accounting
                    .best_new_exact_count
                    .map_or(score.count, |best| best.min(score.count)),
            );
        } else {
            accounting.normal_lower_bounds += 1;
        }
        if count_cache.insert(key.clone(), score).is_some() {
            return Err("normal classification overwrote an existing count cache entry".to_owned());
        }
        let entry = archive.get_mut(key).expect("candidate inserted above");
        entry.score = Some(score);
        entry.normal_stats = Some(result.stats);
        insert_result_witnesses(
            entry,
            result.first_solution,
            result.second_solution,
            options.target_limit(),
        );
        if score.exact && score.count == 1 {
            accounting.unique_found = true;
            unique = Some(key.clone());
            break;
        }
        if options.progress_every != 0
            && accounting
                .normal_count_calls
                .is_multiple_of(options.progress_every)
        {
            eprintln!(
                "normal_counts={} of {} exact={} lower_bounds={} total_solver_calls={}",
                accounting.normal_count_calls,
                candidate_keys.len(),
                accounting.normal_exact,
                accounting.normal_lower_bounds,
                totals.calls
            );
        }
    }
    if unique.is_some() || accounting.ceiling_hit() {
        return Ok(unique);
    }

    accounting.high_cap_probes_eligible = candidate_keys
        .iter()
        .filter(|key| {
            archive[*key]
                .score
                .is_some_and(|score| !score.exact && score.cap == NORMAL_CAP)
        })
        .count() as u64;
    let probes = if options.continuation.is_some() {
        select_probe_keys_by_source(
            archive,
            &candidate_keys,
            current_sources,
            options.exploration_probes,
        )
    } else {
        select_probe_keys(archive, &candidate_keys, options.exploration_probes)
    };
    for key in probes {
        if totals.calls >= options.max_total_solver_calls {
            accounting.total_call_ceiling_hit = true;
            break;
        }
        let solver = Solver::blank_comparisons(&key).map_err(|error| {
            format!(
                "cannot build high-cap probe {}: {error}",
                network_sha256(&key)
            )
        })?;
        let result = solver.count_up_to(options.exploration_cap);
        totals.add(result.stats);
        accounting.high_cap_probe_calls += 1;
        if result.count == 0 {
            return Err(format!(
                "high-cap candidate {} unexpectedly has zero solutions",
                network_sha256(&key)
            ));
        }
        let score = Score::from_result(result.count, result.capped, options.exploration_cap);
        let previous = count_cache
            .get(&key)
            .copied()
            .ok_or_else(|| "high-cap probe is absent from count cache".to_owned())?;
        let score = monotonic_score_upgrade(previous, score)
            .map_err(|error| format!("high-cap probe {}: {error}", network_sha256(&key)))?;
        if score.exact {
            accounting.high_cap_exact += 1;
            accounting.best_new_exact_count = Some(
                accounting
                    .best_new_exact_count
                    .map_or(score.count, |best| best.min(score.count)),
            );
        } else {
            accounting.high_cap_lower_bounds += 1;
        }
        let replaced = count_cache.insert(key.clone(), score);
        if replaced != Some(previous) {
            return Err("count cache changed during high-cap score upgrade".to_owned());
        }
        let entry = archive.get_mut(&key).expect("candidate inserted above");
        entry.score = Some(score);
        entry.high_cap_probed = true;
        entry.probe_stats = Some(result.stats);
        insert_result_witnesses(
            entry,
            result.first_solution,
            result.second_solution,
            options.target_limit(),
        );
    }
    if accounting.normal_exact + accounting.normal_lower_bounds != accounting.normal_count_calls
        || accounting.high_cap_exact + accounting.high_cap_lower_bounds
            != accounting.high_cap_probe_calls
        || accounting.seed_replay_calls
            + accounting.target_pool_calls
            + accounting.normal_count_calls
            + accounting.high_cap_probe_calls
            != totals.calls
    {
        return Err("solver classification accounting does not partition exactly".to_owned());
    }
    Ok(unique)
}

fn select_next_frontier(
    archive: &BTreeMap<Vec<Edge>, ArchiveEntry>,
    target_limit: usize,
) -> Vec<Vec<Edge>> {
    let mut exact = archive
        .iter()
        .filter_map(|(key, entry)| {
            if !entry.has_unexpanded_target(target_limit) {
                return None;
            }
            entry
                .score
                .filter(|score| score.exact)
                .map(|score| (score.count, key.clone()))
        })
        .collect::<Vec<_>>();
    exact.sort_unstable();
    let exploit_count = BEAM_WIDTH - EXPLORATION_FRONTIER_SLOTS;
    let mut selected = exact
        .into_iter()
        .take(exploit_count)
        .map(|(_, key)| key)
        .collect::<Vec<_>>();
    let mut selected_set = selected.iter().cloned().collect::<BTreeSet<_>>();
    let exploration_cmp = |left: &Vec<Edge>, right: &Vec<Edge>| {
        let left_entry = &archive[left];
        let right_entry = &archive[right];
        let left_score = left_entry.score.expect("probed state has score");
        let right_score = right_entry.score.expect("probed state has score");
        match (left_score.exact, right_score.exact) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => left_score
                .count
                .cmp(&right_score.count)
                .then_with(|| left.cmp(right)),
            (false, false) => right_entry
                .probe_stats
                .expect("probed lower bound has stats")
                .nodes
                .cmp(
                    &left_entry
                        .probe_stats
                        .expect("probed lower bound has stats")
                        .nodes,
                )
                .then_with(|| left.cmp(right)),
        }
    };
    let mut buckets: [Vec<Vec<Edge>>; 4] = std::array::from_fn(|_| Vec::new());
    for (key, entry) in archive {
        if !selected_set.contains(key)
            && entry.high_cap_probed
            && entry.has_unexpanded_target(target_limit)
        {
            buckets[probe_bucket(key)].push(key.clone());
        }
    }
    for bucket in &mut buckets {
        bucket.sort_by(exploration_cmp);
    }
    let mut exploration_keys = buckets
        .iter()
        .filter_map(|bucket| bucket.first().cloned())
        .collect::<Vec<_>>();
    if exploration_keys.len() < EXPLORATION_FRONTIER_SLOTS {
        let chosen = exploration_keys.iter().cloned().collect::<BTreeSet<_>>();
        let mut remainder = buckets
            .iter()
            .flatten()
            .filter(|key| !chosen.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        remainder.sort_by(exploration_cmp);
        exploration_keys.extend(
            remainder
                .into_iter()
                .take(EXPLORATION_FRONTIER_SLOTS - exploration_keys.len()),
        );
    }
    for key in exploration_keys {
        selected_set.insert(key.clone());
        selected.push(key);
    }
    if selected.len() < BEAM_WIDTH {
        let mut fallback = archive
            .iter()
            .filter_map(|(key, entry)| {
                if selected_set.contains(key) || !entry.has_unexpanded_target(target_limit) {
                    None
                } else {
                    entry
                        .score
                        .map(|score| (score.count, !score.exact, key.clone()))
                }
            })
            .collect::<Vec<_>>();
        fallback.sort_unstable();
        for (_, _, key) in fallback.into_iter().take(BEAM_WIDTH - selected.len()) {
            selected.push(key);
        }
    }
    selected
}

fn execute_round(seeds: &[SeedInput], options: &Options) -> Result<RunOutcome, String> {
    let (frontier_indices, pending_indices) = select_seed_frontier(seeds)?;
    let (mut archive, mut count_cache, mut totals) =
        replay_seed_archive(seeds, options.max_total_solver_calls)?;
    let initial_frontier = frontier_indices
        .iter()
        .map(|&index| seeds[index].hasse_edges.clone())
        .collect::<Vec<_>>();
    let pending_seed_ordinals = pending_indices
        .iter()
        .map(|&index| seeds[index].ordinal)
        .collect::<Vec<_>>();
    let mut accounting = RoundAccounting {
        seed_archive: seeds.len() as u64,
        seed_frontier: initial_frontier.len() as u64,
        seed_pending: pending_seed_ordinals.len() as u64,
        seed_replay_calls: totals.calls,
        high_cap_probes_requested: options.exploration_probes as u64,
        high_cap_probe_cap: options.exploration_cap,
        ..RoundAccounting::default()
    };
    let mut current_sources = BTreeMap::new();
    let candidates = generate_round_candidates(
        &mut archive,
        &initial_frontier,
        &mut accounting,
        options.target_limit(),
        options.raw_move_hard_max(),
        None,
        &mut current_sources,
    )?;
    let unique_network = classify_candidates(
        &mut archive,
        &mut count_cache,
        candidates,
        options,
        &mut accounting,
        &mut totals,
        &current_sources,
    )?;
    let next_frontier = select_next_frontier(&archive, options.target_limit());
    let status = if accounting.unique_found {
        "unique-found"
    } else if accounting.ceiling_hit() || !accounting.round_complete() {
        "round-incomplete"
    } else {
        "round-complete"
    };
    Ok(RunOutcome {
        archive,
        initial_frontier,
        next_frontier,
        pending_seed_ordinals,
        frontier_roles: BTreeMap::new(),
        frontier_scores_at_selection: BTreeMap::new(),
        next_frontier_roles: BTreeMap::new(),
        target_audits: BTreeMap::new(),
        current_sources,
        continuation: None,
        count_cache,
        accounting,
        solver_totals: totals,
        status,
        unique_network,
    })
}

fn execute_round2(
    seeds: &[SeedInput],
    options: &Options,
    continuation_path: &Path,
    continuation_bytes: &[u8],
) -> Result<RunOutcome, String> {
    let (mut archive, mut count_cache, continuation) =
        load_round1_artifact(continuation_path, continuation_bytes, seeds)?;
    let (initial_frontier, frontier_roles) = select_round2_parent_frontier(&archive)?;
    let frontier_scores_at_selection = initial_frontier
        .iter()
        .map(|key| {
            (
                key.clone(),
                archive[key]
                    .score
                    .expect("round-two source archive is classified"),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let selected_seed_ordinals = initial_frontier
        .iter()
        .filter_map(|key| match archive[key].origin {
            Origin::Seed { ordinal } => Some(ordinal),
            Origin::Generated { .. } => None,
        })
        .collect::<BTreeSet<_>>();
    let pending_seed_ordinals = seeds
        .iter()
        .filter(|seed| {
            !selected_seed_ordinals.contains(&seed.ordinal)
                && archive[&seed.hasse_edges].expanded_targets.is_empty()
        })
        .map(|seed| seed.ordinal)
        .collect::<Vec<_>>();
    let mut accounting = RoundAccounting {
        seed_archive: SEED_COUNT as u64,
        seed_frontier: selected_seed_ordinals.len() as u64,
        seed_pending: pending_seed_ordinals.len() as u64,
        high_cap_probes_requested: options.exploration_probes as u64,
        high_cap_probe_cap: options.exploration_cap,
        ..RoundAccounting::default()
    };
    let mut totals = SolverTotals::default();
    let target_audits = enrich_round2_targets(
        &mut archive,
        &mut count_cache,
        &initial_frontier,
        options,
        &mut accounting,
        &mut totals,
    )?;
    let target_schedules = target_audits
        .iter()
        .map(|(key, audit)| (key.clone(), audit.ordered_targets.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut current_sources = BTreeMap::new();
    let candidates = generate_round_candidates(
        &mut archive,
        &initial_frontier,
        &mut accounting,
        options.target_limit(),
        options.raw_move_hard_max(),
        Some(&target_schedules),
        &mut current_sources,
    )?;
    let unique_network = classify_candidates(
        &mut archive,
        &mut count_cache,
        candidates,
        options,
        &mut accounting,
        &mut totals,
        &current_sources,
    )?;
    let (next_frontier, next_frontier_roles) =
        select_round2_successor_frontier(&archive, &current_sources)?;
    let status = if accounting.unique_found {
        "unique-found"
    } else if accounting.ceiling_hit() || !accounting.round_complete() {
        "round-incomplete"
    } else {
        "round-complete"
    };
    Ok(RunOutcome {
        archive,
        initial_frontier,
        next_frontier,
        pending_seed_ordinals,
        frontier_roles,
        frontier_scores_at_selection,
        next_frontier_roles,
        target_audits,
        current_sources,
        continuation: Some(continuation),
        count_cache,
        accounting,
        solver_totals: totals,
        status,
        unique_network,
    })
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

fn run() -> Result<(), String> {
    let options = parse_options()?;
    validate_paths_distinct(&options.seeds, &options.output)?;
    if options.output.exists() {
        return Err(format!(
            "refusing to overwrite existing output {}",
            options.output.display()
        ));
    }
    if let Some(continuation) = &options.continuation {
        validate_paths_distinct(continuation, &options.output)?;
        let seed_path = fs::canonicalize(&options.seeds)
            .map_err(|error| format!("cannot canonicalize {}: {error}", options.seeds.display()))?;
        let continuation_path = fs::canonicalize(continuation)
            .map_err(|error| format!("cannot canonicalize {}: {error}", continuation.display()))?;
        if paths_equal(&seed_path, &continuation_path) {
            return Err("seed and continuation inputs must differ".to_owned());
        }
    }
    if let Some(record_input) = options
        .root_neighborhood
        .as_ref()
        .and_then(|mode| mode.record_input.as_ref())
    {
        validate_paths_distinct(record_input, &options.output)?;
        let seed_path = fs::canonicalize(&options.seeds)
            .map_err(|error| format!("cannot canonicalize {}: {error}", options.seeds.display()))?;
        let record_path = fs::canonicalize(record_input)
            .map_err(|error| format!("cannot canonicalize {}: {error}", record_input.display()))?;
        if paths_equal(&seed_path, &record_path) {
            return Err("seed and external root-record inputs must differ".to_owned());
        }
    }
    let bytes = fs::read(&options.seeds)
        .map_err(|error| format!("cannot read {}: {error}", options.seeds.display()))?;
    let seeds = load_seed_artifact(&bytes)?;
    let binary = binary_provenance()?;
    if let Some(mode) = &options.root_two_cell_neighborhood {
        let selected = select_root_seed(&seeds, mode.seed_ordinal, mode.network_sha256.as_deref())?;
        let removed_pair_count =
            mode.last_removed_pair_ordinal - mode.first_removed_pair_ordinal + 1;
        eprintln!(
            "mode=root-two-cell-neighborhood root=seed42 removed_pairs={}..={} pair_count={} targets=128 count_cap={} raw_moves={}",
            mode.first_removed_pair_ordinal,
            mode.last_removed_pair_ordinal,
            removed_pair_count,
            mode.count_cap,
            removed_pair_count as u64 * ROOT_TWO_CELL_ADDITION_PAIRS * 128,
        );
        let outcome = execute_root_two_cell_neighborhood(
            &seeds,
            selected,
            mode.first_removed_pair_ordinal,
            mode.last_removed_pair_ordinal,
            mode.count_cap,
            options.progress_every,
        )?;
        emit_root_two_cell_neighborhood_output(&options, &binary, &outcome)?;
        let best = outcome
            .networks
            .values()
            .filter_map(|network| network.score.filter(|score| score.exact))
            .map(|score| score.count)
            .min();
        eprintln!(
            "status={} removed_pairs={}..={} targets=128 raw_moves={} distinct_networks={} direct_counts={} best={} solver_calls={} output={}",
            outcome.status,
            outcome.first_removed_pair_ordinal,
            outcome.last_removed_pair_ordinal,
            outcome.accounting.raw_move_attempts,
            outcome.accounting.distinct_observed_networks,
            outcome.accounting.network_count_calls,
            best.map_or_else(|| "none".to_owned(), |count| count.to_string()),
            outcome.solver_totals.calls,
            options.output.display(),
        );
        return Ok(());
    }
    if let Some(mode) = &options.root_neighborhood {
        let selected = match &mode.record_input {
            Some(path) => load_external_root_record(
                path,
                mode.network_sha256
                    .as_deref()
                    .expect("external root CLI requires SHA-256"),
                mode.record_sha256.as_deref(),
            )?,
            None => select_root_seed(&seeds, mode.seed_ordinal, mode.network_sha256.as_deref())?,
        };
        let source = selected.seed_ordinal().map_or_else(
            || "external-record".to_owned(),
            |ordinal| format!("frozen-seed-{ordinal}"),
        );
        eprintln!(
            "mode=root-neighborhood frozen_seeds={} root_source={} root_sha256={} root_count={} count_cap={} targets={} raw_moves={}",
            seeds.len(),
            source,
            selected.network_sha256,
            selected.solution_count,
            mode.count_cap,
            selected.solution_count,
            selected
                .solution_count
                .saturating_mul(ROOT_MOVES_PER_TARGET),
        );
        let outcome =
            execute_root_neighborhood(&seeds, selected, mode.count_cap, options.progress_every)?;
        emit_root_neighborhood_output(&options, &binary, &outcome)?;
        let best = outcome
            .networks
            .values()
            .filter_map(|network| network.score.filter(|score| score.exact))
            .map(|score| score.count)
            .min();
        eprintln!(
            "status={} root_sha256={} targets={} raw_moves={} distinct_networks={} direct_counts={} best={} solver_calls={} output={}",
            outcome.status,
            outcome.root.network_sha256,
            outcome.root_solutions.len(),
            outcome.accounting.raw_move_attempts,
            outcome.accounting.distinct_observed_networks,
            outcome.accounting.network_count_calls,
            best.map_or_else(|| "none".to_owned(), |count| count.to_string()),
            outcome.solver_totals.calls,
            options.output.display(),
        );
        return Ok(());
    }
    eprintln!(
        "mode={} frozen_seeds={} W={} target_witnesses_max={} normal_cap={} probes<={}@{}",
        if options.continuation.is_some() {
            "round2-continuation"
        } else {
            "round1"
        },
        seeds.len(),
        BEAM_WIDTH,
        options.target_limit(),
        NORMAL_CAP,
        options.exploration_probes,
        options.exploration_cap
    );
    let outcome = match &options.continuation {
        Some(path) => {
            let continuation_bytes = fs::read(path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            execute_round2(&seeds, &options, path, &continuation_bytes)?
        }
        None => execute_round(&seeds, &options)?,
    };
    emit_output(&options, &binary, &outcome)?;
    eprintln!(
        "status={} seed_archive={} seed_frontier={} seed_pending={} raw_moves={} new_networks={} normal_counts={} probes={} best={} solver_calls={} output={}",
        outcome.status,
        outcome.accounting.seed_archive,
        outcome.accounting.seed_frontier,
        outcome.accounting.seed_pending,
        outcome.accounting.raw_move_attempts,
        outcome.accounting.new_canonical_networks,
        outcome.accounting.normal_count_calls,
        outcome.accounting.high_cap_probe_calls,
        outcome
            .archive
            .values()
            .filter_map(|entry| entry
                .score
                .filter(|score| score.exact)
                .map(|score| score.count))
            .min()
            .map_or_else(|| "none".to_owned(), |count| count.to_string()),
        outcome.solver_totals.calls,
        options.output.display()
    );
    Ok(())
}

fn parse_options() -> Result<Options, String> {
    let mut seeds = PathBuf::from("analysis/18c-seeds-v1.jsonl");
    let mut continuation = None;
    let mut output = PathBuf::from("runs/18c-beam-round1-v1.jsonl");
    let mut progress_every = 1_000u64;
    let mut exploration_probes = DEFAULT_EXPLORATION_PROBES;
    let mut exploration_cap = DEFAULT_EXPLORATION_CAP;
    let mut max_new_counts = DEFAULT_MAX_NEW_COUNTS;
    let mut max_total_solver_calls = DEFAULT_MAX_TOTAL_SOLVER_CALLS;
    let mut root_mode = false;
    let mut root_two_cell_mode = false;
    let mut root_seed_ordinal = None;
    let mut root_network_sha256 = None;
    let mut root_record_input = None;
    let mut root_record_sha256 = None;
    let mut root_removed_pair_start = None;
    let mut root_removed_pair_end = None;
    let mut root_count_cap = DEFAULT_ROOT_COUNT_CAP;
    let mut output_explicit = false;
    let mut probes_explicit = false;
    let mut cap_explicit = false;
    let mut max_new_counts_explicit = false;
    let mut max_total_calls_explicit = false;
    let mut root_selector_explicit = false;
    let mut root_cap_explicit = false;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let mut value = || {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        match argument.as_str() {
            "--seeds" => seeds = PathBuf::from(value()?),
            "--continue-from" => continuation = Some(PathBuf::from(value()?)),
            "--root-neighborhood" => root_mode = true,
            "--root-two-cell-neighborhood" => root_two_cell_mode = true,
            "--root-seed-ordinal" => {
                let parsed = parse_u64(&argument, &value()?)?;
                root_seed_ordinal =
                    Some(usize::try_from(parsed).map_err(|_| format!("{argument} exceeds usize"))?);
                root_selector_explicit = true;
            }
            "--root-network-sha256" => {
                root_network_sha256 = Some(value()?.to_ascii_lowercase());
                root_selector_explicit = true;
            }
            "--root-record-input" => {
                root_record_input = Some(PathBuf::from(value()?));
                root_selector_explicit = true;
            }
            "--root-record-sha256" => {
                root_record_sha256 = Some(value()?.to_ascii_lowercase());
                root_selector_explicit = true;
            }
            "--root-removed-pair-start" => {
                let parsed = parse_u64(&argument, &value()?)?;
                root_removed_pair_start =
                    Some(usize::try_from(parsed).map_err(|_| format!("{argument} exceeds usize"))?);
                root_selector_explicit = true;
            }
            "--root-removed-pair-end" => {
                let parsed = parse_u64(&argument, &value()?)?;
                root_removed_pair_end =
                    Some(usize::try_from(parsed).map_err(|_| format!("{argument} exceeds usize"))?);
                root_selector_explicit = true;
            }
            "--count-cap" => {
                root_count_cap = parse_u64(&argument, &value()?)?;
                root_cap_explicit = true;
            }
            "--output" => {
                output = PathBuf::from(value()?);
                output_explicit = true;
            }
            "--progress-every" => progress_every = parse_u64(&argument, &value()?)?,
            "--exploration-probes" => {
                exploration_probes = usize::try_from(parse_u64(&argument, &value()?)?)
                    .map_err(|_| format!("{argument} exceeds usize"))?;
                probes_explicit = true;
            }
            "--exploration-cap" => {
                exploration_cap = parse_u64(&argument, &value()?)?;
                cap_explicit = true;
            }
            "--max-new-counts" => {
                max_new_counts = parse_u64(&argument, &value()?)?;
                max_new_counts_explicit = true;
            }
            "--max-total-solver-calls" => {
                max_total_solver_calls = parse_u64(&argument, &value()?)?;
                max_total_calls_explicit = true;
            }
            "--help" | "-h" => {
                print_usage();
                return Err("help requested".to_owned());
            }
            _ => return Err(format!("unknown option {argument}; use --help")),
        }
    }
    if root_mode && root_two_cell_mode {
        return Err(
            "--root-neighborhood and --root-two-cell-neighborhood are mutually exclusive"
                .to_owned(),
        );
    }
    let any_root_mode = root_mode || root_two_cell_mode;
    if any_root_mode && continuation.is_some() {
        return Err(
            "root-neighborhood modes and --continue-from are mutually exclusive".to_owned(),
        );
    }
    if any_root_mode
        && (probes_explicit || cap_explicit || max_new_counts_explicit || max_total_calls_explicit)
    {
        return Err(
            "beam probe/count ceilings are not valid in root-neighborhood modes".to_owned(),
        );
    }
    if root_seed_ordinal == Some(0) {
        return Err("--root-seed-ordinal must be positive".to_owned());
    }
    if let Some(hash) = &root_network_sha256
        && (hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err("--root-network-sha256 requires 64 hexadecimal characters".to_owned());
    }
    if let Some(hash) = &root_record_sha256
        && (hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err("--root-record-sha256 requires 64 hexadecimal characters".to_owned());
    }
    if any_root_mode && !(2..=MAX_ROOT_COUNT_CAP).contains(&root_count_cap) {
        return Err(format!(
            "--count-cap must be 2..={MAX_ROOT_COUNT_CAP}, got {root_count_cap}"
        ));
    }

    let root_neighborhood = if root_mode {
        if root_removed_pair_start.is_some() || root_removed_pair_end.is_some() {
            return Err("removed-pair selectors require --root-two-cell-neighborhood".to_owned());
        }
        if root_record_input.is_some() && root_seed_ordinal.is_some() {
            return Err(
                "--root-record-input and --root-seed-ordinal are mutually exclusive".to_owned(),
            );
        }
        if root_record_input.is_some() && root_network_sha256.is_none() {
            return Err("--root-record-input requires --root-network-sha256".to_owned());
        }
        if root_record_sha256.is_some() && root_record_input.is_none() {
            return Err("--root-record-sha256 requires --root-record-input".to_owned());
        }
        if !output_explicit {
            let label = root_seed_ordinal.map_or_else(
                || {
                    root_network_sha256.as_ref().map_or_else(
                        || DEFAULT_ROOT_SEED_ORDINAL.to_string(),
                        |hash| format!("sha-{}", &hash[..12]),
                    )
                },
                |ordinal| ordinal.to_string(),
            );
            let revision = if root_record_input.is_some() {
                "v2"
            } else {
                "v1"
            };
            let source = if root_record_input.is_some() {
                "record"
            } else {
                "seed"
            };
            output = PathBuf::from(format!(
                "runs/18c-root-{source}-{label}-radius1-cap{root_count_cap}-{revision}.jsonl"
            ));
        }
        Some(RootNeighborhoodOptions {
            seed_ordinal: root_seed_ordinal,
            network_sha256: root_network_sha256.clone(),
            record_input: root_record_input.clone(),
            record_sha256: root_record_sha256.clone(),
            count_cap: root_count_cap,
        })
    } else {
        None
    };

    let root_two_cell_neighborhood = if root_two_cell_mode {
        if root_record_input.is_some() {
            return Err("external root records are not supported in two-cell root mode".to_owned());
        }
        if root_record_sha256.is_some() {
            return Err(
                "external root record hashes are not supported in two-cell root mode".to_owned(),
            );
        }
        if root_seed_ordinal.is_some_and(|ordinal| ordinal != DEFAULT_ROOT_SEED_ORDINAL) {
            return Err("two-cell root mode is pinned to frozen seed ordinal 42".to_owned());
        }
        if root_network_sha256
            .as_deref()
            .is_some_and(|hash| hash != ROOT_TWO_CELL_SEED_SHA256)
        {
            return Err(format!(
                "two-cell root mode is pinned to seed-42 SHA-256 {ROOT_TWO_CELL_SEED_SHA256}"
            ));
        }
        let first_removed_pair_ordinal = root_removed_pair_start
            .ok_or_else(|| "two-cell root mode requires --root-removed-pair-start".to_owned())?;
        let last_removed_pair_ordinal = root_removed_pair_end
            .ok_or_else(|| "two-cell root mode requires --root-removed-pair-end".to_owned())?;
        if first_removed_pair_ordinal == 0
            || first_removed_pair_ordinal > last_removed_pair_ordinal
            || last_removed_pair_ordinal > ROOT_TWO_CELL_REMOVAL_PAIRS as usize
        {
            return Err(format!(
                "two-cell root removed-pair range {first_removed_pair_ordinal}..={last_removed_pair_ordinal} must lie within 1..={ROOT_TWO_CELL_REMOVAL_PAIRS}"
            ));
        }
        let removed_pair_count = last_removed_pair_ordinal - first_removed_pair_ordinal + 1;
        if removed_pair_count > MAX_ROOT_TWO_CELL_REMOVAL_PAIRS_PER_SHARD {
            return Err(format!(
                "two-cell root shards may contain at most {MAX_ROOT_TWO_CELL_REMOVAL_PAIRS_PER_SHARD} removal pairs; got {removed_pair_count}"
            ));
        }
        if !root_cap_explicit {
            root_count_cap = NORMAL_CAP;
        }
        if root_count_cap < NORMAL_CAP {
            return Err(format!(
                "two-cell root mode requires --count-cap at least {NORMAL_CAP} to classify every possible improvement over count 128"
            ));
        }
        if !output_explicit {
            output = PathBuf::from(format!(
                "runs/18c-root-seed-42-shell2-removed-pairs{first_removed_pair_ordinal}-{last_removed_pair_ordinal}-cap{root_count_cap}-v1.jsonl"
            ));
        }
        Some(RootTwoCellNeighborhoodOptions {
            seed_ordinal: root_seed_ordinal.or(Some(DEFAULT_ROOT_SEED_ORDINAL)),
            network_sha256: root_network_sha256,
            first_removed_pair_ordinal,
            last_removed_pair_ordinal,
            count_cap: root_count_cap,
        })
    } else {
        None
    };

    if !any_root_mode && (root_selector_explicit || root_cap_explicit) {
        return Err(
            "root selectors, removed-pair ranges, and --count-cap require a root-neighborhood mode"
                .to_owned(),
        );
    }
    if continuation.is_some() && !any_root_mode {
        if !output_explicit {
            output = PathBuf::from("runs/18c-beam-round2-v1.jsonl");
        }
        if !probes_explicit {
            exploration_probes = 4_096;
        }
        if !cap_explicit {
            exploration_cap = 4_096;
        }
    }
    if max_new_counts > DEFAULT_MAX_NEW_COUNTS {
        return Err(format!(
            "--max-new-counts may not exceed the audited hard ceiling {DEFAULT_MAX_NEW_COUNTS}"
        ));
    }
    if max_total_solver_calls > DEFAULT_MAX_TOTAL_SOLVER_CALLS {
        return Err(format!(
            "--max-total-solver-calls may not exceed the audited hard ceiling {DEFAULT_MAX_TOTAL_SOLVER_CALLS}"
        ));
    }
    if exploration_probes as u64 > max_new_counts {
        return Err(format!(
            "--exploration-probes {exploration_probes} exceeds --max-new-counts {max_new_counts}"
        ));
    }
    validate_exploration_cap(exploration_cap)?;
    Ok(Options {
        seeds,
        continuation,
        output,
        progress_every,
        exploration_probes,
        exploration_cap,
        max_new_counts,
        max_total_solver_calls,
        root_neighborhood,
        root_two_cell_neighborhood,
    })
}

fn parse_u64(option: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("{option} requires an unsigned integer, got {value:?}"))
}

fn validate_exploration_cap(cap: u64) -> Result<(), String> {
    if (NORMAL_CAP + 1..=MAX_EXPLORATION_CAP).contains(&cap) {
        Ok(())
    } else {
        Err(format!(
            "--exploration-cap must be greater than {NORMAL_CAP} and at most {MAX_EXPLORATION_CAP}, got {cap}"
        ))
    }
}

fn print_usage() {
    eprintln!(
        "Usage: thermo-18c-beam [OPTIONS]\n\
         \n\
         --seeds PATH                   Frozen 71-seed JSONL artifact\n\
         --continue-from PATH           Full round-one JSONL; enables strict round-two continuation\n\
         --root-neighborhood            Exhaust one exact root's all-solution radius-one neighborhood\n\
         --root-two-cell-neighborhood   Exhaust one seed-42 two-exchange removed-pair shard\n\
         --root-seed-ordinal N          Root seed ordinal; default 42 without a selector\n\
         --root-network-sha256 HEX      Root hash; cross-checks an ordinal or selects a record\n\
         --root-record-input PATH       Audited beam-v2 or completed root-neighborhood JSONL\n\
         --root-record-sha256 HEX       Whole-file pin for a completed root-neighborhood input\n\
         --root-removed-pair-start N    Inclusive first removed-pair ordinal for shell-2 shard\n\
         --root-removed-pair-end N      Inclusive last ordinal; at most four pairs per shard\n\
         --count-cap N                  Root count cap; radius1 default 4096, shell2 default 129\n\
         --output PATH                  Atomic no-clobber round report\n\
         --progress-every N             Progress interval; 0 disables\n\
         --exploration-probes N         High-cap probes; default 128, max new-count ceiling\n\
         --exploration-cap N            Probe count cap; 130..=4096, default 512\n\
         --max-new-counts N             Safety ceiling, at most 75000\n\
         --max-total-solver-calls N      Safety ceiling, at most 80000\n"
    );
}

fn validate_paths_distinct(input: &Path, output: &Path) -> Result<(), String> {
    let input = fs::canonicalize(input)
        .map_err(|error| format!("cannot canonicalize {}: {error}", input.display()))?;
    let output = absolute_output_path(output)?;
    if paths_equal(&input, &output) {
        Err("seed input and output paths must differ".to_owned())
    } else {
        Ok(())
    }
}

fn absolute_output_path(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return fs::canonicalize(path)
            .map_err(|error| format!("cannot canonicalize {}: {error}", path.display()));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let parent = match parent {
        Some(parent) => fs::canonicalize(parent)
            .map_err(|error| format!("cannot canonicalize {}: {error}", parent.display()))?,
        None => {
            env::current_dir().map_err(|error| format!("cannot read current directory: {error}"))?
        }
    };
    let name = path
        .file_name()
        .ok_or_else(|| format!("output {} has no file name", path.display()))?;
    Ok(parent.join(name))
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn binary_provenance() -> Result<BinaryProvenance, String> {
    let path =
        env::current_exe().map_err(|error| format!("cannot locate current executable: {error}"))?;
    let path = fs::canonicalize(&path)
        .map_err(|error| format!("cannot canonicalize {}: {error}", path.display()))?;
    let bytes = fs::read(&path)
        .map_err(|error| format!("cannot read executable {}: {error}", path.display()))?;
    Ok(BinaryProvenance {
        path,
        bytes: bytes.len(),
        sha256: sha256_hex(&bytes),
    })
}

fn root_origin_json(origin: &RootMoveOrigin) -> String {
    format!(
        "{{\"target_ordinal\":{},\"coordinate_system\":\"declared-root canonical coordinates\",\"move\":{}}}",
        origin.target_ordinal,
        move_json(origin.move_kind)
    )
}

fn root_output_schema(outcome: &RootNeighborhoodOutcome) -> &'static str {
    if outcome.root.is_external() {
        EXTERNAL_ROOT_NEIGHBORHOOD_SCHEMA
    } else {
        ROOT_NEIGHBORHOOD_SCHEMA
    }
}

fn ordered_root_solutions_sha256(solutions: &[Grid]) -> String {
    let mut bytes = Vec::with_capacity(solutions.len() * CELLS);
    for solution in solutions {
        bytes.extend_from_slice(solution);
    }
    sha256_hex(&bytes)
}

fn hasse_key_set_sha256<'a, I>(keys: I) -> String
where
    I: IntoIterator<Item = &'a Vec<Edge>>,
{
    let mut bytes = Vec::new();
    for key in keys {
        bytes.extend_from_slice(&(key.len() as u32).to_le_bytes());
        for &(lower, upper) in key {
            bytes.extend_from_slice(&[lower, upper]);
        }
    }
    sha256_hex(&bytes)
}

fn root_two_cell_observed_ordinals_json(mask: [u64; 2]) -> String {
    let ordinals = (1usize..=128)
        .filter(|ordinal| {
            let index = (ordinal - 1) / 64;
            let bit = 1u64 << ((ordinal - 1) % 64);
            mask[index] & bit != 0
        })
        .map(|ordinal| ordinal.to_string())
        .collect::<Vec<_>>();
    format!("[{}]", ordinals.join(","))
}

fn root_two_cell_origin_json(origin: RootTwoCellMoveOrigin, root_solutions: &[Grid]) -> String {
    let raw_target = &root_solutions[origin.target_ordinal - 1];
    format!(
        "{{\"target_ordinal\":{},\"root_target\":{},\"coordinate_system\":\"declared-root canonical coordinates\",\"move\":{{\"kind\":\"swap-two\",\"removed_pair_ordinal\":{},\"removed\":[{},{}],\"added\":[{},{}]}}}}",
        origin.target_ordinal,
        json_quote(&grid_string(raw_target)),
        origin.removed_pair_ordinal,
        origin.removed[0],
        origin.removed[1],
        origin.added[0],
        origin.added[1],
    )
}

fn root_two_cell_selected_removed_pairs_json(
    root: &ExactRoot,
    first: usize,
    last: usize,
) -> String {
    let mut ordinal = 0usize;
    let mut selected = Vec::new();
    for first_index in 0..root.cells.len() {
        for second_index in first_index + 1..root.cells.len() {
            ordinal += 1;
            if (first..=last).contains(&ordinal) {
                selected.push(format!(
                    "{{\"ordinal\":{ordinal},\"cells\":[{},{}]}}",
                    root.cells[first_index], root.cells[second_index]
                ));
            }
        }
    }
    format!("[{}]", selected.join(","))
}

fn root_two_cell_header_json(
    options: &Options,
    binary: &BinaryProvenance,
    outcome: &RootTwoCellNeighborhoodOutcome,
) -> String {
    let mode = options
        .root_two_cell_neighborhood
        .as_ref()
        .expect("two-cell root header requires its mode");
    let removed_pair_count =
        outcome.last_removed_pair_ordinal - outcome.first_removed_pair_ordinal + 1;
    let classification_complete = outcome.accounting.unclassified_networks == 0;
    let no_unique = outcome.unique_network.is_none() && classification_complete;
    let improvement_exists = outcome.networks.values().any(|network| {
        network
            .score
            .is_some_and(|score| score.exact && score.count < outcome.root.solution_count)
    });
    let no_improvement = classification_complete && !improvement_exists;
    let below_cap_exists = outcome.networks.values().any(|network| {
        network
            .score
            .is_some_and(|score| score.exact && score.count < mode.count_cap)
    });
    let no_count_below_cap = classification_complete && !below_cap_exists;
    let geometric_survivors =
        outcome.accounting.raw_move_attempts - outcome.accounting.geometric_incidence_rejections;
    format!(
        "{{\"type\":\"header\",\"schema\":{},\"algorithm_revision\":{},\"mode\":\"frozen-seed42-solution-preserving-two-exchange-shell-shard\",\"scope\":{{\"global_18c_exhaustive\":false,\"complete_root_solution_set_enumerated\":true,\"all_128_root_targets_used\":true,\"complete_seed42_solution_preserving_two_exchange_shell_for_declared_removed_pair_range\":true,\"complete_all_153_removed_pairs\":false,\"classification_complete\":{},\"no_unique_in_declared_removed_pair_shard\":{},\"no_improvement_below_root_count_128_in_declared_removed_pair_shard\":{},\"no_exact_solution_count_below_configured_cap_in_declared_removed_pair_shard\":{}}},\"seed_input\":{{\"path\":{},\"bytes\":{SEED_ARTIFACT_BYTES},\"sha256\":{}}},\"binary\":{{\"path\":{},\"bytes\":{},\"sha256\":{}}},\"root\":{{\"seed_ordinal\":42,\"network_sha256\":{},\"state_sha256\":{},\"exact_solution_count\":128,\"canonical_cells\":{},\"hasse_edges\":{},\"representative_full_saturated_edges\":{},\"representative_target\":{},\"ordered_solution_set_sha256\":{},\"ordered_solution_set_encoding\":\"128 lexicographically sorted grids concatenated as 81 numeric u8 bytes 0x01..0x09 per grid, not ASCII\"}},\"removed_pair_shard\":{{\"range_convention\":\"one-based inclusive\",\"pair_order\":\"nested lexicographic pairs of the sorted canonical root cells: first index ascending, then second index ascending\",\"first\":{},\"last\":{},\"count\":{},\"selected_pairs\":{},\"max_pairs_per_artifact\":{MAX_ROOT_TWO_CELL_REMOVAL_PAIRS_PER_SHARD},\"full_shell_proof_requirement\":\"same root, ordered-solution digest, algorithm, and cap at least 129; pairwise-disjoint inclusive ranges whose union is exactly 1..=153; every shard classification complete with no exact count 1..127; per-shard distinct counts must not be summed as a global distinct count\"}},\"model\":{{\"covered_cells\":18,\"constraints\":\"directed unequal king-neighbour comparisons\",\"overlap_and_branching\":true,\"coverage_rule\":\"all 18 cells incident after target saturation\"}},\"configuration\":{{\"count_cap\":{},\"minimum_cap_for_no_improvement\":129,\"root_targets\":128,\"removal_pairs_total\":{ROOT_TWO_CELL_REMOVAL_PAIRS},\"addition_pairs_per_removal\":{ROOT_TWO_CELL_ADDITION_PAIRS},\"raw_moves_per_removed_pair\":{},\"raw_moves_in_shard\":{},\"geometric_survivors_in_shard\":{}}},\"orders\":{{\"root_solutions\":\"complete solutions sorted lexicographically and deduplicated; one-based ordinal\",\"moves\":\"target ordinal ascending, removed-pair ordinal ascending, added outside-cell pair lexicographic\",\"classification\":\"exact canonical Hasse vector ascending; frozen seed exact cache first, otherwise direct count to cap; stop immediately after exact count 1\"}},\"geometry_prune\":\"reject exactly those final 18-cell king graphs with an isolated vertex; the two added cells may be incident only to each other\",\"identity\":\"exact canonical Hasse edge vector; SHA-256 is an identifier only\",\"witness_policy\":\"every distinct key is solver-counted or uses the independently replayed frozen exact cache; observation multiplicity is never used as a solution-count shortcut\",\"replay\":\"apply representative swap-two to the recorded root target in declared-root coordinates, saturate, require exact incident coverage, Hasse-reduce, and canonicalize under D4 plus optional digit complement/global reversal\"}}",
        json_quote(ROOT_TWO_CELL_NEIGHBORHOOD_SCHEMA),
        json_quote(ROOT_TWO_CELL_NEIGHBORHOOD_ALGORITHM_REVISION),
        classification_complete,
        no_unique,
        no_improvement,
        no_count_below_cap,
        json_quote(&options.seeds.display().to_string()),
        json_quote(SEED_ARTIFACT_SHA256),
        json_quote(&binary.path.display().to_string()),
        binary.bytes,
        json_quote(&binary.sha256),
        json_quote(&outcome.root.network_sha256),
        json_quote(&outcome.root.state_sha256),
        u8_list_json(&outcome.root.cells),
        edges_json(&outcome.root.hasse_edges),
        edges_json(&outcome.root.full_edges),
        json_quote(&grid_string(&outcome.root.target)),
        json_quote(&ordered_root_solutions_sha256(&outcome.root_solutions)),
        outcome.first_removed_pair_ordinal,
        outcome.last_removed_pair_ordinal,
        removed_pair_count,
        root_two_cell_selected_removed_pairs_json(
            &outcome.root,
            outcome.first_removed_pair_ordinal,
            outcome.last_removed_pair_ordinal,
        ),
        mode.count_cap,
        ROOT_TWO_CELL_ADDITION_PAIRS * 128,
        outcome.accounting.raw_move_attempts,
        geometric_survivors,
    )
}

fn root_two_cell_solution_json(
    _outcome: &RootTwoCellNeighborhoodOutcome,
    ordinal: usize,
    grid: &Grid,
) -> String {
    format!(
        "{{\"type\":\"root_solution\",\"schema\":{},\"ordinal\":{},\"grid\":{},\"used_by_removed_pair_shard\":true}}",
        json_quote(ROOT_TWO_CELL_NEIGHBORHOOD_SCHEMA),
        ordinal,
        json_quote(&grid_string(grid)),
    )
}

fn root_two_cell_network_json(
    key: &[Edge],
    network: &RootTwoCellNetworkResult,
    root_solutions: &[Grid],
) -> String {
    format!(
        "{{\"type\":\"network\",\"schema\":{},\"network_sha256\":{},\"hasse_edges\":{},\"representative_origin\":{},\"representative_canonical_target\":{},\"observed_target_ordinals\":{},\"occurrences\":{},\"score\":{},\"preexisting_seed_ordinal\":{},\"solver_stats\":{}}}",
        json_quote(ROOT_TWO_CELL_NEIGHBORHOOD_SCHEMA),
        json_quote(&network_sha256(key)),
        edges_json(key),
        root_two_cell_origin_json(network.representative_origin, root_solutions),
        json_quote(&grid_string(&network.representative_canonical_target)),
        root_two_cell_observed_ordinals_json(network.observed_target_mask),
        network.occurrences,
        score_json(network.score),
        network
            .preexisting_seed_ordinal
            .map_or_else(|| "null".to_owned(), |ordinal| ordinal.to_string()),
        optional_solve_stats_json(network.solver_stats),
    )
}

fn root_two_cell_summary_json(
    options: &Options,
    outcome: &RootTwoCellNeighborhoodOutcome,
) -> String {
    let mode = options
        .root_two_cell_neighborhood
        .as_ref()
        .expect("two-cell root summary requires its mode");
    let removed_pair_count =
        outcome.last_removed_pair_ordinal - outcome.first_removed_pair_ordinal + 1;
    let classification_complete = outcome.accounting.unclassified_networks == 0;
    let no_unique = outcome.unique_network.is_none() && classification_complete;
    let occurrence_sum = outcome
        .networks
        .values()
        .map(|network| network.occurrences)
        .sum::<u64>();
    let best_exact = outcome
        .networks
        .values()
        .filter_map(|network| network.score.filter(|score| score.exact))
        .map(|score| score.count)
        .min();
    let improvement_count = outcome
        .networks
        .values()
        .filter(|network| {
            network
                .score
                .is_some_and(|score| score.exact && score.count < outcome.root.solution_count)
        })
        .count();
    let no_improvement = classification_complete && improvement_count == 0;
    let below_cap_count = outcome
        .networks
        .values()
        .filter(|network| {
            network
                .score
                .is_some_and(|score| score.exact && score.count < mode.count_cap)
        })
        .count();
    let no_count_below_cap = classification_complete && below_cap_count == 0;
    format!(
        "{{\"type\":\"summary\",\"schema\":{},\"status\":{},\"terminal_unique\":{},\"generation_complete_for_declared_removed_pair_shard\":true,\"classification_complete\":{},\"no_unique_in_declared_removed_pair_shard\":{},\"no_improvement_below_root_count_128_in_declared_removed_pair_shard\":{},\"no_exact_solution_count_below_configured_cap_in_declared_removed_pair_shard\":{},\"root\":{{\"seed_ordinal\":42,\"network_sha256\":{},\"exact_solution_count\":128,\"ordered_solution_set_sha256\":{}}},\"removed_pair_shard\":{{\"range_convention\":\"one-based inclusive\",\"first\":{},\"last\":{},\"count\":{},\"selected_pairs\":{}}},\"target_enumeration\":{{\"calls\":{},\"solutions\":128,\"all_used\":true,\"exhausted\":true,\"capped\":false,\"solver_stats\":{}}},\"moves\":{{\"raw_moves_per_removed_pair\":{},\"raw_attempts\":{},\"geometric_incidence_rejections\":{},\"coverage_rejections\":{},\"accepted_observations\":{},\"distinct_canonical_networks_within_shard\":{},\"duplicate_observations_within_shard\":{},\"summed_network_occurrences\":{},\"canonical_key_set_sha256\":{},\"canonical_key_set_encoding\":\"keys in exact BTree order; each u32 little-endian edge count followed by lower,upper bytes\"}},\"classification\":{{\"count_cap\":{},\"frozen_seed_cache_hits\":{},\"preexisting_seed_networks_observed\":{},\"new_canonical_networks\":{},\"direct_count_calls\":{},\"exact_networks\":{},\"lower_bound_networks\":{},\"unclassified_after_terminal_unique\":{},\"best_exact_count\":{},\"improvements_over_root_count\":{},\"exact_counts_below_cap\":{}}},\"solver_call_partition\":{{\"seed_replay_calls\":{},\"root_enumeration_calls\":{},\"network_count_calls\":{},\"total_calls\":{}}},\"solver_totals\":{},\"unique_network_sha256\":{}}}",
        json_quote(ROOT_TWO_CELL_NEIGHBORHOOD_SCHEMA),
        json_quote(outcome.status),
        outcome.unique_network.is_some(),
        classification_complete,
        no_unique,
        no_improvement,
        no_count_below_cap,
        json_quote(&outcome.root.network_sha256),
        json_quote(&ordered_root_solutions_sha256(&outcome.root_solutions)),
        outcome.first_removed_pair_ordinal,
        outcome.last_removed_pair_ordinal,
        removed_pair_count,
        root_two_cell_selected_removed_pairs_json(
            &outcome.root,
            outcome.first_removed_pair_ordinal,
            outcome.last_removed_pair_ordinal,
        ),
        outcome.accounting.root_enumeration_calls,
        solve_stats_json(outcome.root_enumeration_stats),
        ROOT_TWO_CELL_ADDITION_PAIRS * 128,
        outcome.accounting.raw_move_attempts,
        outcome.accounting.geometric_incidence_rejections,
        outcome.accounting.coverage_rejections,
        outcome.accounting.accepted_observations,
        outcome.accounting.distinct_observed_networks,
        outcome.accounting.duplicate_observations,
        occurrence_sum,
        json_quote(&hasse_key_set_sha256(outcome.networks.keys())),
        mode.count_cap,
        outcome.accounting.count_cache_hits,
        outcome.accounting.preexisting_seed_networks_observed,
        outcome.accounting.new_canonical_networks,
        outcome.accounting.network_count_calls,
        outcome.accounting.exact_networks,
        outcome.accounting.lower_bound_networks,
        outcome.accounting.unclassified_networks,
        best_exact.map_or_else(|| "null".to_owned(), |count| count.to_string()),
        improvement_count,
        below_cap_count,
        outcome.accounting.seed_replay_calls,
        outcome.accounting.root_enumeration_calls,
        outcome.accounting.network_count_calls,
        outcome.solver_totals.calls,
        solver_totals_json(&outcome.solver_totals),
        outcome
            .unique_network
            .as_ref()
            .map_or_else(|| "null".to_owned(), |key| json_quote(&network_sha256(key))),
    )
}

fn emit_root_two_cell_neighborhood_output(
    options: &Options,
    binary: &BinaryProvenance,
    outcome: &RootTwoCellNeighborhoodOutcome,
) -> Result<(), String> {
    atomic_write(&options.output, |writer| {
        write_json_line(writer, &root_two_cell_header_json(options, binary, outcome))?;
        for (index, grid) in outcome.root_solutions.iter().enumerate() {
            write_json_line(
                writer,
                &root_two_cell_solution_json(outcome, index + 1, grid),
            )?;
        }
        for (key, network) in &outcome.networks {
            write_json_line(
                writer,
                &root_two_cell_network_json(key, network, &outcome.root_solutions),
            )?;
        }
        write_json_line(writer, &root_two_cell_summary_json(options, outcome))?;
        Ok(())
    })
}

fn root_header_json(
    options: &Options,
    binary: &BinaryProvenance,
    outcome: &RootNeighborhoodOutcome,
) -> String {
    let mode = options
        .root_neighborhood
        .as_ref()
        .expect("root-neighborhood header requires root mode");
    let classification_complete = outcome.accounting.unclassified_networks == 0;
    let negative_result = outcome.unique_network.is_none() && classification_complete;
    let expected_raw_moves = u64::try_from(outcome.root_solutions.len())
        .expect("root solution count fits u64")
        * ROOT_MOVES_PER_TARGET;
    format!(
        "{{\"type\":\"header\",\"schema\":{},\"algorithm_revision\":{},\"mode\":\"exact-root-all-solutions-radius-one\",\"scope\":{{\"global_18c_exhaustive\":false,\"complete_root_solution_set\":true,\"complete_saturated_radius_one_generation_for_declared_root\":true,\"classification_complete\":{},\"negative_uniqueness_result_only_for_declared_root_neighborhood\":{}}},\"seed_input\":{{\"path\":{},\"bytes\":{SEED_ARTIFACT_BYTES},\"sha256\":{}}},\"binary\":{{\"path\":{},\"bytes\":{},\"sha256\":{}}},\"root\":{{\"seed_ordinal\":{},\"network_sha256\":{},\"state_sha256\":{},\"exact_solution_count\":{},\"canonical_cells\":{},\"hasse_edges\":{},\"representative_full_saturated_edges\":{},\"representative_target\":{}}},\"model\":{{\"covered_cells\":18,\"constraints\":\"directed unequal king-neighbour comparisons\",\"overlap_and_branching\":true,\"coverage_rule\":\"all 18 cells incident after target saturation\"}},\"configuration\":{{\"count_cap\":{},\"target_count\":{},\"moves_per_target\":{ROOT_MOVES_PER_TARGET},\"raw_move_count\":{},\"same_footprint_moves_per_target\":1,\"swap_moves_per_target\":1134,\"swap_space\":\"all 18 removed root cells times all 63 cells outside the root footprint\"}},\"orders\":{{\"root_solutions\":\"complete solutions sorted lexicographically and deduplicated; one-based ordinal\",\"moves\":\"retarget, then removed root cell ascending, then added outside cell ascending\",\"classification\":\"exact canonical Hasse vector ascending; frozen seed replay cache first, otherwise direct count to cap; stop immediately after exact count 1\"}},\"identity\":\"exact canonical Hasse edge vector; SHA-256 is an identifier only\",\"replay\":\"apply each network representative_origin to the indexed root_solution in declared-root coordinates, saturate, validate exact incident coverage, Hasse-reduce, and canonicalize under D4 plus optional digit complement/global reversal\"}}",
        json_quote(ROOT_NEIGHBORHOOD_SCHEMA),
        json_quote(ROOT_NEIGHBORHOOD_ALGORITHM_REVISION),
        classification_complete,
        negative_result,
        json_quote(&options.seeds.to_string_lossy()),
        json_quote(SEED_ARTIFACT_SHA256),
        json_quote(&binary.path.to_string_lossy()),
        binary.bytes,
        json_quote(&binary.sha256),
        outcome
            .root
            .seed_ordinal()
            .expect("v1 root header requires frozen seed"),
        json_quote(&outcome.root.network_sha256),
        json_quote(&outcome.root.state_sha256),
        outcome.root.solution_count,
        u8_list_json(&outcome.root.cells),
        edges_json(&outcome.root.hasse_edges),
        edges_json(&outcome.root.full_edges),
        json_quote(&grid_string(&outcome.root.target)),
        mode.count_cap,
        outcome.root_solutions.len(),
        expected_raw_moves,
    )
}

fn external_root_header_json(
    options: &Options,
    binary: &BinaryProvenance,
    outcome: &RootNeighborhoodOutcome,
) -> String {
    let mode = options
        .root_neighborhood
        .as_ref()
        .expect("external-root header requires root mode");
    let RootSource::ExternalRecord(provenance) = &outcome.root.source else {
        panic!("external-root header requires external provenance");
    };
    let classification_complete = outcome.accounting.unclassified_networks == 0;
    let negative_result = outcome.unique_network.is_none() && classification_complete;
    let expected_raw_moves = u64::try_from(outcome.root_solutions.len())
        .expect("root solution count fits u64")
        * ROOT_MOVES_PER_TARGET;
    let replay_cap = outcome.root.solution_count.saturating_add(1);
    format!(
        "{{\"type\":\"header\",\"schema\":{},\"algorithm_revision\":{},\"mode\":\"external-exact-root-all-solutions-radius-one\",\"scope\":{{\"global_18c_exhaustive\":false,\"complete_root_solution_set\":true,\"complete_saturated_radius_one_generation_for_declared_root\":true,\"classification_complete\":{},\"negative_uniqueness_result_only_for_declared_root_neighborhood\":{}}},\"seed_input\":{{\"path\":{},\"bytes\":{SEED_ARTIFACT_BYTES},\"sha256\":{}}},\"root_record_input\":{{\"path\":{},\"bytes\":{},\"sha256\":{},\"schema\":{},\"algorithm_revision\":{},\"authentication\":{},\"selected_line\":{},\"selection\":\"exactly one top-level network_sha256 match in a complete same-buffer JSONL scan\"}},\"binary\":{{\"path\":{},\"bytes\":{},\"sha256\":{}}},\"root\":{{\"source\":\"external-network-record\",\"network_sha256\":{},\"state_sha256\":{},\"stored_exact_solution_count\":{},\"independent_replay_cap\":{},\"canonical_cells\":{},\"hasse_edges\":{},\"representative_full_saturated_edges\":{},\"representative_target\":{}}},\"model\":{{\"covered_cells\":18,\"constraints\":\"directed unequal king-neighbour comparisons\",\"overlap_and_branching\":true,\"coverage_rule\":\"all 18 cells incident after target saturation\"}},\"configuration\":{{\"count_cap\":{},\"target_count\":{},\"moves_per_target\":{ROOT_MOVES_PER_TARGET},\"raw_move_count\":{},\"same_footprint_moves_per_target\":1,\"swap_moves_per_target\":1134,\"swap_space\":\"all 18 removed root cells times all 63 cells outside the root footprint\"}},\"orders\":{{\"root_solutions\":\"complete solutions sorted lexicographically and deduplicated; one-based ordinal\",\"moves\":\"retarget, then removed root cell ascending, then added outside cell ascending\",\"classification\":\"exact canonical Hasse vector ascending; external exact-root replay cache, then frozen seed replay cache, otherwise direct count to cap; stop immediately after exact count 1\"}},\"identity\":\"exact canonical Hasse edge vector; SHA-256 is an identifier only\",\"replay\":\"apply each network representative_origin to the indexed root_solution in declared-root canonical coordinates, saturate, validate exact incident coverage, Hasse-reduce, and canonicalize under D4 plus optional digit complement/global reversal\"}}",
        json_quote(EXTERNAL_ROOT_NEIGHBORHOOD_SCHEMA),
        json_quote(EXTERNAL_ROOT_NEIGHBORHOOD_ALGORITHM_REVISION),
        classification_complete,
        negative_result,
        json_quote(&options.seeds.to_string_lossy()),
        json_quote(SEED_ARTIFACT_SHA256),
        json_quote(&provenance.path.to_string_lossy()),
        provenance.bytes,
        json_quote(&provenance.sha256),
        json_quote(&provenance.schema),
        json_quote(&provenance.algorithm_revision),
        json_quote(&provenance.authentication),
        provenance.line_number,
        json_quote(&binary.path.to_string_lossy()),
        binary.bytes,
        json_quote(&binary.sha256),
        json_quote(&outcome.root.network_sha256),
        json_quote(&outcome.root.state_sha256),
        outcome.root.solution_count,
        replay_cap,
        u8_list_json(&outcome.root.cells),
        edges_json(&outcome.root.hasse_edges),
        edges_json(&outcome.root.full_edges),
        json_quote(&grid_string(&outcome.root.target)),
        mode.count_cap,
        outcome.root_solutions.len(),
        expected_raw_moves,
    )
}

fn root_solution_json(outcome: &RootNeighborhoodOutcome, ordinal: usize, grid: &Grid) -> String {
    format!(
        "{{\"type\":\"root_solution\",\"schema\":{},\"root_network_sha256\":{},\"ordinal\":{},\"grid\":{},\"witness_sha256\":{}}}",
        json_quote(root_output_schema(outcome)),
        json_quote(&outcome.root.network_sha256),
        ordinal,
        json_quote(&grid_string(grid)),
        json_quote(&state_sha256(&outcome.root.hasse_edges, grid)),
    )
}

fn root_network_json(
    schema: &str,
    key: &[Edge],
    network: &RootNetworkResult,
    root_key: &[Edge],
) -> String {
    let count_source = if network.external_root_replay_cache {
        "external-root-exact-replay-cache"
    } else if network.preexisting_seed_ordinal.is_some() {
        "frozen-seed-exact-replay-cache"
    } else if network.score.is_some() {
        "direct-root-neighborhood-count"
    } else {
        "unclassified-after-terminal-unique"
    };
    let observed_target_ordinals = network
        .observed_target_ordinals
        .iter()
        .copied()
        .collect::<Vec<_>>();
    format!(
        "{{\"type\":\"network\",\"schema\":{},\"network_sha256\":{},\"hasse_edges\":{},\"canonical_cells\":{},\"representative_full_saturated_edges\":{},\"representative_target\":{},\"representative_state_sha256\":{},\"representative_origin\":{},\"observed_target_ordinals\":{},\"occurrences\":{},\"preexisting_seed_ordinal\":{},\"classification_source\":{},\"score\":{},\"solver_stats\":{},\"is_declared_root\":{}}}",
        json_quote(schema),
        json_quote(&network_sha256(key)),
        edges_json(key),
        u8_list_json(&network.state.cells),
        edges_json(&network.state.full_edges),
        json_quote(&grid_string(&network.state.target)),
        json_quote(&state_sha256(key, &network.state.target)),
        root_origin_json(&network.representative_origin),
        usize_list_json(&observed_target_ordinals),
        network.occurrences,
        network
            .preexisting_seed_ordinal
            .map_or_else(|| "null".to_owned(), |ordinal| ordinal.to_string()),
        json_quote(count_source),
        score_json(network.score),
        optional_solve_stats_json(network.solver_stats),
        key == root_key,
    )
}

fn root_exact_states_json(outcome: &RootNeighborhoodOutcome) -> String {
    let mut exact = outcome
        .networks
        .iter()
        .filter_map(|(key, network)| {
            network
                .score
                .filter(|score| score.exact)
                .map(|score| (score.count, key, network))
        })
        .collect::<Vec<_>>();
    exact.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
    format!(
        "[{}]",
        exact
            .into_iter()
            .map(|(count, key, network)| {
                format!(
                    "{{\"network_sha256\":{},\"count\":{},\"is_declared_root\":{},\"preexisting_seed_ordinal\":{},\"representative_origin\":{}}}",
                    json_quote(&network_sha256(key)),
                    count,
                    key == &outcome.root.hasse_edges,
                    network.preexisting_seed_ordinal.map_or_else(
                        || "null".to_owned(),
                        |ordinal| ordinal.to_string()
                    ),
                    root_origin_json(&network.representative_origin),
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn root_summary_json(options: &Options, outcome: &RootNeighborhoodOutcome) -> String {
    let mode = options
        .root_neighborhood
        .as_ref()
        .expect("root-neighborhood summary requires root mode");
    let root_key = &outcome.root.hasse_edges;
    let generation_occurrences = outcome
        .networks
        .values()
        .map(|network| network.occurrences)
        .sum::<u64>();
    let best_exact = outcome
        .networks
        .values()
        .filter_map(|network| network.score.filter(|score| score.exact))
        .map(|score| score.count)
        .min();
    let best_new_exact = outcome
        .networks
        .values()
        .filter(|network| network.preexisting_seed_ordinal.is_none())
        .filter_map(|network| network.score.filter(|score| score.exact))
        .map(|score| score.count)
        .min();
    let improvements = outcome
        .networks
        .iter()
        .filter(|(key, network)| {
            key.as_slice() != root_key
                && network
                    .score
                    .is_some_and(|score| score.exact && score.count < outcome.root.solution_count)
        })
        .map(|(key, network)| {
            format!(
                "{{\"network_sha256\":{},\"count\":{},\"representative_origin\":{}}}",
                json_quote(&network_sha256(key)),
                network.score.expect("filtered exact score").count,
                root_origin_json(&network.representative_origin),
            )
        })
        .collect::<Vec<_>>();
    let classification_complete = outcome.accounting.unclassified_networks == 0;
    let negative_result = outcome.unique_network.is_none() && classification_complete;
    format!(
        "{{\"type\":\"summary\",\"schema\":{},\"status\":{},\"terminal_unique\":{},\"generation_complete\":true,\"classification_complete\":{},\"negative_uniqueness_result_for_declared_root_neighborhood\":{},\"root\":{{\"seed_ordinal\":{},\"network_sha256\":{},\"exact_solution_count\":{}}},\"target_enumeration\":{{\"calls\":{},\"solutions\":{},\"exhausted\":true,\"capped\":false,\"solver_stats\":{}}},\"moves\":{{\"moves_per_target\":{ROOT_MOVES_PER_TARGET},\"raw_attempts\":{},\"radius_rejections\":{},\"coverage_rejections\":{},\"accepted_observations\":{},\"distinct_canonical_networks\":{},\"duplicate_observations\":{},\"summed_network_occurrences\":{}}},\"classification\":{{\"count_cap\":{},\"frozen_seed_cache_hits\":{},\"preexisting_seed_networks_observed\":{},\"new_canonical_networks\":{},\"direct_count_calls\":{},\"exact_networks\":{},\"lower_bound_networks\":{},\"unclassified_after_terminal_unique\":{},\"best_exact_count\":{},\"best_new_exact_count\":{},\"improvements_over_root\":[{}],\"all_exact_states\":{}}},\"solver_call_partition\":{{\"seed_replay_calls\":{},\"root_enumeration_calls\":{},\"network_count_calls\":{},\"total_calls\":{}}},\"solver_totals\":{},\"unique_network_sha256\":{}}}",
        json_quote(ROOT_NEIGHBORHOOD_SCHEMA),
        json_quote(outcome.status),
        outcome.unique_network.is_some(),
        classification_complete,
        negative_result,
        outcome
            .root
            .seed_ordinal()
            .expect("v1 root summary requires frozen seed"),
        json_quote(&outcome.root.network_sha256),
        outcome.root.solution_count,
        outcome.accounting.root_enumeration_calls,
        outcome.root_solutions.len(),
        solve_stats_json(outcome.root_enumeration_stats),
        outcome.accounting.raw_move_attempts,
        outcome.accounting.radius_rejections,
        outcome.accounting.coverage_rejections,
        outcome.accounting.accepted_observations,
        outcome.accounting.distinct_observed_networks,
        outcome.accounting.duplicate_observations,
        generation_occurrences,
        mode.count_cap,
        outcome.accounting.count_cache_hits,
        outcome.accounting.preexisting_seed_networks_observed,
        outcome.accounting.new_canonical_networks,
        outcome.accounting.network_count_calls,
        outcome.accounting.exact_networks,
        outcome.accounting.lower_bound_networks,
        outcome.accounting.unclassified_networks,
        best_exact.map_or_else(|| "null".to_owned(), |count| count.to_string()),
        best_new_exact.map_or_else(|| "null".to_owned(), |count| count.to_string()),
        improvements.join(","),
        root_exact_states_json(outcome),
        outcome.accounting.seed_replay_calls,
        outcome.accounting.root_enumeration_calls,
        outcome.accounting.network_count_calls,
        outcome.solver_totals.calls,
        solver_totals_json(&outcome.solver_totals),
        outcome
            .unique_network
            .as_ref()
            .map_or_else(|| "null".to_owned(), |key| json_quote(&network_sha256(key))),
    )
}

fn external_root_summary_json(options: &Options, outcome: &RootNeighborhoodOutcome) -> String {
    let mode = options
        .root_neighborhood
        .as_ref()
        .expect("external-root summary requires root mode");
    let RootSource::ExternalRecord(provenance) = &outcome.root.source else {
        panic!("external-root summary requires external provenance");
    };
    let root_key = &outcome.root.hasse_edges;
    let generation_occurrences = outcome
        .networks
        .values()
        .map(|network| network.occurrences)
        .sum::<u64>();
    let best_exact = outcome
        .networks
        .values()
        .filter_map(|network| network.score.filter(|score| score.exact))
        .map(|score| score.count)
        .min();
    let best_non_root_exact = outcome
        .networks
        .iter()
        .filter(|(key, _)| key.as_slice() != root_key)
        .filter_map(|(_, network)| network.score.filter(|score| score.exact))
        .map(|score| score.count)
        .min();
    let improvements = outcome
        .networks
        .iter()
        .filter(|(key, network)| {
            key.as_slice() != root_key
                && network
                    .score
                    .is_some_and(|score| score.exact && score.count < outcome.root.solution_count)
        })
        .map(|(key, network)| {
            format!(
                "{{\"network_sha256\":{},\"count\":{},\"representative_origin\":{}}}",
                json_quote(&network_sha256(key)),
                network.score.expect("filtered exact score").count,
                root_origin_json(&network.representative_origin),
            )
        })
        .collect::<Vec<_>>();
    let classification_complete = outcome.accounting.unclassified_networks == 0;
    let negative_result = outcome.unique_network.is_none() && classification_complete;
    format!(
        "{{\"type\":\"summary\",\"schema\":{},\"status\":{},\"terminal_unique\":{},\"generation_complete\":true,\"classification_complete\":{},\"negative_uniqueness_result_for_declared_root_neighborhood\":{},\"root\":{{\"source\":\"external-network-record\",\"network_sha256\":{},\"exact_solution_count\":{},\"input_sha256\":{},\"input_line\":{}}},\"root_exact_replay\":{{\"calls\":{},\"cap\":{},\"count\":{},\"exact\":true,\"solver_stats\":{}}},\"target_enumeration\":{{\"calls\":{},\"solutions\":{},\"exhausted\":true,\"capped\":false,\"solver_stats\":{}}},\"moves\":{{\"moves_per_target\":{ROOT_MOVES_PER_TARGET},\"raw_attempts\":{},\"radius_rejections\":{},\"coverage_rejections\":{},\"accepted_observations\":{},\"distinct_canonical_networks\":{},\"duplicate_observations\":{},\"summed_network_occurrences\":{}}},\"classification\":{{\"count_cap\":{},\"external_root_cache_hits\":{},\"frozen_seed_cache_hits\":{},\"total_exact_cache_hits\":{},\"preexisting_frozen_seed_networks_observed\":{},\"canonical_networks_not_in_frozen_seed_archive\":{},\"direct_count_calls\":{},\"exact_networks\":{},\"lower_bound_networks\":{},\"unclassified_after_terminal_unique\":{},\"best_exact_count\":{},\"best_non_root_exact_count\":{},\"improvements_over_root\":[{}],\"all_exact_states\":{}}},\"solver_call_partition\":{{\"seed_replay_calls\":{},\"external_root_exact_replay_calls\":{},\"root_enumeration_calls\":{},\"network_count_calls\":{},\"total_calls\":{}}},\"solver_totals\":{},\"unique_network_sha256\":{}}}",
        json_quote(EXTERNAL_ROOT_NEIGHBORHOOD_SCHEMA),
        json_quote(outcome.status),
        outcome.unique_network.is_some(),
        classification_complete,
        negative_result,
        json_quote(&outcome.root.network_sha256),
        outcome.root.solution_count,
        json_quote(&provenance.sha256),
        provenance.line_number,
        outcome.accounting.root_exact_replay_calls,
        outcome.root.solution_count.saturating_add(1),
        outcome.root.solution_count,
        optional_solve_stats_json(outcome.root_exact_replay_stats),
        outcome.accounting.root_enumeration_calls,
        outcome.root_solutions.len(),
        solve_stats_json(outcome.root_enumeration_stats),
        outcome.accounting.raw_move_attempts,
        outcome.accounting.radius_rejections,
        outcome.accounting.coverage_rejections,
        outcome.accounting.accepted_observations,
        outcome.accounting.distinct_observed_networks,
        outcome.accounting.duplicate_observations,
        generation_occurrences,
        mode.count_cap,
        outcome.accounting.external_root_cache_hits,
        outcome.accounting.frozen_seed_cache_hits,
        outcome.accounting.count_cache_hits,
        outcome.accounting.preexisting_seed_networks_observed,
        outcome.accounting.new_canonical_networks,
        outcome.accounting.network_count_calls,
        outcome.accounting.exact_networks,
        outcome.accounting.lower_bound_networks,
        outcome.accounting.unclassified_networks,
        best_exact.map_or_else(|| "null".to_owned(), |count| count.to_string()),
        best_non_root_exact.map_or_else(|| "null".to_owned(), |count| count.to_string()),
        improvements.join(","),
        root_exact_states_json(outcome),
        outcome.accounting.seed_replay_calls,
        outcome.accounting.root_exact_replay_calls,
        outcome.accounting.root_enumeration_calls,
        outcome.accounting.network_count_calls,
        outcome.solver_totals.calls,
        solver_totals_json(&outcome.solver_totals),
        outcome
            .unique_network
            .as_ref()
            .map_or_else(|| "null".to_owned(), |key| json_quote(&network_sha256(key))),
    )
}

fn emit_root_neighborhood_output(
    options: &Options,
    binary: &BinaryProvenance,
    outcome: &RootNeighborhoodOutcome,
) -> Result<(), String> {
    atomic_write(&options.output, |writer| {
        let schema = root_output_schema(outcome);
        write_json_line(
            writer,
            &if outcome.root.is_external() {
                external_root_header_json(options, binary, outcome)
            } else {
                root_header_json(options, binary, outcome)
            },
        )?;
        for (index, grid) in outcome.root_solutions.iter().enumerate() {
            write_json_line(writer, &root_solution_json(outcome, index + 1, grid))?;
        }
        for (key, network) in &outcome.networks {
            write_json_line(
                writer,
                &root_network_json(schema, key, network, &outcome.root.hasse_edges),
            )?;
        }
        write_json_line(
            writer,
            &if outcome.root.is_external() {
                external_root_summary_json(options, outcome)
            } else {
                root_summary_json(options, outcome)
            },
        )?;
        Ok(())
    })
}

fn emit_output(
    options: &Options,
    binary: &BinaryProvenance,
    outcome: &RunOutcome,
) -> Result<(), String> {
    atomic_write(&options.output, |writer| {
        let continuation = options.continuation.is_some();
        write_json_line(
            writer,
            &if continuation {
                header_json_v2(options, binary, outcome)
            } else {
                header_json_v1(options, binary, outcome)
            },
        )?;
        let initial = outcome
            .initial_frontier
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let next = outcome
            .next_frontier
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        for entry in outcome.archive.values() {
            let initial_frontier = initial.contains(&entry.hasse_edges);
            let next_frontier = next.contains(&entry.hasse_edges);
            let json = if continuation {
                network_json_v2(entry, outcome, initial_frontier, next_frontier)
            } else {
                network_json_v1(entry, initial_frontier, next_frontier)
            };
            write_json_line(writer, &json)?;
        }
        write_json_line(
            writer,
            &if continuation {
                summary_json_v2(options, outcome)
            } else {
                summary_json_v1(options, outcome)
            },
        )?;
        Ok(())
    })
}

fn atomic_write<F>(destination: &Path, write: F) -> Result<(), String>
where
    F: FnOnce(&mut BufWriter<File>) -> Result<(), String>,
{
    if destination.exists() {
        return Err(format!(
            "refusing to overwrite existing output {}",
            destination.display()
        ));
    }
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    if let Some(parent) = parent {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let directory = parent.unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| format!("output {} has no file name", destination.display()))?
        .to_string_lossy();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock precedes Unix epoch: {error}"))?
        .as_nanos();
    let temporary = directory.join(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
        let mut writer = BufWriter::new(file);
        write(&mut writer)?;
        writer
            .flush()
            .map_err(|error| format!("cannot flush {}: {error}", temporary.display()))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| format!("cannot sync {}: {error}", temporary.display()))?;
        drop(writer);
        fs::hard_link(&temporary, destination).map_err(|error| {
            if destination.exists() {
                format!(
                    "refusing to overwrite existing output {}",
                    destination.display()
                )
            } else {
                format!(
                    "cannot atomically publish {} as {}: {error}",
                    temporary.display(),
                    destination.display()
                )
            }
        })?;
        fs::remove_file(&temporary)
            .map_err(|error| format!("cannot remove {}: {error}", temporary.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn write_json_line<W: Write>(writer: &mut W, json: &str) -> Result<(), String> {
    writer
        .write_all(json.as_bytes())
        .and_then(|_| writer.write_all(b"\n"))
        .map_err(|error| format!("cannot write JSONL output: {error}"))
}

fn score_json(score: Option<Score>) -> String {
    score.map_or_else(
        || "null".to_owned(),
        |score| {
            format!(
                "{{\"count\":{},\"relation\":{},\"exact\":{},\"cap\":{},\"exact_log2_band_from_128\":{}}}",
                score.count,
                json_quote(score.relation()),
                score.exact,
                score.cap,
                if score.exact {
                    exact_log2_band(score.count).to_string()
                } else {
                    "null".to_owned()
                }
            )
        },
    )
}

fn frontier_schedule_json(
    keys: &[Vec<Edge>],
    roles: &BTreeMap<Vec<Edge>, FrontierRole>,
    archive: &BTreeMap<Vec<Edge>, ArchiveEntry>,
    scores_at_selection: Option<&BTreeMap<Vec<Edge>, Score>>,
) -> String {
    format!(
        "[{}]",
        keys.iter()
            .map(|key| {
                format!(
                    "{{\"network_sha256\":{},\"hasse_edges\":{},\"role\":{},\"score\":{}}}",
                    json_quote(&network_sha256(key)),
                    edges_json(key),
                    roles
                        .get(key)
                        .map_or_else(|| "null".to_owned(), |role| json_quote(role.name())),
                    score_json(
                        scores_at_selection
                            .and_then(|scores| scores.get(key).copied())
                            .or(archive[key].score),
                    ),
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn source_strata_json(sources: Option<&BTreeSet<SourceStratum>>) -> String {
    let values = sources
        .into_iter()
        .flatten()
        .map(|source| {
            format!(
                "{{\"parent_network_sha256\":{},\"parent_hasse_edges\":{},\"target_ordinal\":{}}}",
                json_quote(&network_sha256(&source.parent_hasse_edges)),
                edges_json(&source.parent_hasse_edges),
                source.target_ordinal,
            )
        })
        .collect::<Vec<_>>();
    format!("[{}]", values.join(","))
}

fn target_audit_json(audit: Option<&TargetSelectionAudit>) -> String {
    audit.map_or_else(
        || "null".to_owned(),
        |audit| {
            format!(
                "{{\"pool\":{{\"enumerated_solutions\":{},\"exhausted\":{},\"capped\":{},\"solver_stats\":{}}},\"normalization\":{{\"parent_hasse_stabilizer\":\"D4 plus optional digit complement/global reversal that preserves the exact parent Hasse vector\",\"normalized_candidates\":{},\"unique_relevant_signatures\":{},\"historical_signatures_excluded\":{}}},\"signature\":{{\"alphabet\":\"ternary <,=,>\",\"universe\":\"sorted unordered king-adjacent cell pairs with at least one endpoint in the parent footprint\",\"pairs\":{},\"distance\":\"Hamming\",\"anchor\":\"stored representative stabilizer-normal signature when unexpanded; otherwise lex-first unexpanded signature\",\"selection\":\"farthest-first maximum minimum distance; lexicographic signature tie\"}},\"selected_witnesses\":{},\"ordered_targets\":{}}}",
                audit.enumerated_solutions,
                audit.enumeration_exhausted,
                audit.enumeration_capped,
                solve_stats_json(audit.solver_stats),
                audit.normalized_candidates,
                audit.unique_signatures,
                audit.historical_signatures_excluded,
                audit.signature_pairs,
                audit.selected_witnesses,
                grids_json(&audit.ordered_targets),
            )
        },
    )
}

fn header_json_v2(options: &Options, binary: &BinaryProvenance, outcome: &RunOutcome) -> String {
    let continuation = outcome
        .continuation
        .as_ref()
        .expect("round-two output has continuation provenance");
    let json = format!(
        "{{\"type\":\"header\",\"schema\":{},\"algorithm_revision\":{},\"mode\":\"strict-round1-continuation\",\"round_number\":2,\"scope\":{{\"global_18c_exhaustive\":false,\"negative_result_proof\":false,\"complete_only_for_declared_parent_target_radius_one_round\":true}},\"seed_input\":{{\"path\":{},\"bytes\":{SEED_ARTIFACT_BYTES},\"sha256\":{}}},\"continuation_input\":{{\"path\":{},\"bytes\":{},\"whole_file_sha256\":{},\"records_2_through_eof_sha256\":{},\"required_records_sha256\":{},\"matches_historical_whole_file\":{}}},\"binary\":{{\"path\":{},\"bytes\":{},\"sha256\":{}}},\"identity\":\"exact canonical Hasse edge vector; SHA-256 is an identifier only\",\"configuration\":{{\"W\":{BEAM_WIDTH},\"initial_lanes\":{{\"exact_generated\":8,\"pending_seed_anchors\":{ROUND2_SEED_ANCHOR_SLOTS},\"censored_structural\":4}},\"initial_selection_method\":\"exact generated: raw exact count within new exact-parent/new footprint/new topology passes, then relaxed passes, max two per resolved parent Hasse; pending seeds: raw exact count within footprint/topology passes; censored: parent/footprint/topology novelty then Hasse density and exact-key tie\",\"target_witnesses_per_parent\":{ROUND2_TARGETS_PER_NETWORK},\"target_pool_cap_for_censored\":{ROUND2_TARGET_POOL_CAP},\"normal_cap\":{NORMAL_CAP},\"high_cap_probes_requested\":{},\"high_cap\":{},\"probe_selection\":\"round-robin over exact-key current parent-Hasse-vector and target-ordinal source strata; exact Hasse tie; deterministic fill\",\"raw_move_hard_max\":{ROUND2_RAW_MOVE_HARD_MAX},\"max_new_network_counts\":{},\"max_current_solver_calls\":{}}},\"moves\":[\"same-footprint target resaturation\",\"one-cell replacement whose added cell is king-adjacent to a retained cell\"],\"target_selection\":{{\"normalization\":\"lex-min over exact parent-Hasse stabilizer under D4/complement\",\"signature_universe\":\"sorted unordered king-adjacent pairs with at least one endpoint in parent footprint\",\"signature_alphabet\":\"ternary <,=,>\",\"anchor\":\"stored representative stabilizer-normal signature when unexpanded; otherwise lex-first unexpanded signature\",\"selection\":\"eight farthest-first by maximum minimum Hamming distance; equal historical signatures excluded\"}},\"initial_frontier\":{},\"successor_policy\":{{\"exact_exploit\":8,\"exact_local_factor_bands\":[\"<=4x (includes improvements and equality)\",\"4x..8x\",\"8x..16x\",\">16x\"],\"censored_hot_novelty\":4,\"selection_parent_assignment\":\"lowest-count exact current source parent Hasse; otherwise deterministic current censored parent Hasse; otherwise resolved historical parent Hasse\",\"method\":\"exact exploit: raw count within new-footprint then relaxed passes, max two per selected parent; barrier: lowest exact balanced member from each populated local factor band then raw-count fallback; hot: selected-parent/footprint/topology novelty then Hasse density and exact-key tie\",\"factor_basis\":\"local exact child divided by lowest-count exact current generating parent; no full-lineage peak optimization\",\"censored_not_numeric\":true}}}}",
        json_quote(CONTINUATION_SCHEMA),
        json_quote(CONTINUATION_ALGORITHM_REVISION),
        json_quote(&options.seeds.to_string_lossy()),
        json_quote(SEED_ARTIFACT_SHA256),
        json_quote(&continuation.path.to_string_lossy()),
        continuation.bytes,
        json_quote(&continuation.sha256),
        json_quote(&continuation.records_sha256),
        json_quote(ROUND1_RECORDS_SHA256),
        continuation.historical_whole_file_match,
        json_quote(&binary.path.to_string_lossy()),
        binary.bytes,
        json_quote(&binary.sha256),
        options.exploration_probes,
        options.exploration_cap,
        options.max_new_counts,
        options.max_total_solver_calls,
        frontier_schedule_json(
            &outcome.initial_frontier,
            &outcome.frontier_roles,
            &outcome.archive,
            Some(&outcome.frontier_scores_at_selection),
        ),
    );
    json.replacen(
        "\"round_number\":2,",
        "\"round_number\":2,\"v2_restart_implemented\":false,",
        1,
    )
}

fn network_json_v2(
    entry: &ArchiveEntry,
    outcome: &RunOutcome,
    initial_frontier: bool,
    next_frontier: bool,
) -> String {
    let expanded_targets = entry.expanded_targets.iter().copied().collect::<Vec<_>>();
    format!(
        "{{\"type\":\"network\",\"schema\":{},\"network_sha256\":{},\"hasse_edges\":{},\"canonical_cells\":{},\"representative_full_saturated_edges\":{},\"representative_target\":{},\"witness_reservoir_sorted\":{},\"expanded_targets_cumulative\":{},\"score\":{},\"historical_representative_origin\":{},\"current_observation_sources\":{},\"occurrences_cumulative\":{},\"expanded_in_current_round\":{},\"high_cap_probed_ever\":{},\"normal_solver_stats_current_or_creation\":{},\"probe_solver_stats_current_or_creation\":{},\"initial_frontier\":{},\"initial_frontier_role\":{},\"score_at_initial_selection\":{},\"target_selection_audit\":{},\"next_frontier\":{},\"next_frontier_role\":{}}}",
        json_quote(CONTINUATION_SCHEMA),
        json_quote(&network_sha256(&entry.hasse_edges)),
        edges_json(&entry.hasse_edges),
        u8_list_json(&entry.cells),
        edges_json(&entry.representative_full_edges),
        json_quote(&grid_string(&entry.representative_target)),
        grids_json(&entry.targets.targets),
        grids_json(&expanded_targets),
        score_json(entry.score),
        origin_json(&entry.origin),
        source_strata_json(outcome.current_sources.get(&entry.hasse_edges)),
        entry.occurrences,
        entry.expanded_in_round,
        entry.high_cap_probed,
        optional_solve_stats_json(entry.normal_stats),
        optional_solve_stats_json(entry.probe_stats),
        initial_frontier,
        outcome
            .frontier_roles
            .get(&entry.hasse_edges)
            .map_or_else(|| "null".to_owned(), |role| json_quote(role.name()),),
        score_json(
            outcome
                .frontier_scores_at_selection
                .get(&entry.hasse_edges)
                .copied(),
        ),
        target_audit_json(outcome.target_audits.get(&entry.hasse_edges)),
        next_frontier,
        outcome
            .next_frontier_roles
            .get(&entry.hasse_edges)
            .map_or_else(|| "null".to_owned(), |role| json_quote(role.name()),),
    )
}

fn summary_json_v2(options: &Options, outcome: &RunOutcome) -> String {
    let exact_scores = outcome
        .archive
        .values()
        .filter_map(|entry| entry.score.filter(|score| score.exact))
        .collect::<Vec<_>>();
    let lower_bounds = outcome
        .archive
        .values()
        .filter(|entry| entry.score.is_some_and(|score| !score.exact))
        .count();
    let unclassified = outcome
        .archive
        .values()
        .filter(|entry| entry.score.is_none())
        .count();
    let best = exact_scores.iter().map(|score| score.count).min();
    let best_reached_current = outcome
        .current_sources
        .keys()
        .filter_map(|key| outcome.archive[key].score.filter(|score| score.exact))
        .map(|score| score.count)
        .min();
    format!(
        "{{\"type\":\"summary\",\"schema\":{},\"status\":{},\"round_complete\":{},\"terminal_unique\":{},\"scope_non_exhaustive\":true,\"ceilings\":{{\"raw_hit\":{},\"new_count_hit\":{},\"total_call_hit\":{},\"configured_max_new_counts\":{},\"configured_max_current_solver_calls\":{}}},\"seed_schedule\":{{\"archive\":{},\"frontier_anchors\":{},\"pending_after_round2_selection\":{},\"pending_ordinals\":{}}},\"target_pools\":{{\"calls\":{},\"solutions_returned\":{},\"exact_upgrades_from_censored\":{},\"selected_witnesses\":{},\"historical_signatures_skipped_at_expansion\":{}}},\"moves\":{{\"raw_attempts\":{},\"radius_rejections\":{},\"coverage_rejections\":{},\"accepted_observations\":{},\"visited_observations\":{},\"duplicate_new_observations\":{},\"new_canonical_networks\":{}}},\"classification\":{{\"normal_calls\":{},\"normal_exact\":{},\"normal_lower_bounds\":{},\"high_cap_probes_requested\":{},\"high_cap_probes_eligible\":{},\"high_cap_probe_cap\":{},\"high_cap_probe_calls\":{},\"high_cap_exact\":{},\"high_cap_lower_bounds\":{},\"cache_hits\":{},\"cache_entries\":{},\"archive_exact\":{},\"archive_lower_bounds\":{},\"archive_unclassified\":{},\"incumbent_before\":128,\"best_exact_after\":{},\"best_exact_reached_current_round\":{},\"best_exact_new_canonical_network\":{}}},\"current_solver_totals\":{},\"initial_frontier\":{},\"next_frontier\":{},\"unique_network_sha256\":{}}}",
        json_quote(CONTINUATION_SCHEMA),
        json_quote(outcome.status),
        outcome.accounting.round_complete(),
        outcome.accounting.unique_found,
        outcome.accounting.raw_ceiling_hit,
        outcome.accounting.new_count_ceiling_hit,
        outcome.accounting.total_call_ceiling_hit,
        options.max_new_counts,
        options.max_total_solver_calls,
        outcome.accounting.seed_archive,
        outcome.accounting.seed_frontier,
        outcome.accounting.seed_pending,
        usize_list_json(&outcome.pending_seed_ordinals),
        outcome.accounting.target_pool_calls,
        outcome.accounting.target_pool_solutions,
        outcome.accounting.target_pool_exact_upgrades,
        outcome.accounting.target_witnesses_selected,
        outcome.accounting.previously_expanded_targets_skipped,
        outcome.accounting.raw_move_attempts,
        outcome.accounting.radius_rejections,
        outcome.accounting.coverage_rejections,
        outcome.accounting.accepted_observations,
        outcome.accounting.visited_observations,
        outcome.accounting.duplicate_new_observations,
        outcome.accounting.new_canonical_networks,
        outcome.accounting.normal_count_calls,
        outcome.accounting.normal_exact,
        outcome.accounting.normal_lower_bounds,
        outcome.accounting.high_cap_probes_requested,
        outcome.accounting.high_cap_probes_eligible,
        outcome.accounting.high_cap_probe_cap,
        outcome.accounting.high_cap_probe_calls,
        outcome.accounting.high_cap_exact,
        outcome.accounting.high_cap_lower_bounds,
        outcome.accounting.count_cache_hits,
        outcome.count_cache.len(),
        exact_scores.len(),
        lower_bounds,
        unclassified,
        best.map_or_else(|| "null".to_owned(), |count| count.to_string()),
        best_reached_current.map_or_else(|| "null".to_owned(), |count| count.to_string()),
        outcome
            .accounting
            .best_new_exact_count
            .map_or_else(|| "null".to_owned(), |count| count.to_string()),
        solver_totals_json(&outcome.solver_totals),
        frontier_schedule_json(
            &outcome.initial_frontier,
            &outcome.frontier_roles,
            &outcome.archive,
            Some(&outcome.frontier_scores_at_selection),
        ),
        frontier_schedule_json(
            &outcome.next_frontier,
            &outcome.next_frontier_roles,
            &outcome.archive,
            None,
        ),
        outcome
            .unique_network
            .as_ref()
            .map_or_else(|| "null".to_owned(), |key| json_quote(&network_sha256(key)),),
    )
}

fn header_json_v1(options: &Options, binary: &BinaryProvenance, outcome: &RunOutcome) -> String {
    let initial_ordinals = outcome
        .initial_frontier
        .iter()
        .map(|key| match &outcome.archive[key].origin {
            Origin::Seed { ordinal } => *ordinal,
            Origin::Generated { .. } => unreachable!("round-zero frontier consists of seeds"),
        })
        .collect::<Vec<_>>();
    format!(
        "{{\"type\":\"header\",\"schema\":{},\"algorithm_revision\":{},\"scope\":{{\"global_18c_exhaustive\":false,\"negative_result_proof\":false,\"complete_only_for_declared_round\":true}},\"rounds\":{ROUNDS},\"seed_input\":{{\"path\":{},\"bytes\":{SEED_ARTIFACT_BYTES},\"sha256\":{}}},\"binary\":{{\"path\":{},\"bytes\":{},\"sha256\":{}}},\"model\":{{\"covered_cells\":18,\"constraints\":\"directed unequal king-neighbour comparisons\",\"overlap_and_branching\":true,\"coverage_rule\":\"all 18 cells incident after target saturation\"}},\"configuration\":{{\"W\":{BEAM_WIDTH},\"Q\":{EXPLORATION_FRONTIER_SLOTS},\"Q_semantics\":\"exploration_frontier_slots\",\"exploration_frontier_slots\":{EXPLORATION_FRONTIER_SLOTS},\"exploit_frontier_slots\":{},\"target_witnesses_per_network_max\":{ROUND1_TARGETS_PER_NETWORK},\"target_reservoir_cap\":{ROUND1_TARGETS_PER_NETWORK},\"normal_cap\":{NORMAL_CAP},\"exploration_probes_requested\":{},\"exploration_probe_buckets\":4,\"exploration_cap\":{},\"exploration_cap_hard_max\":{MAX_EXPLORATION_CAP},\"raw_move_hard_max\":{RAW_MOVE_HARD_MAX},\"max_new_network_counts\":{},\"max_total_solver_calls\":{}}},\"moves\":[\"same-footprint target resaturation\",\"one-cell replacement whose added cell is king-adjacent to a retained cell\"],\"identity\":\"exact canonical Hasse edge vector; SHA-256 is an identifier only\",\"initial_selection\":{{\"method\":\"8 global lowest; 4 lowest Hasse-16 best-per-new-footprint; 4 lowest remaining best-per-new-footprint; ties by exact Hasse vector\",\"ordinals\":{}}},\"probe_selection\":\"four SHA buckets; normal-search nodes descending then Hasse key; balanced quota with deterministic fill; all eligible keys when requested probes reach the eligible count\",\"successor_selection\":\"12 lowest exact unexpanded-target states plus one best high-cap-probed unexpanded-target state per nonempty SHA bucket; exact probe counts first, otherwise probe nodes descending; deterministic fill\",\"target_expansion\":\"representative target plus up to three lexicographic witnesses; all available nonempty targets are expanded\",\"seed_schedule\":{{\"archive\":{},\"frontier\":{},\"pending\":{},\"pending_ordinals\":{}}}}}",
        json_quote(SCHEMA),
        json_quote(ALGORITHM_REVISION),
        json_quote(&options.seeds.to_string_lossy()),
        json_quote(SEED_ARTIFACT_SHA256),
        json_quote(&binary.path.to_string_lossy()),
        binary.bytes,
        json_quote(&binary.sha256),
        BEAM_WIDTH - EXPLORATION_FRONTIER_SLOTS,
        options.exploration_probes,
        options.exploration_cap,
        options.max_new_counts,
        options.max_total_solver_calls,
        usize_list_json(&initial_ordinals),
        outcome.accounting.seed_archive,
        outcome.accounting.seed_frontier,
        outcome.accounting.seed_pending,
        usize_list_json(&outcome.pending_seed_ordinals),
    )
}

fn network_json_v1(entry: &ArchiveEntry, initial_frontier: bool, next_frontier: bool) -> String {
    let score = entry.score.map_or_else(
        || "null".to_owned(),
        |score| {
            format!(
                "{{\"count\":{},\"relation\":{},\"exact\":{},\"cap\":{}}}",
                score.count,
                json_quote(score.relation()),
                score.exact,
                score.cap
            )
        },
    );
    let expanded_targets = entry.expanded_targets.iter().copied().collect::<Vec<_>>();
    let unexpanded_target_count = entry
        .expansion_targets(ROUND1_TARGETS_PER_NETWORK)
        .iter()
        .filter(|target| !entry.expanded_targets.contains(*target))
        .count();
    format!(
        "{{\"type\":\"network\",\"schema\":{},\"network_sha256\":{},\"hasse_edges\":{},\"canonical_cells\":{},\"representative_full_saturated_edges\":{},\"representative_target\":{},\"target_reservoir\":{},\"expanded_targets\":{},\"unexpanded_target_count\":{},\"score\":{},\"origin\":{},\"occurrences_in_source_stage\":{},\"expanded_in_round\":{},\"high_cap_probed\":{},\"normal_solver_stats\":{},\"probe_solver_stats\":{},\"initial_frontier\":{},\"next_frontier\":{}}}",
        json_quote(SCHEMA),
        json_quote(&network_sha256(&entry.hasse_edges)),
        edges_json(&entry.hasse_edges),
        u8_list_json(&entry.cells),
        edges_json(&entry.representative_full_edges),
        json_quote(&grid_string(&entry.representative_target)),
        grids_json(&entry.targets.targets),
        grids_json(&expanded_targets),
        unexpanded_target_count,
        score,
        origin_json(&entry.origin),
        entry.occurrences,
        entry.expanded_in_round,
        entry.high_cap_probed,
        optional_solve_stats_json(entry.normal_stats),
        optional_solve_stats_json(entry.probe_stats),
        initial_frontier,
        next_frontier,
    )
}

fn summary_json_v1(options: &Options, outcome: &RunOutcome) -> String {
    let exact_scores = outcome
        .archive
        .values()
        .filter_map(|entry| entry.score.filter(|score| score.exact))
        .collect::<Vec<_>>();
    let lower_bounds = outcome
        .archive
        .values()
        .filter_map(|entry| entry.score.filter(|score| !score.exact))
        .count();
    let unclassified = outcome
        .archive
        .values()
        .filter(|entry| entry.score.is_none())
        .count();
    let best = exact_scores.iter().map(|score| score.count).min();
    format!(
        "{{\"type\":\"summary\",\"schema\":{},\"status\":{},\"round_complete\":{},\"terminal_unique\":{},\"ceilings\":{{\"raw_hit\":{},\"new_count_hit\":{},\"total_call_hit\":{},\"configured_max_new_counts\":{},\"configured_max_total_calls\":{}}},\"seed_schedule\":{{\"archive\":{},\"frontier\":{},\"pending\":{},\"replay_calls\":{}}},\"moves\":{{\"raw_attempts\":{},\"radius_rejections\":{},\"coverage_rejections\":{},\"accepted_observations\":{},\"visited_observations\":{},\"duplicate_new_observations\":{},\"new_canonical_networks\":{}}},\"classification\":{{\"normal_calls\":{},\"normal_exact\":{},\"normal_lower_bounds\":{},\"high_cap_probes_requested\":{},\"high_cap_probes_eligible\":{},\"high_cap_probe_cap\":{},\"high_cap_probe_calls\":{},\"high_cap_exact\":{},\"high_cap_lower_bounds\":{},\"cache_hits\":{},\"cache_entries\":{},\"archive_exact\":{},\"archive_lower_bounds\":{},\"archive_unclassified\":{},\"best_exact_count\":{}}},\"solver_totals\":{},\"next_frontier_network_sha256\":{},\"unique_network_sha256\":{}}}",
        json_quote(SCHEMA),
        json_quote(outcome.status),
        outcome.accounting.round_complete(),
        outcome.accounting.unique_found,
        outcome.accounting.raw_ceiling_hit,
        outcome.accounting.new_count_ceiling_hit,
        outcome.accounting.total_call_ceiling_hit,
        options.max_new_counts,
        options.max_total_solver_calls,
        outcome.accounting.seed_archive,
        outcome.accounting.seed_frontier,
        outcome.accounting.seed_pending,
        outcome.accounting.seed_replay_calls,
        outcome.accounting.raw_move_attempts,
        outcome.accounting.radius_rejections,
        outcome.accounting.coverage_rejections,
        outcome.accounting.accepted_observations,
        outcome.accounting.visited_observations,
        outcome.accounting.duplicate_new_observations,
        outcome.accounting.new_canonical_networks,
        outcome.accounting.normal_count_calls,
        outcome.accounting.normal_exact,
        outcome.accounting.normal_lower_bounds,
        outcome.accounting.high_cap_probes_requested,
        outcome.accounting.high_cap_probes_eligible,
        outcome.accounting.high_cap_probe_cap,
        outcome.accounting.high_cap_probe_calls,
        outcome.accounting.high_cap_exact,
        outcome.accounting.high_cap_lower_bounds,
        outcome.accounting.count_cache_hits,
        outcome.count_cache.len(),
        exact_scores.len(),
        lower_bounds,
        unclassified,
        best.map_or_else(|| "null".to_owned(), |count| count.to_string()),
        solver_totals_json(&outcome.solver_totals),
        string_list_json(
            &outcome
                .next_frontier
                .iter()
                .map(|key| network_sha256(key))
                .collect::<Vec<_>>()
        ),
        outcome
            .unique_network
            .as_ref()
            .map_or_else(|| "null".to_owned(), |key| json_quote(&network_sha256(key))),
    )
}

fn origin_json(origin: &Origin) -> String {
    match origin {
        Origin::Seed { ordinal } => format!("{{\"kind\":\"seed\",\"ordinal\":{ordinal}}}"),
        Origin::Generated {
            parent_network_sha256,
            target_ordinal,
            move_kind,
        } => format!(
            "{{\"kind\":\"generated\",\"parent_network_sha256\":{},\"target_ordinal\":{},\"move\":{}}}",
            json_quote(parent_network_sha256),
            target_ordinal,
            move_json(*move_kind)
        ),
    }
}

fn move_json(move_kind: MoveKind) -> String {
    match move_kind {
        MoveKind::Retarget => "{\"kind\":\"retarget\"}".to_owned(),
        MoveKind::Swap { removed, added } => {
            format!("{{\"kind\":\"swap\",\"removed\":{removed},\"added\":{added}}}")
        }
    }
}

fn solver_totals_json(value: &SolverTotals) -> String {
    format!(
        "{{\"calls\":{},\"nodes\":{},\"branches\":{},\"propagation_rounds\":{},\"comparison_revisions\":{},\"max_depth\":{}}}",
        value.calls,
        value.nodes,
        value.branches,
        value.propagation_rounds,
        value.comparison_revisions,
        value.max_depth
    )
}

fn optional_solve_stats_json(value: Option<SolveStats>) -> String {
    value.map_or_else(|| "null".to_owned(), solve_stats_json)
}

fn solve_stats_json(value: SolveStats) -> String {
    format!(
        "{{\"nodes\":{},\"branches\":{},\"propagation_rounds\":{},\"comparison_revisions\":{},\"max_depth\":{}}}",
        value.nodes,
        value.branches,
        value.propagation_rounds,
        value.thermo_revisions,
        value.max_depth
    )
}

fn edges_json(edges: &[Edge]) -> String {
    let mut result = String::from("[");
    for (index, &(lower, upper)) in edges.iter().enumerate() {
        if index != 0 {
            result.push(',');
        }
        result.push_str(&format!("[{lower},{upper}]"));
    }
    result.push(']');
    result
}

fn grids_json(grids: &[Grid]) -> String {
    string_list_json(&grids.iter().map(grid_string).collect::<Vec<_>>())
}

fn grid_string(grid: &Grid) -> String {
    grid.iter().map(|digit| char::from(b'0' + digit)).collect()
}

fn u8_list_json(values: &[u8]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn usize_list_json(values: &[usize]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn string_list_json(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| json_quote(value))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn json_quote(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 2);
    result.push('\"');
    for character in value.chars() {
        match character {
            '\"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{08}' => result.push_str("\\b"),
            '\u{0c}' => result.push_str("\\f"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            character if character <= '\u{1f}' => {
                result.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => result.push(character),
        }
    }
    result.push('\"');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const FROZEN_SEEDS: &[u8] = include_bytes!("../../../analysis/18c-seeds-v1.jsonl");

    fn frozen() -> Vec<SeedInput> {
        load_seed_artifact(FROZEN_SEEDS).unwrap()
    }

    fn dummy_grid(digit: u8) -> Grid {
        [digit; CELLS]
    }

    fn dummy_entry(key: Vec<Edge>, score: Score) -> ArchiveEntry {
        ArchiveEntry {
            hasse_edges: key,
            representative_full_edges: Vec::new(),
            representative_target: dummy_grid(9),
            cells: Vec::new(),
            targets: TargetReservoir::new(),
            score: Some(score),
            origin: Origin::Seed { ordinal: 1 },
            occurrences: 1,
            expanded_in_round: false,
            expanded_targets: BTreeSet::new(),
            high_cap_probed: false,
            normal_stats: None,
            probe_stats: None,
        }
    }

    fn fixture_external_provenance() -> ExternalRootProvenance {
        ExternalRootProvenance {
            path: PathBuf::from("fixture-round2.jsonl"),
            bytes: 0,
            sha256: "11".repeat(32),
            line_number: 0,
            schema: CONTINUATION_SCHEMA.to_owned(),
            algorithm_revision: CONTINUATION_ALGORITHM_REVISION.to_owned(),
            authentication: ExternalRootArtifactKind::PinnedRound2Landscape
                .authentication()
                .to_owned(),
        }
    }

    fn fixture_root_neighborhood_provenance(bytes: &[u8]) -> ExternalRootProvenance {
        ExternalRootProvenance {
            path: PathBuf::from("fixture-root-neighborhood.jsonl"),
            bytes: bytes.len(),
            sha256: sha256_hex(bytes),
            line_number: 0,
            schema: ROOT_NEIGHBORHOOD_SCHEMA.to_owned(),
            algorithm_revision: ROOT_NEIGHBORHOOD_ALGORITHM_REVISION.to_owned(),
            authentication: ExternalRootArtifactKind::CompletedRootNeighborhoodV1
                .authentication()
                .to_owned(),
        }
    }

    fn fixture_external_network_json(
        root: &ExactRoot,
        score: Score,
        full_edges: &[Edge],
    ) -> String {
        format!(
            "{{\"type\":\"network\",\"schema\":{},\"network_sha256\":{},\"hasse_edges\":{},\"canonical_cells\":{},\"representative_full_saturated_edges\":{},\"representative_target\":{},\"score\":{}}}",
            json_quote(CONTINUATION_SCHEMA),
            json_quote(&root.network_sha256),
            edges_json(&root.hasse_edges),
            u8_list_json(&root.cells),
            edges_json(full_edges),
            json_quote(&grid_string(&root.target)),
            score_json(Some(score)),
        )
    }

    fn fixture_external_artifact(network_records: &[String]) -> Vec<u8> {
        let mut lines = vec![format!(
            "{{\"type\":\"header\",\"schema\":{},\"algorithm_revision\":{}}}",
            json_quote(CONTINUATION_SCHEMA),
            json_quote(CONTINUATION_ALGORITHM_REVISION),
        )];
        lines.extend(network_records.iter().cloned());
        lines.push(format!(
            "{{\"type\":\"summary\",\"schema\":{},\"status\":\"round-complete\",\"round_complete\":true,\"terminal_unique\":false}}",
            json_quote(CONTINUATION_SCHEMA)
        ));
        format!("{}\n", lines.join("\n")).into_bytes()
    }

    fn fixture_root_neighborhood_artifact(
        root: &ExactRoot,
        network_records: &[String],
        classification_complete: bool,
    ) -> Vec<u8> {
        let mut lines = vec![format!(
            "{{\"type\":\"header\",\"schema\":{},\"algorithm_revision\":{},\"scope\":{{\"complete_root_solution_set\":true,\"complete_saturated_radius_one_generation_for_declared_root\":true,\"classification_complete\":true}},\"configuration\":{{\"target_count\":{}}},\"root\":{{\"network_sha256\":{},\"exact_solution_count\":{}}}}}",
            json_quote(ROOT_NEIGHBORHOOD_SCHEMA),
            json_quote(ROOT_NEIGHBORHOOD_ALGORITHM_REVISION),
            root.solution_count,
            json_quote(&root.network_sha256),
            root.solution_count,
        )];
        let mut solutions = Solver::blank_comparisons(&root.hasse_edges)
            .unwrap()
            .enumerate_up_to(root.solution_count as usize)
            .solutions;
        solutions.sort_unstable();
        solutions.dedup();
        assert_eq!(solutions.len(), root.solution_count as usize);
        for (index, solution) in solutions.iter().enumerate() {
            lines.push(format!(
                "{{\"type\":\"root_solution\",\"schema\":{},\"root_network_sha256\":{},\"ordinal\":{},\"grid\":{}}}",
                json_quote(ROOT_NEIGHBORHOOD_SCHEMA),
                json_quote(&root.network_sha256),
                index + 1,
                json_quote(&grid_string(solution)),
            ));
        }
        lines.extend(network_records.iter().cloned());
        lines.push(format!(
            "{{\"type\":\"summary\",\"schema\":{},\"status\":\"root-neighborhood-complete\",\"terminal_unique\":false,\"generation_complete\":true,\"classification_complete\":{classification_complete},\"root\":{{\"network_sha256\":{},\"exact_solution_count\":{}}},\"target_enumeration\":{{\"calls\":1,\"solutions\":{},\"exhausted\":true,\"capped\":false}},\"classification\":{{\"unclassified_after_terminal_unique\":0,\"all_exact_states\":[{{\"network_sha256\":{},\"count\":{}}}]}}}}",
            json_quote(ROOT_NEIGHBORHOOD_SCHEMA),
            json_quote(&root.network_sha256),
            root.solution_count,
            root.solution_count,
            json_quote(&root.network_sha256),
            root.solution_count,
        ));
        format!("{}\n", lines.join("\n")).into_bytes()
    }

    fn fixture_root_neighborhood_network_json(root: &ExactRoot, score: Score) -> String {
        format!(
            "{{\"type\":\"network\",\"schema\":{},\"network_sha256\":{},\"hasse_edges\":{},\"canonical_cells\":{},\"representative_full_saturated_edges\":{},\"representative_target\":{},\"representative_state_sha256\":{},\"score\":{}}}",
            json_quote(ROOT_NEIGHBORHOOD_SCHEMA),
            json_quote(&root.network_sha256),
            edges_json(&root.hasse_edges),
            u8_list_json(&root.cells),
            edges_json(&root.full_edges),
            json_quote(&grid_string(&root.target)),
            json_quote(&root.state_sha256),
            score_json(Some(score)),
        )
    }

    #[test]
    fn frozen_artifact_and_diversity_frontier_are_golden() {
        let seeds = frozen();
        assert_eq!(seeds.len(), SEED_COUNT);
        let (frontier, pending) = select_seed_frontier(&seeds).unwrap();
        let ordinals = frontier
            .iter()
            .map(|&index| seeds[index].ordinal)
            .collect::<Vec<_>>();
        let counts = frontier
            .iter()
            .map(|&index| seeds[index].solution_count)
            .collect::<Vec<_>>();
        assert_eq!(
            ordinals,
            [42, 28, 6, 36, 34, 40, 26, 59, 20, 18, 63, 23, 25, 32, 30, 7]
        );
        assert_eq!(
            counts,
            [
                128, 158, 171, 180, 205, 205, 215, 215, 414, 513, 518, 555, 327, 342, 372, 409
            ]
        );
        let hashes = frontier
            .iter()
            .map(|&index| seeds[index].network_sha256.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            hashes,
            [
                "000686520eb98f01cfee9ef0be013e1d3758bbb258add2b09866094ae31fd7ae",
                "7e76acca9cb0b90182c8b7fc6ac85787d8e7aee9b31cd8b167e05f5b83ebc7c4",
                "dc32e4816bdeeaa663dd06b9cefafa658f50e3b2b94ef780cc33ec0080825edc",
                "d73abe31eca6c5dd904f87012bce9f280d3caf75e3bccd36aae5cc92f8d8ad79",
                "5bf44594e30ce89a5a9b185d09bfb3a16a85626da3de3ff662f25c706ba991a8",
                "8ba0d07318de5fe6e0a3471f55fdd63176cb4d93c7783b9791736165d8202819",
                "136df2652e31dff8174bb79376905b1cc8965d75237d8b68b53c475acda85baa",
                "a278be82b9b10ff9c70b33048bfb18b0491447f09401b064ff31c7ba78879478",
                "e3e3aed9d4e776df28f0ae8404a047f3d59d08528a1713eb4ceb186fd24285b3",
                "80f8cf3ff10af475b52b5ea749b20e4ba2630b0cb305433caf8c482351916aba",
                "292eeb991cae088115f5fe8ad2f647ba7f9340a599515f24dc8e0464ff1a3de5",
                "e8efa9df92f88c74a81bab4962bcc9aae5bbe66a7263bc1fd120b3b2d55bf6e4",
                "61e9d5f8f0110bdfa82eb142c720f2fc06d06ab141ae5ea8840a6592ff846a5e",
                "d9b7d93be9855f4ddb6e5c1e281eeedceb1af6e95f9ce010ce2341ceb1e0fd89",
                "beb8e531063754794854b037192d5bebb5eaa36760ee4101db4ec7b00a03d65f",
                "f02a751125aa70dc73f98e208ccb28404c1b19f06ed3a656158fa1694e0827b1",
            ]
        );
        assert_eq!(
            frontier
                .iter()
                .map(|&index| seeds[index].cells.clone())
                .collect::<BTreeSet<_>>()
                .len(),
            12
        );
        assert_eq!(pending.len(), 55);
    }

    #[test]
    fn strict_artifact_loader_rejects_byte_corruption_and_duplicate_json_keys() {
        let mut corrupted = FROZEN_SEEDS.to_vec();
        corrupted[100] ^= 1;
        assert!(
            load_seed_artifact(&corrupted)
                .unwrap_err()
                .contains("SHA-256")
        );
        assert!(
            JsonParser::new(br#"{"type":"seed","type":"summary"}"#)
                .parse()
                .unwrap_err()
                .contains("duplicate JSON field")
        );
    }

    #[test]
    fn root_seed_42_selectors_and_scope_are_golden() {
        let seeds = frozen();
        let root = select_root_seed(&seeds, Some(42), None).unwrap();
        assert_eq!(root.seed_ordinal(), Some(42));
        assert_eq!(root.solution_count, 128);
        assert_eq!(
            root.network_sha256,
            "000686520eb98f01cfee9ef0be013e1d3758bbb258add2b09866094ae31fd7ae"
        );
        assert_eq!(
            select_root_seed(&seeds, None, Some(&root.network_sha256)).unwrap(),
            root
        );
        assert_eq!(select_root_seed(&seeds, None, None).unwrap(), root);
        assert!(
            select_root_seed(&seeds, Some(42), Some(&seeds[0].network_sha256))
                .unwrap_err()
                .contains("selectors disagree")
        );
        assert_eq!(ROOT_MOVES_PER_TARGET, 1_135);
        assert_eq!(
            root.solution_count * ROOT_MOVES_PER_TARGET,
            ROUND2_RAW_MOVE_HARD_MAX
        );
        assert_ne!(ROOT_NEIGHBORHOOD_SCHEMA, SCHEMA);
        assert_ne!(ROOT_NEIGHBORHOOD_SCHEMA, CONTINUATION_SCHEMA);
        assert_ne!(ROOT_NEIGHBORHOOD_ALGORITHM_REVISION, ALGORITHM_REVISION);
        assert_ne!(
            ROOT_NEIGHBORHOOD_ALGORITHM_REVISION,
            CONTINUATION_ALGORITHM_REVISION
        );
    }

    #[test]
    fn external_root_record_parser_is_strict_about_identity_and_exactness() {
        let root = select_root_seed(&frozen(), Some(42), None).unwrap();
        let exact = fixture_external_network_json(
            &root,
            Score::exact(root.solution_count, SEED_SOLUTION_CAP),
            &root.full_edges,
        );
        let artifact = fixture_external_artifact(std::slice::from_ref(&exact));
        let parsed = extract_external_root_record(
            BufReader::new(Cursor::new(artifact)),
            fixture_external_provenance(),
            &root.network_sha256,
            ExternalRootArtifactKind::PinnedRound2Landscape,
        )
        .unwrap();
        assert_eq!(parsed.network_sha256, root.network_sha256);
        assert_eq!(parsed.solution_count, root.solution_count);
        assert_eq!(parsed.hasse_edges, root.hasse_edges);
        assert!(parsed.is_external());
        let RootSource::ExternalRecord(provenance) = parsed.source else {
            panic!("fixture root must be external");
        };
        assert_eq!(provenance.line_number, 2);

        let duplicate = fixture_external_artifact(&[exact.clone(), exact.clone()]);
        assert!(
            extract_external_root_record(
                BufReader::new(Cursor::new(duplicate)),
                fixture_external_provenance(),
                &root.network_sha256,
                ExternalRootArtifactKind::PinnedRound2Landscape,
            )
            .unwrap_err()
            .contains("repeats network")
        );
        let missing = fixture_external_artifact(std::slice::from_ref(&exact));
        assert!(
            extract_external_root_record(
                BufReader::new(Cursor::new(missing)),
                fixture_external_provenance(),
                &"ff".repeat(32),
                ExternalRootArtifactKind::PinnedRound2Landscape,
            )
            .unwrap_err()
            .contains("lacks network")
        );

        let inexact = fixture_external_network_json(
            &root,
            Score::from_result(SEED_SOLUTION_CAP, true, SEED_SOLUTION_CAP),
            &root.full_edges,
        );
        assert!(
            extract_external_root_record(
                BufReader::new(Cursor::new(fixture_external_artifact(&[inexact]))),
                fixture_external_provenance(),
                &root.network_sha256,
                ExternalRootArtifactKind::PinnedRound2Landscape,
            )
            .unwrap_err()
            .contains("lower bound")
        );

        let mut corrupt_full = root.full_edges.clone();
        corrupt_full.pop();
        let noncanonical = fixture_external_network_json(
            &root,
            Score::exact(root.solution_count, SEED_SOLUTION_CAP),
            &corrupt_full,
        );
        assert!(
            extract_external_root_record(
                BufReader::new(Cursor::new(fixture_external_artifact(&[noncanonical]))),
                fixture_external_provenance(),
                &root.network_sha256,
                ExternalRootArtifactKind::PinnedRound2Landscape,
            )
            .unwrap_err()
            .contains("not its declared exact canonical state")
        );

        let corrupt_json = b"{\"type\":\"header\"}\nnot-json\n".to_vec();
        assert!(
            extract_external_root_record(
                BufReader::new(Cursor::new(corrupt_json)),
                fixture_external_provenance(),
                &root.network_sha256,
                ExternalRootArtifactKind::PinnedRound2Landscape,
            )
            .is_err()
        );
    }

    #[test]
    fn completed_root_neighborhood_record_parser_is_strict_and_schema_aware() {
        let root = select_root_seed(&frozen(), Some(42), None).unwrap();
        let exact = fixture_root_neighborhood_network_json(
            &root,
            Score::exact(root.solution_count, SEED_SOLUTION_CAP),
        );
        let artifact =
            fixture_root_neighborhood_artifact(&root, std::slice::from_ref(&exact), true);
        assert_eq!(
            parse_external_root_artifact_kind(&artifact).unwrap(),
            ExternalRootArtifactKind::CompletedRootNeighborhoodV1
        );
        let parsed = extract_external_root_record(
            BufReader::new(Cursor::new(artifact.clone())),
            fixture_root_neighborhood_provenance(&artifact),
            &root.network_sha256,
            ExternalRootArtifactKind::CompletedRootNeighborhoodV1,
        )
        .unwrap();
        assert_eq!(parsed.network_sha256, root.network_sha256);
        assert_eq!(parsed.solution_count, root.solution_count);
        let RootSource::ExternalRecord(provenance) = parsed.source else {
            panic!("loaded root-neighborhood state must be external");
        };
        assert_eq!(provenance.line_number, 130);
        assert_eq!(provenance.sha256, sha256_hex(&artifact));
        assert_eq!(provenance.authentication, "explicit-whole-file-sha256-pin");

        let incomplete =
            fixture_root_neighborhood_artifact(&root, std::slice::from_ref(&exact), false);
        assert!(
            extract_external_root_record(
                BufReader::new(Cursor::new(incomplete.clone())),
                fixture_root_neighborhood_provenance(&incomplete),
                &root.network_sha256,
                ExternalRootArtifactKind::CompletedRootNeighborhoodV1,
            )
            .unwrap_err()
            .contains("classification_complete")
        );

        let mut trailing = artifact.clone();
        trailing.extend_from_slice(
            format!(
                "{{\"type\":\"root_solution\",\"schema\":{}}}\n",
                json_quote(ROOT_NEIGHBORHOOD_SCHEMA)
            )
            .as_bytes(),
        );
        assert!(
            extract_external_root_record(
                BufReader::new(Cursor::new(trailing.clone())),
                fixture_root_neighborhood_provenance(&trailing),
                &root.network_sha256,
                ExternalRootArtifactKind::CompletedRootNeighborhoodV1,
            )
            .unwrap_err()
            .contains("after its terminal summary")
        );
    }

    #[test]
    #[ignore = "requires the audited frozen-seed-2 radius-one artifact"]
    fn root_neighborhood_304_loader_and_exact_replay_are_golden_when_present() {
        const ROOT_SHA256: &str =
            "b02c3e9aa72ccb7b21759c397a423384b3c3b28d2bf71db31c40e40d54ae2124";
        const ARTIFACT_SHA256: &str =
            "7200eef43617c2fdd17d981fbdd295d279fe56c17e95ca356a30ce66f0041d7e";
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../runs/18c-root-seed-2-radius1-cap4096-v1.jsonl");
        assert!(path.exists(), "missing {}", path.display());
        let root = load_external_root_record(&path, ROOT_SHA256, Some(ARTIFACT_SHA256)).unwrap();
        assert_eq!(root.solution_count, 304);
        assert_eq!(root.network_sha256, ROOT_SHA256);
        let RootSource::ExternalRecord(provenance) = &root.source else {
            panic!("loaded root must be external");
        };
        assert_eq!(provenance.bytes, 2_312_384);
        assert_eq!(provenance.sha256, ARTIFACT_SHA256);
        assert_eq!(provenance.line_number, 1_039);
        assert_eq!(provenance.schema, ROOT_NEIGHBORHOOD_SCHEMA);
        assert_eq!(provenance.authentication, "explicit-whole-file-sha256-pin");
        let solver = Solver::blank_comparisons(&root.hasse_edges).unwrap();
        let replay = solver.count_up_to(305);
        assert_eq!(replay.count, 304);
        assert!(!replay.capped);
        let batch = solver.enumerate_up_to(304);
        assert!(batch.exhausted && !batch.capped);
        let distinct = batch.solutions.into_iter().collect::<BTreeSet<_>>();
        assert_eq!(distinct.len(), 304);
        assert!(distinct.contains(&root.target));
    }

    #[test]
    #[ignore = "requires the audited full round-two landscape artifact"]
    fn external_root_560_loader_and_exact_replay_are_golden_when_present() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../runs/18c-beam-round2-landscape4096-v1.jsonl");
        assert!(path.exists(), "missing {}", path.display());
        let root = load_external_root_record(&path, FIRST_EXTERNAL_ROOT_SHA256, None).unwrap();
        assert_eq!(root.solution_count, FIRST_EXTERNAL_ROOT_COUNT);
        assert_eq!(root.network_sha256, FIRST_EXTERNAL_ROOT_SHA256);
        let RootSource::ExternalRecord(provenance) = &root.source else {
            panic!("loaded root must be external");
        };
        assert_eq!(provenance.bytes, ROUND2_LANDSCAPE_ARTIFACT_BYTES);
        assert_eq!(provenance.sha256, ROUND2_LANDSCAPE_ARTIFACT_SHA256);
        assert_eq!(provenance.line_number, 6_503);
        let solver = Solver::blank_comparisons(&root.hasse_edges).unwrap();
        let replay = solver.count_up_to(FIRST_EXTERNAL_ROOT_COUNT + 1);
        assert_eq!(replay.count, FIRST_EXTERNAL_ROOT_COUNT);
        assert!(!replay.capped);
        let batch = solver.enumerate_up_to(usize::try_from(FIRST_EXTERNAL_ROOT_COUNT).unwrap());
        assert!(batch.exhausted && !batch.capped);
        let distinct = batch.solutions.into_iter().collect::<BTreeSet<_>>();
        assert_eq!(distinct.len(), FIRST_EXTERNAL_ROOT_COUNT as usize);
        assert!(distinct.contains(&root.target));
        let solutions = distinct.into_iter().collect::<Vec<_>>();

        let mut accounting = RootNeighborhoodAccounting::default();
        let networks = generate_root_neighborhood(&root, &solutions, &mut accounting).unwrap();
        assert_eq!(accounting.raw_move_attempts, 635_600);
        assert_eq!(accounting.radius_rejections, 300_720);
        assert_eq!(accounting.coverage_rejections, 5_081);
        assert_eq!(accounting.accepted_observations, 329_799);
        assert_eq!(accounting.distinct_observed_networks, 821);
        assert_eq!(accounting.duplicate_observations, 328_978);
        assert_eq!(
            networks
                .values()
                .map(|network| network.occurrences)
                .sum::<u64>(),
            accounting.accepted_observations
        );

        let mut key_bytes = Vec::new();
        for key in networks.keys() {
            key_bytes.extend_from_slice(
                &u32::try_from(key.len())
                    .expect("Hasse edge count fits u32")
                    .to_le_bytes(),
            );
            for &(lower, upper) in key {
                key_bytes.extend_from_slice(&[lower, upper]);
            }
        }
        assert_eq!(
            sha256_hex(&key_bytes),
            "ff155dd54307a68e21f2198bc5bbb87924407615f3651793b545aaf7445661d5"
        );

        for (key, network) in &networks {
            let origin = &network.representative_origin;
            let target = &solutions[origin.target_ordinal - 1];
            let footprint = match origin.move_kind {
                MoveKind::Retarget => root.cells.clone(),
                MoveKind::Swap { removed, added } => {
                    let mut cells = root
                        .cells
                        .iter()
                        .copied()
                        .filter(|&cell| cell != removed)
                        .collect::<Vec<_>>();
                    cells.push(added);
                    cells.sort_unstable();
                    cells
                }
            };
            let replayed = construct_move_state(&footprint, target).unwrap().unwrap();
            assert_eq!(&replayed.hasse_edges, key);
            assert_eq!(replayed, network.state);
        }
    }

    #[test]
    fn root_radius_one_generation_is_replayable_and_conservative() {
        let seeds = frozen();
        let root = select_root_seed(&seeds, Some(42), None).unwrap();
        let expected = usize::try_from(root.solution_count).unwrap();
        let mut solutions = Solver::blank_comparisons(&root.hasse_edges)
            .unwrap()
            .enumerate_up_to(expected)
            .solutions;
        solutions.sort_unstable();
        solutions.dedup();
        assert_eq!(solutions.len(), expected);
        solutions.truncate(2);

        let mut accounting = RootNeighborhoodAccounting::default();
        let networks = generate_root_neighborhood(&root, &solutions, &mut accounting).unwrap();
        assert_eq!(accounting.raw_move_attempts, 2 * ROOT_MOVES_PER_TARGET);
        assert_eq!(
            accounting.radius_rejections
                + accounting.coverage_rejections
                + accounting.accepted_observations,
            accounting.raw_move_attempts
        );
        assert_eq!(
            accounting.distinct_observed_networks + accounting.duplicate_observations,
            accounting.accepted_observations
        );
        assert_eq!(
            networks
                .values()
                .map(|network| network.occurrences)
                .sum::<u64>(),
            accounting.accepted_observations
        );
        assert!(!networks.is_empty());

        for (key, network) in &networks {
            assert_eq!(key, &network.state.hasse_edges);
            assert_eq!(network_sha256(key).len(), 64);
            assert!(
                network
                    .observed_target_ordinals
                    .iter()
                    .all(|&ordinal| (1..=2).contains(&ordinal))
            );
            let origin = &network.representative_origin;
            let target = &solutions[origin.target_ordinal - 1];
            let footprint = match origin.move_kind {
                MoveKind::Retarget => root.cells.clone(),
                MoveKind::Swap { removed, added } => {
                    let mut cells = root
                        .cells
                        .iter()
                        .copied()
                        .filter(|&cell| cell != removed)
                        .collect::<Vec<_>>();
                    cells.push(added);
                    cells.sort_unstable();
                    cells
                }
            };
            assert_eq!(
                construct_move_state(&footprint, target).unwrap().unwrap(),
                network.state
            );
        }

        let mut repeat_accounting = RootNeighborhoodAccounting::default();
        let repeat = generate_root_neighborhood(&root, &solutions, &mut repeat_accounting).unwrap();
        assert_eq!(repeat_accounting, accounting);
        assert_eq!(
            repeat.keys().collect::<Vec<_>>(),
            networks.keys().collect::<Vec<_>>()
        );
        for key in networks.keys() {
            assert_eq!(
                repeat[key].representative_origin,
                networks[key].representative_origin
            );
            assert_eq!(repeat[key].state, networks[key].state);
            assert_eq!(repeat[key].occurrences, networks[key].occurrences);
        }
    }

    #[test]
    fn root_42_full_radius_one_generation_is_golden() {
        let root = select_root_seed(&frozen(), Some(42), None).unwrap();
        let expected = usize::try_from(root.solution_count).unwrap();
        let batch = Solver::blank_comparisons(&root.hasse_edges)
            .unwrap()
            .enumerate_up_to(expected);
        assert!(batch.exhausted && !batch.capped);
        let mut solutions = batch.solutions;
        solutions.sort_unstable();
        solutions.dedup();
        assert_eq!(solutions.len(), expected);

        let mut accounting = RootNeighborhoodAccounting::default();
        let networks = generate_root_neighborhood(&root, &solutions, &mut accounting).unwrap();
        assert_eq!(accounting.raw_move_attempts, 145_280);
        assert_eq!(accounting.radius_rejections, 72_704);
        assert_eq!(accounting.coverage_rejections, 1_071);
        assert_eq!(accounting.accepted_observations, 71_505);
        assert_eq!(accounting.distinct_observed_networks, 758);
        assert_eq!(accounting.duplicate_observations, 70_747);
        assert_eq!(
            networks
                .values()
                .map(|network| network.occurrences)
                .sum::<u64>(),
            71_505
        );

        let mut key_bytes = Vec::new();
        for key in networks.keys() {
            key_bytes.extend_from_slice(
                &u32::try_from(key.len())
                    .expect("Hasse edge count fits u32")
                    .to_le_bytes(),
            );
            for &(lower, upper) in key {
                key_bytes.extend_from_slice(&[lower, upper]);
            }
        }
        assert_eq!(
            sha256_hex(&key_bytes),
            "b741487987bbddfb94bc51c1ad677ec087266e2285504eea26848703271a1fe5"
        );

        for (key, network) in &networks {
            let origin = &network.representative_origin;
            let target = &solutions[origin.target_ordinal - 1];
            let footprint = match origin.move_kind {
                MoveKind::Retarget => root.cells.clone(),
                MoveKind::Swap { removed, added } => {
                    let mut cells = root
                        .cells
                        .iter()
                        .copied()
                        .filter(|&cell| cell != removed)
                        .collect::<Vec<_>>();
                    cells.push(added);
                    cells.sort_unstable();
                    cells
                }
            };
            let replayed = construct_move_state(&footprint, target).unwrap().unwrap();
            assert_eq!(&replayed.hasse_edges, key);
            assert_eq!(replayed, network.state);
        }
    }

    #[test]
    #[ignore = "measured release golden for the seed42 two-exchange shell"]
    fn root_42_two_cell_removed_pair_one_is_golden() {
        let root = select_root_seed(&frozen(), Some(42), None).unwrap();
        let expected = usize::try_from(root.solution_count).unwrap();
        let batch = Solver::blank_comparisons(&root.hasse_edges)
            .unwrap()
            .enumerate_up_to(expected);
        assert!(batch.exhausted && !batch.capped);
        let mut solutions = batch.solutions;
        solutions.sort_unstable();
        solutions.dedup();
        assert_eq!(solutions.len(), expected);

        let (plan, geometric_rejections) = root_two_cell_footprint_plan(&root).unwrap();
        assert_eq!(
            plan.len() as u64 + geometric_rejections,
            ROOT_TWO_CELL_MOVES_PER_TARGET
        );
        assert_eq!(
            plan.iter()
                .filter(|item| item.removed_pair_ordinal == 1)
                .count(),
            624
        );
        let started = std::time::Instant::now();
        let mut accounting = RootTwoCellAccounting::default();
        let networks =
            generate_root_two_cell_neighborhood(&root, &solutions, 1, 1, &mut accounting).unwrap();
        let elapsed = started.elapsed();
        let retained_payload_lower_bound = networks
            .keys()
            .map(|key| {
                std::mem::size_of::<Vec<Edge>>()
                    + key.capacity() * std::mem::size_of::<Edge>()
                    + std::mem::size_of::<RootTwoCellNetworkResult>()
            })
            .sum::<usize>();
        let mut key_bytes = Vec::new();
        for key in networks.keys() {
            key_bytes.extend_from_slice(&(key.len() as u32).to_le_bytes());
            for &(lower, upper) in key {
                key_bytes.extend_from_slice(&[lower, upper]);
            }
        }
        eprintln!(
            "shell2 removed_pair1 full_plan={} full_geometric_rejections={} raw={} shard_geometric_rejections={} coverage_rejections={} accepted={} distinct={} duplicates={} elapsed_ms={} payload_lower_bound_bytes={} key_digest={}",
            plan.len(),
            geometric_rejections,
            accounting.raw_move_attempts,
            accounting.geometric_incidence_rejections,
            accounting.coverage_rejections,
            accounting.accepted_observations,
            accounting.distinct_observed_networks,
            accounting.duplicate_observations,
            elapsed.as_millis(),
            retained_payload_lower_bound,
            sha256_hex(&key_bytes),
        );
        assert_eq!(accounting.raw_move_attempts, 249_984);
        assert_eq!(accounting.geometric_incidence_rejections, 170_112);
        assert_eq!(accounting.coverage_rejections, 2_638);
        assert_eq!(accounting.accepted_observations, 77_234);
        assert_eq!(accounting.distinct_observed_networks, 1_115);
        assert_eq!(accounting.duplicate_observations, 76_119);
        assert_eq!(
            sha256_hex(&key_bytes),
            "7f3261133501869474fad41d414aad92b404ca78e3d1e3c77503eccc51f4f098"
        );
        assert_eq!(networks.len() as u64, accounting.distinct_observed_networks);
    }

    #[test]
    fn root_two_cell_removed_pair_order_is_frozen() {
        let root = select_root_seed(&frozen(), Some(42), None).unwrap();
        let mut pairs = Vec::new();
        for first_index in 0..root.cells.len() {
            for second_index in first_index + 1..root.cells.len() {
                pairs.push([root.cells[first_index], root.cells[second_index]]);
            }
        }
        assert_eq!(pairs.len(), ROOT_TWO_CELL_REMOVAL_PAIRS as usize);
        for (ordinal, expected) in [
            (1, [2, 3]),
            (39, [4, 29]),
            (40, [4, 43]),
            (77, [19, 28]),
            (78, [19, 29]),
            (115, [29, 61]),
            (116, [29, 68]),
            (153, [68, 69]),
        ] {
            assert_eq!(pairs[ordinal - 1], expected, "removed pair {ordinal}");
        }
    }

    #[test]
    fn root_two_cell_json_records_are_valid_and_shard_scoped() {
        let root = select_root_seed(&frozen(), Some(42), None).unwrap();
        let batch = Solver::blank_comparisons(&root.hasse_edges)
            .unwrap()
            .enumerate_up_to(root.solution_count as usize);
        assert!(batch.exhausted && !batch.capped);
        let mut root_solutions = batch.solutions;
        root_solutions.sort_unstable();
        root_solutions.dedup();
        let (plan, _) = root_two_cell_footprint_plan(&root).unwrap();
        let (footprint_move, state) = plan
            .iter()
            .find_map(|footprint_move| {
                construct_move_state(&footprint_move.footprint, &root_solutions[0])
                    .unwrap()
                    .map(|state| (*footprint_move, state))
            })
            .unwrap();
        let key = state.hasse_edges.clone();
        let network = RootTwoCellNetworkResult {
            representative_origin: RootTwoCellMoveOrigin {
                target_ordinal: 1,
                removed_pair_ordinal: footprint_move.removed_pair_ordinal,
                removed: footprint_move.removed,
                added: footprint_move.added,
            },
            representative_canonical_target: state.target,
            observed_target_mask: [1, 0],
            occurrences: 1,
            score: Some(Score::exact(2, NORMAL_CAP)),
            solver_stats: Some(SolveStats::default()),
            preexisting_seed_ordinal: None,
        };
        let outcome = RootTwoCellNeighborhoodOutcome {
            root,
            root_solutions,
            root_enumeration_stats: batch.stats,
            first_removed_pair_ordinal: 1,
            last_removed_pair_ordinal: 1,
            networks: BTreeMap::from([(key.clone(), network)]),
            accounting: RootTwoCellAccounting {
                seed_replay_calls: SEED_COUNT as u64,
                root_enumeration_calls: 1,
                raw_move_attempts: 249_984,
                geometric_incidence_rejections: 170_112,
                coverage_rejections: 249_984 - 170_112 - 1,
                accepted_observations: 1,
                distinct_observed_networks: 1,
                new_canonical_networks: 1,
                network_count_calls: 1,
                exact_networks: 1,
                ..RootTwoCellAccounting::default()
            },
            solver_totals: SolverTotals {
                calls: SEED_COUNT as u64 + 2,
                ..SolverTotals::default()
            },
            status: "root-two-cell-shard-complete",
            unique_network: None,
        };
        let options = Options {
            seeds: PathBuf::from("analysis/18c-seeds-v1.jsonl"),
            continuation: None,
            output: PathBuf::from("runs/unused-root-two-cell.jsonl"),
            progress_every: 0,
            exploration_probes: DEFAULT_EXPLORATION_PROBES,
            exploration_cap: DEFAULT_EXPLORATION_CAP,
            max_new_counts: DEFAULT_MAX_NEW_COUNTS,
            max_total_solver_calls: DEFAULT_MAX_TOTAL_SOLVER_CALLS,
            root_neighborhood: None,
            root_two_cell_neighborhood: Some(RootTwoCellNeighborhoodOptions {
                seed_ordinal: Some(42),
                network_sha256: Some(ROOT_TWO_CELL_SEED_SHA256.to_owned()),
                first_removed_pair_ordinal: 1,
                last_removed_pair_ordinal: 1,
                count_cap: NORMAL_CAP,
            }),
        };
        let binary = BinaryProvenance {
            path: PathBuf::from("thermo-18c-beam"),
            bytes: 1,
            sha256: "00".repeat(32),
        };
        let records = [
            root_two_cell_header_json(&options, &binary, &outcome),
            root_two_cell_solution_json(&outcome, 1, &outcome.root_solutions[0]),
            root_two_cell_network_json(&key, &outcome.networks[&key], &outcome.root_solutions),
            root_two_cell_summary_json(&options, &outcome),
        ];
        for json in records {
            let parsed = JsonParser::new(json.as_bytes()).parse().unwrap();
            let fields = json_object(&parsed).unwrap();
            require_string_field(fields, "schema", ROOT_TWO_CELL_NEIGHBORHOOD_SCHEMA).unwrap();
        }
        let header =
            JsonParser::new(root_two_cell_header_json(&options, &binary, &outcome).as_bytes())
                .parse()
                .unwrap();
        let scope =
            json_object(json_field(json_object(&header).unwrap(), "scope").unwrap()).unwrap();
        require_bool_field(scope, "all_128_root_targets_used", true).unwrap();
        require_bool_field(scope, "complete_all_153_removed_pairs", false).unwrap();
        require_bool_field(scope, "no_unique_in_declared_removed_pair_shard", true).unwrap();
        require_bool_field(
            scope,
            "no_improvement_below_root_count_128_in_declared_removed_pair_shard",
            false,
        )
        .unwrap();
        require_bool_field(
            scope,
            "no_exact_solution_count_below_configured_cap_in_declared_removed_pair_shard",
            false,
        )
        .unwrap();

        let mut final_key_unique = outcome.clone();
        final_key_unique.status = "unique-found";
        final_key_unique.unique_network = Some(key.clone());
        final_key_unique.networks.get_mut(&key).unwrap().score = Some(Score::exact(1, 2));
        for json in [
            root_two_cell_header_json(&options, &binary, &final_key_unique),
            root_two_cell_summary_json(&options, &final_key_unique),
        ] {
            let parsed = JsonParser::new(json.as_bytes()).parse().unwrap();
            let fields = json_object(&parsed).unwrap();
            let scope_or_summary =
                if json_string(json_field(fields, "type").unwrap()).unwrap() == "header" {
                    json_object(json_field(fields, "scope").unwrap()).unwrap()
                } else {
                    fields
                };
            require_bool_field(scope_or_summary, "classification_complete", true).unwrap();
            require_bool_field(
                scope_or_summary,
                "no_unique_in_declared_removed_pair_shard",
                false,
            )
            .unwrap();
            require_bool_field(
                scope_or_summary,
                "no_improvement_below_root_count_128_in_declared_removed_pair_shard",
                false,
            )
            .unwrap();
        }
    }

    #[test]
    fn root_json_records_are_distinct_valid_and_self_describing() {
        let root = select_root_seed(&frozen(), Some(42), None).unwrap();
        let expected = usize::try_from(root.solution_count).unwrap();
        let batch = Solver::blank_comparisons(&root.hasse_edges)
            .unwrap()
            .enumerate_up_to(expected);
        assert!(batch.exhausted && !batch.capped);
        let mut root_solutions = batch.solutions;
        root_solutions.sort_unstable();
        root_solutions.dedup();
        let target_ordinal = root_solutions.binary_search(&root.target).unwrap() + 1;
        let state = canonical_saturated_state(&root.cells, &root.target).unwrap();
        let key = state.hasse_edges.clone();
        let network = RootNetworkResult {
            state,
            representative_origin: RootMoveOrigin {
                target_ordinal,
                move_kind: MoveKind::Retarget,
            },
            observed_target_ordinals: BTreeSet::from([target_ordinal]),
            occurrences: 1,
            score: Some(Score::exact(root.solution_count, SEED_SOLUTION_CAP)),
            solver_stats: None,
            preexisting_seed_ordinal: root.seed_ordinal(),
            external_root_replay_cache: false,
        };
        let mut networks = BTreeMap::from([(key.clone(), network)]);
        assert!(first_exact_unique(&networks).is_none());
        networks.get_mut(&key).unwrap().score = Some(Score::exact(1, 2));
        assert_eq!(first_exact_unique(&networks), Some(key.clone()));
        networks.get_mut(&key).unwrap().score =
            Some(Score::exact(root.solution_count, SEED_SOLUTION_CAP));
        let outcome = RootNeighborhoodOutcome {
            root: root.clone(),
            root_exact_replay_stats: None,
            root_solutions,
            root_enumeration_stats: batch.stats,
            networks,
            accounting: RootNeighborhoodAccounting {
                seed_replay_calls: SEED_COUNT as u64,
                root_enumeration_calls: 1,
                raw_move_attempts: root.solution_count * ROOT_MOVES_PER_TARGET,
                coverage_rejections: root.solution_count * ROOT_MOVES_PER_TARGET - 1,
                accepted_observations: 1,
                distinct_observed_networks: 1,
                preexisting_seed_networks_observed: 1,
                count_cache_hits: 1,
                frozen_seed_cache_hits: 1,
                exact_networks: 1,
                ..RootNeighborhoodAccounting::default()
            },
            solver_totals: SolverTotals {
                calls: SEED_COUNT as u64 + 1,
                ..SolverTotals::default()
            },
            status: "root-neighborhood-complete",
            unique_network: None,
        };
        let options = Options {
            seeds: PathBuf::from("analysis/18c-seeds-v1.jsonl"),
            continuation: None,
            output: PathBuf::from("runs/unused-root.jsonl"),
            progress_every: 0,
            exploration_probes: DEFAULT_EXPLORATION_PROBES,
            exploration_cap: DEFAULT_EXPLORATION_CAP,
            max_new_counts: DEFAULT_MAX_NEW_COUNTS,
            max_total_solver_calls: DEFAULT_MAX_TOTAL_SOLVER_CALLS,
            root_neighborhood: Some(RootNeighborhoodOptions {
                seed_ordinal: Some(42),
                network_sha256: Some(root.network_sha256.clone()),
                record_input: None,
                record_sha256: None,
                count_cap: DEFAULT_ROOT_COUNT_CAP,
            }),
            root_two_cell_neighborhood: None,
        };
        let binary = BinaryProvenance {
            path: PathBuf::from("thermo-18c-beam"),
            bytes: 1,
            sha256: "00".repeat(32),
        };
        let records = [
            ("header", root_header_json(&options, &binary, &outcome)),
            (
                "root_solution",
                root_solution_json(&outcome, 1, &outcome.root_solutions[0]),
            ),
            (
                "network",
                root_network_json(
                    ROOT_NEIGHBORHOOD_SCHEMA,
                    &key,
                    &outcome.networks[&key],
                    &root.hasse_edges,
                ),
            ),
            ("summary", root_summary_json(&options, &outcome)),
        ];
        for (record_type, json) in records {
            let parsed = JsonParser::new(json.as_bytes()).parse().unwrap();
            let fields = json_object(&parsed).unwrap();
            require_string_field(fields, "type", record_type).unwrap();
            require_string_field(fields, "schema", ROOT_NEIGHBORHOOD_SCHEMA).unwrap();
        }

        let mut final_key_unique = outcome.clone();
        final_key_unique.status = "unique-found";
        final_key_unique.unique_network = Some(key.clone());
        final_key_unique.networks.get_mut(&key).unwrap().score = Some(Score::exact(1, 2));
        for json in [
            root_header_json(&options, &binary, &final_key_unique),
            root_summary_json(&options, &final_key_unique),
        ] {
            let parsed = JsonParser::new(json.as_bytes()).parse().unwrap();
            let fields = json_object(&parsed).unwrap();
            let scope_or_summary = match json_string(json_field(fields, "type").unwrap()).unwrap() {
                "header" => json_object(json_field(fields, "scope").unwrap()).unwrap(),
                "summary" => fields,
                other => panic!("unexpected record {other}"),
            };
            require_bool_field(scope_or_summary, "classification_complete", true).unwrap();
            require_bool_field(
                scope_or_summary,
                if fields.contains_key("scope") {
                    "negative_uniqueness_result_only_for_declared_root_neighborhood"
                } else {
                    "negative_uniqueness_result_for_declared_root_neighborhood"
                },
                false,
            )
            .unwrap();
        }

        let mut external = outcome.clone();
        external.root.source = RootSource::ExternalRecord(fixture_external_provenance());
        external.root_exact_replay_stats = Some(SolveStats::default());
        external
            .networks
            .get_mut(&key)
            .unwrap()
            .preexisting_seed_ordinal = None;
        external
            .networks
            .get_mut(&key)
            .unwrap()
            .external_root_replay_cache = true;
        external.accounting.root_exact_replay_calls = 1;
        external.accounting.preexisting_seed_networks_observed = 0;
        external.accounting.frozen_seed_cache_hits = 0;
        external.accounting.external_root_cache_hits = 1;
        external.accounting.new_canonical_networks = 1;
        external.solver_totals.calls += 1;
        let mut external_options = options.clone();
        let external_mode = external_options.root_neighborhood.as_mut().unwrap();
        external_mode.seed_ordinal = None;
        external_mode.record_input = Some(PathBuf::from("fixture-round2.jsonl"));
        let external_records = [
            external_root_header_json(&external_options, &binary, &external),
            root_solution_json(&external, 1, &external.root_solutions[0]),
            root_network_json(
                EXTERNAL_ROOT_NEIGHBORHOOD_SCHEMA,
                &key,
                &external.networks[&key],
                &external.root.hasse_edges,
            ),
            external_root_summary_json(&external_options, &external),
        ];
        for json in external_records {
            let parsed = JsonParser::new(json.as_bytes()).parse().unwrap();
            let fields = json_object(&parsed).unwrap();
            require_string_field(fields, "schema", EXTERNAL_ROOT_NEIGHBORHOOD_SCHEMA).unwrap();
        }

        let mut external_final_key_unique = external.clone();
        external_final_key_unique.status = "unique-found";
        external_final_key_unique.unique_network = Some(key.clone());
        external_final_key_unique
            .networks
            .get_mut(&key)
            .unwrap()
            .score = Some(Score::exact(1, 2));
        for json in [
            external_root_header_json(&external_options, &binary, &external_final_key_unique),
            external_root_summary_json(&external_options, &external_final_key_unique),
        ] {
            let parsed = JsonParser::new(json.as_bytes()).parse().unwrap();
            let fields = json_object(&parsed).unwrap();
            let scope_or_summary = match json_string(json_field(fields, "type").unwrap()).unwrap() {
                "header" => json_object(json_field(fields, "scope").unwrap()).unwrap(),
                "summary" => fields,
                other => panic!("unexpected record {other}"),
            };
            require_bool_field(scope_or_summary, "classification_complete", true).unwrap();
            require_bool_field(
                scope_or_summary,
                if fields.contains_key("scope") {
                    "negative_uniqueness_result_only_for_declared_root_neighborhood"
                } else {
                    "negative_uniqueness_result_for_declared_root_neighborhood"
                },
                false,
            )
            .unwrap();
        }
    }

    #[test]
    fn representative_target_is_pinned_ahead_of_lexicographic_reservoir() {
        let mut entry = dummy_entry(vec![(0, 1)], Score::exact(4, 5));
        entry.representative_target = dummy_grid(9);
        entry.targets = TargetReservoir::from_sorted(
            vec![dummy_grid(1), dummy_grid(2), dummy_grid(3), dummy_grid(4)],
            ROUND1_TARGETS_PER_NETWORK,
        );
        let expansion = entry.expansion_targets(ROUND1_TARGETS_PER_NETWORK);
        assert_eq!(expansion.len(), ROUND1_TARGETS_PER_NETWORK);
        assert_eq!(expansion[0], dummy_grid(9));
        assert_eq!(
            expansion[1..],
            [dummy_grid(1), dummy_grid(2), dummy_grid(3)]
        );

        entry.targets = TargetReservoir::from_sorted(
            vec![dummy_grid(1), dummy_grid(9)],
            ROUND1_TARGETS_PER_NETWORK,
        );
        let short = entry.expansion_targets(ROUND1_TARGETS_PER_NETWORK);
        assert_eq!(short, [dummy_grid(9), dummy_grid(1)]);
        assert!(entry.has_unexpanded_target(ROUND1_TARGETS_PER_NETWORK));
    }

    #[test]
    fn score_upgrades_are_monotonic_and_accounting_is_conservative() {
        let lower129 = Score::from_result(129, true, 129);
        assert!(monotonic_score_upgrade(lower129, Score::exact(128, 512)).is_err());
        assert_eq!(
            monotonic_score_upgrade(lower129, Score::exact(129, 512)).unwrap(),
            Score::exact(129, 512)
        );
        assert_eq!(
            monotonic_score_upgrade(lower129, Score::from_result(512, true, 512)).unwrap(),
            Score::from_result(512, true, 512)
        );
        assert!(monotonic_score_upgrade(Score::exact(128, 1024), lower129).is_err());
        assert_eq!(
            monotonic_score_upgrade(lower129, Score::exact(777, 1024)).unwrap(),
            Score::exact(777, 1024)
        );
        assert_eq!(
            monotonic_score_upgrade(lower129, Score::from_result(1024, true, 1024)).unwrap(),
            Score::from_result(1024, true, 1024)
        );

        let complete = RoundAccounting {
            new_canonical_networks: 10,
            normal_count_calls: 9,
            count_cache_hits: 1,
            normal_exact: 3,
            normal_lower_bounds: 6,
            high_cap_probe_calls: 4,
            high_cap_exact: 2,
            high_cap_lower_bounds: 2,
            ..RoundAccounting::default()
        };
        assert!(complete.round_complete());
        assert_eq!(
            complete.normal_exact + complete.normal_lower_bounds,
            complete.normal_count_calls
        );
        assert_eq!(
            complete.high_cap_exact + complete.high_cap_lower_bounds,
            complete.high_cap_probe_calls
        );
        let mut incomplete = complete;
        incomplete.total_call_ceiling_hit = true;
        assert!(!incomplete.round_complete());
    }

    #[test]
    fn exploration_cap_boundaries_are_explicit_and_independent_of_seed_cap() {
        for cap in [NORMAL_CAP + 1, SEED_SOLUTION_CAP, MAX_EXPLORATION_CAP] {
            assert!(validate_exploration_cap(cap).is_ok(), "cap {cap}");
        }
        for cap in [0, NORMAL_CAP, MAX_EXPLORATION_CAP + 1, u64::MAX] {
            assert!(validate_exploration_cap(cap).is_err(), "cap {cap}");
        }
        let lower129 = Score::from_result(NORMAL_CAP, true, NORMAL_CAP);
        assert_eq!(
            monotonic_score_upgrade(
                lower129,
                Score::from_result(MAX_EXPLORATION_CAP, true, MAX_EXPLORATION_CAP),
            )
            .unwrap(),
            Score::from_result(MAX_EXPLORATION_CAP, true, MAX_EXPLORATION_CAP)
        );
    }

    #[test]
    fn probe_limit_at_or_above_eligible_count_selects_every_key() {
        let mut archive = BTreeMap::new();
        let mut keys = Vec::new();
        for index in 0u8..12 {
            let key = vec![(index, index + 1)];
            let mut entry = dummy_entry(key.clone(), Score::from_result(129, true, 129));
            entry.normal_stats = Some(SolveStats {
                nodes: 100 - u64::from(index),
                ..SolveStats::default()
            });
            archive.insert(key.clone(), entry);
            keys.push(key);
        }
        let expected = keys.iter().cloned().collect::<BTreeSet<_>>();
        for limit in [keys.len(), keys.len() + 100] {
            let selected = select_probe_keys(&archive, &keys, limit);
            assert_eq!(selected.len(), keys.len());
            assert_eq!(selected.into_iter().collect::<BTreeSet<_>>(), expected);
        }
        assert!(select_probe_keys(&archive, &keys, 0).is_empty());
    }

    #[test]
    fn next_frontier_uses_unexpanded_targets_and_four_probe_buckets() {
        let mut archive = BTreeMap::new();
        for index in 0u8..20 {
            let key = vec![(index, index + 1)];
            archive.insert(
                key.clone(),
                dummy_entry(key, Score::exact(u64::from(index) + 1, 1024)),
            );
        }
        let mut probe_keys = Vec::new();
        let mut cell = 30u8;
        while probe_keys.len() < 8 {
            let key = vec![(cell, cell + 1)];
            if probe_keys
                .iter()
                .all(|existing: &Vec<Edge>| probe_bucket(existing) != probe_bucket(&key))
                || probe_keys.len() >= 4
            {
                let mut entry = dummy_entry(key.clone(), Score::from_result(512, true, 512));
                entry.high_cap_probed = true;
                entry.probe_stats = Some(SolveStats {
                    nodes: u64::from(cell),
                    ..SolveStats::default()
                });
                archive.insert(key.clone(), entry);
                probe_keys.push(key);
            }
            cell += 1;
        }
        let selected = select_next_frontier(&archive, ROUND1_TARGETS_PER_NETWORK);
        assert_eq!(selected.len(), BEAM_WIDTH);
        assert_eq!(
            selected[..12],
            (0u8..12).map(|i| vec![(i, i + 1)]).collect::<Vec<_>>()
        );
        assert_eq!(
            selected[12..]
                .iter()
                .map(|key| probe_bucket(key))
                .collect::<BTreeSet<_>>()
                .len(),
            4
        );

        for key in &selected[..12] {
            let entry = archive.get_mut(key).unwrap();
            entry.expanded_targets.extend(
                entry
                    .expansion_targets(ROUND1_TARGETS_PER_NETWORK)
                    .into_iter(),
            );
        }
        let successor = select_next_frontier(&archive, ROUND1_TARGETS_PER_NETWORK);
        assert!(selected[..12].iter().all(|key| !successor.contains(key)));
    }

    #[test]
    fn frozen_replay_and_move_generation_meet_exact_bounds_and_partitions() {
        let seeds = frozen();
        let (frontier_indices, pending) = select_seed_frontier(&seeds).unwrap();
        let (mut archive, cache, totals) =
            replay_seed_archive(&seeds, DEFAULT_MAX_TOTAL_SOLVER_CALLS).unwrap();
        assert_eq!(archive.len(), 71);
        assert_eq!(cache.len(), 71);
        assert_eq!(totals.calls, 71);
        assert_eq!(pending.len(), 55);
        let frontier = frontier_indices
            .iter()
            .map(|&index| seeds[index].hasse_edges.clone())
            .collect::<Vec<_>>();
        let mut accounting = RoundAccounting::default();
        let mut sources = BTreeMap::new();
        let candidates = generate_round_candidates(
            &mut archive,
            &frontier,
            &mut accounting,
            ROUND1_TARGETS_PER_NETWORK,
            RAW_MOVE_HARD_MAX,
            None,
            &mut sources,
        )
        .unwrap();
        assert_eq!(accounting.raw_move_attempts, RAW_MOVE_HARD_MAX);
        assert!(!accounting.raw_ceiling_hit);
        assert_eq!(candidates.len() as u64, accounting.new_canonical_networks);
        assert_eq!(
            accounting.radius_rejections
                + accounting.coverage_rejections
                + accounting.accepted_observations,
            RAW_MOVE_HARD_MAX
        );
        assert_eq!(
            accounting.visited_observations
                + accounting.duplicate_new_observations
                + accounting.new_canonical_networks,
            accounting.accepted_observations
        );
        assert!(accounting.new_canonical_networks <= DEFAULT_MAX_NEW_COUNTS);
    }

    #[test]
    fn round2_symmetry_keys_and_factor_bands_are_explicit() {
        let cells = vec![0, 1, 10, 20, 30];
        for spatial in 0..8 {
            let transformed = cells
                .iter()
                .map(|&cell| transform_cell(cell, spatial))
                .collect::<Vec<_>>();
            assert_eq!(
                footprint_orbit_key(&cells),
                footprint_orbit_key(&transformed)
            );
        }
        let edges = vec![(0, 1), (0, 9), (1, 10), (9, 10), (10, 11)];
        let mut reversed = edges
            .iter()
            .map(|&(lower, upper)| (upper, lower))
            .collect::<Vec<_>>();
        reversed.sort_unstable();
        assert_eq!(topology_signature(&edges), topology_signature(&reversed));

        assert_eq!(factor_band_role(50, 100), FrontierRole::BarrierLe4x);
        assert_eq!(factor_band_role(100, 100), FrontierRole::BarrierLe4x);
        assert_eq!(factor_band_role(400, 100), FrontierRole::BarrierLe4x);
        assert_eq!(factor_band_role(401, 100), FrontierRole::Barrier4To8x);
        assert_eq!(factor_band_role(800, 100), FrontierRole::Barrier4To8x);
        assert_eq!(factor_band_role(801, 100), FrontierRole::Barrier8To16x);
        assert_eq!(factor_band_role(1_600, 100), FrontierRole::Barrier8To16x);
        assert_eq!(factor_band_role(1_601, 100), FrontierRole::BarrierGt16x);
        assert_eq!(exact_log2_band(1), -7);
        assert_eq!(exact_log2_band(127), -1);
        assert_eq!(exact_log2_band(128), 0);
        assert_eq!(exact_log2_band(255), 0);
        assert_eq!(exact_log2_band(256), 1);
        assert_eq!(exact_log2_band(4_095), 4);
    }

    #[test]
    fn round_modes_keep_separate_revisions_and_target_limits() {
        assert_ne!(SCHEMA, CONTINUATION_SCHEMA);
        assert_ne!(ALGORITHM_REVISION, CONTINUATION_ALGORITHM_REVISION);

        let round1 = Options {
            seeds: PathBuf::from("seeds.jsonl"),
            continuation: None,
            output: PathBuf::from("round1.jsonl"),
            progress_every: 0,
            exploration_probes: DEFAULT_EXPLORATION_PROBES,
            exploration_cap: DEFAULT_EXPLORATION_CAP,
            max_new_counts: DEFAULT_MAX_NEW_COUNTS,
            max_total_solver_calls: DEFAULT_MAX_TOTAL_SOLVER_CALLS,
            root_neighborhood: None,
            root_two_cell_neighborhood: None,
        };
        let round2 = Options {
            continuation: Some(PathBuf::from("round1.jsonl")),
            output: PathBuf::from("round2.jsonl"),
            exploration_probes: 4_096,
            exploration_cap: 4_096,
            ..round1.clone()
        };
        assert_eq!(round1.target_limit(), ROUND1_TARGETS_PER_NETWORK);
        assert_eq!(round1.raw_move_hard_max(), RAW_MOVE_HARD_MAX);
        assert_eq!(round2.target_limit(), ROUND2_TARGETS_PER_NETWORK);
        assert_eq!(round2.raw_move_hard_max(), ROUND2_RAW_MOVE_HARD_MAX);
    }

    #[test]
    fn round2_target_witnesses_have_distinct_relevant_signatures() {
        let seed = frozen().remove(41);
        let batch = Solver::blank_comparisons(&seed.hasse_edges)
            .unwrap()
            .enumerate_up_to(seed.solution_count as usize);
        assert!(batch.exhausted);
        let solutions = batch.solutions;
        let selection = choose_signature_diverse_targets(
            &seed.hasse_edges,
            &seed.cells,
            seed.target,
            solutions.clone(),
            &BTreeSet::new(),
            ROUND2_TARGETS_PER_NETWORK,
        )
        .unwrap();
        let ordered = selection.ordered_targets;
        assert_eq!(
            selection.reservoir.targets.len(),
            ROUND2_TARGETS_PER_NETWORK
        );
        assert_eq!(ordered.len(), ROUND2_TARGETS_PER_NETWORK);
        assert_eq!(
            ordered[0],
            target_normal_form(&seed.hasse_edges, &seed.target)
        );
        assert!(selection.unique_signatures >= ROUND2_TARGETS_PER_NETWORK);
        assert!(selection.signature_pairs > 0);
        assert_eq!(selection.historical_signatures_excluded, 0);
        let pair_list = relevant_comparison_pairs(&seed.cells);
        let signatures = ordered
            .iter()
            .map(|target| target_signature(target, &pair_list))
            .collect::<BTreeSet<_>>();
        assert_eq!(signatures.len(), ROUND2_TARGETS_PER_NETWORK);

        let expanded = BTreeSet::from([ordered[0]]);
        let next_selection = choose_signature_diverse_targets(
            &seed.hasse_edges,
            &seed.cells,
            seed.target,
            solutions,
            &expanded,
            ROUND2_TARGETS_PER_NETWORK,
        )
        .unwrap();
        let next = next_selection.ordered_targets;
        assert!(next_selection.historical_signatures_excluded >= 1);
        let prior = target_signature(
            &target_normal_form(&seed.hasse_edges, &ordered[0]),
            &pair_list,
        );
        assert!(next.iter().all(|target| {
            target_signature(&target_normal_form(&seed.hasse_edges, target), &pair_list) != prior
        }));
    }

    #[test]
    fn round2_probe_order_is_deterministic_and_source_stratified() {
        let mut archive = BTreeMap::new();
        let mut candidates = Vec::new();
        let mut sources = BTreeMap::new();
        for index in 0u8..8 {
            let key = vec![(index, index + 1)];
            archive.insert(
                key.clone(),
                dummy_entry(
                    key.clone(),
                    Score::from_result(NORMAL_CAP, true, NORMAL_CAP),
                ),
            );
            candidates.push(key.clone());
            sources.insert(
                key,
                BTreeSet::from([SourceStratum {
                    parent_hasse_edges: vec![(40 + index % 2, 50 + index % 2)],
                    target_ordinal: usize::from(index % 2) + 1,
                }]),
            );
        }
        let first = select_probe_keys_by_source(&archive, &candidates, &sources, 6);
        let second = select_probe_keys_by_source(&archive, &candidates, &sources, 6);
        assert_eq!(first, second);
        assert_eq!(first.len(), 6);
        assert_eq!(first.iter().cloned().collect::<BTreeSet<_>>().len(), 6);
        assert_ne!(sources[&first[0]], sources[&first[1]]);
    }

    #[test]
    #[ignore = "requires the full ignored round-one artifact"]
    fn strict_round2_restart_and_parent_lanes_match_the_frozen_artifact_when_present() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../runs/18c-beam-round1-landscape4096-v1.jsonl");
        if !path.exists() {
            return;
        }
        let seeds = frozen();
        let bytes = fs::read(&path).unwrap();
        let (archive, count_cache, continuation) =
            load_round1_artifact(&path, &bytes, &seeds).unwrap();
        assert_eq!(archive.len(), ROUND1_NETWORKS);
        assert_eq!(count_cache.len(), ROUND1_NETWORKS);
        assert_eq!(
            archive
                .values()
                .filter(|entry| entry.score.is_some_and(|score| score.exact))
                .count(),
            ROUND1_EXACT_NETWORKS
        );
        let (frontier, roles) = select_round2_parent_frontier(&archive).unwrap();
        assert_eq!(frontier.len(), BEAM_WIDTH);
        assert_eq!(
            frontier
                .iter()
                .map(|key| network_sha256(key))
                .collect::<Vec<_>>(),
            [
                "01f8d095a4b63ba1ffd5c848057dd151c9d36b47ffee47480fcb40ece410acb7",
                "96826f9a29ceb1c3c0be5475464bc1bea5d57f4fe6020970fa83b064925602ee",
                "b06657957225e1ff6d5153a75c1e1b05b0a8dfbb66b93a2f09bf61cebe03948d",
                "35a31eb6effe7ca1d832c96da9c1c8e19af178448bfd44f8dcc079823980c5ac",
                "20c450cc3d07546f11802303a49a862727a3036525c1c025de90356376d4fbd0",
                "ee45698ec020cab7a849f68d2c37c0e08cbd8923a44346a930fb352ae7a40dbc",
                "42c4f9ed9115a5983c40efadfd3399e822d5e66c332d7bb7a5dcf3a76ac5c105",
                "d938d9d0a15cf6aaa04b7ae2d42e1a795e6b309f5868798806fc82680b75bdc2",
                "3e41149191208a9e14b86042a77d248afd2ede4193530befcf40d81c9a151aa3",
                "26b9b8ebb0f42c71dd969017f87b285c93f8af8befe2bd696b2bf6b2e8e6b09a",
                "c48ec6452fccc8b5055c945d817ec9d5ef10d66e9f9072bc6e178192065cd335",
                "0d25f3b4c98e6f1b536e3598e3ba10241f3e8a085a0ea4290ce09016dc6ab375",
                "d38827cd3218ba9dfc98394ff28cd98c9de114de25274487018060f3609b494d",
                "013e9e22d6fce9f7dae67d156252ec3c3b3ce0bcdc1f1ee87a13e0c986d4e176",
                "98990e5a21d5ee9eb3a9eae787d02f689680d0da48508fedfbd9c247d7bd9aff",
                "748c60a325fc4a512acfa627d6f173ec7e6485dbd2acbe9821c17e695ab5b9dd",
            ]
        );
        assert_eq!(
            roles
                .values()
                .filter(|&&role| role == FrontierRole::ExactExploit)
                .count(),
            8
        );
        assert_eq!(
            roles
                .values()
                .filter(|&&role| role == FrontierRole::ExactExploitUntouchedSeed)
                .count(),
            ROUND2_SEED_ANCHOR_SLOTS
        );
        assert_eq!(
            roles
                .values()
                .filter(|&&role| role == FrontierRole::HotNovelty)
                .count(),
            4
        );
        for key in &frontier {
            match roles[key] {
                FrontierRole::ExactExploit => {
                    assert!(matches!(archive[key].origin, Origin::Generated { .. }));
                    assert!(archive[key].score.unwrap().exact);
                }
                FrontierRole::ExactExploitUntouchedSeed => {
                    assert!(matches!(archive[key].origin, Origin::Seed { .. }));
                    assert!(archive[key].expanded_targets.is_empty());
                }
                FrontierRole::HotNovelty => assert!(!archive[key].score.unwrap().exact),
                _ => panic!("factor-band roles belong to the successor frontier"),
            }
        }
        assert!(continuation.historical_whole_file_match);
    }

    #[test]
    #[ignore = "bounded smoke requires the full ignored round-one artifact"]
    fn round2_q8_parent_target_smoke() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../runs/18c-beam-round1-landscape4096-v1.jsonl");
        assert!(path.exists(), "missing {}", path.display());
        let seeds = frozen();
        let bytes = fs::read(&path).unwrap();
        let (mut archive, mut count_cache, _) =
            load_round1_artifact(&path, &bytes, &seeds).unwrap();
        let (frontier, _) = select_round2_parent_frontier(&archive).unwrap();
        let options = Options {
            seeds: PathBuf::from("analysis/18c-seeds-v1.jsonl"),
            continuation: Some(path),
            output: PathBuf::from("runs/unused-round2-smoke.jsonl"),
            progress_every: 0,
            exploration_probes: 4_096,
            exploration_cap: 4_096,
            max_new_counts: DEFAULT_MAX_NEW_COUNTS,
            max_total_solver_calls: DEFAULT_MAX_TOTAL_SOLVER_CALLS,
            root_neighborhood: None,
            root_two_cell_neighborhood: None,
        };
        let mut accounting = RoundAccounting::default();
        let mut totals = SolverTotals::default();
        let audits = enrich_round2_targets(
            &mut archive,
            &mut count_cache,
            &frontier,
            &options,
            &mut accounting,
            &mut totals,
        )
        .unwrap();
        assert_eq!(audits.len(), BEAM_WIDTH);
        assert_eq!(accounting.target_pool_calls, BEAM_WIDTH as u64);
        assert_eq!(accounting.target_pool_solutions, 29_207);
        assert_eq!(accounting.target_pool_exact_upgrades, 0);
        assert_eq!(accounting.target_witnesses_selected, 128);
        assert_eq!(totals.calls, BEAM_WIDTH as u64);
        assert_eq!(
            audits
                .values()
                .filter(|audit| audit.enumeration_exhausted)
                .count(),
            12
        );
        assert_eq!(
            audits
                .values()
                .filter(|audit| audit.enumeration_capped)
                .count(),
            4
        );
        assert_eq!(
            audits
                .values()
                .filter(|audit| audit.enumeration_exhausted)
                .map(|audit| audit.enumerated_solutions)
                .sum::<usize>(),
            12_823
        );
        assert_eq!(
            audits
                .values()
                .map(|audit| audit.historical_signatures_excluded)
                .sum::<usize>(),
            0
        );
        let mut ordered_target_bytes =
            Vec::with_capacity(BEAM_WIDTH * ROUND2_TARGETS_PER_NETWORK * CELLS);
        for key in &frontier {
            let audit = &audits[key];
            assert_eq!(audit.selected_witnesses, ROUND2_TARGETS_PER_NETWORK);
            assert!(audit.unique_signatures >= ROUND2_TARGETS_PER_NETWORK);
            assert_eq!(audit.historical_signatures_excluded, 0);
            assert_eq!(
                audit.ordered_targets[0],
                target_normal_form(key, &archive[key].representative_target)
            );
            for target in &audit.ordered_targets {
                // Fixed-width encoding: 81 raw bytes, one byte per digit 1..=9,
                // for each target in frontier order and then witness order.
                ordered_target_bytes.extend_from_slice(target);
            }
        }
        assert_eq!(
            sha256_hex(&ordered_target_bytes),
            "09dc2dfad8d02c391940da72926ac36ab6397e1a1535c5e0f5bb140fb93b0a51"
        );
    }

    #[test]
    fn atomic_output_never_replaces_an_existing_file() {
        let directory = env::temp_dir().join(format!(
            "thermo-18c-beam-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let output = directory.join("round.jsonl");
        fs::write(&output, b"owned\n").unwrap();
        let error = atomic_write(&output, |writer| {
            writer.write_all(b"replacement\n").unwrap();
            Ok(())
        })
        .unwrap_err();
        assert!(error.contains("refusing to overwrite"));
        assert_eq!(fs::read(&output).unwrap(), b"owned\n");
        fs::remove_file(output).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
