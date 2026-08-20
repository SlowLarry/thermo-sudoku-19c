#!/usr/bin/env node

// Persistent Node/V8 host for the timing-only ThermoBenchInterop export.

import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { pathToFileURL } from 'node:url';

const [, , bundleArgument] = process.argv;
if (!bundleArgument) {
  throw new Error('usage: node wasm_rangsk_adapter.mjs PATH_TO_PUBLISHED_WWWROOT');
}

const bundle = path.resolve(bundleArgument);
const frameworkEntry = path.join(bundle, '_framework', 'dotnet.js');
if (!fs.existsSync(frameworkEntry)) {
  throw new Error(`No published .NET WASM runtime at ${frameworkEntry}`);
}

const request = JSON.parse(fs.readFileSync(0, 'utf8'));
const originalCwd = process.cwd();
process.chdir(bundle);

const runtimeStarted = performance.now();
const { dotnet } = await import(pathToFileURL(frameworkEntry).href);
const { setModuleImports, getAssemblyExports, getConfig } = await dotnet
  .withDiagnosticTracing(false)
  .create();
setModuleImports('solver', { sendResponse: () => {} });

const config = getConfig();
const exports = await getAssemblyExports(config.mainAssemblyName);
const solver = exports.SudokuSolverWasm.SolverInterop;
const thermoBench = exports.SudokuSolverWasm.ThermoBenchInterop;
const runtimeInfo = JSON.parse(solver.GetRuntimeInfo());
solver.Initialize(!runtimeInfo.threadsEnabled);
const runtimeReady = performance.now();

const batchStarted = performance.now();
const response = JSON.parse(thermoBench.RunBatch(JSON.stringify(request)));
const batchFinished = performance.now();

function sha256(file) {
  return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

const frameworkDir = path.join(bundle, '_framework');
const artifactNames = fs.readdirSync(frameworkDir).filter(name => (
  /^dotnet\.native\..*\.wasm$/.test(name)
  || /^SudokuSolver\..*\.wasm$/.test(name)
  || /^SudokuSolverWasm\..*\.wasm$/.test(name)
));

response.runtime = runtimeInfo;
response.nodeVersion = process.version;
response.v8Version = process.versions.v8;
response.runtimeStartupMs = runtimeReady - runtimeStarted;
response.batchWallMs = batchFinished - batchStarted;
response.bundle = bundle;
response.bundleArtifactsSha256 = Object.fromEntries(artifactNames.map(
  name => [name, sha256(path.join(frameworkDir, name))],
));
response.originalCwd = originalCwd;
process.stdout.write(JSON.stringify(response));
process.exit(0);
