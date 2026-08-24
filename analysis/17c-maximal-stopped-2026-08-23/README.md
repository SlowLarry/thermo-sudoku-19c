# Preserved stopped `k=4..7` run

This directory freezes the ten completed shard summaries from the exploratory
16-shard `k=4..7` run started on 2026-08-22 and stopped by the user on
2026-08-23.  Six unfinished shard files were still zero bytes and are not
represented here.  These results are partial evidence only; their line ranges
do not cover the catalogue.

Run scope: simple, cell-disjoint, king-step thermometers covering exactly 17
cells, geometric diagonal crossings allowed, `min_paths=4`, `max_paths=7`,
summary-only output, no layout deduplication, no witness cache, and stop on the
first unique result.  All 13,378,887 classified maximal layout occurrences in
the ten completed shards were multiple; none was unique.

Provenance:

```text
catalogue 17puz49158.txt:
  bytes       4,080,114
  SHA-256     58ef7d83e8cbac32495161f9745877fef82f5e8b3fe58e3cad4eb3fc004a81b9

frozen scanner source thermo-17c-maximal-frozen.rs:
  SHA-256     9863853075f0026b566467cca5a6d3e1470d34cc61fc47d608af227f2d556331

release executable used by the run (not vendored):
  SHA-256     aa900d86974437adafd7d183ac9e0c03975843b999eb8d0ccb5cded40c44d23

solver library source src/lib.rs:
  SHA-256     944bb51fbd6d953d47a7902983e11f67b762df7e04f9221650b2d4119c84b911
```

Preserved artifacts:

| Shard | Inclusive lines | Bytes | SHA-256 |
| ---: | ---: | ---: | --- |
| 02 | 3,074-6,146 | 9,850 | `cf4bd7cd3df90abab4dcf1d4389d62ae06212f9bf5e650e2d781b7fe70aad769` |
| 03 | 6,147-9,219 | 9,866 | `26972fccc9ebeb234ddf06d07fd03d9326f9d57c10ac99b401cfab149ab12e23` |
| 04 | 9,220-12,292 | 9,829 | `85b1b369d98b53d75e26841b942caf78e8333ae16269238c669e887d7b80e892` |
| 05 | 12,293-15,365 | 9,823 | `85b5d18fcb7b379cb72fde9dbfcf21e37bd6512f3a13bc06ff41f6be6d2c91dc` |
| 07 | 18,439-21,510 | 9,799 | `3cde7632dd47adfa8345c51524a058422e61712fff548be12be1543999c02c63` |
| 10 | 27,655-30,726 | 9,818 | `476c4db7eacdc7b6df1c288622acc0e8f947a64063a34764ee2fa8547900f330` |
| 11 | 30,727-33,798 | 9,831 | `670247eb94e9e0301511623f527268aa803499b07f790b994a8239b23751846b` |
| 12 | 33,799-36,870 | 9,819 | `0572aede7b37453fcb84cc68b6db10a213019f2d9a9c88ce028303e8f8e3e57f` |
| 14 | 39,943-43,014 | 9,828 | `d936661724d46a55102d4fee2d1a34ac070f9793afbe36df0716c5999b299ab3` |
| 16 | 46,087-49,158 | 9,740 | `82e32800a8e72f7d488886c21912d1341f3306cdc653adb74a17d2fa0382447e` |

The copied JSONL bytes and frozen source are unchanged from the stopped run.
