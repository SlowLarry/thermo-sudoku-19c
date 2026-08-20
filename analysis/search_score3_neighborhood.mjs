#!/usr/bin/env node

// Exhaust the one-edge replacement neighbourhood of the first known
// three-solution 9+8+2 comparison set.  Interactive Sudoku Solver is used as
// a fast, independent generic-comparison oracle; every inequality is encoded
// as a two-cell thermometer, so overlaps are intentionally allowed.

import path from 'node:path';
import { pathToFileURL } from 'node:url';

const [, , issRootArgument] = process.argv;
if (!issRootArgument) {
  throw new Error('usage: node search_score3_neighborhood.mjs PATH_TO_ISS');
}

const issRoot = path.resolve(issRootArgument);
globalThis.self = globalThis;
globalThis.VERSION_PARAM = '';

const constraintUrl = pathToFileURL(
  path.join(issRoot, 'js', 'sudoku_constraint.js'),
).href;
const builderUrl = pathToFileURL(
  path.join(issRoot, 'js', 'solver', 'sudoku_builder.js'),
).href;
const { SudokuConstraint } = await import(constraintUrl);
const { SudokuBuilder } = await import(builderUrl);

const paths = [
  [19, 29, 28, 20, 11, 12, 13, 3, 4],
  [77, 69, 78, 70, 62, 53, 44, 52],
  [41, 51],
];
const baseEdges = paths.flatMap(pathCells => pathCells.slice(0, -1).map(
  (cell, index) => [cell, pathCells[index + 1]],
));

function edgeKey([lower, upper]) {
  return `${lower},${upper}`;
}

function cellId(index) {
  return `R${Math.floor(index / 9) + 1}C${index % 9 + 1}`;
}

function makeConstraint(edges) {
  return new SudokuConstraint.Container(edges.map(
    ([lower, upper]) => new SudokuConstraint.Thermo(cellId(lower), cellId(upper)),
  ));
}

function count(edges, limit = 2) {
  return SudokuBuilder.build(makeConstraint(edges)).countSolutions(limit);
}

const universe = [];
for (let first = 0; first < 81; first++) {
  for (let second = first + 1; second < 81; second++) {
    const rowDistance = Math.abs(Math.floor(first / 9) - Math.floor(second / 9));
    const columnDistance = Math.abs((first % 9) - (second % 9));
    if (rowDistance <= 1 && columnDistance <= 1) {
      universe.push([first, second], [second, first]);
    }
  }
}

if (baseEdges.length !== 16 || universe.length !== 544 || count(baseEdges, 4) !== 3) {
  throw new Error('unexpected base or universe semantics');
}
for (let warmup = 0; warmup < 10; warmup++) count(baseEdges);

const started = process.hrtime.bigint();
let tested = 0;
let unsatisfiable = 0;
let multiple = 0;
const unique = [];

for (let removed = 0; removed < baseEdges.length; removed++) {
  const remaining = baseEdges.filter((_, index) => index !== removed);
  const remainingKeys = new Set(remaining.map(edgeKey));
  for (const added of universe) {
    if (remainingKeys.has(edgeKey(added)) || edgeKey(added) === edgeKey(baseEdges[removed])) {
      continue;
    }
    const observed = count([...remaining, added]);
    tested++;
    if (observed === 0) {
      unsatisfiable++;
    } else if (observed === 1) {
      unique.push({
        removed_index: removed,
        removed_edge: baseEdges[removed],
        added_edge: added,
      });
    } else {
      multiple++;
    }
  }
  process.stderr.write(
    `removed=${removed + 1}/${baseEdges.length} tested=${tested} unique=${unique.length}\n`,
  );
}

const elapsedSeconds = Number(process.hrtime.bigint() - started) / 1_000_000_000;
process.stdout.write(`${JSON.stringify({
  model: 'classic-sudoku-plus-16-directed-king-comparisons',
  base: 'first-score-3-9+8+2',
  iss_root: issRoot,
  universe_edges: universe.length,
  tested,
  unsatisfiable,
  unique_count: unique.length,
  multiple,
  elapsed_seconds: elapsedSeconds,
  unique,
}, null, 2)}\n`);
