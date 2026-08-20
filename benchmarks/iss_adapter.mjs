#!/usr/bin/env node

// Persistent Node.js adapter for benchmarking Interactive Sudoku Solver.
// Input is one JSON document on stdin; output is one JSON document on stdout.

import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const [, , issRootArgument] = process.argv;
if (!issRootArgument) {
  throw new Error('usage: node iss_adapter.mjs PATH_TO_ISS');
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

const request = JSON.parse(fs.readFileSync(0, 'utf8'));
const warmupRounds = request.warmup_rounds;
const repeats = request.repeats;
if (!Number.isInteger(warmupRounds) || warmupRounds < 0) {
  throw new Error('warmup_rounds must be a non-negative integer');
}
if (!Number.isInteger(repeats) || repeats <= 0) {
  throw new Error('repeats must be a positive integer');
}

function cellId(index) {
  const row = Math.floor(index / 9) + 1;
  const column = index % 9 + 1;
  return `R${row}C${column}`;
}

function makeConstraint(layout) {
  return new SudokuConstraint.Container(layout.map(
    pathCells => new SudokuConstraint.Thermo(...pathCells.map(cellId)),
  ));
}

function milliseconds(start, end) {
  return Number(end - start) / 1_000_000;
}

function classify(testCase, collect, samples) {
  const started = process.hrtime.bigint();
  const solver = SudokuBuilder.build(testCase.constraint);
  const built = process.hrtime.bigint();
  const observed = solver.countSolutions(2);
  const finished = process.hrtime.bigint();
  if (observed !== testCase.expected) {
    throw new Error(
      `${testCase.name}: ISS returned ${observed}, expected ${testCase.expected}`,
    );
  }
  if (collect) {
    samples.build_ms.push(milliseconds(started, built));
    samples.count_ms.push(milliseconds(built, finished));
    samples.total_ms.push(milliseconds(started, finished));
  }
}

// Constraint-object creation is outside the timed region, matching the Rust
// measurement's exclusion of Python validation and ctypes array creation.
const preparedCases = request.cases.map(testCase => ({
  ...testCase,
  constraint: makeConstraint(testCase.layout),
}));
const byName = new Map(preparedCases.map(testCase => [testCase.name, {
  build_ms: [],
  count_ms: [],
  total_ms: [],
}]));

// Round-robin warm-up gives every constraint shape a chance to reach optimized
// code before measurement and avoids favoring the cases encountered first.
for (let round = 0; round < warmupRounds; round++) {
  for (let offset = 0; offset < preparedCases.length; offset++) {
    const testCase = preparedCases[(offset + round) % preparedCases.length];
    classify(testCase, false, null);
  }
}

for (let round = 0; round < repeats; round++) {
  for (let offset = 0; offset < preparedCases.length; offset++) {
    const testCase = preparedCases[(offset + round) % preparedCases.length];
    classify(testCase, true, byName.get(testCase.name));
  }
}

function quantile(sorted, q) {
  if (sorted.length === 1) return sorted[0];
  const position = (sorted.length - 1) * q;
  const lower = Math.floor(position);
  const upper = Math.ceil(position);
  if (lower === upper) return sorted[lower];
  const fraction = position - lower;
  return sorted[lower] * (1 - fraction) + sorted[upper] * fraction;
}

function summarize(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return {
    median: quantile(sorted, 0.5),
    p10: quantile(sorted, 0.1),
    p90: quantile(sorted, 0.9),
    min: sorted[0],
    max: sorted.at(-1),
    samples: values,
  };
}

const cases = preparedCases.map(testCase => {
  const samples = byName.get(testCase.name);
  return {
    name: testCase.name,
    build_ms: summarize(samples.build_ms),
    count_ms: summarize(samples.count_ms),
    total_ms: summarize(samples.total_ms),
  };
});

process.stdout.write(JSON.stringify({
  node_version: process.version,
  v8_version: process.versions.v8,
  iss_root: issRoot,
  warmup_rounds: warmupRounds,
  repeats,
  cases,
}));
