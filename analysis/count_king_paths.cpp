#include <array>
#include <cstdint>
#include <iostream>

std::array<std::array<int, 8>, 81> nbr{};
std::array<int, 81> deg{};

uint64_t dfs(int at, int remaining, uint64_t lo, uint32_t hi) {
    if (remaining == 0) return 1;
    uint64_t total = 0;
    for (int k = 0; k < deg[at]; ++k) {
        int nx = nbr[at][k];
        if (nx < 64) {
            uint64_t bit = uint64_t{1} << nx;
            if (!(lo & bit)) total += dfs(nx, remaining - 1, lo | bit, hi);
        } else {
            uint32_t bit = uint32_t{1} << (nx - 64);
            if (!(hi & bit)) total += dfs(nx, remaining - 1, lo, hi | bit);
        }
    }
    return total;
}

int main(int argc, char **argv) {
    bool orthogonal = argc > 1 && std::string(argv[1]) == "orthogonal";
    for (int r = 0; r < 9; ++r) for (int c = 0; c < 9; ++c) {
        int v = 9 * r + c;
        for (int dr = -1; dr <= 1; ++dr) for (int dc = -1; dc <= 1; ++dc) {
            if (dr == 0 && dc == 0) continue;
            if (orthogonal && dr != 0 && dc != 0) continue;
            int rr = r + dr, cc = c + dc;
            if (0 <= rr && rr < 9 && 0 <= cc && cc < 9) nbr[v][deg[v]++] = 9 * rr + cc;
        }
    }
    for (int cells : {2, 8, 9}) {
        uint64_t total = 0;
        for (int start = 0; start < 81; ++start) {
            uint64_t lo = 0; uint32_t hi = 0;
            if (start < 64) lo |= uint64_t{1} << start;
            else hi |= uint32_t{1} << (start - 64);
            total += dfs(start, cells - 1, lo, hi);
        }
        std::cout << cells << " " << total << "\n";
    }
}
