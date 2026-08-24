# Maximal-forest optimization benchmark

Both artifacts scan the same first 100 eligible catalogue records with
`min_paths=4`, `max_paths=7`, summary-only output, no layout deduplication, no
witness cache, and the authoritative 49,158-record corpus.  They were run
sequentially on the same four-logical-CPU Windows host.

| Measure | Frozen scanner | Optimized scanner | Change |
| --- | ---: | ---: | ---: |
| Elapsed | 96.198 s | 73.900 s | -23.2% |
| Mandatory DFS nodes | 79,039,635 | 63,539,168 | -19.6% |
| Mandatory spatial prunes | 392,239,338 | 310,969,633 | -20.7% |
| Skeletons | 2,396,515 | 2,396,515 | identical |
| Merge DFS nodes | 21,140,910 | 16,594,074 | -21.5% |
| Merge spatial prunes | 264,154,746 | 217,079,024 | -17.8% |
| Maximal/classified occurrences | 246,253 | 246,253 | identical |
| Multiple | 246,253 | 246,253 | identical |
| Unique | 0 | 0 | identical |

All 39 per-partition maximal, classified, multiple, and unique counts match
exactly.  The optimization moves the reversal test up to the earliest prefix
which can no longer end canonically, raises the per-record path floor to its
maximum digit multiplicity, and rejects a smaller competing adjacent-rank
merge before descending into its necessarily noncanonical subtree.  Such a
merge still counts when deciding whether its parent is merge-maximal.

Artifact identities:

```text
baseline-frozen-100-eligible-k4to7.jsonl
  bytes     9,712
  SHA-256   9741899b8e0f1296eca6846e065b5f29d3cd5bd80c4fafac5d34feee5bac17df

optimized-100-eligible-k4to7.jsonl
  bytes     9,742
  SHA-256   8bb3517ee87ac0ed16c97b57cbd1481bdf5632b773391cec55f54fac459ea832

optimized scanner source
  SHA-256   fa2ec18f10f6f3a778c856f433321faddf99ba4d88713012ddfc50cc888a4296

optimized release executable
  SHA-256   d8bf3cacd6b3d1cb2014b4d4a4d8954c25eea6dce500c9e1651536b8d00c7bec
```

Elapsed time is a practical measurement, not a deterministic certificate.
The exact matching candidate and partition counts are the regression check.
