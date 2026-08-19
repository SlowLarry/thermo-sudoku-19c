#include <array>
#include <cstdint>
#include <iostream>
#include <vector>

using u64 = std::uint64_t;

static std::array<std::vector<int>, 81> adjacency(bool diagonal) {
    std::array<std::vector<int>, 81> result;
    for (int r = 0; r < 9; ++r) {
        for (int c = 0; c < 9; ++c) {
            const int v = 9 * r + c;
            for (int dr = -1; dr <= 1; ++dr) {
                for (int dc = -1; dc <= 1; ++dc) {
                    if ((dr == 0 && dc == 0) || (!diagonal && dr != 0 && dc != 0)) continue;
                    const int rr = r + dr, cc = c + dc;
                    if (0 <= rr && rr < 9 && 0 <= cc && cc < 9) result[v].push_back(9 * rr + cc);
                }
            }
        }
    }
    return result;
}

static void walk(const std::array<std::vector<int>, 81>& adj, int v, int depth,
                 u64 lo, u64 hi, std::array<u64, 10>& counts) {
    ++counts[depth];
    if (depth == 9) return;
    for (int w : adj[v]) {
        const bool used = w < 64 ? ((lo >> w) & 1U) : ((hi >> (w - 64)) & 1U);
        if (used) continue;
        if (w < 64) walk(adj, w, depth + 1, lo | (u64{1} << w), hi, counts);
        else walk(adj, w, depth + 1, lo, hi | (u64{1} << (w - 64)), counts);
    }
}

static void report(bool diagonal) {
    const auto adj = adjacency(diagonal);
    std::array<u64, 10> counts{};
    for (int v = 0; v < 81; ++v) {
        const u64 lo = v < 64 ? (u64{1} << v) : 0;
        const u64 hi = v >= 64 ? (u64{1} << (v - 64)) : 0;
        walk(adj, v, 1, lo, hi, counts);
    }
    std::cout << (diagonal ? "king" : "orthogonal") << '\n';
    for (int length = 2; length <= 9; ++length) {
        std::cout << length << ' ' << counts[length] << '\n';
    }
}

int main() {
    report(false);
    report(true);
}
