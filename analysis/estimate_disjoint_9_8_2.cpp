#include <algorithm>
#include <array>
#include <bit>
#include <cmath>
#include <cstdint>
#include <iomanip>
#include <iostream>
#include <random>
#include <vector>

struct Mask { uint64_t lo; uint32_t hi; };
std::array<std::array<int, 8>, 81> nbr{};
std::array<int, 81> deg{};

class RankSampler {
public:
    RankSampler(int cells, uint64_t total, size_t samples, uint64_t seed)
        : cells_(cells) {
        std::mt19937_64 rng(seed);
        std::uniform_int_distribution<uint64_t> pick(0, total - 1);
        targets_.resize(samples);
        for (auto &x : targets_) x = pick(rng);
        std::sort(targets_.begin(), targets_.end());
        samples_.reserve(samples);
    }

    std::vector<Mask> run() {
        for (int start = 0; start < 81; ++start) {
            Mask m{0, 0}; set(m, start);
            dfs(start, cells_ - 1, m);
        }
        return std::move(samples_);
    }

private:
    int cells_;
    uint64_t rank_ = 0;
    size_t target_i_ = 0;
    std::vector<uint64_t> targets_;
    std::vector<Mask> samples_;

    static bool has(Mask m, int x) {
        return x < 64 ? ((m.lo >> x) & 1) : ((m.hi >> (x - 64)) & 1);
    }
    static void set(Mask &m, int x) {
        if (x < 64) m.lo |= uint64_t{1} << x;
        else m.hi |= uint32_t{1} << (x - 64);
    }
    void leaf(Mask m) {
        while (target_i_ < targets_.size() && targets_[target_i_] == rank_) {
            samples_.push_back(m); ++target_i_;
        }
        ++rank_;
    }
    void dfs(int at, int remaining, Mask m) {
        if (remaining == 0) { leaf(m); return; }
        for (int k = 0; k < deg[at]; ++k) {
            int nx = nbr[at][k];
            if (!has(m, nx)) { Mask n = m; set(n, nx); dfs(nx, remaining - 1, n); }
        }
    }
};

static bool disjoint(Mask a, Mask b) { return !(a.lo & b.lo) && !(a.hi & b.hi); }

static int available_directed_edges(Mask used) {
    int degree_sum = 0, twice_internal = 0;
    for (int v = 0; v < 81; ++v) {
        bool present = v < 64 ? ((used.lo >> v) & 1) : ((used.hi >> (v - 64)) & 1);
        if (!present) continue;
        degree_sum += deg[v];
        for (int k = 0; k < deg[v]; ++k) {
            int w = nbr[v][k];
            bool wpresent = w < 64 ? ((used.lo >> w) & 1) : ((used.hi >> (w - 64)) & 1);
            twice_internal += wpresent;
        }
    }
    int internal = twice_internal / 2;
    return 544 - 2 * (degree_sum - internal);
}

int main(int argc, char **argv) {
    size_t samples = argc > 1 ? std::stoull(argv[1]) : 2000000;
    uint64_t seed = argc > 2 ? std::stoull(argv[2]) : 0;
    for (int r = 0; r < 9; ++r) for (int c = 0; c < 9; ++c) {
        int v = 9 * r + c;
        for (int dr = -1; dr <= 1; ++dr) for (int dc = -1; dc <= 1; ++dc) {
            if (dr == 0 && dc == 0) continue;
            int rr = r + dr, cc = c + dc;
            if (0 <= rr && rr < 9 && 0 <= cc && cc < 9) nbr[v][deg[v]++] = 9 * rr + cc;
        }
    }
    constexpr uint64_t n9 = 85743256, n8 = 16418600;
    auto p9 = RankSampler(9, n9, samples, 0x9182abcdu ^ seed).run();
    auto p8 = RankSampler(8, n8, samples, 0x8291dcabu ^ (seed << 1)).run();
    if (p9.size() != samples || p8.size() != samples) return 2;
    std::mt19937_64 shuffle_rng(0xd15a01a7u ^ (seed << 2));
    std::shuffle(p9.begin(), p9.end(), shuffle_rng);
    std::shuffle(p8.begin(), p8.end(), shuffle_rng);

    constexpr int batches = 40;
    std::array<long double, batches> batch_mean{};
    uint64_t dis = 0;
    long double sum = 0, sum_edges_disjoint = 0;
    for (size_t i = 0; i < samples; ++i) {
        long double value = 0;
        if (disjoint(p9[i], p8[i])) {
            ++dis;
            Mask used{p9[i].lo | p8[i].lo, p9[i].hi | p8[i].hi};
            int edges = available_directed_edges(used);
            sum_edges_disjoint += edges;
            value = edges;
        }
        sum += value;
        batch_mean[i * batches / samples] += value;
    }
    size_t per_batch = samples / batches;
    for (auto &x : batch_mean) x /= per_batch;
    long double mean = sum / samples;
    long double var = 0;
    for (auto x : batch_mean) var += (x - mean) * (x - mean);
    var /= (batches - 1);
    long double se_mean = std::sqrt(var / batches);
    long double scale = static_cast<long double>(n9) * n8;
    std::cout << std::setprecision(12)
              << "samples " << samples << "\n"
              << "disjoint " << dis << "\n"
              << "p_disjoint " << static_cast<long double>(dis) / samples << "\n"
              << "mean_edges_given_disjoint " << sum_edges_disjoint / dis << "\n"
              << "mean_indicator_times_edges " << mean << "\n"
              << "config_estimate " << scale * mean << "\n"
              << "config_se " << scale * se_mean << "\n"
              << "config_95ci_low " << scale * (mean - 1.96L * se_mean) << "\n"
              << "config_95ci_high " << scale * (mean + 1.96L * se_mean) << "\n";
}
